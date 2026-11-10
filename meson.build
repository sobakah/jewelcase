project(
  'gatefold',
  version: '0.3.0',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.4.0',
)

fs = import('fs')
i18n = import('i18n')
gnome = import('gnome')

base_id = 'io.github.sobakah.Gatefold'

# Fail early with a clear message when development headers are missing.
dependency('gtk4', version: '>= 4.24')
dependency('libadwaita-1', version: '>= 1.10')

cargo = find_program('cargo')
blueprint_compiler = find_program('blueprint-compiler')
glib_compile_resources = find_program('glib-compile-resources')
glib_compile_schemas = find_program('glib-compile-schemas')
desktop_file_validate = find_program('desktop-file-validate', required: false)
appstreamcli = find_program('appstreamcli', required: false)

version = meson.project_version()
prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
localedir = prefix / get_option('localedir')
datadir = prefix / get_option('datadir')
gettext_package = meson.project_name()

# The development profile gets its own app ID, so it can run next to the
# installed RPM with separate settings and its own striped header bar.
if get_option('profile') == 'development'
  application_id = base_id + '.Devel'
  # The commit, "-dirty" with uncommitted changes. The Flatpak builds
  # without .git: `make flatpak` hands it over in .vcs-revision.
  git_rev = run_command('git', 'describe', '--always', '--dirty', '--exclude=*',
    check: false).stdout().strip()
  if git_rev == '' and fs.is_file('.vcs-revision')
    git_rev = fs.read('.vcs-revision').strip()
  endif
  version_suffix = git_rev != '' ? '-' + git_rev : '-devel'
  # Installed next to the RPM: its own program and menu entry.
  program_name = meson.project_name() + '-devel'
  name_suffix = ' (Development)'
else
  application_id = base_id
  version_suffix = ''
  program_name = meson.project_name()
  name_suffix = ''
endif

subdir('data')
subdir('po')
subdir('app')

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
