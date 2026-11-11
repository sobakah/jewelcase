//! Looks up one artist online and prints what was found.
//! `cargo run -p jewelcase-metadata --example lookup -- NAME [MBID]`;
//! `ALBUMS="Title|Title"` tells artists of the same name apart on Deezer.

use jewelcase_metadata::{ArtistQuery, Client, Providers};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(name) = args.next() else {
        eprintln!("usage: lookup NAME [MBID]");
        std::process::exit(2);
    };
    let query = ArtistQuery {
        name,
        mbid: args.next(),
        release_mbids: Vec::new(),
        album_titles: std::env::var("ALBUMS")
            .map(|a| a.split('|').map(str::to_owned).collect())
            .unwrap_or_default(),
        language: "de".into(),
    };
    let providers = Providers {
        musicbrainz: true,
        wikidata: true,
        deezer: true,
    };
    let client = Client::new(env!("CARGO_PKG_VERSION"));
    match client.lookup(&query, &providers) {
        Ok(info) => println!("{info:#?}"),
        Err(err) => eprintln!("lookup failed: {err}"),
    }
}
