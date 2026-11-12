//! Cover thumbnails in `~/.cache/<app id>/covers/` (CONCEPT.md §4.3).
//!
//! Covers come from files we don't control, so they're decoded by glycin's
//! sandboxed loaders. Only the decoded pixels are resized and written back
//! as JPEG thumbnails, which the UI then loads.

use std::fs;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use glycin::{Loader, MainContextSelector, MemoryFormatSelection};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

/// Thumbnail sizes (longest side in pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoverSize {
    /// Album grid tiles and the mini player.
    Grid,
    /// The album side panel.
    Panel,
    /// The full player view and MPRIS.
    Player,
}

impl CoverSize {
    pub const ALL: [Self; 3] = [Self::Grid, Self::Panel, Self::Player];

    pub fn pixels(self) -> u32 {
        match self {
            Self::Grid => 360,
            Self::Panel => 720,
            Self::Player => 1200,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    #[error("cannot decode cover: {0}")]
    Decode(String),
    #[error("cannot write cover thumbnail: {0}")]
    Io(#[from] io::Error),
    #[error("cannot encode cover thumbnail: {0}")]
    Encode(#[from] image::ImageError),
}

/// File names of album-folder covers, in priority order (without extension).
const FOLDER_COVER_NAMES: &[&str] = &["cover", "folder", "front"];
const FOLDER_COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Clone)]
pub struct CoverCache {
    dir: PathBuf,
}

impl CoverCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Removes cached images whose key starts with one of `kinds` (e.g.
    /// 'e', 'f' album covers, 's' Spotify images, 'm' mosaics) and isn't
    /// in `keep`, once they're a day old (one just made may not be
    /// referred to yet). Returns how many.
    pub fn remove_unused(&self, keep: &std::collections::HashSet<String>, kinds: &[char]) -> usize {
        const MIN_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut removed = std::collections::HashSet::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // "<key>-<size>.jpg" or "<key>.color".
            let key = name.split(['-', '.']).next().unwrap_or_default();
            if !key.starts_with(kinds) || keep.contains(key) {
                continue;
            }
            let young = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_none_or(|age| age < MIN_AGE);
            if young {
                continue;
            }
            if fs::remove_file(entry.path()).is_ok() {
                removed.insert(key.to_owned());
            }
        }
        removed.len()
    }

    /// The thumbnail to show at `size`: the largest one stored up to that
    /// size. A picture smaller than a size isn't stored again under the
    /// bigger name, it would be the same picture (see `write_thumbnails`).
    /// Without any thumbnail, the grid one's (missing) path.
    pub fn path(&self, key: &str, size: CoverSize) -> PathBuf {
        CoverSize::ALL
            .iter()
            .rev()
            .filter(|s| s.pixels() <= size.pixels())
            .map(|&s| self.size_path(key, s))
            .find(|path| path.is_file())
            .unwrap_or_else(|| self.size_path(key, CoverSize::Grid))
    }

    fn size_path(&self, key: &str, size: CoverSize) -> PathBuf {
        self.dir.join(format!("{key}-{}.jpg", size.pixels()))
    }

    /// Whether the thumbnails are made. The grid one is written last.
    pub fn has(&self, key: &str) -> bool {
        self.size_path(key, CoverSize::Grid).is_file()
    }

    /// Removes thumbnails that only repeat the next smaller size: earlier
    /// versions wrote a small picture again under every bigger name (a
    /// 640 px Spotify image as 720 and 1200 px). Returns how many.
    pub fn remove_repeated_sizes(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let grid = format!("-{}.jpg", CoverSize::Grid.pixels());
        let keys: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                name.strip_suffix(grid.as_str()).map(str::to_owned)
            })
            .collect();
        let mut removed = 0;
        for key in keys {
            // Biggest first, so a small picture loses both repeats.
            for (smaller, bigger) in [
                (CoverSize::Panel, CoverSize::Player),
                (CoverSize::Grid, CoverSize::Panel),
            ] {
                let path = self.size_path(&key, bigger);
                let repeats = image::image_dimensions(&path)
                    .is_ok_and(|(w, h)| w.max(h) <= smaller.pixels())
                    && self.size_path(&key, smaller).is_file();
                if repeats && fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }

    fn color_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.color"))
    }

