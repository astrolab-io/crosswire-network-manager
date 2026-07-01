/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * NMVpnEditor implementation: builds the config form in C so this one source
 * compiles against both GTK3 and GTK4 (via nm-crosswire-editor-compat.h). It
 * populates the form from an existing connection and writes user input back
 * into the NMSettingVpn (vpn.data / vpn.secrets) the service's config.rs reads.
 *
 * Built twice — libnm-vpn-plugin-crosswire-editor.so (GTK3) and
 * libnm-gtk4-vpn-plugin-crosswire-editor.so (GTK4) — and dlopen'd by the core
 * plugin, which resolves nm_vpn_editor_factory_crosswire() below.
 */

#include "config.h"
#include <string.h>
#include <gtk/gtk.h>
#include <NetworkManager.h>
#include <nm-vpn-editor.h>

#include "nm-crosswire-editor-plugin.h"
#include "nm-crosswire-editor-compat.h"

#define CROSSWIRE_TYPE_EDITOR (crosswire_editor_get_type())
G_DECLARE_FINAL_TYPE(CrosswireEditor, crosswire_editor, CROSSWIRE, EDITOR, GObject)

struct _CrosswireEditor {
	GObject parent;
	GtkWidget *widget;      /* top-level container returned to the host */

	GtkWidget *provider, *auth;
	GtkWidget *gateway, *port, *user, *realm, *ca_file, *trusted_cert, *otp;
	GtkWidget *password, *cookie;
	GtkWidget *insecure;
	GtkWidget *user_label, *password_label, *otp_label, *cookie_label;
};

static void crosswire_editor_interface_init(NMVpnEditorInterface *iface);

G_DEFINE_TYPE_EXTENDED(CrosswireEditor, crosswire_editor, G_TYPE_OBJECT, 0,
	G_IMPLEMENT_INTERFACE(NM_TYPE_VPN_EDITOR, crosswire_editor_interface_init))

enum { CHANGED, LAST_SIGNAL };
static guint signals[LAST_SIGNAL] = { 0 };

/* provider combo index → vpn.data "provider". crosswire is provider-neutral;
 * today only Fortinet SSL-VPN ships. Append here + to provider_labels below. */
static const char *const PROVIDER_VALUES[] = { "fortinet" };
#define N_PROVIDER G_N_ELEMENTS(PROVIDER_VALUES)

/* auth combo index → vpn.data "auth-type". */
static const char *const AUTH_VALUES[] = { "password", "saml", "cookie" };
#define N_AUTH G_N_ELEMENTS(AUTH_VALUES)

static void
emit_changed(CrosswireEditor *self)
{
	g_signal_emit(self, signals[CHANGED], 0);
}

/* GtkEditable::changed and GTK3 GtkComboBox::changed: (widget, user_data). */
static void
on_changed(GtkWidget *w, gpointer user_data)
{
	(void) w;
	emit_changed(CROSSWIRE_EDITOR(user_data));
}

/* GObject notify (GtkSwitch::active, GTK4 GtkDropDown::selected): 3 args. */
static void
on_notify(GObject *obj, GParamSpec *pspec, gpointer user_data)
{
	(void) obj; (void) pspec;
	emit_changed(CROSSWIRE_EDITOR(user_data));
}

/* Gateway is the one truly required field (the VPN host). Mark it visually when
 * empty so the user sees why Save stays disabled, instead of only finding out on
 * a save attempt. update_connection() still enforces it. */
static void
update_gateway_hint(CrosswireEditor *self)
{
	const char *gw = ot_get_text(self->gateway);
	gboolean empty = !gw || !*gw;
	gtk_entry_set_icon_from_icon_name(GTK_ENTRY(self->gateway),
		GTK_ENTRY_ICON_SECONDARY, empty ? "dialog-warning-symbolic" : NULL);
	gtk_entry_set_icon_tooltip_text(GTK_ENTRY(self->gateway),
		GTK_ENTRY_ICON_SECONDARY, empty ? "A gateway host is required" : NULL);
}

static void
on_gateway_changed(GtkWidget *w, gpointer user_data)
{
	(void) w;
	CrosswireEditor *self = CROSSWIRE_EDITOR(user_data);
	update_gateway_hint(self);
	emit_changed(self);
}

