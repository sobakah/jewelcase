//! Online artist metadata (CONCEPT.md §8): MusicBrainz for identity and
//! links, Wikidata for the Spotify ID, Wikipedia and the Commons portrait,
//! fanart.tv for portraits (needs a free API key).
//!
//! Everything here is blocking and meant for a background thread. The
//! parsing is separate from the HTTP calls, so it can be tested offline.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

/// MusicBrainz allows one request per second.
const MUSICBRAINZ_INTERVAL: Duration = Duration::from_millis(1100);
/// Downloads larger than this are refused.
const MAX_IMAGE_BYTES: u64 = 15 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] ureq::Error),
    #[error("unexpected answer from {0}")]
    Format(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Which providers may be asked (Preferences, §8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Providers {
    pub musicbrainz: bool,
    pub wikidata: bool,
    /// The fanart.tv project API key; `None` disables fanart.tv.
    pub fanart_key: Option<String>,
}

/// What we know about an artist before looking them up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtistQuery {
    pub name: String,
    /// From the tags or an earlier lookup.
    pub mbid: Option<String>,
    /// Release MBIDs of the artist's albums.
    pub release_mbids: Vec<String>,
    /// Wikipedia language, e.g. "de"; English is the fallback.
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub mbid: String,
    pub name: String,
    pub disambiguation: Option<String>,
    pub country: Option<String>,
    pub kind: Option<String>,
    pub score: u32,
    /// Other names: the sort name ("Ushio, Kensuke" as "Kensuke Ushio") and
    /// aliases. Used to accept a search result.
    pub other_names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Links {
    pub wikidata_id: Option<String>,
    pub spotify_id: Option<String>,
    pub website: Option<String>,
    pub wikipedia_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortraitCandidate {
    pub url: String,
    /// "fanart.tv" or "wikimedia".
    pub source: &'static str,
    /// Author and license, shown in a tooltip (Commons images).
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Found(String),
    /// Several plausible artists: the user picks one ("Identify artist…").
    Ambiguous(Vec<Candidate>),
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistInfo {
    pub identity: Identity,
    pub links: Links,
    pub bio: Option<String>,
    /// In priority order: fanart.tv, then Wikimedia Commons.
    pub portraits: Vec<PortraitCandidate>,
}

pub struct Client {
    agent: ureq::Agent,
    last_musicbrainz: Mutex<Option<Instant>>,
}

impl Client {
    /// `version` goes into the User-Agent that MusicBrainz asks for.
    pub fn new(version: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(format!(
                "Gatefold/{version} ( https://github.com/sobakah/gatefold )"
            ))
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Self {
            agent,
            last_musicbrainz: Mutex::new(None),
        }
    }

    /// Finds the artist and everything the providers say about them.
    pub fn lookup(&self, query: &ArtistQuery, providers: &Providers) -> Result<ArtistInfo> {
        let mut info = ArtistInfo {
            identity: Identity::NotFound,
            links: Links::default(),
            bio: None,
            portraits: Vec::new(),
        };
        if !providers.musicbrainz {
            return Ok(info);
        }
        info.identity = match &query.mbid {
            Some(mbid) => Identity::Found(mbid.clone()),
            None => self.identify(query)?,
        };
        let Identity::Found(mbid) = &info.identity else {
            return Ok(info);
        };
        info.links = self.musicbrainz_links(mbid)?;
        // MusicBrainz often lacks the Wikidata link that Wikidata itself
        // has (its "MusicBrainz artist ID", P434): look it up that way.
        if providers.wikidata && info.links.wikidata_id.is_none() {
            match self.wikidata_by_mbid(mbid) {
                Ok(qid) => info.links.wikidata_id = qid,
                Err(err) => tracing::info!("Wikidata search for {mbid} failed: {err}"),
            }
        }
        info.portraits = self.portrait_candidates(mbid, &info.links, providers);

        if providers.wikidata
            && let Some(qid) = info.links.wikidata_id.clone()
        {
            match self.wikidata(&qid, &query.language) {
                Ok(wikidata) => {
                    info.links.spotify_id = info.links.spotify_id.or(wikidata.spotify_id);
                    info.links.wikipedia_url = wikidata.wikipedia_url.or(info.links.wikipedia_url);
                }
                Err(err) => tracing::info!("Wikidata lookup of {qid} failed: {err}"),
            }
            if let Some(url) = &info.links.wikipedia_url {
                info.bio = self.wikipedia_summary(url).unwrap_or_else(|err| {
                    tracing::info!("Wikipedia summary failed: {err}");
                    None
                });
            }
        }
        Ok(info)
    }

    /// The MBID via the artist's releases (exact), else a confident search.
    fn identify(&self, query: &ArtistQuery) -> Result<Identity> {
        for release in query.release_mbids.iter().take(3) {
            let json =
                self.musicbrainz(&format!("release/{release}"), &[("inc", "artist-credits")])?;
            if let Some(mbid) = artist_from_release(&json, &query.name) {
                return Ok(Identity::Found(mbid));
            }
        }
        let candidates = self.search_artists(&query.name)?;
        Ok(pick_candidate(&query.name, candidates))
    }

    /// MusicBrainz artist search ("Identify artist…").
    pub fn search_artists(&self, name: &str) -> Result<Vec<Candidate>> {
        // Unfielded, so aliases and sort names match too.
        let query = format!("\"{}\"", name.replace('"', " "));
        let json = self.musicbrainz("artist", &[("query", &query), ("limit", "8")])?;
        Ok(parse_search(&json))
    }

    fn musicbrainz_links(&self, mbid: &str) -> Result<Links> {
        let json = self.musicbrainz(&format!("artist/{mbid}"), &[("inc", "url-rels")])?;
        Ok(parse_artist_links(&json))
    }

    /// Portrait candidates from every provider, best first.
    pub fn portrait_candidates(
        &self,
        mbid: &str,
        links: &Links,
        providers: &Providers,
    ) -> Vec<PortraitCandidate> {
        let mut candidates = Vec::new();
        if let Some(key) = &providers.fanart_key {
            match self.json(
                &format!("https://webservice.fanart.tv/v3/music/{mbid}"),
                &[("api_key", key)],
            ) {
                Ok(json) => candidates.extend(parse_fanart(&json)),
                // 404: fanart.tv has nothing for this artist.
                Err(err) => tracing::debug!("fanart.tv: {err}"),
            }
        }
        if providers.wikidata
            && let Some(qid) = &links.wikidata_id
        {
            match self.commons_portrait(qid) {
                Ok(Some(candidate)) => candidates.push(candidate),
                Ok(None) => {}
                Err(err) => tracing::info!("Commons portrait of {qid} failed: {err}"),
            }
        }
        candidates
    }

    /// The Wikidata item that names this MusicBrainz artist (P434), if
    /// exactly one does.
    fn wikidata_by_mbid(&self, mbid: &str) -> Result<Option<String>> {
        let json = self.json(
            "https://www.wikidata.org/w/api.php",
            &[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", &format!("haswbstatement:P434={mbid}")),
                ("srlimit", "2"),
                ("format", "json"),
            ],
        )?;
        Ok(parse_wikidata_search(&json))
    }

    fn wikidata(&self, qid: &str, language: &str) -> Result<WikidataInfo> {
        let json = self.json(
            &format!("https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"),
            &[],
        )?;
        parse_wikidata(&json, qid, language).ok_or(Error::Format("Wikidata"))
    }

    fn commons_portrait(&self, qid: &str) -> Result<Option<PortraitCandidate>> {
        let json = self.json(
            &format!("https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"),
            &[],
        )?;
        let Some(file) = parse_wikidata(&json, qid, "en").and_then(|w| w.image_file) else {
            return Ok(None);
        };
        let json = self.json(
            "https://commons.wikimedia.org/w/api.php",
            &[
                ("action", "query"),
                ("format", "json"),
                ("prop", "imageinfo"),
                ("iiprop", "url|extmetadata"),
                ("iiurlwidth", "800"),
                ("titles", &format!("File:{file}")),
            ],
        )?;
        Ok(parse_commons(&json))
    }

    fn wikipedia_summary(&self, url: &str) -> Result<Option<String>> {
        let Some((host, title)) = url
            .strip_prefix("https://")
            .and_then(|rest| rest.split_once("/wiki/"))
        else {
            return Ok(None);
        };
        // One path segment: "AC/DC" must become "AC%2FDC".
        let json = self.json(
            &format!(
                "https://{host}/api/rest_v1/page/summary/{}",
                path_segment(title)
            ),
            &[],
        )?;
        Ok(json
            .get("extract")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned))
    }

    /// Downloads an image (a portrait candidate).
    pub fn download(&self, url: &str) -> Result<Vec<u8>> {
        let mut response = self.agent.get(url).call()?;
        Ok(response
            .body_mut()
            .with_config()
            .limit(MAX_IMAGE_BYTES)
            .read_to_vec()?)
    }

    fn musicbrainz(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        // One request per second, across threads.
        let mut last = self
            .last_musicbrainz
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = *last {
            let wait = MUSICBRAINZ_INTERVAL.saturating_sub(previous.elapsed());
            std::thread::sleep(wait);
        }
        let mut params = params.to_vec();
        params.push(("fmt", "json"));
        let url = format!("https://musicbrainz.org/ws/2/{path}");
        // 503: MusicBrainz is busy or we were too fast; back off and retry.
        let mut attempt = 0;
        loop {
            let result = self.json(&url, &params);
            *last = Some(Instant::now());
            match result {
                Err(Error::Http(ureq::Error::StatusCode(503))) if attempt < 2 => {
                    attempt += 1;
                    std::thread::sleep(MUSICBRAINZ_INTERVAL * 2 * attempt);
                }
                result => return result,
            }
        }
    }

    fn json(&self, url: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut request = self.agent.get(url).header("Accept", "application/json");
        for (key, value) in params {
            request = request.query(*key, *value);
        }
        let mut response = request.call()?;
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_JSON_BYTES)
            .read_to_string()?;
        serde_json::from_str(&text).map_err(|_| Error::Format("a JSON service"))
    }
}

// Parsing -------------------------------------------------------------------

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The artist credit of a release that matches `name` (or the only one).
pub fn artist_from_release(release: &Value, name: &str) -> Option<String> {
    let credits = release.get("artist-credit")?.as_array()?;
    let artists: Vec<(String, String, Option<String>)> = credits
        .iter()
        .filter_map(|credit| {
            let artist = credit.get("artist")?;
            Some((
                text(artist, "id")?,
                text(artist, "name")?,
                text(credit, "name"),
            ))
        })
        .collect();
    let wanted = name.to_lowercase();
    if let Some((id, _, _)) = artists.iter().find(|(_, artist_name, credited)| {
        artist_name.to_lowercase() == wanted
            || credited
                .as_ref()
                .is_some_and(|c| c.to_lowercase() == wanted)
    }) {
        return Some(id.clone());
    }
    // A single credited artist whose release is tagged with our name.
    match artists.as_slice() {
        [(id, _, _)] => Some(id.clone()),
        _ => None,
    }
}

/// The single item of a Wikidata search, or None (none, or ambiguous).
pub fn parse_wikidata_search(json: &Value) -> Option<String> {
    let hits = json.pointer("/query/search")?.as_array()?;
    match hits.as_slice() {
        [only] => only
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| t.starts_with('Q'))
            .map(str::to_owned),
        _ => None,
    }
}

pub fn parse_search(json: &Value) -> Vec<Candidate> {
    json.get("artists")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|artist| {
            Some(Candidate {
                mbid: text(artist, "id")?,
                name: text(artist, "name")?,
                disambiguation: text(artist, "disambiguation"),
                country: text(artist, "country"),
                kind: text(artist, "type"),
                score: artist.get("score").and_then(Value::as_u64).unwrap_or(0) as u32,
                other_names: text(artist, "sort-name")
                    .map(|sort| unsort(&sort))
                    .into_iter()
                    .chain(
                        artist
                            .get("aliases")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|alias| text(alias, "name")),
                    )
                    .collect(),
            })
        })
        .collect()
}

