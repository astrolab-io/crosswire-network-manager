// SPDX-License-Identifier: GPL-3.0-or-later
//! NetworkManager VPN-plugin D-Bus constants.
//!
//! The IP configuration itself is marshalled in C by our pppd plugin (which
//! knows the `NM_VPN_PLUGIN_IP4_CONFIG_*` keys) and delivered to the service via
//! `SetConfig`/`SetIp4Config`; the service only re-emits it. So the Rust side
//! just needs the well-known names/paths and the state/failure enums.

/// D-Bus interface every VPN plugin exports.
pub const VPN_IFACE: &str = "org.freedesktop.NetworkManager.VPN.Plugin";
/// Object path the plugin object is served at.
pub const PLUGIN_PATH: &str = "/org/freedesktop/NetworkManager/VPN/Plugin";
/// Default well-known bus name (overridable via `--bus-name`, which NM passes).
pub const DEFAULT_BUS_NAME: &str = "org.freedesktop.NetworkManager.crosswire";

/// `NMVpnServiceState` — the values carried by the `StateChanged` signal.
/// Some variants are part of the ABI but not emitted by us yet.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ServiceState {
    Unknown = 0,
    Init = 1,
    Shutdown = 2,
    Starting = 3,
    Started = 4,
    Stopping = 5,
    Stopped = 6,
}

/// `NMVpnPluginFailure` — carried by the `Failure` signal.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum Failure {
    LoginFailed = 0,
    ConnectFailed = 1,
    BadIpConfig = 2,
}