    /// The cover's dominant color (CONCEPT.md §4.3), once computed.
    pub fn color(&self, key: &str) -> Option<[u8; 3]> {
        let text = fs::read_to_string(self.color_path(key)).ok()?;
        parse_hex(text.trim())
    }

    /// Computes the dominant color from the (trusted) grid thumbnail, for
    /// covers made before colors existed. Returns false without a thumbnail.
    pub fn ensure_color(&self, key: &str) -> bool {
        if self.color(key).is_some() {
            return true;
        }
        let Ok(image) = image::open(self.path(key, CoverSize::Grid)) else {
            return false;
        };
        self.write_color(key, &image).is_ok()
    }

    fn write_color(&self, key: &str, image: &DynamicImage) -> io::Result<()> {
        let [r, g, b] = dominant_color(image);
        fs::write(self.color_path(key), format!("#{r:02x}{g:02x}{b:02x}\n"))
    }

    /// Decodes `data` in glycin's sandbox and writes all thumbnail sizes.
    pub fn generate(&self, key: &str, data: Vec<u8>) -> Result<(), CoverError> {
        let image = decode(data)?;
        self.write_thumbnails(key, &image)
    }

    /// A 2×2 mosaic of up to four covers (playlists, §4.1), made from their
    /// panel thumbnails. Fewer covers repeat. Returns the mosaic's key.
    pub fn mosaic(&self, keys: &[String]) -> Result<String, CoverError> {
        let key = self.mosaic_key(keys);
        let keys: Vec<&String> = keys.iter().take(4).collect();
        if self.has(&key) {
            return Ok(key);
        }
        let tiles: Vec<DynamicImage> = keys
            .iter()
            .filter_map(|k| image::open(self.path(k, CoverSize::Panel)).ok())
            .collect();
        if tiles.is_empty() {
            return Err(CoverError::Decode(
                "no cover thumbnails for the mosaic".into(),
            ));
        }
        let half = CoverSize::Panel.pixels() / 2;
        let mut canvas = image::RgbImage::new(half * 2, half * 2);
        for (i, (x, y)) in [(0, 0), (half, 0), (0, half), (half, half)]
            .into_iter()
            .enumerate()
        {
            // 1 cover: all four; 2: diagonal; 3: the first repeats.
            let tile = &tiles[match tiles.len() {
                2 => [0, 1, 1, 0][i],
                n => i % n,
            }];
            let tile = tile
                .resize_to_fill(half, half, FilterType::CatmullRom)
                .to_rgb8();
            image::imageops::overlay(&mut canvas, &tile, i64::from(x), i64::from(y));
        }
        self.write_thumbnails(&key, &DynamicImage::ImageRgb8(canvas))?;
        Ok(key)
    }

    /// The key `mosaic` stores the mosaic of these covers under.
    pub fn mosaic_key(&self, keys: &[String]) -> String {
        let joined = keys
            .iter()
            .take(4)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("+");
        cover_key('m', joined.as_bytes())
    }

