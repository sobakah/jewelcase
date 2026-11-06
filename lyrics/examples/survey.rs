//! Loads the lyrics of the given files and prints a summary; with one file,
//! prints its lines. `cargo run -p gatefold-lyrics --example survey -- FILE...`

use gatefold_lyrics::{LyricsState, load};

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let (mut synced, mut dual, mut plain, mut instrumental, mut none) = (0, 0, 0, 0, 0);
    for file in &files {
        match load(file.as_ref(), None) {
            LyricsState::Lyrics(lyrics) => {
                if files.len() == 1 {
                    for line in &lyrics.lines {
                        println!(
                            "{:>8?} {} | {}",
                            line.time_ms,
                            line.primary,
                            line.secondary.as_deref().unwrap_or("")
                        );
                    }
                }
                match (lyrics.synced, lyrics.dual) {
                    (true, true) => dual += 1,
                    (true, false) => synced += 1,
                    _ => plain += 1,
                }
            }
            LyricsState::Instrumental => instrumental += 1,
            LyricsState::None => none += 1,
        }
    }
    println!(
        "{} files: {dual} synced dual-line, {synced} synced single-line, {plain} plain, {instrumental} instrumental, {none} none",
        files.len()
    );
}
