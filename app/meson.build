cargo_options = []
if get_option('profile') == 'development'
  rust_target = 'debug'
else
  rust_target = 'release'
  cargo_options += ['--release']
endif
if get_option('spotify_stream')
  cargo_options += ['--features', 'spotify-stream']
endif

# Read by app/src/config.rs and app/build.rs.
cargo_env = environment()
cargo_env.set('GATEFOLD_APP_ID', application_id)
cargo_env.set('GATEFOLD_VERSION', version + version_suffix)
cargo_env.set('GATEFOLD_PROFILE', get_option('profile'))
cargo_env.set('GATEFOLD_UPDATE_CHECK', get_option('update_check').to_string())
cargo_env.set('GATEFOLD_LOCALEDIR', localedir)
cargo_env.set('GATEFOLD_GETTEXT_PACKAGE', gettext_package)
cargo_env.set('GATEFOLD_GRESOURCE', gresource_bundle.full_path())
cargo_env.set('CARGO_TARGET_DIR', meson.project_build_root() / 'target')

# The development build is installed as gatefold-devel, so it doesn't hide
# the installed app's program (its output stays app/gatefold for make run
# and the headless tests).
development = get_option('profile') == 'development'
cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: not development,
  install_dir: bindir,
  depends: gresource_bundle,
  env: cargo_env,
  command: [
    find_program(meson.project_source_root() / 'build-aux' / 'cargo-build.sh'),
    meson.project_source_root(),
    rust_target,
    '@OUTPUT@',
    cargo,
    cargo_options,
  ],
)
if development
  meson.add_install_script(
    find_program('sh'),
    '-c',
    'install -Dm755 "$1" "${MESON_INSTALL_DESTDIR_PREFIX}/$2/' + program_name + '"',
    'sh',
    cargo_build.full_path(),
    get_option('bindir'),
  )
endif

# Without streaming, the librespot crate isn't built for the tests either
# (it alone takes minutes). A release build of all tests takes a while on
# small builders (RPM %check, CI): up to an hour.
test_options = []
if not get_option('spotify_stream')
  test_options += ['--exclude', 'gatefold-stream']
endif
test(
  'cargo-test',
  cargo,
  args: ['test', '--workspace', test_options, cargo_options],
  env: cargo_env,
  workdir: meson.project_source_root(),
  timeout: 3600,
  is_parallel: false,
)
