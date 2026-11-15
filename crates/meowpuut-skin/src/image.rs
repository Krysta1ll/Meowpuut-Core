//! PNG checking and re-encoding (§3.6 「圖」, §5.2).
//!
//! Accepted input: an 8-bit RGBA, non-interlaced PNG whose first chunk is
//! IHDR. The IHDR is read by hand and its width and height checked against
//! the caller's exact size **before** the decoder allocates anything; the
//! decoder then runs under an 8 MiB allocation limit. Unknown critical
//! chunks, PLTE and a second IHDR are refused; ancillary chunks (text, ICC
//! profiles, APNG `acTL`/`fcTL`/`fdAT`, …) and bytes after IEND are ignored
//! and do not survive: only the default image is decoded.
//!
//! Output ([`encode`]): IHDR, IDAT, IEND only, Paeth filter on every row,
//! zlib level 6 (flate2 on miniz_oxide). The bytes depend only on the pixels
//! and the pinned `png`/`flate2`/`miniz_oxide` versions, so the developer's
//! tool and every client produce the same bytes and the signed SHA-256 holds.
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::image::{decode, encode, Rgba};
//! let img = Rgba { width: 3, height: 2, pixels: vec![200; 24] };
//! let png = encode(&img).unwrap();
//! assert_eq!(decode(&png, 3, 2).unwrap(), img);
//! assert!(decode(&png, 4, 2).is_err());
//! ```

use std::io::Cursor;

use crate::{fail, limits, Code, Error, Result};

/// The PNG signature.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A decoded RGBA image (8 bits per channel, rows top to bottom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// Alpha at (x, y); 0 outside.
    pub fn alpha(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return 0;
        }
        self.pixels[((y as usize) * (self.width as usize) + x as usize) * 4 + 3]
    }

    /// Decoded size in bytes.
    pub fn bytes(&self) -> usize {
        self.pixels.len()
    }
}

fn bad(what: impl Into<String>) -> Error {
    Error::new(Code::Image, what)
}

/// Walks the chunk list: IHDR first and once, no PLTE, no unknown critical
/// chunk, an IEND. Returns (width, height) from the IHDR.
pub fn check_chunks(bytes: &[u8]) -> Result<(u32, u32)> {
    if bytes.len() < 8 + 25 || bytes[..8] != SIGNATURE {
        return Err(bad("not a PNG"));
    }
    let ihdr = &bytes[8..33];
    if ihdr[..8] != [0, 0, 0, 13, b'I', b'H', b'D', b'R'] {
        return Err(bad("IHDR is not the first chunk"));
    }
    let width = u32::from_be_bytes([ihdr[8], ihdr[9], ihdr[10], ihdr[11]]);
    let height = u32::from_be_bytes([ihdr[12], ihdr[13], ihdr[14], ihdr[15]]);
    // bit depth 8, colour type 6 (RGBA), compression 0, filter 0, interlace 0
    if ihdr[16..21] != [8, 6, 0, 0, 0] {
        return Err(bad("not 8-bit RGBA without interlacing"));
    }
    let mut at = 8usize;
    let mut chunks = 0;
    let mut idat = false;
    loop {
        chunks += 1;
        if chunks > 4096 {
            return Err(bad("too many chunks"));
        }
        let head = bytes.get(at..at + 8).ok_or_else(|| bad("truncated"))?;
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let kind: [u8; 4] = [head[4], head[5], head[6], head[7]];
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(bad("chunk type"));
        }
        let end = at.checked_add(12).and_then(|v| v.checked_add(len)).ok_or_else(|| bad("chunk length"))?;
        if end > bytes.len() {
            return Err(bad("truncated"));
        }
        match &kind {
            b"IHDR" if at != 8 => return Err(bad("second IHDR")),
            b"IHDR" => {}
            b"IDAT" => idat = true,
            b"IEND" => break,
            b"PLTE" => return Err(bad("PLTE")),
            _ if kind[0].is_ascii_uppercase() => return Err(bad("unknown critical chunk")),
            _ => {}
        }
        at = end;
    }
    if !idat {
        return Err(bad("no IDAT"));
    }
    Ok((width, height))
}

/// Decodes a PNG that must be exactly `width` × `height` pixels.
pub fn decode(bytes: &[u8], width: u32, height: u32) -> Result<Rgba> {
    let (w, h) = check_chunks(bytes)?;
    if (w, h) != (width, height) {
        return Err(bad(format!("{w}×{h} pixels, expected {width}×{height}")));
    }
    let need = (width as usize).checked_mul(height as usize).and_then(|p| p.checked_mul(4)).ok_or_else(|| bad("size"))?;
    if need > limits::PNG_DECODER / 2 {
        return fail(Code::TooLarge, "image");
    }
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: limits::PNG_DECODER });
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().map_err(|e| bad(format!("decoder: {e}")))?;
    let info = reader.info();
    if (info.width, info.height) != (width, height)
        || info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
        || info.interlaced
    {
        return Err(bad("decoder disagrees with IHDR"));
    }
    let mut pixels = vec![0u8; need];
    let frame = reader.next_frame(&mut pixels).map_err(|e| bad(format!("decoder: {e}")))?;
    if frame.buffer_size() != need || frame.width != width || frame.height != height {
        return Err(bad("frame size"));
    }
    Ok(Rgba { width, height, pixels })
}

/// Re-encodes an image as IHDR, IDAT and IEND only (see the module documentation).
pub fn encode(image: &Rgba) -> Result<Vec<u8>> {
    if image.pixels.len() != image.width as usize * image.height as usize * 4 || image.width == 0 || image.height == 0 {
        return Err(bad("pixel buffer"));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_deflate_compression(png::DeflateCompression::Level(6));
        encoder.set_filter(png::Filter::Paeth);
        let mut writer = encoder.write_header().map_err(|e| bad(format!("encoder: {e}")))?;
        writer.write_image_data(&image.pixels).map_err(|e| bad(format!("encoder: {e}")))?;
        writer.finish().map_err(|e| bad(format!("encoder: {e}")))?;
    }
    Ok(out)
}

/// The chunk types of a PNG in order (for tests and the developer tools).
pub fn chunk_types(bytes: &[u8]) -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    let mut at = 8usize;
    while let Some(head) = bytes.get(at..at + 8) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        out.push([head[4], head[5], head[6], head[7]]);
        if &head[4..8] == b"IEND" {
            break;
        }
        at = at.saturating_add(12).saturating_add(len);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_writes_only_the_three_chunks() {
        let img = Rgba { width: 9, height: 7, pixels: (0..9 * 7 * 4).map(|i| (i * 31 % 251) as u8).collect() };
        let png = encode(&img).unwrap();
        let types = chunk_types(&png);
        assert_eq!(types.first(), Some(b"IHDR"));
        assert_eq!(types.last(), Some(b"IEND"));
        assert!(types[1..types.len() - 1].iter().all(|t| t == b"IDAT"));
        assert_eq!(decode(&png, 9, 7).unwrap(), img);
        assert_eq!(encode(&img).unwrap(), png);
    }
}
