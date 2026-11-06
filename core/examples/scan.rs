//! Scans folders into a throwaway database and prints the albums.
//! `cargo run -p gatefold-core --example scan -- DB_PATH COVER_DIR FOLDER...`

use std::path::PathBuf;

use gatefold_core::db::Database;
use gatefold_core::library::covers::CoverCache;
use gatefold_core::library::scanner::scan;

fn main() {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let (Some(db_path), Some(cover_dir)) = (args.next(), args.next()) else {
        eprintln!("usage: scan DB_PATH COVER_DIR FOLDER...");
        std::process::exit(2);
    };
    let folders: Vec<PathBuf> = args.collect();
    let mut db = Database::open(&db_path).expect("open database");
    let covers = CoverCache::new(cover_dir);
    let start = std::time::Instant::now();
    let summary = scan(&mut db, &folders, &covers, &|_| {}).expect("scan");
    println!("{summary:?} in {:?}", start.elapsed());
    for album in db.albums().expect("albums") {
        println!(
            "{:>4} {:<28} | {:<45} | {:<20} | {:?} {:?}{}{} {} tracks, cover {}",
            album.id,
            album.artist_credit,
            album.title,
            album.edition.as_deref().unwrap_or(""),
            album.year,
            album.release_type,
            if album.soundtrack { " OST" } else { "" },
            if album.compilation { " COMP" } else { "" },
            album.track_count,
            album
                .cover_key
                .as_deref()
                .map_or("-", |k| if covers.has(k) { "ok" } else { "MISSING" }),
        );
    }
}
