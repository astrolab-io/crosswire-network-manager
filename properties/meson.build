# SPDX-License-Identifier: GPL-3.0-or-later

libnm_dep   = dependency('libnm', version: '>= 1.20')
gmodule_dep = dependency('gmodule-2.0')

# --- Core plugin: NOT linked against GTK, so it loads into either a GTK3
#     (nm-connection-editor) or GTK4 (GNOME Settings) host. This is the .so the
#     .name's [libnm] plugin= points at; it dlopens the matching editor below. ---
shared_library(
  'nm-vpn-plugin-crosswire',
  [
    'nm-crosswire-editor-plugin.c',
    'nm-crosswire-import.c',            # provider-agnostic import dispatcher
    'nm-crosswire-import-fortinet.c',   # Fortinet FortiClient XML importer
  ],
  include_directories: config_inc,
  dependencies: [libnm_dep, gmodule_dep],
  c_args: ['-DG_LOG_DOMAIN="nm-crosswire"'],
  name_prefix: 'lib',
  install: true,
  install_dir: plugin_dir,
)

# --- Editor modules: one source (nm-crosswire-editor.c) compiled once per
#     available toolkit. The core picks libnm-vpn-plugin-crosswire-editor.so
#     for GTK3 hosts and libnm-gtk4-vpn-plugin-crosswire-editor.so for GTK4. ---
gtk3_dep = dependency('gtk+-3.0', required: get_option('gtk3'))
gtk4_dep = dependency('gtk4',     required: get_option('gtk4'))

if not gtk3_dep.found() and not gtk4_dep.found()
  warning('neither gtk+-3.0 nor gtk4 found — no editor UI will be built')
endif

if gtk3_dep.found()
  shared_library(
    'nm-vpn-plugin-crosswire-editor',
    'nm-crosswire-editor.c',
    include_directories: config_inc,
    dependencies: [libnm_dep, gtk3_dep],
    c_args: ['-DG_LOG_DOMAIN="nm-crosswire"'],
    name_prefix: 'lib',
    install: true,
    install_dir: plugin_dir,
  )
endif

if gtk4_dep.found()
  shared_library(
    'nm-gtk4-vpn-plugin-crosswire-editor',
    'nm-crosswire-editor.c',
    include_directories: config_inc,
    dependencies: [libnm_dep, gtk4_dep],
    c_args: ['-DG_LOG_DOMAIN="nm-crosswire"'],
    name_prefix: 'lib',
    install: true,
    install_dir: plugin_dir,
  )
endif

# --- Cert-trust dialog: the native "the gateway certificate changed — trust
#     it?" prompt the service launches in the user's session when crosswire
#     rejects a rotated gateway cert. Reuses libnm + GTK3 (already required
#     above), so no new dependency; installed next to the service in libexecdir.
#     GTK3-only: it uses gtk_dialog_run(), which GTK4 dropped. ---
if gtk3_dep.found()
  executable(
    'nm-crosswire-cert-dialog',
    'nm-crosswire-cert-dialog.c',
    include_directories: config_inc,
    dependencies: [libnm_dep, gtk3_dep],
    c_args: ['-DG_LOG_DOMAIN="nm-crosswire"'],
    install: true,
    install_dir: libexecdir,
  )
else
  warning('gtk+-3.0 not found — the native cert-trust dialog will not be built')
endif
