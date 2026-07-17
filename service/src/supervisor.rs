// SPDX-License-Identifier: GPL-3.0-or-later
//! Spawns and supervises the `crosswire` child for one connection.
//!
//! Unlike a stdout-scraping bridge, the IP configuration does **not** come back
//! through this process: our pppd plugin (loaded by crosswire's pppd via
//! `--pppd-plugin`) calls `SetConfig`/`SetIp4Config` on the service's D-Bus
//! object at ip-up, and those handlers emit the `Config`/`Ip4Config`/`Started`
//! signals to NetworkManager. Here we only launch, log, supervise, and report
//! exit — plus one special case: if crosswire rejects the gateway's certificate
//! (a rotated/changed leaf that no longer matches the pinned `trusted-cert`), we
//! recover the presented digest from its output and pop a native "trust this
//! certificate?" dialog instead of pointlessly retrying (see [`cert_trust`]).

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::cert_trust::{CertScanner, ExitAction, classify_exit};
use crate::config::Launch;
use crate::nm::{Failure, ServiceState};
use crate::state::State;

/// A crosswire that dies within this window of launch, while the connection is
/// still `Starting`, failed in its connect phase (auth/config fetch) rather than
/// after a live tunnel — the case worth retrying in place.
const FAST_FAILURE_WINDOW: Duration = Duration::from_secs(10);
/// How many times to re-launch crosswire after such a fast connect failure
/// before giving up and reporting `ConnectFailed` to NetworkManager.
const MAX_RETRIES: u32 = 2;
/// Delay between those re-launch attempts — lets the gateway settle after a
/// prior session teardown (crosswire's own HTTP retry rides over the short
/// window; this backstops the case where the gateway needs longer).
const RETRY_DELAY: Duration = Duration::from_secs(3);
/// Upper bound on waiting for the output readers to drain after crosswire exits,
/// so a wedged pipe can't hang classification (they normally finish at EOF).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Handle to a running child; `stop` tears the tunnel down. The child itself is
/// owned by the watcher task (see `start`) so it can `.wait()` — and thereby
/// reap — it; `stop_tx` asks that task to tear down and acks when done.
pub struct Supervisor {
    stop_tx: mpsc::Sender<oneshot::Sender<()>>,
}

/// One live crosswire process together with the output readers that feed the
/// [`CertScanner`]. Bundled so a re-launch swaps all three atomically.
struct Running {
    child: tokio::process::Child,
    /// Scans this run's stdout+stderr for a rejected-certificate verdict.
    scanner: Arc<Mutex<CertScanner>>,
    /// The stdout/stderr reader tasks; awaited before we inspect `scanner` so it
    /// has observed every line crosswire emitted.
    loggers: Vec<JoinHandle<()>>,
}

impl Supervisor {
    /// Launch crosswire and watch it on a background task. `state` is the
    /// connection state machine the watcher reports an early exit to;
    /// `cert_dialog` is the trust-prompt helper launched on a cert rejection.
    pub async fn start(
        state: State,
        bin: String,
        cert_dialog: String,
        launch: Launch,
    ) -> Result<Self> {
        // The first spawn is synchronous so a bad binary/argv surfaces to the
        // caller (NM's Connect) as it did before; re-launches happen in the task.
        let running = spawn_child(&bin, &launch).await?;

        let (stop_tx, mut stop_rx) = mpsc::channel::<oneshot::Sender<()>>(1);

        // Captured for the cert-trust prompt: which connection to re-pin, and the
        // gateway host to show the user.
        let uuid = launch.uuid.clone();
        let gateway = launch.gateway.clone();

        // The watcher OWNS the child so it can `.wait()` it — that both detects
        // exit and reaps the process. (Polling kill(pid,0) instead would see the
        // unreaped zombie as still alive and never fire, leaving NM to sit until
        // its 60s connect timeout whenever crosswire dies early.) On an
        // unexpected exit it either re-launches (a fast connect-phase failure),
        // prompts to trust a changed certificate, or drives the state machine to
        // Failure+Stopped; the machine ignores that if we're already tearing down.
        tokio::spawn(async move {
            let mut running = running;
            let mut retries: u32 = 0;

            loop {
                let launched = Instant::now();
                let ack: Option<oneshot::Sender<()>> = tokio::select! {
                    biased;
                    req = stop_rx.recv() => req,      // Disconnect asked us to tear down
                    status = running.child.wait() => {        // crosswire exited on its own
                        match status {
                            Ok(s) if s.success() => {
                                // Clean exit (e.g. the server closed the session):
                                // a disconnect, not a failure.
                                tracing::info!("crosswire exited cleanly before Disconnect");
                                state.to(ServiceState::Stopped).await;
                                return;
                            }
                            other => {
                                // Make sure the scanner has seen all of crosswire's
                                // output before we ask it for a verdict — `wait()`
                                // can return before the readers drain the pipes.
                                drain_loggers(&mut running.loggers).await;
                                let cert_change =
                                    running.scanner.lock().unwrap().cert_change().map(String::from);
                                let fast = launched.elapsed() < FAST_FAILURE_WINDOW;
                                let connecting =
                                    matches!(state.current().await, ServiceState::Starting);

                                match classify_exit(
                                    cert_change.as_deref(),
                                    fast,
                                    connecting,
                                    retries,
                                    MAX_RETRIES,
                                ) {
                                    // A fast connect-phase failure right after a
                                    // prior session teardown — retry in place (for
                                    // SAML this re-opens the browser, so keep the
                                    // count low). Mid-session drops are left to NM.
                                    ExitAction::Retry => {
                                        retries += 1;
                                        tracing::warn!(
                                            "crosswire exited during connect ({other:?}); \
                                             re-launching {retries}/{MAX_RETRIES} in {RETRY_DELAY:?}"
                                        );
                                        // A Disconnect during the backoff wins.
                                        tokio::select! {
                                            biased;
                                            req = stop_rx.recv() => {
                                                if let Some(ack) = req {
                                                    let _ = ack.send(());
                                                }
                                                return;
                                            }
                                            _ = tokio::time::sleep(RETRY_DELAY) => {}
                                        }
                                        match spawn_child(&bin, &launch).await {
                                            Ok(r) => {
                                                running = r;
                                                continue;
                                            }
                                            Err(e) => {
                                                tracing::error!("re-launching crosswire failed: {e:#}");
                                                state.fail(Failure::ConnectFailed).await;
                                                return;
                                            }
                                        }
                                    }
                                    // The gateway's certificate changed: retrying
                                    // would just re-reject it. Prompt the user to
                                    // trust the new digest (which re-pins and
                                    // re-activates), and report failure meanwhile.
                                    ExitAction::PromptCert(digest) => {
                                        tracing::warn!(
                                            "gateway certificate not trusted; presented digest \
                                             {digest} — prompting user to trust it"
                                        );
                                        prompt_cert_trust(
                                            cert_dialog.clone(),
                                            uuid.clone(),
                                            gateway.clone(),
                                            digest,
                                        )
                                        .await;
                                        state.fail(Failure::ConnectFailed).await;
                                        return;
                                    }
                                    ExitAction::Fail => {
                                        tracing::warn!("crosswire exited: {other:?}");
                                        state.fail(Failure::ConnectFailed).await;
                                        return;
                                    }
                                }
                            }
                        }
                    }
                };

                // Requested teardown: SIGTERM so crosswire's RAII cleanup runs,
                // then reap (kill_on_drop covers us if we bail before wait done).
                #[cfg(unix)]
                if let Some(pid) = running.child.id() {
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                }
                if tokio::time::timeout(Duration::from_secs(8), running.child.wait())
                    .await
                    .is_err()
                {
                    let _ = running.child.start_kill();
                    let _ = running.child.wait().await;
                }
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
                return;
            }
        });

        Ok(Self { stop_tx })
    }

