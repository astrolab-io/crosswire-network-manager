# CROSSWIRE_* environment contract

crosswire and the NetworkManager pppd plugin live in separate repos but must
agree on how the gateway's network parameters cross the process boundary. Those
parameters (split routes, DNS) come from the gateway's **config response**, not
from PPP/IPCP, so the pppd plugin can't discover them on its own. crosswire
therefore sets them on the **pppd child's environment** (via `execve` in
`spawn_pppd`), and the plugin reads them at ip-up.

## Variables

| Variable | Format | Meaning |
|---|---|---|
| `CROSSWIRE_ROUTES` | `dest/prefix,dest/prefix,…` | Split-tunnel routes. **Empty ⇒ full-tunnel** (plugin adds none; NM installs a default). |
| `CROSSWIRE_DNS` | `a.b.c.d,a.b.c.d,…` | DNS servers. Empty ⇒ plugin falls back to any IPCP `ms-dns`. |
| `CROSSWIRE_DNS_SUFFIX` | a single domain, or unset | Search domain, only when the gateway sent one. |

All are always set except `CROSSWIRE_DNS_SUFFIX`, which is omitted when there is
no suffix. Values are IPv4 dotted-quad; malformed entries are skipped by the
consumer.

## What the plugin does with them

- `CROSSWIRE_ROUTES` → NM `Ip4Config` `routes` (`aau`: `[network, prefix,
  next-hop=0, metric=0]`) plus `never-default = true`, so NM installs exactly
  these instead of a blanket default.
- `CROSSWIRE_DNS` → NM `Ip4Config` `dns` (`au`).
- `CROSSWIRE_DNS_SUFFIX` → NM `Ip4Config` `domain`.

NM still applies the *connection's* own `ipv4.*` policy on top (e.g.
`never-default`, `ignore-auto-routes`, `ignore-auto-dns`, `dns-search`), so a
user override always wins — we only supply the gateway-provided defaults.

## Golden fixture (tested on both sides)

```
CROSSWIRE_ROUTES=10.0.0.0/8,172.16.0.0/12,192.168.1.0/24
CROSSWIRE_DNS=172.31.13.10,172.31.13.11
CROSSWIRE_DNS_SUFFIX=corp.local
```

- **Producer** (crosswire): `transport::ppp::tests::pppd_envp_encodes_*` asserts
  `pppd_envp()` emits exactly these strings for the matching `TunnelParams`.
- **Consumer** (this plugin): `test-netparams` asserts `nmo_parse_routes` /
  `nmo_parse_dns` decode them back to the same values.

Keep the two tests' fixtures in sync when changing the format.
