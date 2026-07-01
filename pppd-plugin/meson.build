# SPDX-License-Identifier: GPL-3.0-or-later

gio_dep = dependency('gio-2.0')

# Pure parsers for the CROSSWIRE_ROUTES / CROSSWIRE_DNS env contract, shared by
# the plugin and a dependency-free unit test (the consumer side of the contract
# with crosswire; see CROSSWIRE_ENV_CONTRACT.md).
netparams_src = files('nm-crosswire-netparams.c')
test('netparams',
  executable('test-netparams', ['test-netparams.c', netparams_src], install: false))

# pppd headers ship no pkg-config file; point at them with -Dpppd_include=...
# (e.g. /usr/include) and install into the versioned pppd plugin dir with
# -Dpppd_plugin_dir=/usr/lib/pppd/<version>.
pppd_inc = get_option('pppd_include')
pppd_plugin_dir = get_option('pppd_plugin_dir')

# The C picks 2.4 vs 2.5 itself (via the PPPD_VERSION macro), so we only need
# the install dir. Auto-detect the versioned pppd plugin dir if unset.
if pppd_plugin_dir == ''
  probe = run_command('sh', '-c',
    'ls -d /usr/lib/pppd/* /usr/lib/*/pppd/* 2>/dev/null | head -n1',
    check: false)
  pppd_plugin_dir = probe.stdout().strip()
endif

if pppd_plugin_dir == ''
  warning('pppd plugin dir not found — skipping; set -Dpppd_plugin_dir=/usr/lib/pppd/<ver>')
else
  message('pppd plugin dir: ' + pppd_plugin_dir)
  incs = []
  if pppd_inc != ''
    incs += include_directories(pppd_inc, is_system: true)
  endif

  shared_module(
    'nm-crosswire-pppd-plugin',
    ['nm-crosswire-pppd-plugin.c', netparams_src],
    include_directories: incs,
    dependencies: [gio_dep],
    name_prefix: '',                 # pppd loads by exact filename
    install: true,
    install_dir: pppd_plugin_dir,
  )
endif