    /// Ask the watcher to tear crosswire down and wait for it to confirm. If the
    /// watcher already exited (child died on its own), the send fails and we
    /// return immediately.
    pub async fn stop(&mut self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self.stop_tx.send(ack_tx).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(10), ack_rx).await;
        }
    }
}

/// Spawn crosswire, feed it any stdin (the session cookie), and forward its
/// stdout/stderr into our tracing log while a fresh [`CertScanner`] watches both
/// streams. Used for the initial launch and re-launches.
async fn spawn_child(bin: &str, launch: &Launch) -> Result<Running> {
    let mut cmd = Command::new(bin);
    cmd.args(&launch.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().with_context(|| format!("spawning {bin}"))?;

    if let Some(data) = launch.stdin.clone()
        && let Some(mut si) = child.stdin.take()
    {
        let _ = si.write_all(data.as_bytes()).await;
        let _ = si.write_all(b"\n").await;
        let _ = si.shutdown().await;
    }

    // A per-run scanner: both readers feed it, the watcher reads its verdict.
    let scanner = Arc::new(Mutex::new(CertScanner::default()));
    let mut loggers = Vec::new();
    // crosswire's tracing goes to stdout, its top-level error to stderr; the
    // cert digest and the rejection message land on different streams, so scan
    // (and log) both.
    if let Some(out) = child.stdout.take() {
        loggers.push(spawn_logger(out, tracing::Level::DEBUG, scanner.clone()));
    }
    if let Some(err) = child.stderr.take() {
        loggers.push(spawn_logger(err, tracing::Level::INFO, scanner.clone()));
    }

    Ok(Running {
        child,
        scanner,
        loggers,
    })
}

fn spawn_logger<R>(
    reader: R,
    level: tracing::Level,
    scanner: Arc<Mutex<CertScanner>>,
) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            scanner.lock().unwrap().observe(&line);
            match level {
                tracing::Level::INFO => tracing::info!(target: "crosswire", "{line}"),
                _ => tracing::debug!(target: "crosswire", "{line}"),
            }
        }
    })
}

/// Await the output readers (bounded by [`DRAIN_TIMEOUT`]) so the scanner has
/// observed every line crosswire wrote before its pipes closed.
async fn drain_loggers(loggers: &mut Vec<JoinHandle<()>>) {
    for h in loggers.drain(..) {
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, h).await;
    }
}

/// Launch the native cert-trust dialog in the user's session (off the async
/// runtime, since it shells out). On success the helper re-pins `trusted-cert`
/// and re-activates; if we can't reach a session, tell the log how to re-pin.
async fn prompt_cert_trust(
    cert_dialog: String,
    uuid: Option<String>,
    gateway: Option<String>,
    digest: String,
) {
    let result = tokio::task::spawn_blocking(move || {
        let mut args = vec!["--digest".to_string(), digest.clone()];
        if let Some(g) = &gateway {
            args.push("--gateway".to_string());
            args.push(g.clone());
        }
        if let Some(u) = &uuid {
            args.push("--uuid".to_string());
            args.push(u.clone());
        }
        (
            crate::user_session::spawn_in_user_session(&cert_dialog, &args),
            digest,
        )
    })
    .await;

    match result {
        Ok((true, _)) => {
            tracing::info!("opened the cert-trust dialog for the changed gateway certificate")
        }
        Ok((false, digest)) => tracing::warn!(
            "no active graphical session to prompt in; re-pin manually by setting \
             trusted-cert = {digest} on the connection"
        ),
        Err(e) => tracing::error!("cert-trust dialog task failed: {e}"),
    }
}