    /// A cached cover as a JPEG of at most `max_bytes` and 640 px (what
    /// Spotify takes as a playlist cover): from the panel thumbnail,
    /// with less quality until it fits.
    pub fn upload_jpeg(&self, key: &str, max_bytes: usize) -> Result<Vec<u8>, CoverError> {
        let path = self.path(key, CoverSize::Panel);
        let image = image::open(&path).map_err(|e| CoverError::Decode(e.to_string()))?;
        let image = image.resize(640, 640, FilterType::Lanczos3).to_rgb8();
        for quality in [90, 80, 70, 60, 50, 40] {
            let mut data = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, quality);
            DynamicImage::ImageRgb8(image.clone()).write_with_encoder(encoder)?;
            if data.len() <= max_bytes {
                return Ok(data);
            }
        }
        Err(CoverError::Decode("too large even at low quality".into()))
    }

    fn write_thumbnails(&self, key: &str, image: &DynamicImage) -> Result<(), CoverError> {
        fs::create_dir_all(&self.dir)?;
        let longest = image.width().max(image.height());
        // Biggest first: the grid thumbnail is the sign they're all made
        // (`has`).
        for (i, size) in CoverSize::ALL.into_iter().enumerate().rev() {
            // A size the picture doesn't reach would repeat the one below;
            // `path` shows that one instead.
            if i > 0 && longest <= CoverSize::ALL[i - 1].pixels() {
                continue;
            }
            let pixels = size.pixels();
            let scaled = if longest > pixels {
                image.resize(pixels, pixels, FilterType::CatmullRom)
            } else {
                image.clone()
            };
            let path = self.size_path(key, size);
            let partial = path.with_extension("jpg.part");
            {
                let file = BufWriter::new(fs::File::create(&partial)?);
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 90);
                DynamicImage::ImageRgb8(scaled.to_rgb8()).write_with_encoder(encoder)?;
            }
            fs::rename(&partial, &path)?;
        }
        self.write_color(key, image)?;
        Ok(())
    }
}

fn decode(data: Vec<u8>) -> Result<DynamicImage, CoverError> {
    let decode_error = |err: glycin::Error| CoverError::Decode(err.to_string());
    futures_lite::future::block_on(async move {
        let mut loader = Loader::new_vec(data);
        loader
            .accepted_memory_formats(MemoryFormatSelection::R8g8b8a8)
            .main_context_selector(MainContextSelector::Managed);
        let mut image = loader.load().await.map_err(decode_error)?;
        let frame = image.next_frame().await.map_err(decode_error)?;

        let (width, height) = (frame.width(), frame.height());
        let stride = frame.stride() as usize;
        let row = width as usize * 4;
        let buf = frame.buf_slice();
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = y * stride;
            pixels.extend_from_slice(buf.get(start..start + row).ok_or_else(|| {
                CoverError::Decode("frame buffer is smaller than its size".into())
            })?);
        }
        RgbaImage::from_raw(width, height, pixels)
            .map(DynamicImage::ImageRgba8)
            .ok_or_else(|| CoverError::Decode("frame buffer is smaller than its size".into()))
    })
}

/// The most common color, preferring colorful over gray pixels: pixels go
/// into 3-bit-per-channel buckets weighted by chroma² (so near-black,
/// near-white and gray pixels count little); the heaviest bucket's
/// average wins.
/// Mostly black-and-white covers stay gray.
pub fn dominant_color(image: &DynamicImage) -> [u8; 3] {
    let small = image.thumbnail(48, 48).to_rgb8();
    let mut buckets: std::collections::HashMap<u16, (f64, [f64; 3])> = Default::default();
    for pixel in small.pixels() {
        let [r, g, b] = pixel.0;
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let chroma = f64::from(max - min) / 255.0;
        let weight = 0.002 + chroma * chroma;
        let bucket = (u16::from(r >> 5) << 6) | (u16::from(g >> 5) << 3) | u16::from(b >> 5);
        let entry = buckets.entry(bucket).or_insert((0.0, [0.0; 3]));
        entry.0 += weight;
        for (sum, channel) in entry.1.iter_mut().zip([r, g, b]) {
            *sum += weight * f64::from(channel);
        }
    }
    let Some((weight, sums)) = buckets
        .into_values()
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .filter(|(weight, _)| *weight > 0.0)
    else {
        return [128, 128, 128];
    };
    sums.map(|sum| (sum / weight).round().clamp(0.0, 255.0) as u8)
}

fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// A cache key for cover bytes: `<prefix><FNV-1a 64 hash>`.
pub fn cover_key(prefix: char, data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{prefix}{hash:016x}")
}

