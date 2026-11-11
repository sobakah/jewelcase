#!/bin/sh
# Runs a command in a private, invisible session, for UI tests that must not
# disturb the desktop:
#
# - a headless mutter with a virtual monitor (nothing appears on screen)
# - its own runtime dir and D-Bus session bus (no clash with a running Jewelcase, no MPRIS
#   entry in GNOME Shell)
# - in-memory GSettings (the real settings stay untouched)
# - its own data and cache folders, in $JEWELCASE_HEADLESS_STATE
#   (default: _build/headless), kept between runs so scans stay quick
# - silent audio (JEWELCASE_SILENT=1), no browser, no keyring
#
# Usage: build-aux/headless.sh COMMAND [ARGS...]
# JEWELCASE_HEADLESS_MONITOR=WxH sets the virtual monitor (default 1100x760).
# Example: JEWELCASE_SCREENSHOT=/tmp/shot.png build-aux/headless.sh _build/app/jewelcase
set -eu

if [ $# -eq 0 ]; then
    echo "usage: $0 COMMAND [ARGS...]" >&2
    exit 2
fi

state=${JEWELCASE_HEADLESS_STATE:-$PWD/_build/headless}
mkdir -p "$state/data" "$state/cache"

# A private runtime dir. Everything the private bus starts on demand (the
# document portal, gvfs, other portals) inherits the bus daemon's
# environment, so these variables must be set *before* dbus-run-session.
# With the real /run/user/$UID, the private document portal takes over
# /run/user/$UID/doc, the desktop's portal exits, and Flatpak apps can no
# longer open files or links until it is restarted.
runtime=$(mktemp -d "${TMPDIR:-/tmp}/jewelcase-headless.XXXXXX")
chmod 700 "$runtime"
cleanup() {
    for mount in "$runtime/doc" "$runtime/gvfs"; do
        fusermount3 -u -q "$mount" 2>/dev/null || true
    done
    # Never descend into a mount that is still there (the document portal
    # exposes real files).
    rm -rf --one-file-system "$runtime" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

export XDG_RUNTIME_DIR="$runtime"
export XDG_DATA_HOME="$state/data" XDG_CACHE_HOME="$state/cache"
# Never the user's own soloistd (it would control their real Soloist).
export JEWELCASE_SOLOISTD_CONFIG="$state/soloistd/config.json"
export GSETTINGS_BACKEND=memory JEWELCASE_SILENT=1 JEWELCASE_NO_BROWSER=1 JEWELCASE_NO_KEYRING=1
# Keep GTK away from portals, gvfs and the accessibility bus.
export GDK_DEBUG=no-portals GIO_USE_VFS=local GTK_A11Y=none
export WAYLAND_DISPLAY="jewelcase-headless-$$"
unset DISPLAY

status=0
dbus-run-session -- sh -c '
    mutter --headless --wayland --no-x11 --wayland-display "$WAYLAND_DISPLAY" \
        --virtual-monitor "${JEWELCASE_HEADLESS_MONITOR:-1100x760}" >/dev/null 2>&1 &
    mutter=$!
    for _ in $(seq 100); do
        [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] && break
        sleep 0.1
    done

    status=0
    "$@" || status=$?
    kill "$mutter" 2>/dev/null || true
    wait "$mutter" 2>/dev/null || true
    exit "$status"
' sh "$@" || status=$?
exit "$status"
