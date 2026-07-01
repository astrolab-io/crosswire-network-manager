// SPDX-License-Identifier: GPL-3.0-or-later
//! `nm-crosswire-service` — the NetworkManager VPN service plugin for crosswire.
//!
//! NM D-Bus-activates this binary (per the `.name` file), passing `--bus-name`.
//! We claim that name on the **system** bus, export the VPN.Plugin object, and
//! translate NM's Connect/Disconnect into a supervised `crosswire` process.

mod config;
mod nm;
mod plugin;
mod state;
mod supervisor;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Mutex, watch};
use zbus::connection;

use plugin::{Shared, VpnPlugin};

#[derive(Parser, Debug)]
#[command(
    name = "nm-crosswire-service",
    about = "NetworkManager VPN service plugin for crosswire"
)]
struct Args {
    /// Well-known bus name to claim (NM passes this on activation).
    #[arg(long, default_value = nm::DEFAULT_BUS_NAME)]
    bus_name: String,

    /// Path to the crosswire binary to supervise.
    #[arg(long, default_value = "/usr/sbin/crosswire")]
    crosswire_bin: String,

    /// Absolute path to our pppd plugin `.so` (passed to crosswire via
    /// `--pppd-plugin`; it reports IP config back to us at ip-up).
    #[arg(long, default_value = "/usr/lib/pppd/nm-crosswire-pppd-plugin.so")]
    pppd_plugin: String,

    /// Tunnel interface name handed to crosswire's `--pppd-ifname`.
    #[arg(long, default_value = "ppp0")]
    ifname: String,

    /// Kept for NM CLI compatibility (`--persist`); we always run foreground.
    #[arg(long, default_value_t = false)]
    persist: bool,

    /// Serve on the session bus instead of the system bus (for testing without
    /// root / the system D-Bus policy). NM always uses the system bus.
    #[arg(long, default_value_t = false)]
    session_bus: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,nm_crosswire_service=debug".into()),
        )
        .init();

    let args = Args::parse();

    let shared = Arc::new(Shared {
        crosswire_bin: args.crosswire_bin.clone(),
        pppd_plugin: args.pppd_plugin.clone(),
        ifname: args.ifname.clone(),
        conn: tokio::sync::OnceCell::new(),
        state: tokio::sync::OnceCell::new(),
        gateway: Mutex::new(None),
        supervisor: Mutex::new(None),
    });

    let iface = VpnPlugin {
        shared: shared.clone(),
    };

    // Serve on the SYSTEM bus (NM runs as root there); --session-bus for tests.
    let builder = if args.session_bus {
        connection::Builder::session().context("connecting to the session bus")?
    } else {
        connection::Builder::system().context("connecting to the system bus")?
    };
    let conn = builder
        .name(args.bus_name.as_str())
        .context("claiming the bus name")?
        .serve_at(nm::PLUGIN_PATH, iface)
        .context("exporting the VPN.Plugin object")?
        .build()
        .await
        .context("building the D-Bus connection")?;

    // Inject the live connection so the interface can emit signals, and build
    // the state machine that owns every StateChanged emission.
    shared
        .conn
        .set(conn.clone())
        .map_err(|_| anyhow::anyhow!("connection already set"))?;
    let (state, mut active_rx) = state::State::new(conn.clone());
    shared
        .state
        .set(state)
        .map_err(|_| anyhow::anyhow!("state already set"))?;

    tracing::info!(bus = %args.bus_name, "nm-crosswire-service ready");

    // Run until NM stops us (SIGTERM, or SIGINT when run by hand) or we've been
    // idle past the quit timer (parity with NMVpnServicePlugin; --persist keeps
    // us resident). Any of these paths tears down a live tunnel on the way out.
    let mut sigterm = signal(SignalKind::terminate()).context("installing SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("installing SIGINT handler")?;
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("SIGTERM received; shutting down"),
        _ = sigint.recv()  => tracing::info!("SIGINT received; shutting down"),
        _ = idle_quit(&mut active_rx, args.persist) => tracing::info!("idle timeout; quitting"),
    }
    if let Some(mut sup) = shared.supervisor.lock().await.take() {
        sup.stop().await;
    }
    Ok(())
}

/// Resolves once the plugin has been idle (no active connection) for the quit
/// timeout, so NM can re-activate us on demand instead of us lingering. With
/// `--persist` we never resolve (stay resident).
async fn idle_quit(active_rx: &mut watch::Receiver<bool>, persist: bool) {
    const QUIT_TIMER: Duration = Duration::from_secs(180); // matches NMVpnServicePlugin
    if persist {
        std::future::pending::<()>().await;
        return;
    }
    loop {
        // Wait until inactive.
        while *active_rx.borrow() {
            if active_rx.changed().await.is_err() {
                return;
            }
        }
        // Idle: quit after the timeout unless a connection starts first.
        tokio::select! {
            _ = tokio::time::sleep(QUIT_TIMER) => return,
            r = active_rx.changed() => {
                if r.is_err() {
                    return;
                }
            }
        }
    }
}
