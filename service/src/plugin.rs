// SPDX-License-Identifier: GPL-3.0-or-later
//! The D-Bus object NetworkManager drives: `org.freedesktop.NetworkManager.VPN.Plugin`.
//!
//! NM calls `Connect`/`NeedSecrets`/`Disconnect` on us; we answer and drive the
//! [`Supervisor`], which emits the `Config`/`Ip4Config`/`StateChanged` signals
//! back. The `Set*` methods exist for ABI completeness (libnm's helper re-emits
//! the matching signal); here they simply forward to the same emit path.

use std::sync::Arc;

use tokio::sync::Mutex;
use zbus::Connection;
use zbus::zvariant::{OwnedValue, Value};

use crate::config::{self, Connection as NmConnection};
use crate::nm::ServiceState;
use crate::state::State;
use crate::supervisor::Supervisor;

/// Shared plugin state. Cloned into the served interface; the connection is
/// injected once the bus is up (chicken/egg with `serve_at`).
pub struct Shared {
    pub crosswire_bin: String,
    /// Absolute path to our installed pppd plugin `.so`, passed to crosswire
    /// via `--pppd-plugin`; it reports IP config back via `Set*` below.
    pub pppd_plugin: String,
    /// Tunnel interface name handed to `--pppd-ifname` (single connection).
    pub ifname: String,
    /// Path to the native cert-trust dialog helper, launched in the user's
    /// session when crosswire rejects the gateway's (changed) certificate.
    pub cert_dialog: String,
    pub conn: tokio::sync::OnceCell<Connection>,
    /// The connection state machine; the single owner of `StateChanged`.
    pub state: tokio::sync::OnceCell<State>,
    /// External VPN gateway host of the current connection, used to inject the
    /// server route into the Config we hand NM.
    pub gateway: Mutex<Option<String>>,
    pub supervisor: Mutex<Option<Supervisor>>,
}

#[derive(Clone)]
pub struct VpnPlugin {
    pub shared: Arc<Shared>,
}

impl VpnPlugin {
    async fn conn(&self) -> Connection {
        self.shared
            .conn
            .get()
            .expect("connection injected at startup")
            .clone()
    }

