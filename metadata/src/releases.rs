//! New releases of followed artists, from Deezer (CONCEPT.md §9.8): no
//! account or key, and none of Spotify's tight limits on artist requests.
//!
//! Deezer's list of an artist's albums isn't sorted by date (a release of
//! last week can be 70th), so all of it is read; it also has reissues
//! under their new date, which are left out.

use std::time::Duration;

use serde_json::Value;

use super::{Client, DeezerArtist, Error, Result, album_key, album_overlap, deezer_error, text};

/// Between Deezer requests (Deezer allows 50 in 5 seconds).
pub const DEEZER_INTERVAL: Duration = Duration::from_millis(250);
/// Barcodes tried to find an artist.
const UPC_TRIES: usize = 3;
/// Deezer's page size for an artist's albums.
const PAGE: u32 = 100;
/// More pages than this aren't read (a few artists have 150 releases).
const MAX_PAGES: u32 = 5;

/// Development aid: debug builds ask a local stand-in for Deezer when
/// `JEWELCASE_DEEZER_TEST_SERVER` is set (e.g. `http://127.0.0.1:8992`).
fn deezer_base() -> String {
    let server = cfg!(debug_assertions)
        .then(|| std::env::var("JEWELCASE_DEEZER_TEST_SERVER").ok())
        .flatten();
    server.unwrap_or_else(|| "https://api.deezer.com".to_owned())
}

/// A release on Deezer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeezerRelease {
    pub id: u64,
    pub title: String,
    /// YYYY-MM-DD.
    pub date: String,
    /// "album", "ep", "single" or "compile".
    pub kind: String,
    pub cover_url: Option<String>,
}

impl Client {
    fn deezer(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        std::thread::sleep(DEEZER_INTERVAL);
        let json = self.json(&format!("{}{path}", deezer_base()), params)?;
        // Code 4: "Quota limit exceeded" (it counts 5 seconds).
        if json["error"]["code"].as_u64() == Some(4) {
            return Err(Error::Busy(Duration::from_secs(5)));
        }
        deezer_error(&json)?;
        Ok(json)
    }

    /// The Deezer artist of a Spotify artist: through the barcode of one of
    /// their albums (the same on both), else by name, as for portraits
    /// (one artist of exactly the name; of several, the one sharing the
    /// most album titles, or one far better known).
    pub fn deezer_artist(
        &self,
        name: &str,
        upcs: &[String],
        titles: &[String],
    ) -> Result<Option<u64>> {
        for upc in upcs.iter().take(UPC_TRIES) {
            match self.deezer(&format!("/album/upc:{upc}"), &[]) {
                Ok(album) => {
                    if let Some(id) = artist_in_album(&album, name) {
                        return Ok(Some(id));
                    }
                }
                // Not on Deezer ("no data"): try the next.
                Err(Error::Format(_)) => {}
                Err(err) => return Err(err),
            }
        }
        let json = self.deezer("/search/artist", &[("q", name), ("limit", "10")])?;
        let found = artists_named(&json, name);
        let wanted: Vec<String> = titles.iter().map(|t| album_key(t)).collect();
        let mut ranked = Vec::new();
        for artist in found.iter().take(super::DEEZER_COMPARED) {
            let overlap = if found.len() == 1 || wanted.is_empty() {
                0
            } else {
                let albums = self.deezer(
                    &format!("/artist/{}/albums", artist.id),
                    &[("limit", "100")],
                )?;
                album_overlap(&albums, &wanted)
            };
            ranked.push((overlap, artist));
        }
        Ok(super::pick_deezer(&ranked).map(|artist| artist.id))
    }

    /// All of an artist's releases on Deezer, in Deezer's order.
    pub fn deezer_releases(&self, artist_id: u64) -> Result<Vec<DeezerRelease>> {
        let mut releases = Vec::new();
        for page in 0..MAX_PAGES {
            let index = (page * PAGE).to_string();
            let json = self.deezer(
                &format!("/artist/{artist_id}/albums"),
                &[("limit", &PAGE.to_string()), ("index", &index)],
            )?;
            let found = parse_releases(&json);
            let done = found.len() < PAGE as usize || json.get("next").is_none();
            releases.extend(found);
            if done {
                break;
            }
        }
        Ok(releases)
    }

    /// A release's barcode.
    pub fn deezer_upc(&self, album_id: u64) -> Result<Option<String>> {
        let json = self.deezer(&format!("/album/{album_id}"), &[])?;
        Ok(text(&json, "upc"))
    }
}

/// The artist of this name among an album's artists.
pub fn artist_in_album(album: &Value, name: &str) -> Option<u64> {
    let wanted = name.trim().to_lowercase();
    let contributors = album.get("contributors").and_then(Value::as_array);
    contributors
        .into_iter()
        .flatten()
        .chain(album.get("artist"))
        .find(|a| text(a, "name").is_some_and(|n| n.to_lowercase() == wanted))
        .and_then(|a| a.get("id")?.as_u64())
}

/// The artists of exactly this name (ignoring case), with or without a
/// picture, in Deezer's order.
pub fn artists_named(json: &Value, name: &str) -> Vec<DeezerArtist> {
    let wanted = name.trim().to_lowercase();
    json.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|artist| text(artist, "name").is_some_and(|n| n.to_lowercase() == wanted))
        .filter_map(|artist| {
            Some(DeezerArtist {
                id: artist.get("id")?.as_u64()?,
                fans: artist.get("nb_fan").and_then(Value::as_u64).unwrap_or(0),
                picture: text(artist, "picture_xl").unwrap_or_default(),
            })
        })
        .collect()
}

