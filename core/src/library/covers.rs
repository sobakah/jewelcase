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

    pub fn path(&self, key: &str, size: CoverSize) -> PathBuf {
        self.dir.join(format!("{key}-{}.jpg", size.pixels()))
    }

    pub fn has(&self, key: &str) -> bool {
        CoverSize::ALL
            .iter()
            .all(|&size| self.path(key, size).is_file())
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

    fn write_thumbnails(&self, key: &str, image: &DynamicImage) -> Result<(), CoverError> {
        fs::create_dir_all(&self.dir)?;
        for size in CoverSize::ALL {
            let pixels = size.pixels();
            let scaled = if image.width() > pixels || image.height() > pixels {
                image.resize(pixels, pixels, FilterType::CatmullRom)
            } else {
                image.clone()
            };
            let path = self.path(key, size);
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
