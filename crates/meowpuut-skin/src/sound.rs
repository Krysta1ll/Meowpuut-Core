//! WAV checking, measuring and re-encoding (§3.6 「聲音」, §5.2).
//!
//! Accepted input: RIFF/WAVE with a `fmt ` chunk (PCM, 1 channel, 44.1 kHz,
//! 16 bits) before exactly one `data` chunk; other chunks are skipped and do
//! not survive. Output ([`encode`]): the fixed 44-byte header and the samples.
//!
//! Levels follow `scripts/key_sounds.py`: samples are read as `s / 32768`,
//! the A-weighted level is the A-weighted energy (zero-padded FFT, at least
//! 8192 points, one-sided power) as a mean square over 100 ms in dB.
//!
//! # Examples
//!
//! ```
//! use meowpuut_skin::sound::{encode, parse};
//! let wav = encode(&[0, 100, -100, 0]);
//! assert_eq!(wav.len(), 44 + 8);
//! assert_eq!(parse(&wav).unwrap(), vec![0, 100, -100, 0]);
//! ```

use crate::{Code, Error, Result};

/// Sample rate.
pub const RATE: u32 = 44_100;

fn bad(what: impl Into<String>) -> Error {
    Error::new(Code::Sound, what)
}

/// Parses a WAV into its samples (see the module documentation).
pub fn parse(bytes: &[u8]) -> Result<Vec<i16>> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not RIFF/WAVE"));
    }
    let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if riff < 4 || riff.checked_add(8).is_none_or(|end| end > bytes.len()) {
        return Err(bad("RIFF length"));
    }
    let body = &bytes[..8 + riff];
    let mut at = 12usize;
    let mut format_ok = false;
    let mut samples: Option<Vec<i16>> = None;
    let mut chunks = 0;
    while at < body.len() {
        chunks += 1;
        if chunks > 64 {
            return Err(bad("too many chunks"));
        }
        let head = body.get(at..at + 8).ok_or_else(|| bad("truncated chunk"))?;
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let data_at = at + 8;
        let end = data_at.checked_add(len).filter(|e| *e <= body.len()).ok_or_else(|| bad("chunk length"))?;
        let data = &body[data_at..end];
        match &head[..4] {
            b"fmt " => {
                if format_ok || len < 16 {
                    return Err(bad("fmt chunk"));
                }
                let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
                let u32_at = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
                if u16_at(0) != 1 || u16_at(2) != 1 || u32_at(4) != RATE || u32_at(8) != RATE * 2 || u16_at(12) != 2 || u16_at(14) != 16 {
                    return Err(bad("not 16-bit mono PCM at 44.1 kHz"));
                }
                format_ok = true;
            }
            b"data" => {
                if !format_ok || samples.is_some() {
                    return Err(bad("data chunk before fmt or twice"));
                }
                if !len.is_multiple_of(2) {
                    return Err(bad("odd data length"));
                }
                samples = Some(data.chunks_exact(2).map(|p| i16::from_le_bytes([p[0], p[1]])).collect());
            }
            _ => {}
        }
        at = end + (len & 1);
    }
    let samples = samples.ok_or_else(|| bad("no data chunk"))?;
    if samples.is_empty() {
        return Err(bad("no samples"));
    }
    Ok(samples)
}

/// The canonical WAV: 44-byte header and the samples.
pub fn encode(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Measurements of one sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    /// Duration in milliseconds.
    pub duration_ms: f64,
    /// Peak in dBFS.
    pub peak_db: f64,
    /// DC power in dB.
    pub dc_db: f64,
    /// A-weighted level in dB (re full scale, mean square over 100 ms).
    pub la_db: f64,
    /// Last time the 2 ms envelope is within 40 dB of its top, in ms.
    pub gone_ms: f64,
}

fn db(v: f64) -> f64 {
    10.0 * v.max(1e-12).log10()
}

fn a_weight(f: f64) -> f64 {
    let f2 = f * f;
    let r = 12194f64.powi(2) * f2 * f2
        / ((f2 + 20.6f64.powi(2)) * ((f2 + 107.7f64.powi(2)) * (f2 + 737.9f64.powi(2))).sqrt() * (f2 + 12194f64.powi(2)));
    (r * 10f64.powf(2.0 / 20.0)).powi(2)
}

