//! The length of a FLAC file whose header doesn't say (STREAMINFO's total
//! samples is 0, "unknown", which encoders writing a stream may leave):
//! found from the last audio frame, whose header numbers it. Only the
//! file's tail is read.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of the file's end is searched for frame headers.
const TAIL: u64 = 256 * 1024;

/// What the stream holds, as far as the length goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLength {
    pub samples: u64,
    pub sample_rate: u32,
    /// Bytes of audio (after the metadata blocks), for the bitrate.
    pub audio_bytes: u64,
}

impl StreamLength {
    pub fn duration_ms(&self) -> u64 {
        self.samples * 1000 / u64::from(self.sample_rate.max(1))
    }

    /// Average bitrate in kbit/s.
    pub fn bitrate(&self) -> Option<u32> {
        let ms = self.duration_ms();
        (ms > 0).then(|| u32::try_from(self.audio_bytes * 8 / ms).unwrap_or(u32::MAX))
    }
}

/// The file's length from its last frame; `None` when it isn't FLAC or no
/// sure frame header is found.
pub fn stream_length(path: &Path) -> Option<StreamLength> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let (info, audio_start) = read_stream_info(&mut file)?;
    let audio_bytes = size.checked_sub(audio_start)?;
    let from = size.saturating_sub(TAIL).max(audio_start);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut tail = Vec::new();
    file.take(size - from).read_to_end(&mut tail).ok()?;
    let samples = samples_from_tail(&tail, &info)?;
    Some(StreamLength {
        samples,
        sample_rate: info.sample_rate,
        audio_bytes,
    })
}

#[derive(Debug, Clone, Copy)]
struct StreamInfo {
    max_block: u32,
    sample_rate: u32,
}

/// STREAMINFO and where the audio starts (after an ID3v2 tag, if any).
fn read_stream_info(file: &mut File) -> Option<(StreamInfo, u64)> {
    let mut head = [0u8; 10];
    file.read_exact(&mut head[..4]).ok()?;
    let mut pos = 0u64;
    if &head[..3] == b"ID3" {
        file.read_exact(&mut head[4..10]).ok()?;
        let size = head[6..10]
            .iter()
            .fold(0u64, |n, b| (n << 7) | u64::from(b & 0x7f));
        let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
        pos = 10 + size + footer;
        file.seek(SeekFrom::Start(pos)).ok()?;
        file.read_exact(&mut head[..4]).ok()?;
    }
    if &head[..4] != b"fLaC" {
        return None;
    }
    pos += 4;
    let mut info = None;
    loop {
        let mut block = [0u8; 4];
        file.read_exact(&mut block).ok()?;
        let last = block[0] & 0x80 != 0;
        let kind = block[0] & 0x7f;
        let length = u64::from(u32::from_be_bytes([0, block[1], block[2], block[3]]));
        pos += 4;
        if kind == 0 && length >= 18 {
            let mut data = [0u8; 18];
            file.read_exact(&mut data).ok()?;
            let max_block = u32::from(u16::from_be_bytes([data[2], data[3]]));
            let sample_rate =
                (u32::from(data[10]) << 12) | (u32::from(data[11]) << 4) | u32::from(data[12] >> 4);
            info = Some(StreamInfo {
                max_block,
                sample_rate,
            });
            file.seek(SeekFrom::Current(length as i64 - 18)).ok()?;
        } else {
            file.seek(SeekFrom::Current(length as i64)).ok()?;
        }
        pos += length;
        if last {
            break;
        }
    }
    let info = info.filter(|i| i.sample_rate > 0 && i.max_block > 0)?;
    Some((info, pos))
}

/// A frame header found in the data.
#[derive(Debug, Clone, Copy)]
struct Frame {
    offset: usize,
    /// The first sample's number in the stream.
    first: u64,
    block: u32,
}

