//! Online artist metadata (CONCEPT.md §8): MusicBrainz for identity and
//! links, Wikidata for the Spotify ID, Wikipedia and the Commons portrait,
//! Deezer for portraits (by name, checked against the artist's albums).
//! Spotify's portraits come from the app, which has the account.
//!
//! Everything here is blocking and meant for a background thread. The
//! parsing is separate from the HTTP calls, so it can be tested offline.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Deezer artists of the same name whose albums are compared.
const DEEZER_COMPARED: usize = 3;
/// MusicBrainz allows one request per second.
const MUSICBRAINZ_INTERVAL: Duration = Duration::from_millis(1100);
/// ListenBrainz asks for at most one request a second.
const LISTENBRAINZ_INTERVAL: Duration = Duration::from_millis(1100);
/// Downloads larger than this are refused.
const MAX_IMAGE_BYTES: u64 = 15 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] ureq::Error),
    /// The service asks to wait (429, `Retry-After`), or is failing (5xx).
    #[error("the service asks to wait {0:?}")]
    Busy(Duration),
    #[error("unexpected answer from {0}")]
    Format(&'static str),
}

impl Error {
    /// The service refused this request itself (4xx other than "too many
    /// requests"): asking again won't help, other requests may work.
    pub fn is_refused(&self) -> bool {
        matches!(self, Self::Http(ureq::Error::StatusCode(status))
            if (400..500).contains(status) && *status != 429)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Which providers may be asked (Preferences, §8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Providers {
    pub musicbrainz: bool,
    pub wikidata: bool,
    /// Deezer's artist pictures (no account or key needed).
    pub deezer: bool,
}

/// What we know about an artist before looking them up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtistQuery {
    pub name: String,
    /// From the tags or an earlier lookup.
    pub mbid: Option<String>,
    /// Release MBIDs of the artist's albums.
    pub release_mbids: Vec<String>,
    /// The artist's album titles in the library, to tell artists of the
    /// same name apart on Deezer.
    pub album_titles: Vec<String>,
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
    /// "spotify", "deezer" or "wikimedia".
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
    /// In priority order: Deezer, then Wikimedia Commons.
    pub portraits: Vec<PortraitCandidate>,
    /// A portrait source couldn't be asked (offline, Deezer's quota):
    /// worth trying again soon.
    pub incomplete: bool,
}

pub struct Client {
    agent: ureq::Agent,
    last_musicbrainz: Mutex<Option<Instant>>,
    last_listenbrainz: Mutex<Option<Instant>>,
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
            last_listenbrainz: Mutex::new(None),
        }
    }

    /// Finds the artist and everything the providers say about them.
    pub fn lookup(&self, query: &ArtistQuery, providers: &Providers) -> Result<ArtistInfo> {
        let mut info = ArtistInfo {
            identity: Identity::NotFound,
            links: Links::default(),
            bio: None,
            portraits: Vec::new(),
            incomplete: false,
        };
        if providers.musicbrainz {
            info.identity = match &query.mbid {
                Some(mbid) => Identity::Found(mbid.clone()),
                None => self.identify(query)?,
            };
            if let Identity::Found(mbid) = info.identity.clone() {
                self.links_and_bio(&mbid, query, providers, &mut info)?;
            }
        }
        // Deezer goes by name: also for artists MusicBrainz doesn't know.
        (info.portraits, info.incomplete) = self.portrait_candidates(query, &info.links, providers);
        Ok(info)
    }