pub fn parse_releases(json: &Value) -> Vec<DeezerRelease> {
    json.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|album| {
            Some(DeezerRelease {
                id: album.get("id")?.as_u64()?,
                title: text(album, "title")?,
                date: text(album, "release_date").unwrap_or_default(),
                kind: text(album, "record_type").unwrap_or_else(|| "album".into()),
                cover_url: text(album, "cover_xl").or_else(|| text(album, "cover_big")),
            })
        })
        .collect()
}

/// The releases from `since` to `today` (YYYY-MM-DD), newest first:
/// no compilations, no upcoming ones, and no reissues (a title the
/// artist released before, under an earlier date); of the same title
/// twice (clean and explicit), one.
pub fn recent_releases(all: &[DeezerRelease], since: &str, today: &str) -> Vec<DeezerRelease> {
    // Every letter and digit, also in brackets: "Blue Hour (Japanese
    // ver.)" is a release of its own.
    let key = |title: &str| -> String {
        title
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let dated = |r: &&DeezerRelease| r.date.len() == 10 && !r.date.starts_with("0000");
    let mut recent: Vec<DeezerRelease> = all
        .iter()
        .filter(dated)
        .filter(|r| r.kind != "compile" && r.date.as_str() >= since && r.date.as_str() <= today)
        .filter(|r| {
            !all.iter()
                .filter(dated)
                .any(|other| other.date < r.date && key(&other.title) == key(&r.title))
        })
        .cloned()
        .collect();
    recent.sort_by(|a, b| b.date.cmp(&a.date).then(a.id.cmp(&b.id)));
    let mut seen = std::collections::HashSet::new();
    recent.retain(|r| seen.insert(key(&r.title)));
    recent
}

/// A Unix time's date (UTC) as YYYY-MM-DD.
pub fn date_string(unix: i64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(id: u64, title: &str, date: &str, kind: &str) -> DeezerRelease {
        DeezerRelease {
            id,
            title: title.into(),
            date: date.into(),
            kind: kind.into(),
            cover_url: None,
        }
    }

    #[test]
    fn dates() {
        assert_eq!(date_string(0), "1970-01-01");
        assert_eq!(date_string(951_782_400), "2000-02-29");
        assert_eq!(date_string(1_794_441_600), "2026-11-12");
        assert_eq!(date_string(1_794_441_600 + 86_399), "2026-11-12");
    }

    #[test]
    fn recent_releases_leave_out_old_upcoming_reissued_and_compilations() {
        // In Deezer's order, which isn't by date.
        let all = [
            release(1, "Reissued", "2015-11-27", "album"),
            release(2, "Blue Hour", "2025-10-13", "album"),
            release(3, "Reissued", "2026-10-30", "album"),
            release(4, "Coming Soon (Japanese ver.)", "2026-12-09", "album"),
            release(5, "Blue Hour (Japanese ver.)", "2026-10-28", "ep"),
            release(6, "Hits", "2026-11-01", "compile"),
            release(7, "Late Night", "2026-11-05", "single"),
            release(8, "Late Night", "2026-11-05", "single"),
            release(9, "Odd Song", "2026-10-21", "single"),
            release(10, "Undated", "0000-00-00", "single"),
            release(11, "Old Single", "2026-10-01", "single"),
        ];
        let titles: Vec<String> = recent_releases(&all, "2026-10-13", "2026-11-12")
            .into_iter()
            .map(|r| format!("{} {}", r.id, r.title))
            .collect();
        assert_eq!(
            titles,
            ["7 Late Night", "5 Blue Hour (Japanese ver.)", "9 Odd Song"]
        );
    }

    #[test]
    fn parsing() {
        let page = json!({"data": [
            {"id": 7, "title": "Echo", "release_date": "2026-11-01",
             "record_type": "single", "cover_xl": "https://cdn/x.jpg"},
            {"id": 8, "title": "", "release_date": "2026-11-01"},
            {"title": "No ID"}
        ], "total": 3});
        assert_eq!(
            parse_releases(&page),
            [DeezerRelease {
                id: 7,
                title: "Echo".into(),
                date: "2026-11-01".into(),
                kind: "single".into(),
                cover_url: Some("https://cdn/x.jpg".into()),
            }]
        );

        let album = json!({
            "contributors": [{"id": 1, "name": "Singer A"}, {"id": 2, "name": "Singer B"}],
            "artist": {"id": 1, "name": "Singer A"}
        });
        assert_eq!(artist_in_album(&album, "singer b"), Some(2));
        assert_eq!(artist_in_album(&album, "Someone Else"), None);
        let solo = json!({"artist": {"id": 5, "name": "Producer"}});
        assert_eq!(artist_in_album(&solo, "Producer"), Some(5));

        let search = json!({"data": [
            {"id": 1, "name": "NOVA", "nb_fan": 900_000, "picture_xl": "https://cdn/k.jpg"},
            {"id": 2, "name": "Nova", "nb_fan": 300},
            {"id": 3, "name": "Novas", "nb_fan": 5}
        ]});
        let found = artists_named(&search, "NOVA");
        assert_eq!(found.iter().map(|a| a.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(found[1].picture, "");
    }
}
