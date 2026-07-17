# SPDX-License-Identifier: GPL-3.0-or-later
project('NetworkManager-crosswire', 'c',
  version: '0.3.0',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 0.60',
  default_options: ['c_std=gnu11', 'warning_level=2'],
)

prefix     = get_option('prefix')
libexecdir = prefix / get_option('libexecdir')
# NetworkManager plugin dir; override with -Dplugin_dir=... on odd distros.
plugin_dir = get_option('plugin_dir')
if plugin_dir == ''
  plugin_dir = prefix / get_option('libdir') / 'NetworkManager'
endif

conf = configuration_data()
conf.set_quoted('LIBEXECDIR', libexecdir)
conf.set_quoted('PLUGINDIR', plugin_dir)
conf.set_quoted('VERSION', meson.project_version())
configure_file(output: 'config.h', configuration: conf)
add_project_arguments('-DHAVE_CONFIG_H', language: 'c')
config_inc = include_directories('.')

# --- The Rust D-Bus service, built via cargo and installed as a helper. ---
find_program('cargo')
sh = find_program('sh')
service_manifest = meson.project_source_root() / 'service' / 'Cargo.toml'
service_target = meson.project_source_root() / 'service' / 'target' / 'release' / 'nm-crosswire-service'
service_bin = custom_target('nm-crosswire-service',
  output: 'nm-crosswire-service',
  command: [sh, '-c',
    'cargo build --release --manifest-path "@0@" && cp "@1@" "@OUTPUT@"'.format(
      service_manifest, service_target)],
  build_by_default: true,
  install: true,
  install_dir: libexecdir,
)

subdir('properties')
subdir('pppd-plugin')

# --- Data: the .name descriptor (path-substituted) and the D-Bus policy. ---
configure_file(
  input: 'data/nm-crosswire-service.name.in',
  output: 'nm-crosswire-service.name',
  configuration: {'LIBEXECDIR': libexecdir, 'PLUGINDIR': plugin_dir},
  install: true,
  install_dir: get_option('sysconfdir') / 'NetworkManager' / 'VPN',
)

install_data('data/nm-crosswire-service.conf',
  install_dir: get_option('datadir') / 'dbus-1' / 'system.d')

summary({
  'libexecdir': libexecdir,
  'plugin_dir': plugin_dir,
}, section: 'Install paths')