/// `cover.*`, `folder.*` or `front.*` in a folder (case-insensitive, no symlinks).
pub fn find_folder_cover(folder: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(folder).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        let (stem, ext) = (stem.to_lowercase(), ext.to_lowercase());
        if !FOLDER_COVER_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        if let Some(rank) = FOLDER_COVER_NAMES.iter().position(|n| *n == stem)
            && best.as_ref().is_none_or(|(r, _)| rank < *r)
        {
            best = Some((rank, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([200, 40, 90]),
        ))
    }

    fn stored(cache: &CoverCache, key: &str) -> Vec<u32> {
        CoverSize::ALL
            .iter()
            .filter(|&&s| cache.size_path(key, s).is_file())
            .map(|s| s.pixels())
            .collect()
    }

    #[test]
    fn small_pictures_are_stored_once_per_size_they_reach() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = CoverCache::new(dir.path());
        // Spotify's 640 px: grid and panel; the player shows the panel's.
        cache.write_thumbnails("s640", &picture(640, 640)).unwrap();
        assert_eq!(stored(&cache, "s640"), [360, 720]);
        assert!(cache.has("s640"));
        assert_eq!(
            cache.path("s640", CoverSize::Player),
            cache.size_path("s640", CoverSize::Panel)
        );
        assert_eq!(
            image::image_dimensions(cache.path("s640", CoverSize::Player)).unwrap(),
            (640, 640)
        );
        // Smaller than the grid size: one thumbnail for everything.
        cache.write_thumbnails("s300", &picture(300, 200)).unwrap();
        assert_eq!(stored(&cache, "s300"), [360]);
        assert_eq!(
            cache.path("s300", CoverSize::Player),
            cache.size_path("s300", CoverSize::Grid)
        );
        // Exactly a size: not repeated at the next one.
        cache.write_thumbnails("e720", &picture(720, 500)).unwrap();
        assert_eq!(stored(&cache, "e720"), [360, 720]);
        // Big enough for all three.
        cache
            .write_thumbnails("e1500", &picture(1500, 1500))
            .unwrap();
        assert_eq!(stored(&cache, "e1500"), [360, 720, 1200]);
        assert_eq!(
            image::image_dimensions(cache.path("e1500", CoverSize::Player)).unwrap(),
            (1200, 1200)
        );
        // Nothing stored: the grid path, missing.
        assert!(!cache.has("missing"));
        assert!(!cache.path("missing", CoverSize::Player).is_file());
    }

    #[test]
    fn repeated_sizes_from_earlier_versions_are_removed() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = CoverCache::new(dir.path());
        // What earlier versions wrote: the picture under every bigger name.
        let old = |key: &str, image: &DynamicImage| {
            for size in CoverSize::ALL {
                let pixels = size.pixels();
                let scaled = image.resize(pixels, pixels, FilterType::Triangle);
                let scaled = if image.width().max(image.height()) > pixels {
                    scaled
                } else {
                    image.clone()
                };
                scaled.save(cache.size_path(key, size)).unwrap();
            }
        };
        old("s640", &picture(640, 640));
        old("s300", &picture(300, 300));
        old("e1500", &picture(1500, 1000));
        assert_eq!(cache.remove_repeated_sizes(), 3);
        assert_eq!(stored(&cache, "s640"), [360, 720]);
        assert_eq!(stored(&cache, "s300"), [360]);
        assert_eq!(stored(&cache, "e1500"), [360, 720, 1200]);
        assert_eq!(cache.remove_repeated_sizes(), 0);
    }

    #[test]
    fn unused_covers_are_removed_and_others_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = CoverCache::new(dir.path());
        for name in [
            "eused-360.jpg",
            "eused.color",
            "eold-360.jpg",
            "eold-720.jpg",
            "eold.color",
            "sgone-360.jpg",
            "mmosaic-360.jpg",
            "cchosen-360.jpg",
        ] {
            let path = dir.path().join(name);
            fs::write(&path, b"x").unwrap();
            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 3600);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        // Made just now: kept even though unused.
        fs::write(dir.path().join("enew-360.jpg"), b"x").unwrap();
        let keep: std::collections::HashSet<String> = ["eused".to_owned()].into();
        assert_eq!(cache.remove_unused(&keep, &['e', 'f', 's']), 2);
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "cchosen-360.jpg",
                "enew-360.jpg",
                "eused-360.jpg",
                "eused.color",
                "mmosaic-360.jpg"
            ]
        );
    }

    #[test]
    fn keys_are_stable() {
        assert_eq!(cover_key('e', b""), "ecbf29ce484222325");
        assert_ne!(cover_key('e', b"a"), cover_key('e', b"b"));
    }

    #[test]
    fn dominant_colors() {
        // Mostly red with a white border: red wins over white.
        let mut image = image::RgbImage::from_pixel(40, 40, image::Rgb([255, 255, 255]));
        for x in 5..35 {
            for y in 5..35 {
                image.put_pixel(x, y, image::Rgb([200, 30, 30]));
            }
        }
        let [r, g, b] = dominant_color(&DynamicImage::ImageRgb8(image));
        assert!(r > 150 && g < 80 && b < 80, "{r} {g} {b}");

        // Dark pants and hair on a pale cover: the skin tone wins, not black.
        let mut photo = image::RgbImage::from_pixel(40, 40, image::Rgb([240, 240, 245]));
        for x in 0..40 {
            for y in 20..40 {
                photo.put_pixel(x, y, image::Rgb([20, 2, 6]));
            }
            for y in 8..20 {
                photo.put_pixel(x, y, image::Rgb([220, 170, 150]));
            }
        }
        let [r, g, _] = dominant_color(&DynamicImage::ImageRgb8(photo));
        assert!(r > 150 && g > 120, "{r} {g}");

        let gray = image::RgbImage::from_pixel(10, 10, image::Rgb([90, 90, 90]));
        assert_eq!(dominant_color(&DynamicImage::ImageRgb8(gray)), [90, 90, 90]);
        assert_eq!(parse_hex("#0a0b0c"), Some([10, 11, 12]));
    }

    #[test]
    fn mosaics() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = CoverCache::new(dir.path());
        let colors = [[255, 0, 0], [0, 255, 0]];
        let keys: Vec<String> = colors
            .iter()
            .enumerate()
            .map(|(i, rgb)| {
                let key = format!("t{i}");
                let tile =
                    DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb(*rgb)));
                cache.write_thumbnails(&key, &tile).unwrap();
                key
            })
            .collect();
        let mosaic = cache.mosaic(&keys).unwrap();
        assert!(cache.has(&mosaic));
        let image = image::open(cache.path(&mosaic, CoverSize::Panel))
            .unwrap()
            .to_rgb8();
        let [r, g, _] = image.get_pixel(10, 10).0;
        assert!(r > 200 && g < 60, "top left is the first cover");
        let [r, g, _] = image.get_pixel(image.width() - 10, 10).0;
        assert!(g > 200 && r < 60, "top right is the second cover");
        assert_eq!(cache.mosaic(&keys).unwrap(), mosaic, "cached");
        assert!(cache.mosaic(&["missing".into()]).is_err());
    }

    #[test]
    fn folder_cover_priority() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(find_folder_cover(dir.path()), None);
        fs::write(dir.path().join("Front.PNG"), b"").unwrap();
        fs::write(dir.path().join("notes.txt"), b"").unwrap();
        assert_eq!(
            find_folder_cover(dir.path()),
            Some(dir.path().join("Front.PNG"))
        );
        fs::write(dir.path().join("cover.jpg"), b"").unwrap();
        assert_eq!(
            find_folder_cover(dir.path()),
            Some(dir.path().join("cover.jpg"))
        );
    }
}