    fn links_and_bio(
        &self,
        mbid: &str,
        query: &ArtistQuery,
        providers: &Providers,
        info: &mut ArtistInfo,
    ) -> Result<()> {
        info.links = self.musicbrainz_links(mbid)?;
        // MusicBrainz often lacks the Wikidata link that Wikidata itself
        // has (its "MusicBrainz artist ID", P434): look it up that way.
        if providers.wikidata && info.links.wikidata_id.is_none() {
            match self.wikidata_by_mbid(mbid) {
                Ok(qid) => info.links.wikidata_id = qid,
                Err(err) => tracing::info!("Wikidata search for {mbid} failed: {err}"),
            }
        }
        if providers.wikidata
            && let Some(qid) = info.links.wikidata_id.clone()
        {
            match self.wikidata(&qid, &query.language) {
                Ok(wikidata) => {
                    info.links.spotify_id = info.links.spotify_id.take().or(wikidata.spotify_id);
                    info.links.wikipedia_url =
                        wikidata.wikipedia_url.or(info.links.wikipedia_url.take());
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
        Ok(())
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

    /// Portrait candidates from every provider, best first, and whether
    /// a provider couldn't be asked.
    pub fn portrait_candidates(
        &self,
        query: &ArtistQuery,
        links: &Links,
        providers: &Providers,
    ) -> (Vec<PortraitCandidate>, bool) {
        let mut candidates = Vec::new();
        let mut incomplete = false;
        if providers.deezer {
            match self.deezer_portrait(query) {
                Ok(Some(candidate)) => candidates.push(candidate),
                Ok(None) => {}
                Err(err) => {
                    tracing::info!("Deezer portrait of {} failed: {err}", query.name);
                    incomplete = true;
                }
            }
        }
        if providers.wikidata
            && let Some(qid) = &links.wikidata_id
        {
            match self.commons_portrait(qid) {
                Ok(Some(candidate)) => candidates.push(candidate),
                Ok(None) => {}
                Err(err) => {
                    tracing::info!("Commons portrait of {qid} failed: {err}");
                    incomplete = true;
                }
            }
        }
        (candidates, incomplete)
    }

    /// The artist's picture on Deezer: an artist of exactly this name,
    /// and of several, the one with the library's albums.
    fn deezer_portrait(&self, query: &ArtistQuery) -> Result<Option<PortraitCandidate>> {
        let json = self.json(
            "https://api.deezer.com/search/artist",
            &[("q", &query.name), ("limit", "10")],
        )?;
        deezer_error(&json)?;
        let found = parse_deezer_artists(&json, &query.name);
        let wanted: Vec<String> = query.album_titles.iter().map(|t| album_key(t)).collect();
        let mut ranked = Vec::new();
        for artist in found.iter().take(DEEZER_COMPARED) {
            // One of that name: no need to compare.
            let overlap = if found.len() == 1 || wanted.is_empty() {
                0
            } else {
                let albums = self.json(
                    &format!("https://api.deezer.com/artist/{}/albums", artist.id),
                    &[("limit", "100")],
                )?;
                deezer_error(&albums)?;
                album_overlap(&albums, &wanted)
            };
            ranked.push((overlap, artist));
        }
        Ok(pick_deezer(&ranked).map(|artist| PortraitCandidate {
            url: artist.picture.clone(),
            source: "deezer",
            attribution: Some("Deezer".to_owned()),
        }))
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

    /// The ISRCs of a release's songs (one request): (disc, track
    /// number, ISRC), the first ISRC of each recording.
    pub fn release_isrcs(&self, release_mbid: &str) -> Result<Vec<(u32, u32, String)>> {
        let json = self.musicbrainz(
            &format!("release/{release_mbid}"),
            &[("inc", "recordings+isrcs")],
        )?;
        Ok(parse_release_isrcs(&json))
    }

    /// Related artists from ListenBrainz' similar-artists dataset (labs
    /// API, no account): (MusicBrainz ID, score), the closest first. At
    /// most one request a second, as ListenBrainz asks; a 429 or 5xx is
    /// `Error::Busy` (with `X-RateLimit-Reset-In` if given).
    pub fn similar_artists(&self, mbid: &str) -> Result<Vec<(String, i64)>> {
        const ALGORITHM: &str = "session_based_days_7500_session_300_contribution_5_threshold_10_limit_100_filter_True_skip_30";
        let mut last = self
            .last_listenbrainz
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = *last {
            std::thread::sleep(LISTENBRAINZ_INTERVAL.saturating_sub(previous.elapsed()));
        }
        let result = self
            .agent
            .get("https://labs.api.listenbrainz.org/similar-artists/json")
            .config()
            .http_status_as_error(false)
            .build()
            .header("Accept", "application/json")
            .query("artist_mbids", mbid)
            .query("algorithm", ALGORITHM)
            .call();
        *last = Some(Instant::now());
        let mut response = result?;
        match response.status().as_u16() {
            200 => {}
            // Unknown to ListenBrainz.
            400 | 404 => return Ok(Vec::new()),
            429 | 500..=599 => {
                let wait = ["x-ratelimit-reset-in", "retry-after"]
                    .iter()
                    .find_map(|name| {
                        response
                            .headers()
                            .get(*name)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.trim().parse::<u64>().ok())
                    })
                    .unwrap_or(0);
                return Err(Error::Busy(Duration::from_secs(wait)));
            }
            _ => return Err(Error::Format("ListenBrainz")),
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_JSON_BYTES)
            .read_to_string()?;
        let json: Value = serde_json::from_str(&text).map_err(|_| Error::Format("ListenBrainz"))?;
        Ok(parse_similar_artists(&json, mbid))
    }

    /// Lyrics from LRCLIB (`/api/get`: title, artist, album, length in
    /// seconds). `Ok(None)`: LRCLIB doesn't have the song. A 429 or 5xx
    /// is `Error::Busy` with how long to wait (`Retry-After`, if given).
    pub fn lrclib(
        &self,
        title: &str,
        artist: &str,
        album: Option<&str>,
        duration_s: u64,
    ) -> Result<Option<OnlineLyrics>> {
        let mut request = self
            .agent
            .get("https://lrclib.net/api/get")
            .config()
            .http_status_as_error(false)
            .build()
            .header("Accept", "application/json")
            .query("track_name", title)
            .query("artist_name", artist)
            .query("duration", duration_s.to_string());
        if let Some(album) = album.filter(|a| !a.is_empty()) {
            request = request.query("album_name", album);
        }
        let mut response = request.call()?;
        let status = response.status().as_u16();
        match status {
            404 => return Ok(None),
            429 | 500..=599 => {
                let wait = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                return Err(Error::Busy(Duration::from_secs(wait)));
            }
            200 => {}
            _ => return Err(Error::Format("LRCLIB")),
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_JSON_BYTES)
            .read_to_string()?;
        let json: Value = serde_json::from_str(&text).map_err(|_| Error::Format("LRCLIB"))?;
        Ok(Some(parse_lrclib(&json)))
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
/// The similar artists ListenBrainz lists for `mbid` (its own entry left
/// out), closest first.
pub fn parse_similar_artists(json: &Value, mbid: &str) -> Vec<(String, i64)> {
    let mut list: Vec<(String, i64)> = json
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry["reference_mbid"]
                .as_str()
                .is_none_or(|reference| reference == mbid)
        })
        .filter_map(|entry| {
            let id = entry["artist_mbid"].as_str()?;
            (id != mbid).then(|| (id.to_owned(), entry["score"].as_i64().unwrap_or(0)))
        })
        .collect();
    list.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    list
}

/// Lyrics from an online source (LRCLIB).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OnlineLyrics {
    /// LRC text with timestamps.
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub instrumental: bool,
}

pub fn parse_lrclib(json: &Value) -> OnlineLyrics {
    OnlineLyrics {
        synced: text(json, "syncedLyrics"),
        plain: text(json, "plainLyrics"),
        instrumental: json["instrumental"].as_bool().unwrap_or(false),
    }
}

pub fn parse_release_isrcs(release: &Value) -> Vec<(u32, u32, String)> {
    let number = |value: &Value| value.as_u64().and_then(|n| u32::try_from(n).ok());
    let mut isrcs = Vec::new();
    for medium in release["media"].as_array().into_iter().flatten() {
        let Some(disc) = number(&medium["position"]) else {
            continue;
        };
        for track in medium["tracks"].as_array().into_iter().flatten() {
            let Some(position) = number(&track["position"]) else {
                continue;
            };
            if let Some(isrc) = track["recording"]["isrcs"]
                .as_array()
                .and_then(|list| list.iter().find_map(Value::as_str))
                .map(|isrc| isrc.trim().to_uppercase())
                .filter(|isrc| isrc.len() == 12)
            {
                isrcs.push((disc, position, isrc));
            }
        }
    }
    isrcs
}

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

/// A Deezer artist from a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeezerArtist {
    pub id: u64,
    pub fans: u64,
    /// The largest picture (1000 px).
    pub picture: String,
}

/// Deezer answers errors (e.g. "Quota limit exceeded") with HTTP 200 and
/// an `error` object.
fn deezer_error(json: &Value) -> Result<()> {
    match json.get("error") {
        Some(error) => {
            tracing::info!("Deezer: {error}");
            Err(Error::Format("Deezer"))
        }
        None => Ok(()),
    }
}

/// The artists of exactly this name (ignoring case) that have a picture,
/// in Deezer's order.
pub fn parse_deezer_artists(json: &Value, name: &str) -> Vec<DeezerArtist> {
    let wanted = name.trim().to_lowercase();
    json.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|artist| text(artist, "name").is_some_and(|n| n.trim().to_lowercase() == wanted))
        .filter_map(|artist| {
            let picture = text(artist, "picture_xl").or_else(|| text(artist, "picture_big"))?;
            // Artists without a picture have an empty image ID here.
            if picture.contains("/images/artist//") {
                return None;
            }
            Some(DeezerArtist {
                id: artist.get("id")?.as_u64()?,
                fans: artist.get("nb_fan").and_then(Value::as_u64).unwrap_or(0),
                picture,
            })
        })
        .collect()
}

/// How many of the library's albums (`wanted`, see `album_key`) a Deezer
/// album list has.
pub fn album_overlap(json: &Value, wanted: &[String]) -> usize {
    let titles: std::collections::HashSet<String> = json
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|album| text(album, "title"))
        .map(|title| album_key(&title))
        .collect();
    wanted.iter().filter(|w| titles.contains(*w)).count()
}

