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
#   meson setup build -Dpppd_include=/usr/include \
#       -Dpppd_plugin_dir=/usr/lib/pppd/$(pppd --version 2>&1 | awk '{print $NF}')
#   meson compile -C build                                     # editors, pppd .so, service
#   cargo build --release --manifest-path ../crosswire/Cargo.toml   # the crosswire binary
#
# It resolves each artifact across the layouts we produce: a meson build nests
# them (build/properties, build/pppd-plugin) with the service in build/, while a
# manual gcc build may drop them flat in build/ — both are found.
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
PPPD_SO="nm-crosswire-pppd-plugin.so"
CERT_DIALOG_BIN="nm-crosswire-cert-dialog"
BUILD="$REPO/build"

# Resolve a built artifact by basename across the candidate dirs; print the
# first hit, empty if none. Meson nests outputs per subdir; a flat build/ from a
# manual gcc build is also searched.
locate() {
    local name="$1"; shift
    local d
    for d in "$@"; do
        if [ -f "$d/$name" ]; then printf '%s\n' "$d/$name"; return 0; fi
    done
    return 1
}

core_path="$(locate "$CORE" "$BUILD/properties" "$BUILD" || true)"
editor3_path="$(locate "$EDITOR_GTK3" "$BUILD/properties" "$BUILD" || true)"
editor4_path="$(locate "$EDITOR_GTK4" "$BUILD/properties" "$BUILD" || true)"
pppd_path="$(locate "$PPPD_SO" "$BUILD/pppd-plugin" "$BUILD" || true)"
certdlg_path="$(locate "$CERT_DIALOG_BIN" "$BUILD/properties" "$BUILD" || true)"
# meson copies the service to build/; a bare cargo build leaves it under target/.
service_path="$(locate "nm-crosswire-service" "$BUILD" "$REPO/service/target/release" || true)"
crosswire_path="$(locate "crosswire" "$CROSSWIRE/target/release" || true)"

need() { [ -n "$1" ] && [ -f "$1" ] || { echo "missing artifact: $2 (build it first)" >&2; exit 1; }; }
need "$core_path"      "$CORE"
need "$pppd_path"      "$PPPD_SO"
need "$service_path"   "nm-crosswire-service"
if [ -z "$editor3_path" ] && [ -z "$editor4_path" ]; then
    echo "missing artifact: no editor .so ($EDITOR_GTK3 or $EDITOR_GTK4) — build one first" >&2
    exit 1
fi
# crosswire lives in a separate repo. If it isn't built but is already installed,
# keep the existing one (it lets you iterate on just the NM plugin); only insist
# on building it for a first-time install.
INSTALLED_CROSSWIRE="/usr/sbin/crosswire"
if [ -z "$crosswire_path" ]; then
    if [ -x "$INSTALLED_CROSSWIRE" ]; then
        echo "crosswire not built — keeping the installed $INSTALLED_CROSSWIRE."
    else
        echo "missing artifact: crosswire (build it: cargo build --release --manifest-path $CROSSWIRE/Cargo.toml)" >&2
        exit 1
    fi
fi

echo "Installing artifacts…"
install -Dm755 "$core_path" "$NM_PLUGIN_DIR/$CORE"
installed_editors=""
for e in "$editor3_path" "$editor4_path"; do
    [ -n "$e" ] || continue
    base="$(basename "$e")"
    install -Dm755 "$e" "$NM_PLUGIN_DIR/$base"
    installed_editors="$installed_editors  $NM_PLUGIN_DIR/$base"$'\n'
done
install -Dm755 "$pppd_path"    "$PPPD_PLUGIN"
install -Dm755 "$service_path" "$LIBEXEC/nm-crosswire-service"
if [ -n "$crosswire_path" ]; then
    install -Dm755 "$crosswire_path" "$INSTALLED_CROSSWIRE"
fi
install -Dm644 "$REPO/data/nm-crosswire-service.conf" "$DBUS_DIR/nm-crosswire-service.conf"

# Native cert-trust dialog (GTK3): shown when the gateway's pinned certificate
# changes. Installed next to the service so its default sibling path resolves.
# Optional — only present if a GTK3 toolkit was available at build time.
installed_cert_dialog=""
if [ -n "$certdlg_path" ]; then
    install -Dm755 "$certdlg_path" "$LIBEXEC/nm-crosswire-cert-dialog"
    installed_cert_dialog="  $LIBEXEC/nm-crosswire-cert-dialog"$'\n'
fi

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
${installed_cert_dialog}  $INSTALLED_CROSSWIRE
  $NM_NAME_DIR/nm-crosswire-service.name
  $DBUS_DIR/nm-crosswire-service.conf

Verify with:     bash contrib/verify.sh
Uninstall with:  sudo bash contrib/uninstall.sh
EOF
