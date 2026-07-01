// SPDX-License-Identifier: GPL-3.0-or-later
//! The connection state machine.
//!
//! NetworkManager's UI reflects whatever `StateChanged` we emit, so those
//! emissions must track the *real* tunnel state — not optimistic guesses. This
//! is the single owner of the current `NMVpnServiceState`: every transition
//! goes through [`State`], which emits `StateChanged` only for legal, real
//! changes. That guarantees, e.g., `Started` (NM shows "connected") is only ever
//! reached from `Starting` — so a `SetIp4Config` racing in after teardown can't
//! resurrect a dead connection, and a duplicate event can't re-announce a state.
//!
//! Real events → transitions:
//!   Connect                         → Starting
//!   pppd plugin SetIp4Config (up)   → Started
//!   crosswire exits while active   → Failure + Stopped
//!   Disconnect                      → Stopping → Stopped

use std::sync::Arc;

use tokio::sync::{Mutex, watch};
use zbus::Connection;

use crate::nm::{self, Failure, ServiceState};

/// Cheap-to-clone handle to the one shared state machine.
#[derive(Clone)]
pub struct State(Arc<Mutex<Inner>>);

struct Inner {
    conn: Connection,
    state: ServiceState,
    /// Mirrors "is a connection active" for the main loop's idle-quit timer.
    active_tx: watch::Sender<bool>,
}

impl State {
    /// Build the state machine plus a receiver tracking whether a connection is
    /// active (drives the idle-quit timer in `main`).
    pub fn new(conn: Connection) -> (Self, watch::Receiver<bool>) {
        let (active_tx, active_rx) = watch::channel(false);
        let this = Self(Arc::new(Mutex::new(Inner {
            conn,
            state: ServiceState::Stopped,
            active_tx,
        })));
        (this, active_rx)
    }

    /// Request a transition; emits `StateChanged` only if it is legal and a real
    /// change. Illegal/duplicate transitions are ignored (and logged).
    pub async fn to(&self, next: ServiceState) {
        let mut inner = self.0.lock().await;
        if inner.state == next {
            return;
        }
        if !legal(inner.state, next) {
            tracing::debug!("ignoring transition {:?} -> {:?}", inner.state, next);
            return;
        }
        inner.set(next).await;
    }

    /// Report a failure of the *active* connection: emit `Failure`, then settle
    /// in `Stopped`. A no-op if we're not currently connecting/connected, so a
    /// child exiting during our own teardown doesn't double-report.
    pub async fn fail(&self, reason: Failure) {
        let mut inner = self.0.lock().await;
        if !matches!(inner.state, ServiceState::Starting | ServiceState::Started) {
            return;
        }
        emit_failure(&inner.conn, reason).await;
        inner.set(ServiceState::Stopped).await;
    }
}

impl Inner {
    /// Commit a state, publish activity, and emit `StateChanged`.
    async fn set(&mut self, next: ServiceState) {
        self.state = next;
        let active = !matches!(
            next,
            ServiceState::Stopped
                | ServiceState::Init
                | ServiceState::Shutdown
                | ServiceState::Unknown
        );
        let _ = self.active_tx.send(active);
        emit_state(&self.conn, next).await;
    }
}

/// The allowed edges of the state machine.
fn legal(from: ServiceState, to: ServiceState) -> bool {
    use ServiceState::*;
    match to {
        // (Re)connect only from an inactive state.
        Starting => matches!(from, Unknown | Init | Stopped),
        // "Connected" is only reachable from an in-progress connect — this is
        // what stops a stray/late SetIp4Config from faking a live tunnel.
        Started => matches!(from, Starting),
        Stopping => matches!(from, Starting | Started),
        Stopped => matches!(from, Starting | Started | Stopping),
        // We never drive NM into these.
        Init | Shutdown | Unknown => false,
    }
}

async fn emit_state(conn: &Connection, state: ServiceState) {
    if let Err(e) = conn
        .emit_signal(
            None::<()>,
            nm::PLUGIN_PATH,
            nm::VPN_IFACE,
            "StateChanged",
            &(state as u32,),
        )
        .await
    {
        tracing::error!("emit StateChanged: {e}");
    }
}

async fn emit_failure(conn: &Connection, reason: Failure) {
    if let Err(e) = conn
        .emit_signal(
            None::<()>,
            nm::PLUGIN_PATH,
            nm::VPN_IFACE,
            "Failure",
            &(reason as u32,),
        )
        .await
    {
        tracing::error!("emit Failure: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::legal;
    use crate::nm::ServiceState::*;

    #[test]
    fn started_only_from_starting() {
        assert!(legal(Starting, Started));
        // The race we care about: a late SetIp4Config after teardown must not
        // be able to announce "connected".
        assert!(!legal(Stopped, Started));
        assert!(!legal(Started, Started)); // (also caught by the == guard)
    }

    #[test]
    fn connect_only_when_inactive() {
        assert!(legal(Stopped, Starting));
        assert!(legal(Init, Starting));
        assert!(!legal(Starting, Starting));
        assert!(!legal(Started, Starting));
    }

    #[test]
    fn teardown_paths() {
        assert!(legal(Starting, Stopping));
        assert!(legal(Started, Stopping));
        assert!(legal(Stopping, Stopped));
        assert!(legal(Started, Stopped));
        assert!(!legal(Stopped, Stopping));
    }
}
