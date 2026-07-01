/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Fortinet provider importer: parses a FortiClient XML configuration export
 * (<forticlient_configuration>) and extracts the first SSL-VPN connection —
 * server, name, username and SSO flag — into crosswire's vpn.data keys.
 *
 * We only read the vpn/sslvpn/connections/connection subtree; the rest of the
 * FortiClient profile (proxy, update, ipsec, logging, encrypted blobs) is
 * irrelevant to an crosswire SSL-VPN connection and deliberately ignored.
 */

#include "config.h"
#include <string.h>
#include <NetworkManager.h>

#include "nm-crosswire-editor-plugin.h"
#include "nm-crosswire-import.h"

static gboolean
fortinet_sniff(const char *data, gsize len)
{
	return g_strstr_len(data, len, "<forticlient_configuration") != NULL;
}

/* Which direct child of <connection> we're currently reading text for. */
typedef enum {
	F_NONE,
	F_NAME,
	F_DESCRIPTION,
	F_SERVER,
	F_USERNAME,
	F_SSO,
} FField;

typedef struct {
	gboolean in_sslvpn;      /* inside <sslvpn> (not <ipsecvpn>) */
	gboolean in_connection;  /* inside a <sslvpn>…<connection> */
	gboolean done;           /* already captured the first sslvpn connection */
	FField   reading;        /* current leaf being accumulated */
	GString *text;           /* accumulator for the current leaf */

	char    *name;
	char    *description;
	char    *server;
	char    *username;
	gboolean sso;
} FParse;

static FField
field_for(const char *name)
{
	if (!strcmp(name, "name"))        return F_NAME;
	if (!strcmp(name, "description")) return F_DESCRIPTION;
	if (!strcmp(name, "server"))      return F_SERVER;
	if (!strcmp(name, "username"))    return F_USERNAME;
	if (!strcmp(name, "sso_enabled")) return F_SSO;
	return F_NONE;
}

static void
start_element(GMarkupParseContext *ctx, const char *name,
              const char **attr_names, const char **attr_values,
              gpointer user_data, GError **error)
{
	(void) ctx; (void) attr_names; (void) attr_values; (void) error;
	FParse *p = user_data;

	if (!strcmp(name, "sslvpn")) {
		p->in_sslvpn = TRUE;
	} else if (p->in_sslvpn && !p->done && !strcmp(name, "connection")) {
		p->in_connection = TRUE;
	} else if (p->in_connection) {
		/* Only direct-child leaves carry the fields we want; nested subtrees
		 * (ui, certificate, azure_auto_login, on_connect) don't reuse these
		 * element names, so a name match inside <connection> is unambiguous. */
		FField f = field_for(name);
		if (f != F_NONE) {
			p->reading = f;
			g_string_set_size(p->text, 0);
		}
	}
}

static void
text_cb(GMarkupParseContext *ctx, const char *text, gsize len,
        gpointer user_data, GError **error)
{
	(void) ctx; (void) error;
	FParse *p = user_data;
	if (p->reading != F_NONE)
		g_string_append_len(p->text, text, len);
}

static void
commit_leaf(FParse *p)
{
	char *val = g_strdup(p->text->str);
	g_strstrip(val);

	switch (p->reading) {
	case F_NAME:        g_free(p->name);        p->name = val;        break;
	case F_DESCRIPTION: g_free(p->description); p->description = val; break;
	case F_SERVER:      g_free(p->server);      p->server = val;      break;
	case F_USERNAME:    g_free(p->username);    p->username = val;    break;
	case F_SSO:         p->sso = (!strcmp(val, "1") || !g_ascii_strcasecmp(val, "true")); g_free(val); break;
	default:            g_free(val);            break;
	}
	p->reading = F_NONE;
}

static void
end_element(GMarkupParseContext *ctx, const char *name, gpointer user_data, GError **error)
{
	(void) ctx; (void) error;
	FParse *p = user_data;

	if (p->reading != F_NONE)
		commit_leaf(p);

	if (p->in_connection && !strcmp(name, "connection")) {
		p->in_connection = FALSE;
		if (p->server && *p->server)
			p->done = TRUE;   /* first SSL-VPN connection wins */
	} else if (!strcmp(name, "sslvpn")) {
		p->in_sslvpn = FALSE;
	}
}

/* Split "host:port" → gateway (+ port if the tail is all digits). */
static void
apply_server(NMSettingVpn *s_vpn, const char *server)
{
	const char *colon = strrchr(server, ':');
	if (colon && colon[1]) {
		gboolean digits = TRUE;
		for (const char *c = colon + 1; *c; c++)
			if (!g_ascii_isdigit(*c)) { digits = FALSE; break; }
		if (digits) {
			char *host = g_strndup(server, colon - server);
			nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_GATEWAY, host);
			nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_PORT, colon + 1);
			g_free(host);
			return;
		}
	}
	nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_GATEWAY, server);
}

static gboolean
fortinet_parse(const char *data, gsize len, NMSettingVpn *s_vpn,
               char **out_id, GError **error)
{
	static const GMarkupParser parser = {
		.start_element = start_element,
		.end_element   = end_element,
		.text          = text_cb,
	};
	FParse p = { .text = g_string_new(NULL) };

	GMarkupParseContext *ctx =
		g_markup_parse_context_new(&parser, 0, &p, NULL);
	gboolean ok = g_markup_parse_context_parse(ctx, data, len, error)
	           && g_markup_parse_context_end_parse(ctx, error);
	g_markup_parse_context_free(ctx);
	g_string_free(p.text, TRUE);

	if (ok && !(p.server && *p.server)) {
		g_set_error_literal(error, NM_VPN_PLUGIN_ERROR, NM_VPN_PLUGIN_ERROR_FAILED,
		                    "no SSL-VPN connection found in FortiClient config");
		ok = FALSE;
	}

	if (ok) {
		apply_server(s_vpn, p.server);

		/* SSO → SAML; otherwise password. Username is only meaningful for
		 * password auth (SAML identity is established at the IdP). */
		const char *auth = p.sso ? "saml" : "password";
		nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_AUTH_TYPE, auth);
		if (!p.sso && p.username && *p.username)
			nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_USER, p.username);

		*out_id = (p.name && *p.name) ? g_strdup(p.name)
		        : (p.description && *p.description) ? g_strdup(p.description)
		        : NULL;
	}

	g_free(p.name);
	g_free(p.description);
	g_free(p.server);
	g_free(p.username);
	return ok;
}

const CrosswireImporter crosswire_importer_fortinet = {
	.provider = "fortinet",
	.label    = "FortiClient XML",
	.sniff    = fortinet_sniff,
	.parse    = fortinet_parse,
};
