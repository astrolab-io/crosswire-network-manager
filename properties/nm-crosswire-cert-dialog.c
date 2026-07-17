/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Native "trust this gateway certificate?" dialog for the crosswire NM plugin.
 *
 * crosswire pins the gateway's TLS leaf by SHA-256 (vpn.data[trusted-cert]).
 * When the gateway rotates its certificate the pin no longer matches and
 * crosswire aborts the handshake *before* the SAML/login step — so the desktop
 * never even sees a browser. The root D-Bus service detects that failure,
 * recovers the presented digest, and launches this helper in the logged-in
 * user's graphical session (see service/src/user_session.rs).
 *
 * We show the new fingerprint and let the user decide. On "Trust", we persist
 * the digest into the connection's vpn.data[trusted-cert] via libnm and
 * re-activate it — so the next connect matches the pin and proceeds to SSO.
 * Everything rides on libnm + GTK, both already required by the editor plugins:
 * no new dependency, and libnm applies the settings change under the user's own
 * polkit identity, exactly as nm-connection-editor would.
 *
 * Deliberately shows only the fingerprint (the security-relevant fact), not a
 * full X.509 dump: it keeps the helper free of a TLS dependency, and the pinned
 * digest is precisely what crosswire compares.
 */

#include "config.h"
#include <gtk/gtk.h>
#include <NetworkManager.h>

static char *opt_uuid = NULL;
static char *opt_gateway = NULL;
static char *opt_digest = NULL;

static GOptionEntry entries[] = {
	{ "uuid", 0, 0, G_OPTION_ARG_STRING, &opt_uuid,
	  "NM connection UUID to re-pin and re-activate", "UUID" },
	{ "gateway", 0, 0, G_OPTION_ARG_STRING, &opt_gateway,
	  "Gateway host, for display", "HOST" },
	{ "digest", 0, 0, G_OPTION_ARG_STRING, &opt_digest,
	  "Presented leaf certificate SHA-256 (lowercase hex)", "HEX" },
	{ NULL }
};

/* Re-activation completion: report and quit the loop so the process can exit. */
static void
on_activated (GObject *source, GAsyncResult *res, gpointer user_data)
{
	GMainLoop *loop = user_data;
	GError *error = NULL;
	NMActiveConnection *ac =
		nm_client_activate_connection_finish (NM_CLIENT (source), res, &error);

	if (error) {
		g_warning ("re-activating the connection failed: %s", error->message);
		g_error_free (error);
	} else {
		g_clear_object (&ac);
	}
	g_main_loop_quit (loop);
}

/* Persist the new digest into vpn.data[trusted-cert] and re-activate. Returns
 * TRUE on a committed change (re-activation is best-effort). */
static gboolean
trust_and_reconnect (GError **error)
{
	NMClient *client = nm_client_new (NULL, error);
	if (!client)
		return FALSE;

	NMRemoteConnection *conn =
		nm_client_get_connection_by_uuid (client, opt_uuid);
	if (!conn) {
		g_set_error (error, NM_CLIENT_ERROR, NM_CLIENT_ERROR_FAILED,
		             "connection %s not found",
		             opt_uuid ? opt_uuid : "(none)");
		g_object_unref (client);
		return FALSE;
	}

	NMSettingVpn *svpn =
		nm_connection_get_setting_vpn (NM_CONNECTION (conn));
	if (!svpn) {
		g_set_error (error, NM_CLIENT_ERROR, NM_CLIENT_ERROR_FAILED,
		             "connection %s has no VPN setting", opt_uuid);
		g_object_unref (client);
		return FALSE;
	}

	/* Replace the pin with the freshly-presented leaf: the old cert is gone,
	 * so keeping it would only leave a stale, never-matching digest behind. */
	nm_setting_vpn_add_data_item (svpn, "trusted-cert", opt_digest);

	/* The synchronous commit is marked deprecated in favour of the async form,
	 * but it is exactly what we want here: a one-shot helper that must persist
	 * the pin before it re-activates. Guard just this call. */
	gboolean committed;
	G_GNUC_BEGIN_IGNORE_DEPRECATIONS
	committed = nm_remote_connection_commit_changes (conn, TRUE, NULL, error);
	G_GNUC_END_IGNORE_DEPRECATIONS
	if (!committed) {
		g_object_unref (client);
		return FALSE;
	}

	/* Kick off a fresh activation; wait for it to be accepted so the D-Bus
	 * call actually goes out before we exit. */
	GMainLoop *loop = g_main_loop_new (NULL, FALSE);
	nm_client_activate_connection_async (client, NM_CONNECTION (conn),
	                                     NULL, NULL, NULL,
	                                     on_activated, loop);
	g_main_loop_run (loop);
	g_main_loop_unref (loop);

	g_object_unref (client);
	return TRUE;
}

