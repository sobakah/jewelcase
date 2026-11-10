//! "Mix in similar songs" and "keep playing similar music" (CONCEPT.md §7):
//! scores library songs against a seed by the chosen criteria and picks one
//! of the best at random, so suggestions don't repeat.

use std::collections::{HashMap, HashSet};

use rand::Rng;
use rand::seq::SliceRandom;

/// What similarity looks at for one song.
#[derive(Debug, Clone, PartialEq)]
pub struct SongFeatures {
    pub track_id: i64,
    pub album_id: Option<i64>,
    /// The album artist (lowercase), for "same artist".
    pub artist: Option<String>,
    /// Lowercase genres.
    pub genres: Vec<String>,
    /// A BPM tag wins over the detected tempo.
    pub bpm: Option<f64>,
    /// bliss features, once analyzed.
    pub sound: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Criteria {
    pub genre: bool,
    pub bpm: bool,
    pub artist: bool,
    pub sound: bool,
    /// "Similar BPM": within this fraction (0.08 = ±8 %).
    pub bpm_tolerance: f64,
}

impl Criteria {
    fn any(&self) -> bool {
        self.genre || self.bpm || self.artist || self.sound
    }
}

/// Related artists (ListenBrainz' similar artists, §7) by artist name
/// (lowercase): the related artist and how close (0..1, 1 the closest).
pub type RelatedArtists = HashMap<String, HashMap<String, f64>>;

/// A related artist counts at most this much of "same artist".
const RELATED_WEIGHT: f64 = 0.6;

/// How many of the best candidates a suggestion is drawn from.
pub const TOP_K: usize = 8;

/// A song like `seed` from `pool`, not in `exclude`; `None` if nothing is
/// left. Without any criteria chosen, genre and sound are used. "Same
/// artist" also counts `related` artists, less.
pub fn suggest(
    seed: &SongFeatures,
    pool: &[SongFeatures],
    exclude: &HashSet<i64>,
    criteria: &Criteria,
    related: &RelatedArtists,
    rng: &mut impl Rng,
) -> Option<i64> {
    let seed_related = seed.artist.as_ref().and_then(|a| related.get(a));
    let criteria = if criteria.any() {
        *criteria
    } else {
        Criteria {
            genre: true,
            sound: true,
            ..*criteria
        }
    };
    let candidates: Vec<&SongFeatures> = pool
        .iter()
        .filter(|c| c.track_id != seed.track_id && !exclude.contains(&c.track_id))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    // Sound distances, scaled by the largest among the candidates.
    let distances: Vec<Option<f32>> = candidates
        .iter()
        .map(|c| match (&seed.sound, &c.sound) {
            (Some(a), Some(b)) if criteria.sound && a.len() == b.len() => Some(distance(a, b)),
            _ => None,
        })
        .collect();
    let max_distance = distances
        .iter()
        .flatten()
        .copied()
        .fold(0.0f32, f32::max)
        .max(f32::EPSILON);
    let mut scored: Vec<(f64, i64)> = candidates
        .iter()
        .zip(&distances)
        .map(|(candidate, distance)| {
            let mut score = 0.0;
            if criteria.genre {
                score += genre_score(seed, candidate);
            }
            if criteria.bpm {
                score += bpm_score(seed.bpm, candidate.bpm, criteria.bpm_tolerance);
            }
            if criteria.artist && seed.artist.is_some() {
                if seed.artist == candidate.artist {
                    score += 1.0;
                } else if let (Some(related), Some(artist)) = (seed_related, &candidate.artist)
                    && let Some(closeness) = related.get(artist)
                {
                    score += RELATED_WEIGHT * closeness.clamp(0.0, 1.0);
                }
            }
            if let Some(distance) = distance {
                score += f64::from(1.0 - distance / max_distance);
            }
            (score, candidate.track_id)
        })
        .collect();
    // Equal scores in random order (the sort keeps it): otherwise the best
    // of many equally good songs (one genre, say) are always the same few,
    // in database order.
    scored.shuffle(rng);
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Prefer real matches; with none, any song keeps the music going.
    let best: Vec<i64> = scored
        .iter()
        .filter(|(score, _)| *score > 0.0)
        .take(TOP_K)
        .map(|(_, id)| *id)
        .collect();
    let pick_from = if best.is_empty() {
        scored.iter().map(|(_, id)| *id).collect()
    } else {
        best
    };
    Some(pick_from[rng.random_range(0..pick_from.len())])
}

/// Share of the seed's genres the candidate has (0..1).
fn genre_score(seed: &SongFeatures, candidate: &SongFeatures) -> f64 {
    if seed.genres.is_empty() {
        return 0.0;
    }
    let shared = seed
        .genres
        .iter()
        .filter(|g| candidate.genres.contains(g))
        .count();
    shared as f64 / seed.genres.len() as f64
}

/// 1 for the same tempo, falling to 0 at the tolerance. Half and double
/// tempo count as close: detectors often report either.
fn bpm_score(seed: Option<f64>, candidate: Option<f64>, tolerance: f64) -> f64 {
    let (Some(a), Some(b)) = (seed, candidate) else {
        return 0.0;
    };
    if a <= 0.0 || b <= 0.0 || tolerance <= 0.0 {
        return 0.0;
    }
    let ratio = [b, b * 2.0, b / 2.0]
        .iter()
        .map(|b| (a - b).abs() / a)
        .fold(f64::INFINITY, f64::min);
    (1.0 - ratio / tolerance).max(0.0)
}

/// Euclidean distance of two bliss feature vectors.
fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    fn song(id: i64, genres: &[&str], bpm: Option<f64>, sound: Option<Vec<f32>>) -> SongFeatures {
        SongFeatures {
            track_id: id,
            album_id: Some(id * 10),
            artist: Some(format!("artist {}", id % 2)),
            genres: genres.iter().map(|g| (*g).to_owned()).collect(),
            bpm,
            sound,
        }
    }