    fn state(&self) -> State {
        self.shared
            .state
            .get()
            .expect("state injected at startup")
            .clone()
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.VPN.Plugin")]
impl VpnPlugin {
    /// NM asks us to bring the tunnel up with a fully-resolved connection.
    async fn connect(&self, connection: NmConnection) -> zbus::fdo::Result<()> {
        self.do_connect(connection).await
    }

    /// Interactive variant (extra hints in `details`); same behaviour for us.
    async fn connect_interactive(
        &self,
        connection: NmConnection,
        _details: std::collections::HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.do_connect(connection).await
    }

    /// NM asks which `vpn.secrets` key (if any) it must still collect from the
    /// user/keyring before `Connect` can succeed. Empty string = none needed.
    async fn need_secrets(&self, connection: NmConnection) -> zbus::fdo::Result<String> {
        let probe = config::map_connection(
            &connection,
            &self.shared.pppd_plugin,
            &self.shared.ifname,
            true,
        );
        Ok(probe.missing_secret.unwrap_or_default())
    }

    /// Fresh secrets supplied after a `SecretsRequired`; re-drive connect.
    async fn new_secrets(&self, connection: NmConnection) -> zbus::fdo::Result<()> {
        self.do_connect(connection).await
    }

    /// Tear the tunnel down.
    async fn disconnect(&self) -> zbus::fdo::Result<()> {
        let state = self.state();
        state.to(ServiceState::Stopping).await;
        if let Some(mut sup) = self.shared.supervisor.lock().await.take() {
            sup.stop().await;
        }
        state.to(ServiceState::Stopped).await;
        Ok(())
    }

    // --- Set* methods: our pppd plugin calls these at ip-up (and NM's own
    //     helper model uses them too); we re-emit the matching signal to NM. ---

    async fn set_config(&self, mut config: std::collections::HashMap<String, OwnedValue>) {
        // The pppd plugin can't know the external gateway (it only sees PPP
        // addresses), so inject it here: NM uses Config's "gateway" to pin a host
        // route to the VPN server through the pre-VPN default route. Without it a
        // default-routed tunnel routes its own TLS transport into itself and dies.
        if let Some(host) = self.shared.gateway.lock().await.clone()
            && let Some(gw) = resolve_gateway_u32(host).await
            && let Ok(v) = Value::U32(gw).try_to_owned()
        {
            config.insert("gateway".to_string(), v);
        }
        let _ = self
            .conn()
            .await
            .emit_signal(
                None::<()>,
                crate::nm::PLUGIN_PATH,
                crate::nm::VPN_IFACE,
                "Config",
                &(config,),
            )
            .await;
    }

    /// Receiving the IPv4 config is what marks the tunnel fully up: re-emit it,
    /// then transition NM to `Started`.
    async fn set_ip4_config(&self, config: std::collections::HashMap<String, OwnedValue>) {
        let conn = self.conn().await;
        let _ = conn
            .emit_signal(
                None::<()>,
                crate::nm::PLUGIN_PATH,
                crate::nm::VPN_IFACE,
                "Ip4Config",
                &(config,),
            )
            .await;
        // Real ip-up from the pppd plugin — the one legitimate path to "Started".
        // The state machine drops this if we're not currently Starting.
        self.state().to(ServiceState::Started).await;
    }

    async fn set_ip6_config(&self, config: std::collections::HashMap<String, OwnedValue>) {
        let _ = self
            .conn()
            .await
            .emit_signal(
                None::<()>,
                crate::nm::PLUGIN_PATH,
                crate::nm::VPN_IFACE,
                "Ip6Config",
                &(config,),
            )
            .await;
    }

    async fn set_failure(&self, reason: String) {
        tracing::warn!("SetFailure: {reason}");
    }

    // --- Signals (declared so NM's introspection sees them). ---

    #[zbus(signal)]
    async fn state_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        state: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn secrets_required(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        message: String,
        secrets: Vec<String>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn config(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        config: std::collections::HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn ip4_config(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        config: std::collections::HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn ip6_config(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        config: std::collections::HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn login_banner(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        banner: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn failure(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        reason: u32,
    ) -> zbus::Result<()>;
}

/// Resolve `host` to the network-order u32 NM expects for a config address
/// (matching the pppd plugin, which passes `sin_addr.s_addr` raw). Blocking
/// resolution is offloaded so we don't stall the async runtime.
async fn resolve_gateway_u32(host: String) -> Option<u32> {
    tokio::task::spawn_blocking(move || {
        use std::net::{IpAddr, ToSocketAddrs};
        (host.as_str(), 443u16)
            .to_socket_addrs()
            .ok()?
            .find_map(|a| match a.ip() {
                IpAddr::V4(v4) => Some(u32::from_ne_bytes(v4.octets())),
                IpAddr::V6(_) => None,
            })
    })
    .await
    .ok()
    .flatten()
}

impl VpnPlugin {
    async fn do_connect(&self, connection: NmConnection) -> zbus::fdo::Result<()> {
        let launch = config::map_connection(
            &connection,
            &self.shared.pppd_plugin,
            &self.shared.ifname,
            false,
        );
        *self.shared.gateway.lock().await = launch.gateway.clone();
        if let Some(secret) = launch.missing_secret {
            // Signal NM to collect the missing secret, then it will call again.
            let conn = self.conn().await;
            let _ = conn
                .emit_signal(
                    None::<()>,
                    crate::nm::PLUGIN_PATH,
                    crate::nm::VPN_IFACE,
                    "SecretsRequired",
                    &(
                        "Additional credentials are required".to_string(),
                        vec![secret],
                    ),
                )
                .await;
            return Ok(());
        }

        let state = self.state();
        // Replace any previous session, resetting to Stopped so the re-arm below
        // is a legal Stopped -> Starting transition.
        if let Some(mut old) = self.shared.supervisor.lock().await.take() {
            old.stop().await;
            state.to(ServiceState::Stopped).await;
        }

        // Enter Starting *before* spawning so the watcher's fail() (which only
        // fires while active) is armed if crosswire dies immediately.
        state.to(ServiceState::Starting).await;
        match Supervisor::start(
            state.clone(),
            self.shared.crosswire_bin.clone(),
            self.shared.cert_dialog.clone(),
            launch,
        )
        .await
        {
            Ok(sup) => {
                *self.shared.supervisor.lock().await = Some(sup);
                Ok(())
            }
            Err(e) => {
                tracing::error!("failed to start crosswire: {e:#}");
                state.fail(crate::nm::Failure::ConnectFailed).await;
                Err(zbus::fdo::Error::Failed(format!("start crosswire: {e}")))
            }
        }
    }
}
