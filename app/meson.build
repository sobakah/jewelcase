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
cargo_env.set('GATEFOLD_LOCALEDIR', localedir)
cargo_env.set('GATEFOLD_GETTEXT_PACKAGE', gettext_package)
cargo_env.set('GATEFOLD_GRESOURCE', gresource_bundle.full_path())
cargo_env.set('CARGO_TARGET_DIR', meson.project_build_root() / 'target')

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
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

test(
  'cargo-test',
  cargo,
  args: ['test', '--workspace', cargo_options],
  env: cargo_env,
  workdir: meson.project_source_root(),
  timeout: 900,
  is_parallel: false,
)
