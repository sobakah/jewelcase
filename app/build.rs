//! Embeds the GResource bundle that Meson compiled (Blueprint UI, CSS, icons).
//!
//! Meson passes its path in `JEWELCASE_GRESOURCE`. Without it (plain `cargo
//! check`/`clippy`, rust-analyzer) an empty bundle is embedded; the code still
//! compiles, but the binary only runs when built through Meson.

use std::path::PathBuf;
use std::{env, fs};

fn main() {
    println!("cargo::rerun-if-env-changed=JEWELCASE_GRESOURCE");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"))
        .join("jewelcase.gresource");

    match env::var_os("JEWELCASE_GRESOURCE") {
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo::rerun-if-changed={}", path.display());
            fs::copy(&path, &out)
                .unwrap_or_else(|err| panic!("cannot copy {}: {err}", path.display()));
        }
        None => fs::write(&out, []).expect("cannot write empty resource bundle"),
    }
}