/// The artist to use, from (shared albums, artist): with one of the name,
/// that one; with several, the one sharing the most albums, or, sharing
/// none, one that is far better known than the others. Else none (a wrong
/// face is worse than none).
pub fn pick_deezer<'a>(ranked: &[(usize, &'a DeezerArtist)]) -> Option<&'a DeezerArtist> {
    if let [(_, only)] = ranked {
        return Some(only);
    }
    let best = ranked
        .iter()
        .max_by_key(|(overlap, artist)| (*overlap, artist.fans))?;
    if best.0 > 0 {
        return Some(best.1);
    }
    let mut fans: Vec<u64> = ranked.iter().map(|(_, a)| a.fans).collect();
    fans.sort_unstable_by(|a, b| b.cmp(a));
    (fans.len() > 1 && fans[0] >= 10 * fans[1].max(1)).then_some(best.1)
}

/// An album title for comparing: lowercase, without "(Deluxe)"-style
/// additions and punctuation.
pub fn album_key(title: &str) -> String {
    let mut depth = 0u32;
    let mut key = String::new();
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 && c.is_alphanumeric() => key.extend(c.to_lowercase()),
            _ => {}
        }
    }
    key
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
    fn commons_portrait_with_attribution() {
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
    }

    #[test]
    fn deezer_artists_are_matched_by_name_and_albums() {
        let search = json!({"data": [
            {"id": 1, "name": "Stray Kids", "nb_fan": 1_000_000,
             "picture_xl": "https://cdn/images/artist/aaa/1000x1000.jpg"},
            {"id": 2, "name": "Stray Kids (스트레이 키즈)", "nb_fan": 1700,
             "picture_xl": "https://cdn/images/artist/bbb/1000x1000.jpg"},
            {"id": 3, "name": "stray kids", "nb_fan": 300,
             "picture_xl": "https://cdn/images/artist/ccc/1000x1000.jpg"},
            {"id": 4, "name": "Stray Kids", "nb_fan": 5,
             "picture_xl": "https://cdn/images/artist//1000x1000.jpg"}
        ]});
        let found = parse_deezer_artists(&search, "Stray Kids");
        // Exact names only, and only with a picture.
        assert_eq!(found.iter().map(|a| a.id).collect::<Vec<_>>(), [1, 3]);

        let wanted = vec![album_key("ODDINARY"), album_key("5-STAR (Deluxe)")];
        let real = json!({"data": [{"title": "5-STAR"}, {"title": "THE SOUND"}]});
        let other = json!({"data": [{"title": "Dejavu"}]});
        assert_eq!(album_overlap(&real, &wanted), 1);
        assert_eq!(album_overlap(&other, &wanted), 0);

        let (big, small) = (&found[0], &found[1]);
        // Shared albums decide, even against fame.
        assert_eq!(pick_deezer(&[(0, big), (1, small)]).unwrap().id, 3);
        // None shared: the far better known one.
        assert_eq!(pick_deezer(&[(0, big), (0, small)]).unwrap().id, 1);
        // None shared, both alike: neither.
        let alike = DeezerArtist {
            fans: 250,
            ..small.clone()
        };
        assert!(pick_deezer(&[(0, small), (0, &alike)]).is_none());
        // The only one of that name.
        assert_eq!(pick_deezer(&[(0, small)]).unwrap().id, 3);
    }

    #[test]
    fn release_isrcs_by_disc_and_number() {
        let release = serde_json::json!({
            "media": [
                {"position": 1, "tracks": [
                    {"position": 1, "number": "A1", "recording": {"isrcs": ["kra382000123", "USX1"]}},
                    {"position": 2, "recording": {"isrcs": []}},
                ]},
                {"position": 2, "tracks": [
                    {"position": 1, "recording": {"isrcs": ["GBAYE0000001"]}},
                ]},
            ]
        });
        assert_eq!(
            parse_release_isrcs(&release),
            [
                (1, 1, "KRA382000123".to_owned()),
                (2, 1, "GBAYE0000001".to_owned())
            ]
        );
    }

    #[test]
    fn similar_artists_closest_first() {
        let json = serde_json::json!([
            {"artist_mbid": "b", "name": "B", "score": 5, "reference_mbid": "q"},
            {"artist_mbid": "a", "name": "A", "score": 9, "reference_mbid": "q"},
            {"artist_mbid": "q", "name": "Q", "score": 99, "reference_mbid": "q"},
        ]);
        assert_eq!(
            parse_similar_artists(&json, "q"),
            [("a".to_owned(), 9), ("b".to_owned(), 5)]
        );
    }
}
