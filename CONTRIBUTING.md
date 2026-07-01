# Contributing to CrossWire for NetworkManager

Thanks for helping improve the CrossWire NetworkManager plugin! Bug fixes,
toolkit/portability work, new provider importers, and docs are all welcome.

## Ground rules

- **License.** This project is `GPL-3.0-or-later`. By contributing you agree your
  work is licensed the same way. Start every new source file with an
  `SPDX-License-Identifier: GPL-3.0-or-later` header.
- **Security issues:** report privately (see the repository's Security policy)
  rather than in a public issue.

## Layout

- `service/` — the D-Bus VPN service (Rust). Owns the connection state machine,
  spawns/supervises CrossWire, forwards IP config to NM.
- `properties/` — the editor: a GTK-agnostic **core** plugin plus a **GTK3** and a
  **GTK4** editor built from one source, and provider config **importers**.
- `pppd-plugin/` — the pppd plugin (C) that reports the tunnel config to the
  service at ip-up, plus the pure `CROSSWIRE_*` parsers.
- `data/` — the `.name` descriptor and D-Bus policy.
- `contrib/` — local install/uninstall/verify scripts.

## Development setup

```sh
# Rust service
cargo test --manifest-path service/Cargo.toml
cargo clippy --manifest-path service/Cargo.toml --all-targets -- -D warnings

# C plugin + editors + unit tests (meson)
meson setup build -Dpppd_include=/usr/include -Dpppd_plugin_dir=/usr/lib/pppd/<ver>
meson compile -C build
meson test -C build
```

Build deps: `meson`, `ninja`, `gcc`, `libnm-dev`, `libglib2.0-dev`,
`libgtk-3-dev` and/or `libgtk-4-dev`, pppd headers, and a Rust toolchain. To try a
change live, `sudo bash contrib/install-prebuilt.sh` then reconnect; it replaces
any resident service daemon so the new binary is used.

## Making a change

1. Branch off `main`; keep commits focused with clear messages (no
   `Co-authored-by` trailers for tooling).
2. Add/adjust tests. Prefer **pure functions** for parsing and mapping — e.g. the
   `CROSSWIRE_*` parsers in `pppd-plugin/nm-crosswire-netparams.c` are covered by
   `test-netparams`; the argv/route/DNS mapping in `service/src/config.rs` by Rust
   unit tests.
3. Run the Rust and meson test suites; keep clippy `-D warnings` clean.
4. Open a PR describing the change and how you verified it (gateway/OS if it was a
   live connection).

## Adding a provider importer

Import is provider-specific. Drop a `properties/nm-crosswire-import-<provider>.c`
implementing the `CrosswireImporter` interface (sniff + parse), register it in
`nm-crosswire-import.c`, and add it to `properties/meson.build`. See the Fortinet
FortiClient-XML importer for the pattern.

## The CROSSWIRE_* contract

Split routes and DNS cross from CrossWire to this plugin via `CROSSWIRE_*`
environment variables — see
[`pppd-plugin/CROSSWIRE_ENV_CONTRACT.md`](pppd-plugin/CROSSWIRE_ENV_CONTRACT.md).
The format is tested on **both** sides (this plugin's `test-netparams` and
CrossWire's `pppd_envp` tests, sharing one golden fixture). If you change it,
update both and the contract doc.
