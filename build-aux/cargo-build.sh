#!/bin/sh
# Builds the gatefold binary with Cargo and copies it to where Meson expects it.
# Usage: cargo-build.sh SOURCE_ROOT PROFILE OUTPUT CARGO [CARGO_ARGS...]
# CARGO_TARGET_DIR and the GATEFOLD_* variables come from app/meson.build.
set -eu

source_root=$1
profile=$2
output=$(realpath -m "$3") # relative to the build dir; resolve before cd
cargo=$4
shift 4

# Cargo looks for .cargo/config.toml from the working directory. The RPM
# build puts the vendored-sources config there (%cargo_prep).
cd "$source_root"
"$cargo" build --package gatefold "$@"
cp "$CARGO_TARGET_DIR/$profile/gatefold" "$output"