/* A short modal error, so a failed re-pin isn't silent. */
static void
show_error (const char *primary, const char *detail)
{
	GtkWidget *d = gtk_message_dialog_new (NULL, 0, GTK_MESSAGE_ERROR,
	                                       GTK_BUTTONS_CLOSE, "%s", primary);
	if (detail)
		gtk_message_dialog_format_secondary_text (GTK_MESSAGE_DIALOG (d),
		                                          "%s", detail);
	gtk_window_set_title (GTK_WINDOW (d), "CrossWire VPN");
	gtk_dialog_run (GTK_DIALOG (d));
	gtk_widget_destroy (d);
}

int
main (int argc, char **argv)
{
	GError *error = NULL;

	if (!gtk_init_with_args (&argc, &argv,
	                         "- trust a changed CrossWire VPN gateway certificate",
	                         entries, NULL, &error)) {
		g_printerr ("%s\n", error ? error->message : "argument error");
		g_clear_error (&error);
		return 1;
	}

	if (!opt_digest || !*opt_digest) {
		g_printerr ("no --digest given; nothing to trust\n");
		return 1;
	}

	char *gw = g_markup_escape_text (opt_gateway && *opt_gateway
	                                 ? opt_gateway : "the VPN gateway", -1);
	char *dg = g_markup_escape_text (opt_digest, -1);

	GtkWidget *dialog = gtk_message_dialog_new (
		NULL, 0, GTK_MESSAGE_WARNING, GTK_BUTTONS_NONE,
		"The VPN gateway's security certificate has changed");
	gtk_message_dialog_format_secondary_markup (
		GTK_MESSAGE_DIALOG (dialog),
		"The certificate now presented by <b>%s</b> does not match the one "
		"previously trusted, so the connection was refused.\n\n"
		"New SHA-256 fingerprint:\n<tt>%s</tt>\n\n"
		"Trust it <b>only</b> if you expected the certificate to change — for "
		"example a scheduled renewal you can confirm with your VPN "
		"administrator. If you did not expect this, cancel and verify: a "
		"changed certificate can also mean the connection is being intercepted.",
		gw, dg);
	g_free (gw);
	g_free (dg);

	gtk_dialog_add_button (GTK_DIALOG (dialog), "_Cancel", GTK_RESPONSE_CANCEL);
	GtkWidget *trust = gtk_dialog_add_button (GTK_DIALOG (dialog),
	                                          "_Trust and reconnect",
	                                          GTK_RESPONSE_ACCEPT);
	/* Cancel is the safe default; make the affirmative action deliberate. */
	gtk_dialog_set_default_response (GTK_DIALOG (dialog), GTK_RESPONSE_CANCEL);
	gtk_style_context_add_class (gtk_widget_get_style_context (trust),
	                             "destructive-action");
	gtk_window_set_title (GTK_WINDOW (dialog), "CrossWire VPN");

	gint resp = gtk_dialog_run (GTK_DIALOG (dialog));
	gtk_widget_destroy (dialog);

	if (resp != GTK_RESPONSE_ACCEPT)
		return 2; /* user declined */

	if (!opt_uuid || !*opt_uuid) {
		show_error ("Cannot trust this certificate",
		            "The connection to update was not identified "
		            "(missing UUID).");
		return 1;
	}

	if (!trust_and_reconnect (&error)) {
		show_error ("Could not update the connection",
		            error ? error->message : "unknown error");
		g_clear_error (&error);
		return 1;
	}

	return 0;
}
