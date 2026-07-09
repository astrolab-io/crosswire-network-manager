// SPDX-License-Identifier: GPL-3.0-or-later
//! Spawns and supervises the `crosswire` child for one connection.
//!
//! Unlike a stdout-scraping bridge, the IP configuration does **not** come back
//! through this process: our pppd plugin (loaded by crosswire's pppd via
//! `--pppd-plugin`) calls `SetConfig`/`SetIp4Config` on the service's D-Bus
//! object at ip-up, and those handlers emit the `Config`/`Ip4Config`/`Started`
//! signals to NetworkManager. Here we only launch, log, supervise, and report exit.

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

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

/// Handle to a running child; `stop` tears the tunnel down. The child itself is
/// owned by the watcher task (see `start`) so it can `.wait()` — and thereby
/// reap — it; `stop_tx` asks that task to tear down and acks when done.
pub struct Supervisor {
    stop_tx: mpsc::Sender<oneshot::Sender<()>>,
}

impl Supervisor {
    /// Launch crosswire and watch it on a background task. `state` is the
    /// connection state machine the watcher reports an early exit to.
    pub async fn start(state: State, bin: String, launch: Launch) -> Result<Self> {
        // The first spawn is synchronous so a bad binary/argv surfaces to the
        // caller (NM's Connect) as it did before; re-launches happen in the task.
        let child = spawn_child(&bin, &launch).await?;

        let (stop_tx, mut stop_rx) = mpsc::channel::<oneshot::Sender<()>>(1);

        // The watcher OWNS the child so it can `.wait()` it — that both detects
        // exit and reaps the process. (Polling kill(pid,0) instead would see the
        // unreaped zombie as still alive and never fire, leaving NM to sit until
        // its 60s connect timeout whenever crosswire dies early.) On an
        // unexpected exit it either re-launches (a fast connect-phase failure) or
        // drives the state machine to Failure+Stopped; the machine ignores that
        // if we're already tearing down.
        tokio::spawn(async move {
            let mut child = child;
            let mut retries: u32 = 0;

            loop {
                let launched = Instant::now();
                let ack: Option<oneshot::Sender<()>> = tokio::select! {
                    biased;
                    req = stop_rx.recv() => req,      // Disconnect asked us to tear down
                    status = child.wait() => {        // crosswire exited on its own
                        match status {
                            Ok(s) if s.success() => {
                                // Clean exit (e.g. the server closed the session):
                                // a disconnect, not a failure.
                                tracing::info!("crosswire exited cleanly before Disconnect");
                                state.to(ServiceState::Stopped).await;
                                return;
                            }
                            other => {
                                // Retry only a *fast* failure that happened while
                                // still connecting (never reached Started) — i.e. a
                                // connect-phase failure right after a prior session
                                // teardown, not a mid-session drop (leave those to
                                // NM's reconnect policy). Note: for SAML this
                                // re-opens the browser, so keep the count low.
                                let fast = launched.elapsed() < FAST_FAILURE_WINDOW;
                                let connecting =
                                    matches!(state.current().await, ServiceState::Starting);
                                if retries < MAX_RETRIES && fast && connecting {
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
                                        Ok(c) => {
                                            child = c;
                                            continue;
                                        }
                                        Err(e) => {
                                            tracing::error!("re-launching crosswire failed: {e:#}");
                                            state.fail(Failure::ConnectFailed).await;
                                            return;
                                        }
                                    }
                                }
                                tracing::warn!("crosswire exited: {other:?}");
                                state.fail(Failure::ConnectFailed).await;
                                return;
                            }
                        }
                    }
                };

                // Requested teardown: SIGTERM so crosswire's RAII cleanup runs,
                // then reap (kill_on_drop covers us if we bail before wait done).
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                }
                if tokio::time::timeout(Duration::from_secs(8), child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
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
/// stdout/stderr into our tracing log. Used for the initial launch and re-launches.
async fn spawn_child(bin: &str, launch: &Launch) -> Result<tokio::process::Child> {
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

    // Forward crosswire's stdout/stderr into our tracing log.
    if let Some(out) = child.stdout.take() {
        spawn_logger(out, tracing::Level::DEBUG);
    }
    if let Some(err) = child.stderr.take() {
        spawn_logger(err, tracing::Level::INFO);
    }

    Ok(child)
}

fn spawn_logger<R>(reader: R, level: tracing::Level)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match level {
                tracing::Level::INFO => tracing::info!(target: "crosswire", "{line}"),
                _ => tracing::debug!(target: "crosswire", "{line}"),
            }
        }
    });
}
