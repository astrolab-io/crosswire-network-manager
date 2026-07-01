/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The NMVpnEditorPlugin GObject that nm-connection-editor / GNOME Control
 * Center / plasma-nm dlopen via nm_vpn_editor_plugin_factory(). It advertises
 * capabilities and hands back an NMVpnEditor (the config form).
 *
 * Skeleton adapted from the standard libnm VPN-plugin pattern (cf.
 * NetworkManager-fortisslvpn/properties). Import/export are left as TODOs.
 */

#ifndef _GNU_SOURCE
#define _GNU_SOURCE   /* dladdr */
#endif
#include "config.h"
#include <string.h>
#include <dlfcn.h>
#include <gmodule.h>
#include <NetworkManager.h>
#include <nm-vpn-editor-plugin.h>

#include "nm-crosswire-editor-plugin.h"
#include "nm-crosswire-import.h"

static void crosswire_editor_plugin_interface_init(NMVpnEditorPluginInterface *iface);

struct _CrosswireEditorPlugin {
	GObject parent;
};

G_DEFINE_TYPE_EXTENDED(CrosswireEditorPlugin, crosswire_editor_plugin, G_TYPE_OBJECT, 0,
	G_IMPLEMENT_INTERFACE(NM_TYPE_VPN_EDITOR_PLUGIN,
	                      crosswire_editor_plugin_interface_init))

enum { PROP_0, PROP_NAME, PROP_DESC, PROP_SERVICE };

static guint32
get_capabilities(NMVpnEditorPlugin *plugin)
{
	/* Advertise import so nm-connection-editor offers "Import a saved VPN
	 * configuration" and calls import_from_file(). Export is not implemented
	 * yet; no IPv6 tunnel support. */
	return NM_VPN_EDITOR_PLUGIN_CAPABILITY_IMPORT;
}

/* Locate an editor module beside this core plugin (found via dladdr), so it
 * resolves regardless of install prefix; fall back to the build-time dir. */
static char *
editor_module_path(const char *module)
{
	Dl_info info;
	if (dladdr((gpointer) editor_module_path, &info) && info.dli_fname && *info.dli_fname) {
		char *dir  = g_path_get_dirname(info.dli_fname);
		char *path = g_build_filename(dir, module, NULL);
		g_free(dir);
		return path;
	}
	return g_build_filename(PLUGINDIR, module, NULL);
}

/* The editor form is GTK-toolkit-specific, but this core plugin is not linked
 * against GTK so it can load into either a GTK3 (nm-connection-editor) or GTK4
 * (GNOME Settings) host. We detect the host's toolkit — gtk_container_add is a
 * GTK3-only symbol, dropped in GTK4 — and dlopen the matching editor module
 * from our own plugin dir, then call its nm_vpn_editor_factory_crosswire(). */
static NMVpnEditor *
get_editor(NMVpnEditorPlugin *plugin, NMConnection *connection, GError **error)
{
	gpointer gtk3_symbol = NULL;
	GModule *self = g_module_open(NULL, G_MODULE_BIND_LAZY);
	if (self) {
		g_module_symbol(self, "gtk_container_add", &gtk3_symbol);
		g_module_close(self);
	}

	const char *module = gtk3_symbol
		? "libnm-vpn-plugin-crosswire-editor.so"
		: "libnm-gtk4-vpn-plugin-crosswire-editor.so";
	char *path = editor_module_path(module);

	GModule *m = g_module_open(path, G_MODULE_BIND_LOCAL | G_MODULE_BIND_LAZY);
	g_free(path);
	if (!m) {
		g_set_error(error, NM_VPN_PLUGIN_ERROR, NM_VPN_PLUGIN_ERROR_FAILED,
		            "could not load editor module %s: %s", module, g_module_error());
		return NULL;
	}

	NMVpnEditor *(*factory)(NMConnection *, GError **) = NULL;
	if (!g_module_symbol(m, "nm_vpn_editor_factory_crosswire", (gpointer *) &factory) || !factory) {
		g_set_error(error, NM_VPN_PLUGIN_ERROR, NM_VPN_PLUGIN_ERROR_FAILED,
		            "editor module %s lacks nm_vpn_editor_factory_crosswire", module);
		g_module_close(m);
		return NULL;
	}

	/* Keep the module resident for the process lifetime — the returned editor's
	 * code lives in it. */
	g_module_make_resident(m);
	return factory(connection, error);
}

static NMConnection *
import_from_file(NMVpnEditorPlugin *plugin, const char *path, GError **error)
{
	return crosswire_import_from_file(path, error);
}

static gboolean
export_to_file(NMVpnEditorPlugin *plugin, const char *path,
               NMConnection *connection, GError **error)
{
	/* TODO: emit an /etc/crosswire/config file from the connection. */
	g_set_error_literal(error, NM_VPN_PLUGIN_ERROR,
	                    NM_VPN_PLUGIN_ERROR_FAILED,
	                    "export not implemented yet");
	return FALSE;
}

static void
get_property(GObject *obj, guint prop_id, GValue *value, GParamSpec *pspec)
{
	switch (prop_id) {
	case PROP_NAME:    g_value_set_string(value, "CrossWire"); break;
	case PROP_DESC:    g_value_set_string(value, "Generic PPP-over-TLS VPN client (FortiGate provider)."); break;
	case PROP_SERVICE: g_value_set_string(value, NM_DBUS_SERVICE_CROSSWIRE); break;
	default:           G_OBJECT_WARN_INVALID_PROPERTY_ID(obj, prop_id, pspec);
	}
}

static void
crosswire_editor_plugin_init(CrosswireEditorPlugin *plugin) {}

static void
crosswire_editor_plugin_class_init(CrosswireEditorPluginClass *klass)
{
	GObjectClass *object_class = G_OBJECT_CLASS(klass);
	object_class->get_property = get_property;

	g_object_class_override_property(object_class, PROP_NAME,    NM_VPN_EDITOR_PLUGIN_NAME);
	g_object_class_override_property(object_class, PROP_DESC,    NM_VPN_EDITOR_PLUGIN_DESCRIPTION);
	g_object_class_override_property(object_class, PROP_SERVICE, NM_VPN_EDITOR_PLUGIN_SERVICE);
}

static void
crosswire_editor_plugin_interface_init(NMVpnEditorPluginInterface *iface)
{
	iface->get_editor       = get_editor;
	iface->get_capabilities = get_capabilities;
	iface->import_from_file = import_from_file;
	iface->export_to_file   = export_to_file;
}

/* Entry point the UI dlopen()s. */
G_MODULE_EXPORT NMVpnEditorPlugin *
nm_vpn_editor_plugin_factory(GError **error)
{
	return g_object_new(CROSSWIRE_TYPE_EDITOR_PLUGIN, NULL);
}
