/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Provider-specific config import. crosswire is provider-neutral, but each
 * upstream client ships its own config file format, so importing a file is
 * delegated to per-provider parsers (mirroring crosswire's providers/ layout).
 * The dispatcher sniffs the file and hands it to the first importer that claims
 * it; add a provider by dropping in nm-crosswire-import-<provider>.c and
 * registering it in nm-crosswire-import.c.
 */
#ifndef __NM_CROSSWIRE_IMPORT_H__
#define __NM_CROSSWIRE_IMPORT_H__

#include <glib.h>
#include <NetworkManager.h>

typedef struct {
	const char *provider;   /* vpn.data "provider" value this yields, e.g. "fortinet" */
	const char *label;      /* human name, for diagnostics */

	/* TRUE if this importer recognises the file contents. */
	gboolean (*sniff)(const char *data, gsize len);

	/* Fill s_vpn (vpn.data / vpn.secrets) from data and set *out_id to a
	 * suggested connection name. Return FALSE + set error on malformed input or
	 * if the file carries no usable connection. */
	gboolean (*parse)(const char *data, gsize len, NMSettingVpn *s_vpn,
	                  char **out_id, GError **error);
} CrosswireImporter;

/* Read path, pick the first provider importer that recognises it, and build a
 * VPN connection. Returns NULL + error if nothing matches or parsing fails. */
NMConnection *crosswire_import_from_file(const char *path, GError **error);

/* Registered provider importers (each defined in its own translation unit). */
extern const CrosswireImporter crosswire_importer_fortinet;

#endif /* __NM_CROSSWIRE_IMPORT_H__ */