/* Show only the credential rows relevant to the chosen auth type. */
static void
sync_auth_rows(CrosswireEditor *self)
{
	guint a = ot_combo_get(self->auth);
	gboolean pw = (a == 0), cookie = (a == 2);
	/* Username is only used for password auth — SAML identity is established at
	 * the IdP in the browser, and cookie auth carries an already-authed session.
	 * (Realm stays visible in all modes; config.rs passes it unconditionally.) */
	gtk_widget_set_visible(self->user_label, pw);
	gtk_widget_set_visible(self->user, pw);
	gtk_widget_set_visible(self->password_label, pw);
	gtk_widget_set_visible(self->password, pw);
	gtk_widget_set_visible(self->otp_label, pw);
	gtk_widget_set_visible(self->otp, pw);
	gtk_widget_set_visible(self->cookie_label, cookie);
	gtk_widget_set_visible(self->cookie, cookie);
}

#if GTK_MAJOR_VERSION >= 4
static void
on_auth(GObject *obj, GParamSpec *pspec, gpointer user_data)
{
	(void) obj; (void) pspec;
#else
static void
on_auth(GtkWidget *obj, gpointer user_data)
{
	(void) obj;
#endif
	CrosswireEditor *self = CROSSWIRE_EDITOR(user_data);
	sync_auth_rows(self);
	emit_changed(self);
}

/* Attach a right-aligned mnemonic label + its widget as one grid row. */
static GtkWidget *
add_row(GtkWidget *grid, int row, const char *mnemonic, GtkWidget *w)
{
	GtkWidget *l = gtk_label_new_with_mnemonic(mnemonic);
	gtk_label_set_xalign(GTK_LABEL(l), 1.0);
	gtk_label_set_mnemonic_widget(GTK_LABEL(l), w);
	gtk_grid_attach(GTK_GRID(grid), l, 0, row, 1, 1);
	gtk_grid_attach(GTK_GRID(grid), w, 1, row, 1, 1);
	return l;
}

static GtkWidget *
entry_with_placeholder(const char *placeholder)
{
	GtkWidget *e = gtk_entry_new();
	if (placeholder)
		gtk_entry_set_placeholder_text(GTK_ENTRY(e), placeholder);
	return e;
}

static void
build_ui(CrosswireEditor *self)
{
	GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
	gtk_widget_set_margin_start(box, 12);
	gtk_widget_set_margin_end(box, 12);
	gtk_widget_set_margin_top(box, 12);
	gtk_widget_set_margin_bottom(box, 12);

	GtkWidget *grid = gtk_grid_new();
	gtk_grid_set_row_spacing(GTK_GRID(grid), 6);
	gtk_grid_set_column_spacing(GTK_GRID(grid), 12);
	ot_box_append(box, grid);

	int r = 0;

	static const char *const provider_labels[] = { "Fortinet SSL-VPN", NULL };
	self->provider = ot_combo_new(provider_labels);
	add_row(grid, r++, "_Provider", self->provider);

	self->gateway = entry_with_placeholder("vpn.example.com");
	gtk_widget_set_hexpand(self->gateway, TRUE);
	add_row(grid, r++, "_Gateway", self->gateway);

	self->port = entry_with_placeholder("443");
	add_row(grid, r++, "_Port", self->port);

	static const char *const auth_labels[] = {
		"Username / Password", "SAML / SSO (browser)", "Session cookie", NULL };
	self->auth = ot_combo_new(auth_labels);
	add_row(grid, r++, "_Authentication", self->auth);

	self->user = entry_with_placeholder(NULL);
	self->user_label = add_row(grid, r++, "_Username", self->user);

	self->realm = entry_with_placeholder(NULL);
	add_row(grid, r++, "_Realm", self->realm);

	self->password = ot_password_new();
	self->password_label = add_row(grid, r++, "_Password", self->password);

	self->otp = entry_with_placeholder("optional one-time code");
	self->otp_label = add_row(grid, r++, "_OTP (2FA)", self->otp);

	self->cookie = ot_password_new();
	self->cookie_label = add_row(grid, r++, "_Cookie", self->cookie);

	self->ca_file = entry_with_placeholder("optional PEM CA bundle path");
	add_row(grid, r++, "_CA file", self->ca_file);

	self->trusted_cert = entry_with_placeholder("optional pinned leaf digest(s), comma-separated");
	add_row(grid, r++, "_Trusted cert (SHA256)", self->trusted_cert);

	self->insecure = gtk_switch_new();
	gtk_widget_set_halign(self->insecure, GTK_ALIGN_START);
	add_row(grid, r++, "Allow _insecure SSL", self->insecure);

	/* These rows are shown/hidden per auth type by sync_auth_rows(); keep the
	 * host's show_all from forcing them visible (see ot_set_no_show_all). */
	GtkWidget *conditional[] = {
		self->user_label,     self->user,
		self->password_label, self->password,
		self->otp_label,      self->otp,
		self->cookie_label,   self->cookie,
	};
	for (guint i = 0; i < G_N_ELEMENTS(conditional); i++)
		ot_set_no_show_all(conditional[i]);

	self->widget = box;
}

static void
connect_signals(CrosswireEditor *self)
{
	GtkWidget *entries[] = {
		self->port, self->user, self->realm,
		self->ca_file, self->trusted_cert, self->otp,
		self->password, self->cookie,
	};
	for (guint i = 0; i < G_N_ELEMENTS(entries); i++)
		g_signal_connect(entries[i], "changed", G_CALLBACK(on_changed), self);

	/* Gateway drives the required-field hint in addition to re-validation. */
	g_signal_connect(self->gateway, "changed", G_CALLBACK(on_gateway_changed), self);
	g_signal_connect(self->insecure, "notify::active", G_CALLBACK(on_notify), self);

#if GTK_MAJOR_VERSION >= 4
	g_signal_connect(self->provider, OT_COMBO_SIGNAL, G_CALLBACK(on_notify), self);
#else
	g_signal_connect(self->provider, OT_COMBO_SIGNAL, G_CALLBACK(on_changed), self);
#endif
	g_signal_connect(self->auth, OT_COMBO_SIGNAL, G_CALLBACK(on_auth), self);
}

static void
fill_from_connection(CrosswireEditor *self, NMConnection *connection)
{
	NMSettingVpn *s_vpn = connection ? nm_connection_get_setting_vpn(connection) : NULL;

	if (s_vpn) {
		const char *v;
#define SET_ENTRY(w, key) \
	do { v = nm_setting_vpn_get_data_item(s_vpn, key); if (v) ot_set_text(self->w, v); } while (0)
		SET_ENTRY(gateway,      NM_CROSSWIRE_KEY_GATEWAY);
		SET_ENTRY(port,         NM_CROSSWIRE_KEY_PORT);
		SET_ENTRY(user,         NM_CROSSWIRE_KEY_USER);
		SET_ENTRY(realm,        NM_CROSSWIRE_KEY_REALM);
		SET_ENTRY(ca_file,      NM_CROSSWIRE_KEY_CA_FILE);
		SET_ENTRY(trusted_cert, NM_CROSSWIRE_KEY_TRUSTED_CERT);
#undef SET_ENTRY

		const char *provider = nm_setting_vpn_get_data_item(s_vpn, NM_CROSSWIRE_KEY_PROVIDER);
		guint psel = 0;
		for (guint i = 0; provider && i < N_PROVIDER; i++)
			if (!g_strcmp0(provider, PROVIDER_VALUES[i])) { psel = i; break; }
		ot_combo_set(self->provider, psel);

		const char *auth = nm_setting_vpn_get_data_item(s_vpn, NM_CROSSWIRE_KEY_AUTH_TYPE);
		guint asel = 0;
		for (guint i = 0; auth && i < N_AUTH; i++)
			if (!g_strcmp0(auth, AUTH_VALUES[i])) { asel = i; break; }
		ot_combo_set(self->auth, asel);

		const char *insec = nm_setting_vpn_get_data_item(s_vpn, NM_CROSSWIRE_KEY_INSECURE);
		gtk_switch_set_active(GTK_SWITCH(self->insecure), g_strcmp0(insec, "yes") == 0);

		const char *pw = nm_setting_vpn_get_secret(s_vpn, NM_CROSSWIRE_KEY_PASSWORD);
		if (pw) ot_set_text(self->password, pw);
		const char *ck = nm_setting_vpn_get_secret(s_vpn, NM_CROSSWIRE_KEY_COOKIE);
		if (ck) ot_set_text(self->cookie, ck);
	}

	sync_auth_rows(self);
	update_gateway_hint(self);
}

static GObject *
get_widget(NMVpnEditor *editor)
{
	return G_OBJECT(CROSSWIRE_EDITOR(editor)->widget);
}

static void
add_if_set(NMSettingVpn *s_vpn, const char *key, const char *val, gboolean secret)
{
	if (val && *val) {
		if (secret)
			nm_setting_vpn_add_secret(s_vpn, key, val);
		else
			nm_setting_vpn_add_data_item(s_vpn, key, val);
	}
}

static gboolean
update_connection(NMVpnEditor *editor, NMConnection *connection, GError **error)
{
	CrosswireEditor *self = CROSSWIRE_EDITOR(editor);
	NMSettingVpn *s_vpn = NM_SETTING_VPN(nm_setting_vpn_new());
	g_object_set(s_vpn, NM_SETTING_VPN_SERVICE_TYPE, NM_DBUS_SERVICE_CROSSWIRE, NULL);

	add_if_set(s_vpn, NM_CROSSWIRE_KEY_GATEWAY,      ot_get_text(self->gateway), FALSE);
	add_if_set(s_vpn, NM_CROSSWIRE_KEY_PORT,         ot_get_text(self->port), FALSE);
	add_if_set(s_vpn, NM_CROSSWIRE_KEY_USER,         ot_get_text(self->user), FALSE);
	add_if_set(s_vpn, NM_CROSSWIRE_KEY_REALM,        ot_get_text(self->realm), FALSE);
	add_if_set(s_vpn, NM_CROSSWIRE_KEY_CA_FILE,      ot_get_text(self->ca_file), FALSE);
	add_if_set(s_vpn, NM_CROSSWIRE_KEY_TRUSTED_CERT, ot_get_text(self->trusted_cert), FALSE);

	guint p = ot_combo_get(self->provider);
	nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_PROVIDER,
	                             PROVIDER_VALUES[p < N_PROVIDER ? p : 0]);

	guint a = ot_combo_get(self->auth);
	nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_AUTH_TYPE,
	                             AUTH_VALUES[a < N_AUTH ? a : 0]);

	if (gtk_switch_get_active(GTK_SWITCH(self->insecure)))
		nm_setting_vpn_add_data_item(s_vpn, NM_CROSSWIRE_KEY_INSECURE, "yes");

	if (a == 0) {
		add_if_set(s_vpn, NM_CROSSWIRE_KEY_PASSWORD, ot_get_text(self->password), TRUE);
		add_if_set(s_vpn, NM_CROSSWIRE_KEY_OTP,      ot_get_text(self->otp), TRUE);
	} else if (a == 2) {
		add_if_set(s_vpn, NM_CROSSWIRE_KEY_COOKIE,   ot_get_text(self->cookie), TRUE);
	}

	if (!nm_setting_vpn_get_data_item(s_vpn, NM_CROSSWIRE_KEY_GATEWAY)) {
		g_set_error_literal(error, NM_CONNECTION_ERROR,
		                    NM_CONNECTION_ERROR_INVALID_PROPERTY,
		                    "a gateway host is required");
		g_object_unref(s_vpn);
		return FALSE;
	}

	nm_connection_add_setting(connection, NM_SETTING(s_vpn));
	return TRUE;
}

