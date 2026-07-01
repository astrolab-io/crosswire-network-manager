/* SPDX-License-Identifier: GPL-3.0-or-later */
#ifndef __NM_CROSSWIRE_EDITOR_PLUGIN_H__
#define __NM_CROSSWIRE_EDITOR_PLUGIN_H__

#include <glib-object.h>
#include <NetworkManager.h>

#define CROSSWIRE_TYPE_EDITOR_PLUGIN (crosswire_editor_plugin_get_type())
G_DECLARE_FINAL_TYPE(CrosswireEditorPlugin, crosswire_editor_plugin,
                     CROSSWIRE, EDITOR_PLUGIN, GObject)

/* Toolkit-specific factory exported by each editor module
 * (libnm[-gtk4]-vpn-plugin-crosswire-editor.so) and resolved by the core
 * plugin's get_editor() after it detects the host's GTK version. */
NMVpnEditor *nm_vpn_editor_factory_crosswire(NMConnection *connection, GError **error);

/* Connection service-type this plugin handles (must match the .name file). */
#define NM_DBUS_SERVICE_CROSSWIRE "org.freedesktop.NetworkManager.crosswire"

/* vpn.data / vpn.secrets keys shared with the service's config.rs mapping. */
#define NM_CROSSWIRE_KEY_PROVIDER     "provider"    /* fortinet (extensible) */
#define NM_CROSSWIRE_KEY_GATEWAY      "gateway"
#define NM_CROSSWIRE_KEY_PORT         "port"
#define NM_CROSSWIRE_KEY_USER         "user"
#define NM_CROSSWIRE_KEY_REALM        "realm"
#define NM_CROSSWIRE_KEY_AUTH_TYPE    "auth-type"   /* password | saml | cookie */
#define NM_CROSSWIRE_KEY_CA_FILE      "ca-file"
#define NM_CROSSWIRE_KEY_TRUSTED_CERT "trusted-cert"
#define NM_CROSSWIRE_KEY_INSECURE     "insecure-ssl"
#define NM_CROSSWIRE_KEY_PASSWORD     "password"    /* secret */
#define NM_CROSSWIRE_KEY_OTP          "otp"         /* secret */
#define NM_CROSSWIRE_KEY_COOKIE       "cookie"      /* secret */

#endif /* __NM_CROSSWIRE_EDITOR_PLUGIN_H__ */
