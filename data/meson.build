subdir('resources')

app_conf = configuration_data()
app_conf.set('app_id', application_id)
app_conf.set('program', program_name)
app_conf.set('name_suffix', name_suffix)
app_conf.set('schema_path', '/' + application_id.replace('.', '/') + '/')

# Desktop file
desktop_file = i18n.merge_file(
  input: configure_file(
    input: base_id + '.desktop.in.in',
    output: application_id + '.desktop.in',
    configuration: app_conf,
  ),
  output: application_id + '.desktop',
  type: 'desktop',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'applications',
)
if desktop_file_validate.found()
  test('validate-desktop', desktop_file_validate, args: [desktop_file])
endif

# AppStream metainfo
metainfo_file = i18n.merge_file(
  input: configure_file(
    input: base_id + '.metainfo.xml.in.in',
    output: application_id + '.metainfo.xml.in',
    configuration: app_conf,
  ),
  output: application_id + '.metainfo.xml',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'metainfo',
)
if appstreamcli.found()
  test('validate-metainfo', appstreamcli, args: ['validate', '--no-net', '--explain', metainfo_file])
endif

# GSettings schema. Compiled into the build directory too, so the app runs
# uninstalled with GSETTINGS_SCHEMA_DIR=<builddir>/data (see Makefile).
gschema_file = configure_file(
  input: base_id + '.gschema.xml.in',
  output: application_id + '.gschema.xml',
  configuration: app_conf,
  install: true,
  install_dir: datadir / 'glib-2.0' / 'schemas',
)
custom_target(
  'gschemas.compiled',
  input: gschema_file,
  output: 'gschemas.compiled',
  command: [glib_compile_schemas, '--strict', '--targetdir', meson.current_build_dir(), meson.current_build_dir()],
  build_by_default: true,
)

# Icons
install_data(
  'icons/hicolor/scalable/apps' / base_id + '.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
  rename: application_id + '.svg',
)
install_data(
  'icons/hicolor/symbolic/apps' / base_id + '-symbolic.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'symbolic' / 'apps',
  rename: application_id + '-symbolic.svg',
)
