// SPDX-License-Identifier: GPL-3.0-or-later
//! Maps an NM connection dict (`a{sa{sv}}`) → an `crosswire` invocation.
//!
//! We drive crosswire with the same knobs NetworkManager-fortisslvpn uses for
//! openfortivpn: `--set-routes false --set-dns false` (NM owns routing/DNS),
//! `--pppd-ifname <dev>`, and `--pppd-plugin <our .so>`. The pppd plugin reports
//! the negotiated IP config back to us at ip-up; crosswire itself is unchanged.

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

/// An NM connection as delivered over D-Bus: setting name → (key → value).
pub type Connection = HashMap<String, HashMap<String, OwnedValue>>;

/// Pull a nested `a{ss}` field (`"data"` or `"secrets"`) out of the `vpn`
/// setting. NM serialises `NMSettingVpn` with its options *nested* under these
/// keys, not flattened — so we descend one level and decode the string map.
fn ass(vpn: &HashMap<String, OwnedValue>, field: &str) -> HashMap<String, String> {
    vpn.get(field)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| HashMap::<String, String>::try_from(v).ok())
        .unwrap_or_default()
}

fn get(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(key).cloned()
}

/// Everything the supervisor needs to launch crosswire for one connection.
#[derive(Debug, Default)]
pub struct Launch {
    /// argv after the binary name.
    pub args: Vec<String>,
    /// Fed to crosswire's stdin (currently the session cookie, if any).
    pub stdin: Option<String>,
    /// The `vpn.secrets` key NM must still supply before we can connect;
    /// `None` means we have what we need.
    pub missing_secret: Option<String>,
    /// The external VPN gateway host, so the service can tell NM to pin a route
    /// to the server (otherwise the default-routed tunnel swallows its own TLS
    /// transport). `None` if the connection carried no gateway.
    pub gateway: Option<String>,
}

