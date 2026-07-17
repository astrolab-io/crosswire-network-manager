#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Remove the artifacts placed by contrib/install-prebuilt.sh (or a local
# `meson install`) from the system NetworkManager/pppd/dbus dirs. This is the
# counterpart to install-prebuilt.sh and derives the same paths, so the two
# stay in sync.
#
#   sudo bash contrib/uninstall.sh            # remove + restart NetworkManager
#   bash contrib/uninstall.sh --dry-run       # just list what would be removed
#
# Note: existing NM VPN *connections* you created are left untouched (delete
# them with `nmcli connection delete <name>` if you want them gone too).
set -euo pipefail

dry_run=0
for arg in "$@"; do
    case "$arg" in
        -n|--dry-run) dry_run=1 ;;
        -h|--help) sed -n '4,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg (try --dry-run)" >&2; exit 2 ;;
    esac
done

if [ "$dry_run" -eq 0 ] && [ "${EUID:-$(id -u)}" -ne 0 ]; then
    echo "This script removes files from system directories; run with sudo (or use --dry-run)." >&2
    exit 1
fi

ARCH="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || echo x86_64-linux-gnu)"

# Same locations install-prebuilt.sh writes; the .name descriptor is listed for
# both the prebuilt dir (/usr/lib/NetworkManager/VPN) and the meson/sysconfdir
# dir (/etc/NetworkManager/VPN) so this cleans up either install path.
TARGETS=(
    "/usr/lib/$ARCH/NetworkManager/libnm-vpn-plugin-crosswire.so"
    "/usr/lib/$ARCH/NetworkManager/libnm-vpn-plugin-crosswire-editor.so"
    "/usr/lib/$ARCH/NetworkManager/libnm-gtk4-vpn-plugin-crosswire-editor.so"
    "/usr/lib/pppd/nm-crosswire-pppd-plugin.so"
    "/usr/libexec/nm-crosswire-service"
    "/usr/libexec/nm-crosswire-cert-dialog"
    "/usr/sbin/crosswire"
    "/usr/lib/NetworkManager/VPN/nm-crosswire-service.name"
    "/etc/NetworkManager/VPN/nm-crosswire-service.name"
    "/usr/share/dbus-1/system.d/nm-crosswire-service.conf"
)

removed=0
echo "Removing artifacts…"
for f in "${TARGETS[@]}"; do
    if [ -e "$f" ]; then
        if [ "$dry_run" -eq 1 ]; then
            echo "  would remove  $f"
        else
            rm -f "$f" && echo "  removed       $f"
        fi
        removed=$((removed + 1))
    else
        echo "  absent        $f"
    fi
done

if [ "$dry_run" -eq 1 ]; then
    echo
    echo "Dry run: $removed file(s) would be removed. Re-run with sudo to apply."
    exit 0
fi

if [ "$removed" -eq 0 ]; then
    echo
    echo "Nothing to remove — no artifacts were installed."
    exit 0
fi

echo "Reloading NetworkManager…"
systemctl restart NetworkManager

echo
echo "Removed $removed file(s). Verify the plugin is gone with:  bash contrib/verify.sh"