/// Accepts the best-ranked artist with exactly that name (or alias), if
/// MusicBrainz is confident about it. With several artists of the same
/// name, its ranking (which favours the better-known one) has proven
/// right; "Identify Artist…" picks another (§8).
pub fn pick_candidate(name: &str, candidates: Vec<Candidate>) -> Identity {
    let wanted = name.to_lowercase();
    let same_name: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| {
            c.name.to_lowercase() == wanted
                || c.other_names.iter().any(|n| n.to_lowercase() == wanted)
        })
        .collect();
    match same_name.first() {
        Some(first) if first.score >= 95 => Identity::Found(first.mbid.clone()),
        _ if candidates.is_empty() => Identity::NotFound,
        _ => Identity::Ambiguous(candidates.into_iter().take(5).collect()),
    }
}

pub fn parse_artist_links(artist: &Value) -> Links {
    let mut links = Links::default();
    for relation in artist
        .get("relations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(url) = relation.get("url").and_then(|u| text(u, "resource")) else {
            continue;
        };
        if relation.get("ended").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        match relation.get("type").and_then(Value::as_str) {
            Some("wikidata") => {
                links.wikidata_id = url.rsplit('/').next().map(str::to_owned);
            }
            Some("official homepage") => {
                links.website.get_or_insert(url);
            }
            Some("wikipedia") => {
                links.wikipedia_url.get_or_insert(url);
            }
            _ => {
                if let Some(id) = url
                    .strip_prefix("https://open.spotify.com/artist/")
                    .map(|id| id.split(['?', '/']).next().unwrap_or(id))
                {
                    links.spotify_id.get_or_insert(id.to_owned());
                }
            }
        }
    }
    links
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WikidataInfo {
    /// Commons file name of the image (`P18`).
    pub image_file: Option<String>,
    /// `P1902`.
    pub spotify_id: Option<String>,
    /// In `language`, else English.
    pub wikipedia_url: Option<String>,
}