/* Toolkit-specific entry point resolved by the core plugin's get_editor(). */
G_MODULE_EXPORT NMVpnEditor *
nm_vpn_editor_factory_crosswire(NMConnection *connection, GError **error)
{
	(void) error;
	CrosswireEditor *self = g_object_new(CROSSWIRE_TYPE_EDITOR, NULL);

	build_ui(self);
	g_object_ref_sink(self->widget);
	ot_show_all(self->widget);

	connect_signals(self);
	fill_from_connection(self, connection);
	return NM_VPN_EDITOR(self);
}

static void
crosswire_editor_init(CrosswireEditor *self) {}

static void
dispose(GObject *object)
{
	CrosswireEditor *self = CROSSWIRE_EDITOR(object);
	g_clear_object(&self->widget);
	G_OBJECT_CLASS(crosswire_editor_parent_class)->dispose(object);
}

static void
crosswire_editor_class_init(CrosswireEditorClass *klass)
{
	G_OBJECT_CLASS(klass)->dispose = dispose;
}

static void
crosswire_editor_interface_init(NMVpnEditorInterface *iface)
{
	iface->get_widget        = get_widget;
	iface->update_connection = update_connection;
	signals[CHANGED] = g_signal_lookup("changed", NM_TYPE_VPN_EDITOR);
}