/// In-place iterative radix-2 FFT.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let half = size / 2;
        let step = -2.0 * std::f64::consts::PI / size as f64;
        for start in (0..n).step_by(size) {
            for k in 0..half {
                let (s, c) = (step * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + half);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        size *= 2;
    }
}

/// Measures one sound.
pub fn measure(samples: &[i16]) -> Levels {
    let x: Vec<f64> = samples.iter().map(|&s| f64::from(s) / 32768.0).collect();
    let n = x.len().max(1);
    let peak = x.iter().fold(0f64, |m, v| m.max(v.abs()));
    let mean = x.iter().sum::<f64>() / n as f64;
    let size = n.next_power_of_two().max(8192);
    let mut re = vec![0f64; size];
    let mut im = vec![0f64; size];
    re[..x.len()].copy_from_slice(&x);
    fft(&mut re, &mut im);
    let mut energy = 0f64;
    for k in 1..=size / 2 {
        let p = (re[k] * re[k] + im[k] * im[k]) / size as f64 * if k == size / 2 { 1.0 } else { 2.0 };
        energy += p * a_weight(k as f64 * f64::from(RATE) / size as f64);
    }
    // 2 ms RMS envelope for the decay time.
    let h = (0.002 * f64::from(RATE) / 2.0) as usize;
    let mut sums = vec![0f64; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        sums[i + 1] = sums[i] + v * v;
    }
    let env: Vec<f64> = (0..x.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(h), (i + h).min(x.len()));
            ((sums[b] - sums[a]) / (b - a).max(1) as f64).sqrt()
        })
        .collect();
    let top = env.iter().fold(0f64, |m, v| m.max(*v));
    let floor = top * 10f64.powf(-40.0 / 20.0);
    let last = env.iter().rposition(|e| *e >= floor && top > 0.0).unwrap_or(0);
    Levels {
        duration_ms: x.len() as f64 * 1000.0 / f64::from(RATE),
        peak_db: 20.0 * peak.max(1e-12).log10(),
        dc_db: db(mean * mean),
        la_db: db(energy / (0.1 * f64::from(RATE))),
        gone_ms: last as f64 * 1000.0 / f64::from(RATE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f64, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
                (amplitude * w * (2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 32767.0).round() as i16
            })
            .collect()
    }

    #[test]
    fn a_weighting_is_zero_db_at_1khz() {
        assert!((db(a_weight(1000.0))).abs() < 0.01);
        assert!(db(a_weight(100.0)) < -18.0);
    }

    #[test]
    fn a_full_scale_sine_measures_as_expected() {
        // 100 ms of a 1 kHz sine at amplitude 1: mean square 0.5 → -3 dB.
        let x: Vec<i16> =
            (0..4410).map(|i| ((2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44100.0).sin() * 32767.0) as i16).collect();
        let m = measure(&x);
        assert!((m.la_db + 3.0).abs() < 0.2, "{}", m.la_db);
        assert!(m.peak_db.abs() < 0.01);
    }

    #[test]
    fn rewrite_drops_extra_chunks() {
        let samples = tone(0.07, 2205);
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        wav.extend_from_slice(b"LIST\x05\0\0\0hello\0");
        let clean = encode(&samples);
        wav.extend_from_slice(&clean[12..]);
        wav.extend_from_slice(b"junk\x02\0\0\0zz");
        let riff = (wav.len() - 8) as u32;
        wav[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(parse(&wav).unwrap(), samples);
        assert_eq!(encode(&parse(&wav).unwrap()), clean);
    }

    #[test]
    fn refuses_other_formats() {
        let clean = encode(&[0, 1, 0]);
        for (at, value) in [(20usize, 3u8), (22, 2), (24, 0x22), (34, 8)] {
            let mut w = clean.clone();
            w[at] = value;
            assert!(parse(&w).is_err(), "byte {at}");
        }
        let mut w = clean.clone();
        w[4] = 0xFF;
        assert!(parse(&w).is_err());
        assert!(parse(&clean[..43]).is_err());
    }
}
