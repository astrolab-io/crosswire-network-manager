<h1 align="center">CrossWire for NetworkManager</h1>

<p align="center">
  <strong>A NetworkManager VPN plugin that drives <a href="https://github.com/astrolab-io/crosswire">CrossWire</a> from your desktop.</strong><br>
  <em>Connect FortiGate SSL-VPN from the GNOME/KDE network menu — with browser SSO, split-tunnel, and split-DNS.</em>
</p>

<p align="center">
  <a href="https://www.gnu.org/licenses/gpl-3.0"><img alt="License: GPL v3" src="https://img.shields.io/badge/License-GPLv3-blue.svg"></a>
  <img alt="Status: beta" src="https://img.shields.io/badge/status-beta-orange.svg">
</p>

---

This plugin makes [CrossWire](https://github.com/astrolab-io/crosswire) a
first-class NetworkManager VPN type: configure and connect from the GNOME network
menu, the KDE Plasma applet, `nm-connection-editor`, or `nmcli`. Write one NM
plugin and every NM-based desktop picks it up — exactly how
`NetworkManager-fortisslvpn` wraps `openfortivpn`.

It has been verified end-to-end against a live FortiGate SAML gateway: browser
SSO, PPP, correct address/route/DNS hand-off to NetworkManager, split-tunnel
routing identical to the CrossWire CLI, and a stable connection.

## How it fits together

```
GNOME Settings / nm-connection-editor / plasma-nm
        │  dlopen()  →  libnm-vpn-plugin-crosswire.so   (core, no GTK)
        │                 → loads the GTK3 or GTK4 editor for the host
        ▼                   renders the config form, declares required secrets
NetworkManager (root) ── D-Bus ──▶ nm-crosswire-service   (Rust + zbus)
   Connect / NeedSecrets / Disconnect        │  implements org.freedesktop.NetworkManager.VPN.Plugin
   ◀── StateChanged / Config / Ip4Config ────┤  spawns crosswire with --pppd-plugin
                     ▲ SetConfig/SetIp4Config │
                     │  (D-Bus, at ip-up)     ▼
       nm-crosswire-pppd-plugin.so  ◀── loaded by ── crosswire's pppd  ⇄  FortiGate
```

| Artifact | Source | Role |
|---|---|---|
| `nm-crosswire-service` | `service/` (Rust) | D-Bus VPN service; owns the connection **state machine**, spawns/supervises `crosswire`, and re-emits IP config/state to NM |
| `libnm-vpn-plugin-crosswire.so` | `properties/` (C, no GTK) | Core editor plugin (the `.name`'s `[libnm] plugin=`); detects the host's GTK version and `dlopen`s the matching editor |
| `libnm-vpn-plugin-crosswire-editor.so` | `properties/` (C, GTK3) | Config form for GTK3 hosts (`nm-connection-editor`) |
| `libnm-gtk4-vpn-plugin-crosswire-editor.so` | `properties/` (C, GTK4) | Config form for GTK4 hosts (GNOME Settings) |
| `nm-crosswire-pppd-plugin.so` | `pppd-plugin/` (C) | pppd plugin; at ip-up reports address/DNS/routes to the service |
| `nm-crosswire-service.name` / `.conf` | `data/` | NM plugin descriptor + D-Bus system policy |

## Design: NetworkManager owns the network

The plugin hands the tunnel's address, routes, and DNS **to NetworkManager** and
lets it apply them (that's what integrates split-DNS and connection state with the
rest of the desktop). CrossWire is run with `--set-routes false --set-dns false
--set-ip false`, and the pppd plugin reports the negotiated config back over
D-Bus. The service adds the one thing pppd can't know — a **host route to the VPN
server** so the tunnel doesn't route its own transport into itself.

Split routes and DNS come from the gateway's config response, not PPP, so
CrossWire passes them to the pppd plugin through the **`CROSSWIRE_*` environment
contract** (documented in
[`pppd-plugin/CROSSWIRE_ENV_CONTRACT.md`](pppd-plugin/CROSSWIRE_ENV_CONTRACT.md)),
tested on both sides. The connection state machine emits `Started` only after a
real ip-up, so the desktop's "connected" indicator reflects reality.

## Install

### From a release (`.deb` / `.rpm`)

The packages are **self-contained** — they bundle the `crosswire` binary, so no
extra repo or second package is needed. Pick the build matching your system's
pppd series (`pppd --version`): `pppd2.4` (pop-os, Debian 12, Ubuntu 22.04) or
`pppd2.5` (Ubuntu 24.04+, Debian 13) — a pppd plugin only loads into the version
it was built against.

```sh
sudo apt install ./network-manager-crosswire_*-pppd2.4_amd64.deb   # Debian/Ubuntu (pppd 2.4)
sudo dnf install ./network-manager-crosswire-*-pppd2.5.x86_64.rpm   # Fedora/RHEL (pppd 2.5)
```

(Once CrossWire is in official distro repos, this package will instead depend on
the separate `crosswire` package rather than bundling it.)

### Prebuilt (local, no meson/apt)

If the `.so`s and binaries are already built (see **Build**), install them into
the system NM/pppd/dbus directories:

```sh
sudo bash contrib/install-prebuilt.sh    # installs + restarts NetworkManager
bash contrib/verify.sh                    # confirms NM recognises the VPN type
sudo bash contrib/uninstall.sh            # remove again (--dry-run to preview)
```

### From source (meson)

```sh
meson setup build \
    -Dpppd_include=/usr/include \
    -Dpppd_plugin_dir=/usr/lib/pppd/$(pppd --version 2>&1 | awk '{print $NF}')
meson compile -C build
meson test    -C build           # unit tests (env-contract parsers, …)
sudo meson install -C build
```

Build deps: `meson`, `ninja`, a C compiler, `libnm-dev`, `libglib2.0-dev`,
`libgtk-3-dev` and/or `libgtk-4-dev` (each toolkit's editor is optional), pppd
headers, and a Rust toolchain for the service. `.deb`/`.rpm` packages are
attached to each [release](https://github.com/astrolab-io/crosswire-network-manager/releases).

CrossWire itself must also be installed (`/usr/sbin/crosswire`) — see the
[CrossWire repo](https://github.com/astrolab-io/crosswire).

## Usage

1. **Add a VPN** in GNOME Settings / `nm-connection-editor` and pick **CrossWire**.
2. Fill in the gateway and pick an **authentication** method (Username/Password,
   SAML/SSO, or Session cookie). The form shows only the fields each method needs.
3. Connect. For **SSO**, your default browser opens in your session automatically
   (via `systemd-logind`), so an existing IdP session logs you in directly.

### Import a FortiClient profile

**Add → "Import a saved VPN configuration…"** and choose a FortiClient XML export
(`*.conf`). The Fortinet importer maps the first SSL-VPN connection to
gateway/port/auth-type/user (SSO → SAML).

### Certificate pinning

If your gateway omits its intermediate certificate (common with FortiGate),
set **Trusted cert (SHA256)** in the editor to the leaf's digest —
`echo | openssl s_client -connect host:443 2>/dev/null | openssl x509 -noout -fingerprint -sha256`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The service is Rust (`cargo test`); the
plugin/editors and pppd plugin are C (`meson test`). The `CROSSWIRE_*` env format
is a cross-repo contract — keep both sides' tests in sync when changing it.

## License

`GPL-3.0-or-later`, matching CrossWire and NetworkManager. Every source file
carries an `SPDX-License-Identifier` header; see [`LICENSE`](LICENSE).