pub fn parse_wikidata(json: &Value, qid: &str, language: &str) -> Option<WikidataInfo> {
    let entity = json.get("entities")?.get(qid)?;
    let claim = |property: &str| {
        entity
            .get("claims")?
            .get(property)?
            .as_array()?
            .first()?
            .get("mainsnak")?
            .get("datavalue")?
            .get("value")?
            .as_str()
            .map(str::to_owned)
    };
    let sitelink = |lang: &str| {
        let link = entity.get("sitelinks")?.get(format!("{lang}wiki"))?;
        text(link, "url").or_else(|| {
            let title = text(link, "title")?.replace(' ', "_");
            Some(format!("https://{lang}.wikipedia.org/wiki/{title}"))
        })
    };
    Some(WikidataInfo {
        image_file: claim("P18"),
        spotify_id: claim("P1902"),
        wikipedia_url: sitelink(language).or_else(|| sitelink("en")),
    })
}

pub fn parse_commons(json: &Value) -> Option<PortraitCandidate> {
    let pages = json.get("query")?.get("pages")?.as_object()?;
    let info = pages
        .values()
        .next()?
        .get("imageinfo")?
        .as_array()?
        .first()?;
    let url = text(info, "thumburl").or_else(|| text(info, "url"))?;
    let meta = info.get("extmetadata");
    let field = |name: &str| {
        meta.and_then(|m| m.get(name))
            .and_then(|f| text(f, "value"))
            .map(|v| strip_html(&v))
            .filter(|v| !v.is_empty())
    };
    let attribution = match (field("Artist"), field("LicenseShortName")) {
        (Some(artist), Some(license)) => Some(format!("{artist}, {license}, Wikimedia Commons")),
        (None, Some(license)) => Some(format!("{license}, Wikimedia Commons")),
        (Some(artist), None) => Some(format!("{artist}, Wikimedia Commons")),
        (None, None) => Some("Wikimedia Commons".to_owned()),
    };
    Some(PortraitCandidate {
        url,
        source: "wikimedia",
        attribution,
    })
}

