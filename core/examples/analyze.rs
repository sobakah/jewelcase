//! Development aid: `cargo run -p jewelcase-core --example analyze -- FILE…`
//! analyzes files like the background worker and prints the results.

use std::path::Path;
use std::time::Instant;

fn main() {
    gst::init().expect("GStreamer");
    for arg in std::env::args().skip(1) {
        let path = Path::new(&arg);
        let start = Instant::now();
        let key = jewelcase_core::analysis::content_key(path);
        match jewelcase_core::analysis::analyze(path) {
            Ok(result) => println!(
                "{arg}\n  key {:?}, {} features, BPM {:?}, edges {:?}, {:.1} s",
                key.ok(),
                result.features.len(),
                result.bpm,
                result.edges,
                start.elapsed().as_secs_f64()
            ),
            Err(err) => println!("{arg}\n  failed: {err}"),
        }
    }
}
