//! Looks up one artist online and prints what was found.
//! `cargo run -p gatefold-metadata --example lookup -- NAME [MBID]`

use gatefold_metadata::{ArtistQuery, Client, Providers};

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
        language: "de".into(),
    };
    let providers = Providers {
        musicbrainz: true,
        wikidata: true,
        fanart_key: std::env::var("FANART_KEY").ok(),
    };
    let client = Client::new(env!("CARGO_PKG_VERSION"));
    match client.lookup(&query, &providers) {
        Ok(info) => println!("{info:#?}"),
        Err(err) => eprintln!("lookup failed: {err}"),
    }
}