pub fn parse_fanart(json: &Value) -> Vec<PortraitCandidate> {
    let mut thumbs: Vec<(u64, String)> = json
        .get("artistthumb")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|thumb| {
            let likes = text(thumb, "likes")
                .and_then(|l| l.parse().ok())
                .unwrap_or(0);
            Some((likes, text(thumb, "url")?))
        })
        .collect();
    thumbs.sort_by_key(|t| std::cmp::Reverse(t.0));
    thumbs
        .into_iter()
        .map(|(_, url)| PortraitCandidate {
            url,
            source: "fanart.tv",
            attribution: Some("fanart.tv".to_owned()),
        })
        .collect()
}

/// "Ushio, Kensuke" → "Kensuke Ushio".
fn unsort(sort_name: &str) -> String {
    match sort_name.split_once(", ") {
        Some((last, first)) if !first.contains(',') => format!("{first} {last}"),
        _ => sort_name.to_owned(),
    }
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A Wikipedia title as one URL path segment: percent-encoded except
/// unreserved characters; `%XX` already in it stays.
fn path_segment(title: &str) -> String {
    let bytes = title.as_bytes();
    let mut out = String::with_capacity(title.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let encoded_already = b == b'%'
            && bytes.len() > i + 2
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit();
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || encoded_already {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikipedia_titles_are_one_path_segment() {
        assert_eq!(path_segment("AC/DC"), "AC%2FDC");
        assert_eq!(path_segment("Stray_Kids"), "Stray_Kids");
        assert_eq!(path_segment("BTS_(band)"), "BTS_%28band%29");
        assert_eq!(path_segment("Bj%C3%B6rk"), "Bj%C3%B6rk");
        assert_eq!(path_segment("Björk"), "Bj%C3%B6rk");
    }
    use serde_json::json;

    fn candidate(mbid: &str, name: &str, score: u32) -> Candidate {
        Candidate {
            mbid: mbid.into(),
            name: name.into(),
            disambiguation: None,
            country: None,
            kind: None,
            score,
            other_names: Vec::new(),
        }
    }

    #[test]
    fn release_credit_matching() {
        let release = json!({"artist-credit": [
            {"name": "Get Well Soon", "artist": {"id": "gws", "name": "Get Well Soon"}, "joinphrase": " & "},
            {"name": "Kat Frankie", "artist": {"id": "kf", "name": "Kat Frankie"}}
        ]});
        assert_eq!(
            artist_from_release(&release, "kat frankie").as_deref(),
            Some("kf")
        );
        assert_eq!(artist_from_release(&release, "Someone"), None);
        let single =
            json!({"artist-credit": [{"name": "BTS", "artist": {"id": "bts", "name": "BTS"}}]});
        assert_eq!(
            artist_from_release(&single, "방탄소년단").as_deref(),
            Some("bts")
        );
    }

    #[test]
    fn wikidata_search_takes_only_a_single_hit() {
        let one = serde_json::json!({"query": {"search": [{"title": "Q62396954"}]}});
        assert_eq!(parse_wikidata_search(&one).as_deref(), Some("Q62396954"));
        let two = serde_json::json!({"query": {"search": [{"title": "Q1"}, {"title": "Q2"}]}});
        assert_eq!(parse_wikidata_search(&two), None);
        let none = serde_json::json!({"query": {"search": []}});
        assert_eq!(parse_wikidata_search(&none), None);
    }

    #[test]
    fn only_confident_search_results_are_accepted() {
        let found = pick_candidate(
            "Adele",
            vec![
                candidate("a", "Adele", 100),
                candidate("b", "Adele Givens", 60),
            ],
        );
        assert_eq!(found, Identity::Found("a".into()));

        let homonyms = vec![
            candidate("n1", "Nirvana", 100),
            candidate("n2", "Nirvana", 100),
        ];
        assert_eq!(
            pick_candidate("Nirvana", homonyms),
            Identity::Found("n1".into())
        );

        let mut native = candidate("ku", "牛尾憲輔", 100);
        native.other_names = vec![unsort("Ushio, Kensuke")];
        assert_eq!(
            pick_candidate("kensuke ushio", vec![native]),
            Identity::Found("ku".into())
        );

        let fuzzy = vec![candidate("x", "Adelle", 90)];
        assert!(matches!(
            pick_candidate("Adele", fuzzy),
            Identity::Ambiguous(_)
        ));
        assert_eq!(pick_candidate("Nobody", Vec::new()), Identity::NotFound);
    }

    #[test]
    fn musicbrainz_links() {
        let artist = json!({"relations": [
            {"type": "wikidata", "url": {"resource": "https://www.wikidata.org/wiki/Q13580495"}},
            {"type": "free streaming", "url": {"resource": "https://open.spotify.com/artist/3Nrfpe0tUJi4K4DXYWgMUX?si=x"}},
            {"type": "official homepage", "url": {"resource": "https://ibighit.com/bts"}},
            {"type": "official homepage", "ended": true, "url": {"resource": "https://old.example"}}
        ]});
        assert_eq!(
            parse_artist_links(&artist),
            Links {
                wikidata_id: Some("Q13580495".into()),
                spotify_id: Some("3Nrfpe0tUJi4K4DXYWgMUX".into()),
                website: Some("https://ibighit.com/bts".into()),
                wikipedia_url: None,
            }
        );
    }

    #[test]
    fn wikidata_entity() {
        let json = json!({"entities": {"Q1": {
            "claims": {
                "P18": [{"mainsnak": {"datavalue": {"value": "BTS 2019.jpg"}}}],
                "P1902": [{"mainsnak": {"datavalue": {"value": "spotify-id"}}}]
            },
            "sitelinks": {
                "enwiki": {"title": "BTS", "url": "https://en.wikipedia.org/wiki/BTS"},
                "dewiki": {"title": "BTS (Band)"}
            }
        }}});
        let info = parse_wikidata(&json, "Q1", "de").unwrap();
        assert_eq!(info.image_file.as_deref(), Some("BTS 2019.jpg"));
        assert_eq!(info.spotify_id.as_deref(), Some("spotify-id"));
        assert_eq!(
            info.wikipedia_url.as_deref(),
            Some("https://de.wikipedia.org/wiki/BTS_(Band)")
        );
        let english = parse_wikidata(&json, "Q1", "fr").unwrap();
        assert_eq!(
            english.wikipedia_url.as_deref(),
            Some("https://en.wikipedia.org/wiki/BTS")
        );
    }

    #[test]
    fn commons_and_fanart() {
        let commons = json!({"query": {"pages": {"123": {"imageinfo": [{
            "url": "https://upload.wikimedia.org/full.jpg",
            "thumburl": "https://upload.wikimedia.org/800px.jpg",
            "extmetadata": {
                "Artist": {"value": "<a href=\"x\">Jane Doe</a>"},
                "LicenseShortName": {"value": "CC BY-SA 4.0"}
            }
        }]}}}});
        let portrait = parse_commons(&commons).unwrap();
        assert_eq!(portrait.url, "https://upload.wikimedia.org/800px.jpg");
        assert_eq!(
            portrait.attribution.as_deref(),
            Some("Jane Doe, CC BY-SA 4.0, Wikimedia Commons")
        );

        let fanart = json!({"artistthumb": [
            {"url": "https://a/1.jpg", "likes": "1"},
            {"url": "https://a/2.jpg", "likes": "5"}
        ]});
        let urls: Vec<_> = parse_fanart(&fanart).into_iter().map(|c| c.url).collect();
        assert_eq!(urls, ["https://a/2.jpg", "https://a/1.jpg"]);
    }
}
