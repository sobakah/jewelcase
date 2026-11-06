blueprints = custom_target(
  'blueprints',
  input: files(
    'ui/album_panel.blp',
    'ui/mini_player.blp',
    'ui/player_view.blp',
    'ui/preferences_dialog.blp',
    'ui/shortcuts_dialog.blp',
    'ui/window.blp',
  ),
  output: '.',
  command: [blueprint_compiler, 'batch-compile', '@OUTPUT@', '@CURRENT_SOURCE_DIR@', '@INPUT@'],
)

# A standalone bundle, embedded into the binary by app/build.rs.
gresource_bundle = custom_target(
  'gresource',
  input: 'resources.gresource.xml',
  output: 'gatefold.gresource',
  depfile: 'gatefold.gresource.d',
  depends: blueprints,
  command: [
    glib_compile_resources,
    '--sourcedir', meson.current_build_dir(),
    '--sourcedir', meson.current_source_dir(),
    '--target', '@OUTPUT@',
    '--dependency-file', '@DEPFILE@',
    '@INPUT@',
  ],
)
