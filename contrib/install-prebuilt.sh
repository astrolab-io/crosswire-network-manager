#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Install the already-built artifacts into the system NetworkManager/pppd dirs.
# No apt/meson needed: the .so files were compiled against this machine's own
# library versions, and gtk4/libnm/glib/ppp runtimes are already present.
#
#   sudo bash contrib/install-prebuilt.sh
#
# Build the artifacts first (if not present):
#   cargo build --release --manifest-path service/Cargo.toml   # service
#   cargo build --release --manifest-path ../crosswire/Cargo.toml
#   # editor + pppd .so: see README (meson) or the manual gcc lines below.
set -euo pipefail

if [ "${EUID:-$(id -u)}" -ne 0 ]; then
    echo "This script installs into system directories; run with sudo." >&2
    exit 1
fi

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CROSSWIRE="${CROSSWIRE_REPO:-$REPO/../crosswire}"
ARCH="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || echo x86_64-linux-gnu)"

NM_PLUGIN_DIR="/usr/lib/$ARCH/NetworkManager"
NM_NAME_DIR="/usr/lib/NetworkManager/VPN"
LIBEXEC="/usr/libexec"
DBUS_DIR="/usr/share/dbus-1/system.d"
PPPD_PLUGIN="/usr/lib/pppd/nm-crosswire-pppd-plugin.so"   # matches service default

# The editor UI is split like the stock NM plugins: a GTK-agnostic core plugin
# (the [libnm] plugin=) plus a per-toolkit editor the core dlopens. At least one
# editor must be present; install whichever were built.
CORE="libnm-vpn-plugin-crosswire.so"
EDITOR_GTK3="libnm-vpn-plugin-crosswire-editor.so"
EDITOR_GTK4="libnm-gtk4-vpn-plugin-crosswire-editor.so"

need() { [ -f "$1" ] || { echo "missing artifact: $1 (build it first)" >&2; exit 1; }; }
need "$REPO/build/$CORE"
need "$REPO/build/nm-crosswire-pppd-plugin.so"
need "$REPO/service/target/release/nm-crosswire-service"
need "$CROSSWIRE/target/release/crosswire"
if [ ! -f "$REPO/build/$EDITOR_GTK3" ] && [ ! -f "$REPO/build/$EDITOR_GTK4" ]; then
    echo "missing artifact: no editor .so ($EDITOR_GTK3 or $EDITOR_GTK4) — build one first" >&2
    exit 1
fi

echo "Installing artifacts…"
install -Dm755 "$REPO/build/$CORE" "$NM_PLUGIN_DIR/$CORE"
installed_editors=""
for e in "$EDITOR_GTK3" "$EDITOR_GTK4"; do
    if [ -f "$REPO/build/$e" ]; then
        install -Dm755 "$REPO/build/$e" "$NM_PLUGIN_DIR/$e"
        installed_editors="$installed_editors  $NM_PLUGIN_DIR/$e"$'\n'
    fi
done
install -Dm755 "$REPO/build/nm-crosswire-pppd-plugin.so"   "$PPPD_PLUGIN"
install -Dm755 "$REPO/service/target/release/nm-crosswire-service" "$LIBEXEC/nm-crosswire-service"
install -Dm755 "$CROSSWIRE/target/release/crosswire"      "/usr/sbin/crosswire"
install -Dm644 "$REPO/data/nm-crosswire-service.conf"      "$DBUS_DIR/nm-crosswire-service.conf"

# Generate the .name descriptor with real paths. The template declares the
# editor plugin under [libnm] (modern) so nm-connection-editor loads the config
# UI; secrets are handled inline via external-ui-mode (no auth-dialog binary).
mkdir -p "$NM_NAME_DIR"
sed -e "s|@LIBEXECDIR@|$LIBEXEC|g" \
    -e "s|@PLUGINDIR@|$NM_PLUGIN_DIR|g" \
    "$REPO/data/nm-crosswire-service.name.in" > "$NM_NAME_DIR/nm-crosswire-service.name"

# Replace any resident service daemon. It's D-Bus-activated and owns the bus
# name for its whole lifetime, so an old process would keep serving the stale
# binary and NM would never launch the one we just installed.
if pkill -f "$LIBEXEC/nm-crosswire-service" 2>/dev/null; then
    echo "Stopped the previous nm-crosswire-service so the new binary is used."
fi

echo "Reloading NetworkManager…"
systemctl restart NetworkManager

cat <<EOF

Installed:
  $NM_PLUGIN_DIR/$CORE
${installed_editors}  $PPPD_PLUGIN
  $LIBEXEC/nm-crosswire-service
  /usr/sbin/crosswire
  $NM_NAME_DIR/nm-crosswire-service.name
  $DBUS_DIR/nm-crosswire-service.conf

Verify with:     bash contrib/verify.sh
Uninstall with:  sudo bash contrib/uninstall.sh
EOF
