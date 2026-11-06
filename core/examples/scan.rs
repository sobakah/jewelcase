//! Scans folders into a throwaway database and prints albums and artists.
//! `cargo run -p gatefold-core --example scan -- DB_PATH COVER_DIR FOLDER...`

use std::path::PathBuf;

use gatefold_core::db::Database;
use gatefold_core::library::artists::{AlbumRole, Aliases};
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
    let aliases = Aliases::from_lyrembed();
    let start = std::time::Instant::now();
    let summary = scan(&mut db, &folders, &aliases, &covers, &|_| {}).expect("scan");
    println!("{summary:?} in {:?}", start.elapsed());

    for album in db.albums().expect("albums") {
        let cover = match album.cover_key.as_deref() {
            None => "-",
            Some(key) if covers.has(key) => "ok",
            Some(_) => "MISSING",
        };
        println!(
            "{:>4} {:<28} | {:<45} | {:<20} | {:?} {:?}{}{} {} tracks, cover {cover}",
            album.id,
            album.artist_credit,
            album.title,
            album.edition.as_deref().unwrap_or(""),
            album.year,
            album.release_type,
            if album.soundtrack { " OST" } else { "" },
            if album.compilation { " COMP" } else { "" },
            album.track_count,
        );
    }
    for artist in db.artists().expect("artists") {
        let albums = db.artist_albums(artist.id).expect("albums");
        let collaborations = albums
            .iter()
            .filter(|(_, role)| *role == AlbumRole::Collaboration)
            .count();
        let appears = db.artist_appears_on(artist.id).expect("appears on").len();
        println!(
            "artist {:>3} {:<40} {} albums, {} songs, {collaborations} collaborations, appears on {appears}",
            artist.id, artist.name, artist.album_count, artist.track_count
        );
    }
}