    fn criteria(genre: bool, bpm: bool, artist: bool, sound: bool) -> Criteria {
        Criteria {
            genre,
            bpm,
            artist,
            sound,
            bpm_tolerance: 0.08,
        }
    }

    #[test]
    fn bpm_counts_half_and_double_tempo() {
        assert_eq!(bpm_score(Some(120.0), Some(120.0), 0.08), 1.0);
        assert!(bpm_score(Some(120.0), Some(60.0), 0.08) > 0.99);
        assert!(bpm_score(Some(120.0), Some(125.0), 0.08) > 0.4);
        assert_eq!(bpm_score(Some(120.0), Some(140.0), 0.08), 0.0);
        assert_eq!(bpm_score(None, Some(140.0), 0.08), 0.0);
    }

    #[test]
    fn suggestions_follow_the_criteria() {
        let seed = song(1, &["k-pop"], Some(120.0), Some(vec![0.0, 0.0]));
        let pool = vec![
            seed.clone(),
            song(2, &["k-pop"], Some(90.0), Some(vec![5.0, 5.0])),
            song(3, &["rock"], Some(121.0), Some(vec![0.1, 0.0])),
        ];
        let mut rng = StdRng::seed_from_u64(1);
        let none = HashSet::new();
        // Only genre: song 2 is the only match.
        for _ in 0..10 {
            let pick = suggest(
                &seed,
                &pool,
                &none,
                &criteria(true, false, false, false),
                &RelatedArtists::default(),
                &mut rng,
            );
            assert_eq!(pick, Some(2));
        }
        // Tempo and sound: song 3 scores far higher (but top-k keeps 2 in).
        let mut threes = 0;
        for _ in 0..50 {
            if suggest(
                &seed,
                &pool,
                &none,
                &criteria(false, true, false, true),
                &RelatedArtists::default(),
                &mut rng,
            ) == Some(3)
            {
                threes += 1;
            }
        }
        assert!(threes > 15);
    }

    #[test]
    fn equally_good_songs_all_get_their_turn() {
        let seed = song(1, &["k-pop"], None, None);
        let mut pool = vec![seed.clone()];
        pool.extend((2..22).map(|id| song(id, &["k-pop"], None, None)));
        let mut rng = StdRng::seed_from_u64(3);
        let none = HashSet::new();
        let picked: HashSet<i64> = (0..200)
            .filter_map(|_| {
                suggest(
                    &seed,
                    &pool,
                    &none,
                    &criteria(true, false, false, false),
                    &RelatedArtists::default(),
                    &mut rng,
                )
            })
            .collect();
        // Not just the first TOP_K of the pool.
        assert!(picked.len() > TOP_K, "{picked:?}");
    }

    #[test]
    fn excluded_songs_are_never_suggested() {
        let seed = song(1, &["pop"], None, None);
        let pool = vec![
            seed.clone(),
            song(2, &["pop"], None, None),
            song(3, &["jazz"], None, None),
        ];
        let mut rng = StdRng::seed_from_u64(2);
        let exclude = HashSet::from([2]);
        for _ in 0..10 {
            let pick = suggest(
                &seed,
                &pool,
                &exclude,
                &criteria(true, false, false, false),
                &RelatedArtists::default(),
                &mut rng,
            );
            // No genre match left: any remaining song keeps playing.
            assert_eq!(pick, Some(3));
        }
        let all = HashSet::from([2, 3]);
        assert_eq!(
            suggest(
                &seed,
                &pool,
                &all,
                &criteria(true, false, false, false),
                &RelatedArtists::default(),
                &mut rng
            ),
            None
        );
    }

    #[test]
    fn related_artists_count_less_than_the_same_artist() {
        let artist = |id: i64, name: &str| SongFeatures {
            artist: Some(name.to_owned()),
            ..song(id, &[], None, None)
        };
        let seed = artist(1, "queen");
        let pool = vec![
            seed.clone(),
            artist(2, "david bowie"),
            artist(3, "abba"),
            artist(4, "queen"),
        ];
        let related: RelatedArtists =
            HashMap::from([("queen".into(), HashMap::from([("david bowie".into(), 1.0)]))]);
        let mut rng = StdRng::seed_from_u64(3);
        let only_artist = criteria(false, false, true, false);
        let mut picks = HashSet::new();
        for _ in 0..40 {
            picks.insert(suggest(
                &seed,
                &pool,
                &HashSet::new(),
                &only_artist,
                &related,
                &mut rng,
            ));
        }
        // Queen and Bowie, never ABBA (no relation).
        assert_eq!(picks, HashSet::from([Some(2), Some(4)]));
        // With Queen's song gone, Bowie only.
        let exclude = HashSet::from([4]);
        assert_eq!(
            suggest(&seed, &pool, &exclude, &only_artist, &related, &mut rng),
            Some(2)
        );
    }
}
