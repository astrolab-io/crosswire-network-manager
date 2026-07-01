// SPDX-License-Identifier: GPL-3.0-or-later
//! Spawns and supervises the `crosswire` child for one connection.
//!
//! Unlike a stdout-scraping bridge, the IP configuration does **not** come back
//! through this process: our pppd plugin (loaded by crosswire's pppd via
//! `--pppd-plugin`) calls `SetConfig`/`SetIp4Config` on the service's D-Bus
//! object at ip-up, and those handlers emit the `Config`/`Ip4Config`/`Started`
//! signals to NetworkManager. Here we only launch, log, and report exit.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use crate::config::Launch;
use crate::nm::{Failure, ServiceState};
use crate::state::State;

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
        let mut cmd = Command::new(&bin);
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

        let (stop_tx, mut stop_rx) = mpsc::channel::<oneshot::Sender<()>>(1);

        // The watcher OWNS the child so it can `.wait()` it — that both detects
        // exit and reaps the process. (Polling kill(pid,0) instead would see the
        // unreaped zombie as still alive and never fire, leaving NM to sit until
        // its 60s connect timeout whenever crosswire dies early.) On an
        // unexpected exit it drives the state machine to Failure+Stopped; the
        // machine ignores that if we're already tearing down.
        tokio::spawn(async move {
            let mut child = child;
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
                        }
                        other => {
                            tracing::warn!("crosswire exited: {other:?}");
                            state.fail(Failure::ConnectFailed).await;
                        }
                    }
                    return;
                }
            };

            // Requested teardown: SIGTERM so crosswire's RAII cleanup runs, then
            // reap (kill_on_drop covers us if we bail before wait completes).
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