/// The total samples: the end of the last frame that follows on from the
/// frame before it (two in a row are no accident in the audio data).
fn samples_from_tail(tail: &[u8], info: &StreamInfo) -> Option<u64> {
    let frames: Vec<Frame> = (0..tail.len().saturating_sub(1))
        .filter(|&i| tail[i] == 0xff && tail[i + 1] & 0xfe == 0xf8)
        .filter_map(|i| frame_header(&tail[i..], info).map(|(first, block)| (i, first, block)))
        .map(|(offset, first, block)| Frame {
            offset,
            first,
            block,
        })
        .collect();
    frames.iter().rev().find_map(|last| {
        frames
            .iter()
            .any(|f| f.offset < last.offset && f.first + u64::from(f.block) == last.first)
            .then(|| last.first + u64::from(last.block))
    })
}

/// A frame header's first sample and block size, if its checksum holds.
fn frame_header(data: &[u8], info: &StreamInfo) -> Option<(u64, u32)> {
    if data.len() < 6 {
        return None;
    }
    let variable = data[1] & 1 != 0;
    let block_code = data[2] >> 4;
    let rate_code = data[2] & 0x0f;
    let channels = data[3] >> 4;
    let size_code = (data[3] >> 1) & 0x07;
    if block_code == 0 || rate_code == 15 || channels > 10 || size_code == 3 || data[3] & 1 != 0 {
        return None;
    }
    // The frame (fixed blocks) or sample (variable) number, UTF-8 coded.
    let lead = data[4].leading_ones() as usize;
    let (mut number, extra) = match lead {
        0 => (u64::from(data[4]), 0),
        2..=7 => (u64::from(data[4] & (0x7f >> lead)), lead - 1),
        _ => return None,
    };
    let mut at = 5;
    for _ in 0..extra {
        let byte = *data.get(at)?;
        if byte & 0xc0 != 0x80 {
            return None;
        }
        number = (number << 6) | u64::from(byte & 0x3f);
        at += 1;
    }
    let block = match block_code {
        1 => 192,
        2..=5 => 576 << (block_code - 2),
        6 => {
            at += 1;
            u32::from(*data.get(at - 1)?) + 1
        }
        7 => {
            at += 2;
            u32::from(u16::from_be_bytes([*data.get(at - 2)?, *data.get(at - 1)?])) + 1
        }
        _ => 256 << (block_code - 8),
    };
    at += match rate_code {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    if crc8(data.get(..at)?) != *data.get(at)? {
        return None;
    }
    let first = if variable {
        number
    } else {
        number * u64::from(info.max_block)
    };
    Some((first, block))
}

/// CRC-8 of frame headers (polynomial x⁸ + x² + x + 1).
fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut crc, &byte| {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
        crc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed-block frame header numbered `frame` (one byte), 4096
    /// samples, with its checksum, and some made-up audio after it.
    fn frame(frame: u8, block_code: u8) -> Vec<u8> {
        let mut header = vec![0xff, 0xf8, (block_code << 4) | 0x09, 0x18, frame];
        header.push(crc8(&header));
        header.extend([0x12, 0x34, 0x56, 0xff, 0x00]);
        header
    }

    #[test]
    fn finds_the_last_frames_end() {
        let info = StreamInfo {
            max_block: 4096,
            sample_rate: 44100,
        };
        let mut tail = vec![0xff, 0xf8, 0x00];
        tail.extend(frame(40, 12));
        tail.extend(frame(41, 12));
        // The last frame is shorter: 1152 samples (code 3).
        tail.extend(frame(42, 3));
        assert_eq!(samples_from_tail(&tail, &info), Some(42 * 4096 + 1152));
    }

    #[test]
    fn a_lone_header_is_not_trusted() {
        let info = StreamInfo {
            max_block: 4096,
            sample_rate: 44100,
        };
        let mut tail = frame(40, 12);
        tail.extend(frame(97, 12));
        assert_eq!(samples_from_tail(&tail, &info), None);
    }

    #[test]
    fn crc8_matches_the_reference() {
        // The FLAC reference's check value for "123456789".
        assert_eq!(crc8(b"123456789"), 0xf4);
    }
}
