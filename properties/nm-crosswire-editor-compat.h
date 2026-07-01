/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Thin GTK3/GTK4 shims so nm-crosswire-editor.c compiles from one source
 * against both toolkits. The core plugin (nm-crosswire-editor-plugin.c) picks
 * the matching build at runtime and dlopen()s it:
 *   - GTK3 hosts (nm-connection-editor): libnm-vpn-plugin-crosswire-editor.so
 *   - GTK4 hosts (GNOME Settings):       libnm-gtk4-vpn-plugin-crosswire-editor.so
 *
 * Only the handful of operations that differ between GTK3 and GTK4 are wrapped;
 * grid attach, labels, switches and visibility share the same API in both.
 */
#ifndef __NM_CROSSWIRE_EDITOR_COMPAT_H__
#define __NM_CROSSWIRE_EDITOR_COMPAT_H__

#include <gtk/gtk.h>

#if GTK_MAJOR_VERSION >= 4

static inline void ot_box_append(GtkWidget *box, GtkWidget *child)
{ gtk_box_append(GTK_BOX(box), child); }

static inline const char *ot_get_text(GtkWidget *w)
{ return gtk_editable_get_text(GTK_EDITABLE(w)); }

static inline void ot_set_text(GtkWidget *w, const char *s)
{ gtk_editable_set_text(GTK_EDITABLE(w), s ? s : ""); }

static inline GtkWidget *ot_password_new(void)
{ return gtk_password_entry_new(); }

static inline GtkWidget *ot_combo_new(const char *const *labels)
{
	GtkStringList *m = gtk_string_list_new(NULL);
	for (guint i = 0; labels[i]; i++)
		gtk_string_list_append(m, labels[i]);
	return gtk_drop_down_new(G_LIST_MODEL(m), NULL);
}

static inline guint ot_combo_get(GtkWidget *c)
{
	guint s = gtk_drop_down_get_selected(GTK_DROP_DOWN(c));
	return s == GTK_INVALID_LIST_POSITION ? 0 : s;
}

static inline void ot_combo_set(GtkWidget *c, guint i)
{ gtk_drop_down_set_selected(GTK_DROP_DOWN(c), i); }

/* GTK4 widgets are visible by default; no show_all / no-show-all machinery. */
static inline void ot_show_all(GtkWidget *w) { (void) w; }
static inline void ot_set_no_show_all(GtkWidget *w) { (void) w; }

#define OT_COMBO_SIGNAL "notify::selected"

#else /* GTK 3 */

static inline void ot_box_append(GtkWidget *box, GtkWidget *child)
{ gtk_box_pack_start(GTK_BOX(box), child, FALSE, FALSE, 0); }

static inline const char *ot_get_text(GtkWidget *w)
{ return gtk_entry_get_text(GTK_ENTRY(w)); }

static inline void ot_set_text(GtkWidget *w, const char *s)
{ gtk_entry_set_text(GTK_ENTRY(w), s ? s : ""); }

static inline GtkWidget *ot_password_new(void)
{
	GtkWidget *e = gtk_entry_new();
	gtk_entry_set_visibility(GTK_ENTRY(e), FALSE);
	gtk_entry_set_input_purpose(GTK_ENTRY(e), GTK_INPUT_PURPOSE_PASSWORD);
	return e;
}

static inline GtkWidget *ot_combo_new(const char *const *labels)
{
	GtkWidget *c = gtk_combo_box_text_new();
	for (guint i = 0; labels[i]; i++)
		gtk_combo_box_text_append_text(GTK_COMBO_BOX_TEXT(c), labels[i]);
	return c;
}

static inline guint ot_combo_get(GtkWidget *c)
{
	int s = gtk_combo_box_get_active(GTK_COMBO_BOX(c));
	return s < 0 ? 0 : (guint) s;
}

static inline void ot_combo_set(GtkWidget *c, guint i)
{ gtk_combo_box_set_active(GTK_COMBO_BOX(c), (int) i); }

static inline void ot_show_all(GtkWidget *w) { gtk_widget_show_all(w); }

/* Exclude a widget from gtk_widget_show_all so the host re-showing the whole
 * form can't override our per-auth-type visibility; we drive it explicitly. */
static inline void ot_set_no_show_all(GtkWidget *w) { gtk_widget_set_no_show_all(w, TRUE); }

#define OT_COMBO_SIGNAL "changed"

#endif /* GTK_MAJOR_VERSION */

#endif /* __NM_CROSSWIRE_EDITOR_COMPAT_H__ */
