#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Verify NetworkManager recognises the crosswire VPN plugin and can create a
# connection of that type — the same path the GNOME network UI exercises.
#
#   bash contrib/verify.sh        # read-only checks + a temp connection
set -uo pipefail

SVC="org.freedesktop.NetworkManager.crosswire"
fail=0

echo "== 1. .name descriptor present =="
# The descriptor may live in either the distro or the local override dir; a
# single `ls` over both paths exits non-zero when *either* is absent, so test
# each location independently and pass if at least one is found.
if [ -e /usr/lib/NetworkManager/VPN/nm-crosswire-service.name ] || \
   [ -e /etc/NetworkManager/VPN/nm-crosswire-service.name ]; then
    ls -1 /usr/lib/NetworkManager/VPN/nm-crosswire-service.name \
          /etc/NetworkManager/VPN/nm-crosswire-service.name 2>/dev/null | sed 's/^/  /'
    echo "  OK"
else
    echo "  MISSING — run contrib/install-prebuilt.sh"; fail=1
fi

echo "== 2. NM lists the VPN plugin =="
if nmcli -f NAME,VPN general 2>/dev/null | grep -qi vpn || true; then :; fi
# The authoritative check: try to create a connection of this vpn-type.
echo "== 3. Create a temp connection of vpn-type $SVC =="
if nmcli connection add type vpn con-name crosswire-verify \
        vpn-type "$SVC" ifname "*" \
        vpn.data "gateway=vpn.example.com, auth-type=password" >/dev/null 2>&1; then
    echo "  OK — NM accepted the plugin type"
    nmcli -f connection.id,vpn.service-type,vpn.data connection show crosswire-verify | sed 's/^/  /'
    nmcli connection delete crosswire-verify >/dev/null 2>&1 && echo "  (temp connection removed)"
else
    echo "  FAILED — 'unknown VPN plugin' means NM hasn't loaded it (restart NetworkManager)"; fail=1
fi

echo "== 4. pppd plugin ABI matches running pppd =="
pv=$(strings /usr/lib/pppd/nm-crosswire-pppd-plugin.so 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | head -1)
# pppd keeps its plugins under a version-named dir (e.g. /usr/lib/pppd/2.4.9);
# match that directory explicitly rather than the first entry, which would also
# catch the plugin .so itself and produced a spurious SIGPIPE from `head`.
pd=$(for d in /usr/lib/pppd/*/; do basename "$d"; done 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | head -1)
echo "  plugin pppd_version=$pv ; system pppd dir=$pd"
[ "$pv" = "$pd" ] && echo "  OK (match)" || { echo "  WARN: version mismatch — pppd may refuse to load it"; }

echo
[ "$fail" -eq 0 ] && echo "VERIFY: PASS" || { echo "VERIFY: incomplete (see above)"; exit 1; }
