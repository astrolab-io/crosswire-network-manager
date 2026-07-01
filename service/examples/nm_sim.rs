// SPDX-License-Identifier: GPL-3.0-or-later
//! Live D-Bus driver that exercises a running `nm-crosswire-service` exactly as
//! NetworkManager (and the pppd plugin) do — over a real bus. Run the service
//! with `--session-bus`, then `cargo run --example nm_sim`.
//!
//! It calls NeedSecrets / Connect / SetConfig / SetIp4Config / Disconnect and
//! prints the signals the service emits back, demonstrating the full
//! NM ↔ service ↔ (crosswire spawn) ↔ pppd-plugin protocol chain end-to-end.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use zbus::proxy;
use zbus::zvariant::{OwnedValue, Value};

type Ass = HashMap<String, OwnedValue>;
type Conn = HashMap<String, Ass>;

#[proxy(
    interface = "org.freedesktop.NetworkManager.VPN.Plugin",
    default_service = "org.freedesktop.NetworkManager.crosswire",
    default_path = "/org/freedesktop/NetworkManager/VPN/Plugin"
)]
trait VpnPlugin {
    fn connect(&self, connection: Conn) -> zbus::Result<()>;
    fn need_secrets(&self, connection: Conn) -> zbus::Result<String>;
    fn disconnect(&self) -> zbus::Result<()>;
    fn set_config(&self, config: Ass) -> zbus::Result<()>;
    fn set_ip4_config(&self, config: Ass) -> zbus::Result<()>;

    #[zbus(signal)]
    fn state_changed(&self, state: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn config(&self, config: Ass) -> zbus::Result<()>;
    #[zbus(signal)]
    fn ip4_config(&self, config: Ass) -> zbus::Result<()>;
    #[zbus(signal)]
    fn failure(&self, reason: u32) -> zbus::Result<()>;
}

fn sname(s: u32) -> &'static str {
    match s {
        1 => "Init",
        2 => "Shutdown",
        3 => "Starting",
        4 => "Started",
        5 => "Stopping",
        6 => "Stopped",
        _ => "Unknown",
    }
}

fn owned(v: Value<'_>) -> OwnedValue {
    v.try_to_owned().unwrap()
}

/// Build an NM connection dict with nested data/secrets a{ss}.
fn conn(data: &[(&str, &str)], secrets: &[(&str, &str)]) -> Conn {
    let mut vpn: Ass = HashMap::new();
    vpn.insert("data".into(), owned(Value::from(str_map(data))));
    vpn.insert("secrets".into(), owned(Value::from(str_map(secrets))));
    let mut c: Conn = HashMap::new();
    c.insert("vpn".into(), vpn);
    c
}

fn str_map(kv: &[(&str, &str)]) -> HashMap<String, String> {
    kv.iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[tokio::main]
async fn main() -> zbus::Result<()> {
    let c = zbus::Connection::session().await?;
    let proxy = VpnPluginProxy::new(&c).await?;

    // Subscribe to the signals NM listens for.
    let mut st = proxy.receive_state_changed().await?;
    tokio::spawn(async move {
        while let Some(s) = st.next().await {
            let v = s.args().unwrap().state;
            println!("   << StateChanged({v}) = {}", sname(v));
        }
    });
    let mut cfg = proxy.receive_config().await?;
    tokio::spawn(async move {
        while let Some(s) = cfg.next().await {
            let d = s.args().unwrap().config;
            println!("   << Config: tundev={:?}", d.get("tundev"));
        }
    });
    let mut ip4 = proxy.receive_ip4_config().await?;
    tokio::spawn(async move {
        while let Some(s) = ip4.next().await {
            let d = s.args().unwrap().config;
            let mut keys: Vec<_> = d.keys().cloned().collect();
            keys.sort();
            println!("   << Ip4Config: {keys:?}");
        }
    });
    let mut fail = proxy.receive_failure().await?;
    tokio::spawn(async move {
        while let Some(s) = fail.next().await {
            println!("   << Failure({})", s.args().unwrap().reason);
        }
    });

    let with_pw = conn(
        &[
            ("gateway", "vpn.example.com"),
            ("auth-type", "password"),
            ("user", "alice"),
        ],
        &[("password", "s3cr3t")],
    );

    println!(
        "-> NeedSecrets(no secret)  => {:?}",
        proxy.need_secrets(conn(&[("gateway", "gw")], &[])).await?
    );
    println!(
        "-> NeedSecrets(with pw)    => {:?}",
        proxy.need_secrets(with_pw.clone()).await?
    );

    println!("-> Connect(...)   [service spawns the stub crosswire]");
    proxy.connect(with_pw).await?;
    tokio::time::sleep(Duration::from_millis(600)).await;

    println!("-> (acting as the pppd plugin at ip-up) SetConfig + SetIp4Config");
    let mut generic: Ass = HashMap::new();
    generic.insert("tundev".into(), owned(Value::from("ppp0")));
    generic.insert("has-ip4".into(), owned(Value::from(true)));
    proxy.set_config(generic).await?;

    let mut ip: Ass = HashMap::new();
    ip.insert("address".into(), owned(Value::from(0x0a_80_a8_c0u32))); // 192.168.128.10-ish
    ip.insert("prefix".into(), owned(Value::from(32u32)));
    ip.insert("dns".into(), owned(Value::from(vec![0x01_00_a8_c0u32])));
    proxy.set_ip4_config(ip).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;

    println!("-> Disconnect(...)");
    proxy.disconnect().await?;
    tokio::time::sleep(Duration::from_millis(400)).await;

    println!("\nDONE — observed the full NM ↔ service signal exchange over live D-Bus.");
    Ok(())
}
