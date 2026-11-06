#!/bin/sh
# Runs a command in a private, invisible session, for UI tests that must not
# disturb the desktop:
#
# - a headless mutter with a virtual monitor (nothing appears on screen)
# - its own D-Bus session bus (no clash with a running Gatefold, no MPRIS
#   entry in GNOME Shell)
# - in-memory GSettings (the real settings stay untouched)
# - its own data and cache folders, in $GATEFOLD_HEADLESS_STATE
#   (default: _build/headless), kept between runs so scans stay quick
# - silent audio (GATEFOLD_SILENT=1)
#
# Usage: build-aux/headless.sh COMMAND [ARGS...]
# Example: GATEFOLD_SCREENSHOT=/tmp/shot.png build-aux/headless.sh _build/app/gatefold
set -eu

if [ $# -eq 0 ]; then
    echo "usage: $0 COMMAND [ARGS...]" >&2
    exit 2
fi

state=${GATEFOLD_HEADLESS_STATE:-$PWD/_build/headless}
mkdir -p "$state/data" "$state/cache"

export GATEFOLD_HEADLESS_DATA="$state/data" GATEFOLD_HEADLESS_CACHE="$state/cache"
exec dbus-run-session -- sh -c '
    export XDG_DATA_HOME="$GATEFOLD_HEADLESS_DATA" XDG_CACHE_HOME="$GATEFOLD_HEADLESS_CACHE"
    export GSETTINGS_BACKEND=memory GATEFOLD_SILENT=1 GATEFOLD_NO_BROWSER=1
    # No portals or gvfs inside the private session.
    export GDK_DEBUG=no-portals GIO_USE_VFS=local GTK_A11Y=none
    export WAYLAND_DISPLAY="gatefold-headless-$$"
    unset DISPLAY

    mutter --headless --wayland --no-x11 --wayland-display "$WAYLAND_DISPLAY" \
        --virtual-monitor 1100x760 >/dev/null 2>&1 &
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
' sh "$@"
