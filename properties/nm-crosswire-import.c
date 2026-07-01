/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Provider-agnostic import dispatcher. Reads a config file and delegates to the
 * first registered provider importer that recognises it (see
 * nm-crosswire-import.h).
 */

#include "config.h"
#include <string.h>
#include <NetworkManager.h>

#include "nm-crosswire-editor-plugin.h"
#include "nm-crosswire-import.h"

static const CrosswireImporter *const importers[] = {
	&crosswire_importer_fortinet,
};

NMConnection *
crosswire_import_from_file(const char *path, GError **error)
{
	char *data = NULL;
	gsize len = 0;
	if (!g_file_get_contents(path, &data, &len, error))
		return NULL;

	const CrosswireImporter *imp = NULL;
	for (guint i = 0; i < G_N_ELEMENTS(importers); i++) {
		if (importers[i]->sniff(data, len)) {
			imp = importers[i];
			break;
		}
	}
	if (!imp) {
		g_set_error_literal(error, NM_VPN_PLUGIN_ERROR, NM_VPN_PLUGIN_ERROR_FAILED,
		                    "unrecognised VPN config format (no matching provider importer)");
		g_free(data);
		return NULL;
	}

	NMSettingVpn *s_vpn = NM_SETTING_VPN(nm_setting_vpn_new());
	g_object_set(s_vpn, NM_SETTING_VPN_SERVICE_TYPE, NM_DBUS_SERVICE_CROSSWIRE, NULL);
	nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_PROVIDER, imp->provider);

	char *id = NULL;
	gboolean ok = imp->parse(data, len, s_vpn, &id, error);
	g_free(data);
	if (!ok) {
		g_object_unref(s_vpn);
		g_free(id);
		return NULL;
	}

	NMConnection *connection = nm_simple_connection_new();
	NMSettingConnection *s_con = NM_SETTING_CONNECTION(nm_setting_connection_new());
	char *uuid = g_uuid_string_random();
	g_object_set(s_con,
	             NM_SETTING_CONNECTION_ID,   (id && *id) ? id : "crosswire",
	             NM_SETTING_CONNECTION_TYPE, NM_SETTING_VPN_SETTING_NAME,
	             NM_SETTING_CONNECTION_UUID, uuid,
	             NULL);
	g_free(uuid);
	g_free(id);

	nm_connection_add_setting(connection, NM_SETTING(s_con));
	nm_connection_add_setting(connection, NM_SETTING(s_vpn));
	return connection;
}
