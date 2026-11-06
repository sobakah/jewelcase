//! Plays the last seconds of one file, then a second one gaplessly, and
//! prints the position. Add GATEFOLD_SILENT=1 to play into a fakesink and
//! NO_SEEK=1 to play A from the start.
//! `cargo run -p gatefold-core --example gapless -- A B`

use std::time::Duration;

use gatefold_core::playback::{Player, PlayerEvent, file_uri};

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [a, b] = &args[..] else {
        eprintln!("usage: gapless FILE_A FILE_B");
        std::process::exit(2);
    };
    let (player, events) = Player::new().expect("player");
    let (a, b) = (file_uri(a.as_ref()).unwrap(), file_uri(b.as_ref()).unwrap());
    player.load(&a, 0, true);
    std::thread::sleep(Duration::from_millis(500));
    let duration = loop {
        if let Some(d) = player.duration_ms() {
            break d;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if std::env::var_os("NO_SEEK").is_none() {
        player.seek(duration.saturating_sub(2000));
    }
    player.set_next(Some((b, 1)));
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(500));
        while let Ok(event) = events.try_recv() {
            player.handle_event(&event);
            if event != PlayerEvent::AsyncDone {
                println!("event {event:?}");
            }
        }
        println!(
            "position {:?} duration {:?}",
            player.position_ms(),
            player.duration_ms()
        );
    }
}