/// Map an NM connection into an crosswire command line. `interactive_probe`
/// mirrors `NeedSecrets`: when true we only report what's missing, no argv.
/// `pppd_plugin` is the absolute path to our installed pppd plugin `.so`;
/// `dev` is the tunnel interface name the service picked for this connection.
pub fn map_connection(
    conn: &Connection,
    pppd_plugin: &str,
    dev: &str,
    interactive_probe: bool,
) -> Launch {
    let empty = HashMap::new();
    let vpn = conn.get("vpn").unwrap_or(&empty);
    // Merge vpn.data + vpn.secrets into one lookup (secrets win on key clash).
    let mut data = ass(vpn, "data");
    data.extend(ass(vpn, "secrets"));
    let data = &data;
    let mut l = Launch::default();

    let host = get(data, "gateway").unwrap_or_default();
    l.gateway = (!host.is_empty()).then(|| host.clone());
    let auth = get(data, "auth-type").unwrap_or_else(|| "password".into());
    // Provider selection from the editor. crosswire is provider-neutral but so
    // far only implements the Fortinet SSL-VPN provider (main.rs hardcodes it —
    // there is no `--provider` flag yet), so we record the choice without adding
    // an argument. When crosswire grows `--provider`, emit it from right here.
    let _provider = get(data, "provider").unwrap_or_else(|| "fortinet".into());

    // Secret sufficiency check (drives NeedSecrets).
    let cookie = get(data, "cookie");
    let password = get(data, "password");
    match auth.as_str() {
        "saml" => { /* browser-driven, no stored secret required */ }
        "cookie" if cookie.is_none() => l.missing_secret = Some("cookie".into()),
        _ if password.is_none() && cookie.is_none() => l.missing_secret = Some("password".into()),
        _ => {}
    }
    if interactive_probe {
        return l;
    }

    // Delegated networking (openfortivpn-parity): NM owns the address, routes,
    // and DNS; pppd negotiates the address and our pppd plugin reports it back
    // to the service. `--set-ip false` is essential — without it crosswire also
    // runs `ip addr add`, which collides with the address pppd/NM already
    // assigned ("Address already assigned"), and crosswire treats that as fatal
    // and exits, tearing the tunnel down right after it comes up.
    l.args.push("--set-routes".into());
    l.args.push("false".into());
    l.args.push("--set-dns".into());
    l.args.push("false".into());
    l.args.push("--set-ip".into());
    l.args.push("false".into());
    l.args.push("--pppd-ifname".into());
    l.args.push(dev.to_string());
    l.args.push("--pppd-plugin".into());
    l.args.push(pppd_plugin.to_string());

    if let Some(p) = get(data, "port") {
        l.args.push("--port".into());
        l.args.push(p);
    }
    if let Some(realm) = get(data, "realm") {
        l.args.push("--realm".into());
        l.args.push(realm);
    }
    if let Some(cert) = get(data, "trusted-cert") {
        for c in cert.split([',', ' ']).filter(|s| !s.is_empty()) {
            l.args.push("--trusted-cert".into());
            l.args.push(c.to_string());
        }
    }
    if let Some(ca) = get(data, "ca-file") {
        l.args.push("--ca-file".into());
        l.args.push(ca);
    }
    if get(data, "insecure-ssl").as_deref() == Some("yes") {
        l.args.push("--insecure-ssl".into());
    }

    match auth.as_str() {
        "saml" => l.args.push("--saml-login".into()),
        "cookie" => {
            l.args.push("--cookie-on-stdin".into());
            l.stdin = cookie;
        }
        _ => {
            if let Some(u) = get(data, "user") {
                l.args.push("--username".into());
                l.args.push(u);
            }
            if let Some(p) = password {
                l.args.push("--password".into());
                l.args.push(p);
            }
            if let Some(otp) = get(data, "otp") {
                l.args.push("--otp".into());
                l.args.push(otp);
            }
        }
    }

    // Positional host goes last, behind a `--` separator: `--saml-login` takes
    // an optional PORT (num_args=0..=1), so without `--` clap would swallow the
    // host as that flag's value ("invalid digit found in string") and crosswire
    // would exit before connecting.
    l.args.push("--".into());
    l.args.push(host);
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    /// Build a `vpn` setting the way NM serialises it: nested `data`/`secrets`
    /// `a{ss}` maps inside the setting's `a{sv}`.
    fn conn(data: &[(&str, &str)], secrets: &[(&str, &str)]) -> Connection {
        let to_map = |kv: &[(&str, &str)]| {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<String, String>>()
        };
        let mut vpn: HashMap<String, OwnedValue> = HashMap::new();
        vpn.insert(
            "data".into(),
            Value::from(to_map(data)).try_to_owned().unwrap(),
        );
        vpn.insert(
            "secrets".into(),
            Value::from(to_map(secrets)).try_to_owned().unwrap(),
        );
        let mut c: Connection = HashMap::new();
        c.insert("vpn".into(), vpn);
        c
    }

    fn has_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    #[test]
    fn password_auth_builds_delegated_argv() {
        let c = conn(
            &[
                ("gateway", "vpn.example.com"),
                ("port", "10443"),
                ("user", "alice"),
                ("auth-type", "password"),
            ],
            &[("password", "s3cr3t")],
        );
        let l = map_connection(&c, "/lib/pppd/nm.so", "ppp0", false);
        // NM owns networking; pppd plugin reports back.
        assert!(has_pair(&l.args, "--set-routes", "false"));
        assert!(has_pair(&l.args, "--set-dns", "false"));
        // Must delegate the address too, or crosswire's own `ip addr add`
        // collides with pppd/NM and it exits, dropping the tunnel.
        assert!(has_pair(&l.args, "--set-ip", "false"));
        assert!(has_pair(&l.args, "--pppd-ifname", "ppp0"));
        assert!(has_pair(&l.args, "--pppd-plugin", "/lib/pppd/nm.so"));
        assert!(has_pair(&l.args, "--port", "10443"));
        assert!(has_pair(&l.args, "--username", "alice"));
        // Secret pulled from the nested vpn.secrets map, not vpn.data.
        assert!(has_pair(&l.args, "--password", "s3cr3t"));
        // Positional host is last.
        assert_eq!(l.args.last().unwrap(), "vpn.example.com");
        assert!(l.missing_secret.is_none());
        // Gateway is captured so the service can pin the server route in Config.
        assert_eq!(l.gateway.as_deref(), Some("vpn.example.com"));
    }

    #[test]
    fn missing_password_is_reported() {
        let c = conn(&[("gateway", "vpn.example.com")], &[]);
        let l = map_connection(&c, "/lib/pppd/nm.so", "ppp0", true);
        assert_eq!(l.missing_secret.as_deref(), Some("password"));
    }

    #[test]
    fn cookie_auth_uses_stdin() {
        let c = conn(
            &[("gateway", "gw"), ("auth-type", "cookie")],
            &[("cookie", "SVPNCOOKIE=abc")],
        );
        let l = map_connection(&c, "/p.so", "ppp0", false);
        assert!(l.args.iter().any(|a| a == "--cookie-on-stdin"));
        assert_eq!(l.stdin.as_deref(), Some("SVPNCOOKIE=abc"));
    }

    #[test]
    fn saml_needs_no_stored_secret() {
        let c = conn(&[("gateway", "gw"), ("auth-type", "saml")], &[]);
        let l = map_connection(&c, "/p.so", "ppp0", false);
        assert!(l.args.iter().any(|a| a == "--saml-login"));
        assert!(l.missing_secret.is_none());
    }

    #[test]
    fn host_is_separated_from_optional_value_flags() {
        // --saml-login has an optional PORT value; without a `--` before the
        // positional host, clap parses the host as that port and crosswire
        // aborts. The host must be the last arg, preceded by `--`.
        let c = conn(
            &[("gateway", "vpn.example.com"), ("auth-type", "saml")],
            &[],
        );
        let l = map_connection(&c, "/p.so", "ppp0", false);
        let n = l.args.len();
        assert_eq!(
            &l.args[n - 2..],
            &["--".to_string(), "vpn.example.com".to_string()]
        );
    }
}
