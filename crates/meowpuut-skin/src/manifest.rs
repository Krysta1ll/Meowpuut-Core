//! The validator (§3.6) and the canonical rewrite of a skin (§5.2).
//!
//! The one validator of the project: the developer's tool runs it before
//! issuing, the client when redeeming. Loading does not run it again; it
//! checks the signature and hashes instead (§5.4).
//!
//! # manifest.json, schema 1 (input; every key listed, no other)
//!
//! ```text
//! {"schema":1,"id":"<skin id>","rev":<u32>,"serial":"<16 lowercase hex>",
//!  "names":{"hant":…,"hans":…,"ja":…,"en":…},                         1–24 characters each
//!  "base":"current"|"classic"|"pixel",
//!  "festivals":"suppress"|"allow",                                       optional, default suppress; allow only on current
//!  "palette":{"light":P,"dark":P,"contrastPill":"#RRGGBB"},
//!      P = {"paper","paper2","ink","inkSecondary","pill":[top,bottom],"pillText","stamp","accent"} (#RRGGBB)
//!  "glass":{"shape":"capsule"|"wave"|"cloud","lockShape":bool,"strength":"soft"|"balanced"|"vivid","deckTint":0–0.12},
//!  "keys":{"tint":0–0.22,"rim":bool,"light":K,"dark":K},
//!      K = {"character","utility","text"} (#RRGGBB)
//!  "cats":{"seating":"perched"|"edges",
//!          "sprites":{"<EDGE_SPRITES name>":{"image":"<stem>","size":[w,h],"anchor":[x,y]},…},   optional, any subset
//!          "trim":{"image":"<stem>","size":[w,h],"caps":[t,l,b,r],"safeText":[t,l,b,r]},         optional
//!          "ends":{"left":{"image","size"},"right":{"image","size"}}},                           optional, either side optional
//!  "sounds":{"files":{"<role>":["<stem>",…],…},"gainDb":-12–3}}                                  optional
//! ```
//!
//! An image stem names `<stem>.png`, a sound stem `<stem>.wav` whose part
//! before the first `-` is its role. Every archive file must be used
//! exactly once. `bounds`, `cut`, `paws` and `rim` are not accepted from the
//! manifest: they are measured from the pixels and written into the
//! rewritten manifest.
//!
//! # Output
//!
//! The rewritten files: every PNG re-encoded ([`crate::image::encode`]),
//! every WAV re-written ([`crate::sound::encode`]) and `manifest.json` as
//! canonical JSON of the validated fields plus the measurements (colours in
//! upper case, `festivals` always present).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::archive::Entry;
use crate::codec::{canonical_json, hex_array};
use crate::image::{decode, encode, Rgba};
use crate::names::{file_name, is_display_name, is_name, FileKind};
use crate::sound;
use crate::{fail, limits, Code, Error, Result};

/// Cat sprites a skin may replace (`EDGE_SPRITES` + whole-body interaction poses) and their kind.
pub const EDGE_SPRITES: [(&str, Pose); 14] = [
    ("mimimo_edge_rest", Pose::Top),
    ("mimimo_edge_peek", Pose::Top),
    ("mimimo_edge_lounge", Pose::Top),
    ("mimimo_side_hold", Pose::Side),
    ("mimimo_grab", Pose::Whole),
    ("mimimo_happy", Pose::Whole),
    ("mimimo_sleep", Pose::Whole),
    ("momo_edge_rest", Pose::Top),
    ("momo_edge_peek", Pose::Top),
    ("momo_edge_lounge", Pose::Top),
    ("momo_side_hold", Pose::Side),
    ("momo_grab", Pose::Whole),
    ("momo_happy", Pose::Whole),
    ("momo_sleep", Pose::Whole),
];

/// Sound roles.
pub const ROLES: [&str; 11] = ["delete", "key", "key0", "key1", "key2", "key3", "key4", "key5", "pick", "return", "space"];

/// Supported keystroke particle shapes for `keys.particles.kind`.
pub const PARTICLE_KINDS: [&str; 3] = ["petal", "sparkle", "snow"];

/// How a sprite sits on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    /// Paws on the bar's upper edge, body cut flat on the anchor row.
    Top,
    /// Holding the bar's end at the anchor.
    Side,
    /// Whole-body interactive pose (grab, happy, sleep) with transparent margins.
    Whole,
}

/// Pixels per point: images are @3x only.
pub const SCALE: u32 = 3;
const OPAQUE: u8 = 128;
const VISIBLE: u8 = 24;
const RIM_ALPHA: u8 = 60;

/// The validated, rewritten skin.
#[derive(Debug, Clone)]
pub struct Rewritten {
    /// Every installed file, sorted by name, `manifest.json` included.
    pub files: Vec<(String, Vec<u8>)>,
    /// The per-recipient serial.
    pub serial: String,
    /// The base skin.
    pub base: String,
    /// Display names by language.
    pub names: BTreeMap<String, String>,
    /// Problems that do not stop the skin (sound body and decay times).
    pub warnings: Vec<String>,
}

fn err(path: &str, what: impl std::fmt::Display) -> Error {
    Error::new(Code::Manifest, format!("{path}: {what}"))
}

fn object<'a>(value: &'a Value, path: &str, required: &[&str], optional: &[&str]) -> Result<&'a Map<String, Value>> {
    let map = value.as_object().ok_or_else(|| err(path, "not an object"))?;
    for key in map.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(err(path, format!("unknown field {key:?}")));
        }
    }
    for key in required {
        if !map.contains_key(*key) {
            return Err(err(path, format!("missing {key}")));
        }
    }
    Ok(map)
}

fn choice<'a>(value: &'a Value, path: &str, allowed: &[&str]) -> Result<&'a str> {
    value.as_str().filter(|s| allowed.contains(s)).ok_or_else(|| err(path, format!("one of {allowed:?}")))
}

fn boolean(value: &Value, path: &str) -> Result<bool> {
    value.as_bool().ok_or_else(|| err(path, "true or false"))
}

/// A finite number in `lo..=hi` with at most `decimals` decimals.
fn number(value: &Value, path: &str, lo: f64, hi: f64, decimals: i32) -> Result<f64> {
    let v = value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| err(path, "a number"))?;
    if !(lo..=hi).contains(&v) {
        return Err(err(path, format!("{v} outside {lo}..{hi}")));
    }
    let scaled = v * 10f64.powi(decimals);
    if (scaled - scaled.round()).abs() > 1e-6 {
        return Err(err(path, format!("more than {decimals} decimals")));
    }
    Ok(v)
}

fn integer(value: &Value, path: &str, lo: u32, hi: u32) -> Result<u32> {
    let v = value.as_u64().ok_or_else(|| err(path, "a whole number"))?;
    u32::try_from(v).ok().filter(|v| (lo..=hi).contains(v)).ok_or_else(|| err(path, format!("{v} outside {lo}..{hi}")))
}

fn size(value: &Value, path: &str, max_w: u32, max_h: u32) -> Result<(u32, u32)> {
    let list = value.as_array().filter(|l| l.len() == 2).ok_or_else(|| err(path, "[width, height]"))?;
    Ok((integer(&list[0], path, 8, max_w)?, integer(&list[1], path, 8, max_h)?))
}

fn insets(value: &Value, path: &str, w: u32, h: u32) -> Result<[f64; 4]> {
    let list = value.as_array().filter(|l| l.len() == 4).ok_or_else(|| err(path, "[top, left, bottom, right]"))?;
    let mut out = [0f64; 4];
    for (i, v) in list.iter().enumerate() {
        out[i] = number(v, path, 0.0, f64::from(if i % 2 == 0 { h } else { w }), 3)?;
    }
    if out[1] + out[3] >= f64::from(w) || out[0] + out[2] >= f64::from(h) {
        return Err(err(path, "the insets leave no middle"));
    }
    Ok(out)
}

/// `#RRGGBB` (either case) → RGB.
pub fn parse_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#').filter(|h| h.len() == 6)?;
    let lower = hex.to_ascii_lowercase();
    hex_array::<3>(&lower)
}

fn color(value: &Value, path: &str) -> Result<[u8; 3]> {
    value.as_str().and_then(parse_color).ok_or_else(|| err(path, "#RRGGBB"))
}

fn color_text(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// WCAG 2.x relative luminance (the sRGB threshold of `import_festivals.py`).
pub fn luminance(rgb: [u8; 3]) -> f64 {
    let ch = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(rgb[0]) + 0.7152 * ch(rgb[1]) + 0.0722 * ch(rgb[2])
}

/// WCAG contrast ratio.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

const WHITE: [u8; 3] = [255, 255, 255];

fn need_contrast(path: &str, what: &str, a: [u8; 3], b: [u8; 3], ratio: f64) -> Result<()> {
    let c = contrast(a, b);
    if c + 1e-9 < ratio {
        return Err(err(path, format!("{what}: contrast {c:.2} < {ratio}")));
    }
    Ok(())
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round_ties_even() / 10_000.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round_ties_even() / 100.0
}

/// Opaque column runs `[first, last]` of one row (`render_cats.py` `runs`).
fn runs(img: &Rgba, row: i64, threshold: u8) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut start = None;
    for x in 0..i64::from(img.width) {
        let solid = img.alpha(x, row) > threshold;
        match (solid, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                out.push((s, x - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, i64::from(img.width) - 1));
    }
    out
}

/// Measures a cat sprite like `render_cats.py` `measure`: bounds for every
/// pose; the flat cut and paws for a top pose; the gripping paw for a side pose.
fn measure_sprite(img: &Rgba, anchor: (f64, f64), pose: Pose, path: &str) -> Result<Map<String, Value>> {
    let (w, h) = (i64::from(img.width), i64::from(img.height));
    let ierr = |what: &str| Error::new(Code::Image, format!("{path}: {what}"));
    if !img.pixels.chunks_exact(4).any(|p| p[3] == 0) {
        return Err(ierr("no transparent background"));
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0i64, 0i64);
    for y in 0..h {
        for x in 0..w {
            if img.alpha(x, y) > VISIBLE {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 == 0 || x0 == 0 || y0 == 0 || x1 == w || y1 == h {
        return Err(ierr("the cat touches the canvas edge; ears and tail need a transparent margin"));
    }
    let (wf, hf) = (w as f64, h as f64);
    let mut out = Map::new();
    out.insert("bounds".into(), json!([round4(x0 as f64 / wf), round4(y0 as f64 / hf), round4(x1 as f64 / wf), round4(y1 as f64 / hf)]));
    let row = (anchor.1 * hf).round_ties_even() as i64;
    match pose {
        Pose::Top => {
            let above = runs(img, row - 3, OPAQUE);
            let body = above.iter().copied().max_by_key(|r| r.1 - r.0).ok_or_else(|| ierr("no body above the anchor row"))?;
            if ((body.1 - body.0) as f64) < 0.4 * (x1 - x0) as f64 {
                return Err(ierr("no body above the anchor row"));
            }
            let ax = anchor.0 * wf;
            if !(body.0 as f64 <= ax && ax <= body.1 as f64) {
                return Err(ierr("the anchor is not over the body"));
            }
            let covered: i64 = runs(img, row + 2, OPAQUE).iter().map(|(a, b)| b - a + 1).sum();
            if covered as f64 > 0.75 * (body.1 - body.0 + 1) as f64 {
                return Err(ierr("the body is not cut on the anchor row"));
            }
            let still = runs(img, row - 6, OPAQUE);
            if still.iter().map(|(a, b)| b - a).max().is_none_or(|m| (m as f64) < 0.9 * (body.1 - body.0) as f64) {
                return Err(ierr("the body ends above the anchor row"));
            }
            let mut paws = Vec::new();
            for (first, last) in runs(img, row, OPAQUE) {
                let middle = (first + last).div_euclid(2);
                let mut depth = 0;
                while row + depth < h && img.alpha(middle, row + depth) > OPAQUE {
                    depth += 1;
                }
                if depth >= 2 && last - first >= 5 {
                    paws.push(json!([
                        round4((first + last + 1) as f64 / 2.0 / wf),
                        round4((last - first + 1) as f64 / 2.0 / wf),
                        round4(depth as f64 / hf)
                    ]));
                }
            }
            if paws.is_empty() {
                return Err(ierr("no paw reaches below the anchor row"));
            }
            if paws.len() > 4 {
                return Err(ierr("more than four paws"));
            }
            if (y1 - row) as f64 > 0.2 * hf {
                return Err(ierr("something hangs far below the anchor row"));
            }
            let ends: Vec<(i64, i64)> = above.into_iter().filter(|r| r.1 - r.0 >= 5).collect();
            // A body narrower than 5 px leaves no run to cut along: refuse it
            // instead of indexing an empty list.
            let (Some(&first), Some(&last)) = (ends.first(), ends.last()) else {
                return Err(ierr("the body above the anchor row is too narrow to cut"));
            };
            out.insert("cut".into(), json!([round4(first.0 as f64 / wf), round4((last.1 + 1) as f64 / wf)]));
            out.insert("paws".into(), Value::Array(paws));
        }
        Pose::Side => {
            let x = (anchor.0 * wf).round_ties_even() as i64;
            let reach = (0.07 * wf).round_ties_even() as i64;
            let mut found = false;
            for dx in -reach..=reach {
                for dy in -reach..=reach {
                    let px = (x + dx).clamp(0, w - 1);
                    let py = (row + dy).clamp(0, h - 1);
                    found |= img.alpha(px, py) > OPAQUE;
                }
            }
            if !found {
                return Err(ierr("no paw at the anchor"));
            }
        }
        Pose::Whole => {}
    }
    Ok(out)
}

/// Where the trim's rim runs (`import_festivals.py` `rim`): insets in points
/// to the middle of the rim's stroke, measured down the middle column.
fn measure_rim(img: &Rgba, path: &str) -> Result<[f64; 4]> {
    let x = i64::from(img.width / 2);
    let mut found = Vec::new();
    let mut start = None;
    for y in 0..=i64::from(img.height) {
        let solid = img.alpha(x, y) > RIM_ALPHA;
        match (solid, start) {
            (true, None) => start = Some(y),
            (false, Some(s)) => {
                found.push((s, y - 1));
                start = None;
            }
            _ => {}
        }
    }
    if found.len() < 2 {
        return Err(Error::new(Code::Image, format!("{path}: expected two rim strokes down the middle of the trim")));
    }
    let (a, b) = found[0];
    let (c, d) = found[found.len() - 1];
    let s = f64::from(SCALE);
    let top = round2((a + b + 1) as f64 / 2.0 / s);
    let bottom = round2((f64::from(img.height) - (c + d + 1) as f64 / 2.0) / s);
    Ok([top, top, bottom, top])
}

struct Files<'a> {
    entries: BTreeMap<&'a str, &'a [u8]>,
    used: BTreeSet<String>,
    decoded: usize,
    pcm: usize,
}

impl<'a> Files<'a> {
    fn take(&mut self, stem: &Value, kind: &str, path: &str) -> Result<(String, &'a [u8])> {
        let stem = stem.as_str().filter(|s| is_name(s)).ok_or_else(|| err(path, "a file stem"))?;
        let name = format!("{stem}.{kind}");
        let data = *self.entries.get(name.as_str()).ok_or_else(|| err(path, format!("{name} is not in the archive")))?;
        if !self.used.insert(name.clone()) {
            return Err(err(path, format!("{name} is used twice")));
        }
        Ok((name, data))
    }

    fn image(&mut self, stem: &Value, (w, h): (u32, u32), path: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<Rgba> {
        let (name, data) = self.take(stem, "png", path)?;
        let img = decode(data, w * SCALE, h * SCALE).map_err(|e| Error::new(e.code, format!("{path} ({name}): {}", e.detail)))?;
        self.decoded += img.bytes();
        if self.decoded > limits::DECODED_IMAGES {
            return fail(Code::TooLarge, "decoded images");
        }
        out.push((name, encode(&img)?));
        Ok(img)
    }
}

/// Validates the archive of skin `id@rev` and returns its rewritten files.
pub fn validate(entries: &[Entry<'_>], id: &str, rev: u32) -> Result<Rewritten> {
    let mut files = Files { entries: entries.iter().map(|e| (e.name, e.data)).collect(), used: BTreeSet::new(), decoded: 0, pcm: 0 };
    if files.entries.len() != entries.len() {
        return fail(Code::Archive, "duplicate names");
    }
    let raw = *files.entries.get("manifest.json").ok_or_else(|| Error::new(Code::Archive, "no manifest.json"))?;
    files.used.insert("manifest.json".into());
    if raw.len() > limits::MANIFEST {
        return fail(Code::TooLarge, "manifest.json");
    }
    let value: Value = serde_json::from_slice(raw).map_err(|e| err("manifest.json", e))?;
    let m = object(
        &value,
        "manifest",
        &["base", "cats", "glass", "id", "keys", "names", "palette", "rev", "schema", "serial"],
        &["festivals", "sounds"],
    )?;
    if m["schema"].as_u64() != Some(1) {
        return Err(err("schema", "must be 1"));
    }
    if m["id"].as_str() != Some(id) || !is_name(id) {
        return Err(err("id", "differs from the header"));
    }
    if m["rev"].as_u64() != Some(u64::from(rev)) {
        return Err(err("rev", "differs from the header"));
    }
    let serial = m["serial"].as_str().filter(|s| hex_array::<8>(s).is_some()).ok_or_else(|| err("serial", "16 lowercase hex digits"))?;

    let names_map = object(&m["names"], "names", &["en", "hans", "hant", "ja"], &[])?;
    let mut names = BTreeMap::new();
    for (lang, name) in names_map {
        let name = name.as_str().filter(|n| is_display_name(n)).ok_or_else(|| err(&format!("names.{lang}"), "1–24 characters, no control or format characters"))?;
        names.insert(lang.clone(), name.to_string());
    }

    let base = choice(&m["base"], "base", &["current", "classic", "pixel"])?;
    let festivals = match m.get("festivals") {
        Some(v) => choice(v, "festivals", &["suppress", "allow"])?,
        None => "suppress",
    };
    if festivals == "allow" && base != "current" {
        return Err(err("festivals", "allow is only for base current"));
    }

    // Palette and keys.
    let p = object(&m["palette"], "palette", &["contrastPill", "dark", "light"], &[])?;
    let contrast_pill = color(&p["contrastPill"], "palette.contrastPill")?;
    need_contrast("palette.contrastPill", "white text", WHITE, contrast_pill, 4.5)?;
    let k = object(&m["keys"], "keys", &["dark", "light", "rim", "tint"], &["particles", "spaceMark"])?;
    let key_tint = number(&k["tint"], "keys.tint", 0.0, 0.22, 3)?;
    let rim = boolean(&k["rim"], "keys.rim")?;
    let mut palette_out = Map::new();
    let mut keys_out = Map::new();
    for mode in ["light", "dark"] {
        let pp = format!("palette.{mode}");
        let t = object(&p[mode], &pp, &["accent", "ink", "inkSecondary", "paper", "paper2", "pill", "pillText", "stamp"], &[])?;
        let c = |key: &str| color(&t[key], &format!("{pp}.{key}"));
        let (paper, paper2, ink, ink2, pill_text, stamp, accent) =
            (c("paper")?, c("paper2")?, c("ink")?, c("inkSecondary")?, c("pillText")?, c("stamp")?, c("accent")?);
        let pill = t["pill"].as_array().filter(|l| l.len() == 2).ok_or_else(|| err(&format!("{pp}.pill"), "[top, bottom]"))?;
        let pill = [color(&pill[0], &format!("{pp}.pill"))?, color(&pill[1], &format!("{pp}.pill"))?];
        need_contrast(&pp, "ink on paper", ink, paper, 4.5)?;
        need_contrast(&pp, "ink on paper2", ink, paper2, 4.5)?;
        need_contrast(&pp, "inkSecondary on paper", ink2, paper, 4.5)?;
        need_contrast(&pp, "pillText on the pill's top", pill_text, pill[0], 4.5)?;
        need_contrast(&pp, "pillText on the pill's bottom", pill_text, pill[1], 4.5)?;
        need_contrast(&pp, "white on accent", WHITE, accent, 4.5)?;
        palette_out.insert(
            mode.into(),
            json!({"accent": color_text(accent), "ink": color_text(ink), "inkSecondary": color_text(ink2), "paper": color_text(paper),
                   "paper2": color_text(paper2), "pill": [color_text(pill[0]), color_text(pill[1])], "pillText": color_text(pill_text),
                   "stamp": color_text(stamp)}),
        );
        let kp = format!("keys.{mode}");
        let kt = object(&k[mode], &kp, &["character", "text", "utility"], &[])?;
        let (character, utility, text) =
            (color(&kt["character"], &kp)?, color(&kt["utility"], &kp)?, color(&kt["text"], &kp)?);
        need_contrast(&kp, "text on character keys", text, character, 4.5)?;
        need_contrast(&kp, "text on utility keys", text, utility, 4.5)?;
        if (contrast(character, paper) < 3.0 || contrast(utility, paper) < 3.0) && !rim {
            return Err(err(&kp, "keys under 3:1 against the keyboard (paper) need rim: true"));
        }
        keys_out.insert(mode.into(), json!({"character": color_text(character), "text": color_text(text), "utility": color_text(utility)}));
    }
    palette_out.insert("contrastPill".into(), json!(color_text(contrast_pill)));
    keys_out.insert("rim".into(), json!(rim));
    keys_out.insert("tint".into(), json!(key_tint));

    let g = object(&m["glass"], "glass", &["deckTint", "lockShape", "shape", "strength"], &[])?;
    let glass_out = json!({
        "deckTint": number(&g["deckTint"], "glass.deckTint", 0.0, 0.12, 3)?,
        "lockShape": boolean(&g["lockShape"], "glass.lockShape")?,
        "shape": choice(&g["shape"], "glass.shape", &["capsule", "wave", "cloud"])?,
        "strength": choice(&g["strength"], "glass.strength", &["soft", "balanced", "vivid"])?,
    });

    // Cats, trim, ends, and optional key decorations.
    let mut out_files: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(mark) = k.get("spaceMark") {
        let sm = object(mark, "keys.spaceMark", &["image", "size"], &[])?;
        let wh = size(&sm["size"], "keys.spaceMark.size", 32, 32)?;
        files.image(&sm["image"], wh, "keys.spaceMark", &mut out_files)?;
        keys_out.insert("spaceMark".into(), json!({"image": sm["image"].clone(), "size": [wh.0, wh.1]}));
    }
    if let Some(particles) = k.get("particles") {
        let po = object(particles, "keys.particles", &["dark", "kind", "light"], &[])?;
        let kind = choice(&po["kind"], "keys.particles.kind", &PARTICLE_KINDS)?;
        let parse_tints = |val: &Value, ppath: &str| -> Result<Vec<String>> {
            let arr = val.as_array().filter(|l| (1..=4).contains(&l.len()))
                .ok_or_else(|| err(ppath, "1..4 #RRGGBB colors"))?;
            arr.iter().enumerate().map(|(i, c)| {
                let rgb = color(c, &format!("{ppath}[{i}]"))?;
                Ok(color_text(rgb))
            }).collect()
        };
        let light_tints = parse_tints(&po["light"], "keys.particles.light")?;
        let dark_tints = parse_tints(&po["dark"], "keys.particles.dark")?;
        keys_out.insert("particles".into(), json!({"dark": dark_tints, "kind": kind, "light": light_tints}));
    }
    let cats = object(&m["cats"], "cats", &["seating"], &["ends", "sprites", "trim"])?;
    let mut cats_out = Map::new();
    cats_out.insert("seating".into(), json!(choice(&cats["seating"], "cats.seating", &["perched", "edges"])?));
    if let Some(sprites) = cats.get("sprites") {
        let sprites = sprites.as_object().ok_or_else(|| err("cats.sprites", "not an object"))?;
        let mut sprites_out = Map::new();
        for (name, sprite) in sprites {
            let path = format!("cats.sprites.{name}");
            let pose = EDGE_SPRITES.iter().find(|(n, _)| n == name).map(|(_, p)| *p).ok_or_else(|| err(&path, "not a cat sprite name"))?;
            let s = object(sprite, &path, &["anchor", "image", "size"], &[])?;
            let wh = size(&s["size"], &format!("{path}.size"), 128, 128)?;
            let a = s["anchor"].as_array().filter(|l| l.len() == 2).ok_or_else(|| err(&path, "anchor [x, y]"))?;
            let anchor = (number(&a[0], &format!("{path}.anchor"), 0.0, 1.0, 3)?, number(&a[1], &format!("{path}.anchor"), 0.0, 1.0, 3)?);
            let img = files.image(&s["image"], wh, &path, &mut out_files)?;
            let mut measured = measure_sprite(&img, anchor, pose, &path)?;
            measured.insert("anchor".into(), json!([anchor.0, anchor.1]));
            measured.insert("image".into(), s["image"].clone());
            measured.insert("size".into(), json!([wh.0, wh.1]));
            sprites_out.insert(name.clone(), Value::Object(measured));
        }
        cats_out.insert("sprites".into(), Value::Object(sprites_out));
    }
    if let Some(trim) = cats.get("trim") {
        let t = object(trim, "cats.trim", &["caps", "image", "safeText", "size"], &[])?;
        let (w, h) = size(&t["size"], "cats.trim.size", 400, 96)?;
        let caps = insets(&t["caps"], "cats.trim.caps", w, h)?;
        let safe = insets(&t["safeText"], "cats.trim.safeText", w, h)?;
        let img = files.image(&t["image"], (w, h), "cats.trim", &mut out_files)?;
        let rim_insets = measure_rim(&img, "cats.trim")?;
        if (0..4).any(|i| safe[i] + 1e-9 < rim_insets[i]) {
            return Err(err("cats.trim.safeText", format!("must stay inside the rim {rim_insets:?}")));
        }
        // Nothing visible may be drawn where candidates go (review L5).
        let s = f64::from(SCALE);
        let (left, top) = ((safe[1] * s).floor() as i64, (safe[0] * s).floor() as i64);
        let right = i64::from(img.width) - (safe[3] * s).floor() as i64;
        let bottom = i64::from(img.height) - (safe[2] * s).floor() as i64;
        for y in top..bottom {
            for x in left..right {
                if img.alpha(x, y) > VISIBLE {
                    return Err(Error::new(Code::Image, format!("cats.trim: visible pixel at ({x}, {y}) inside safeText")));
                }
            }
        }
        cats_out.insert(
            "trim".into(),
            json!({"caps": caps, "image": t["image"].clone(), "rim": rim_insets, "safeText": safe, "size": [w, h]}),
        );
    }
    if let Some(ends) = cats.get("ends") {
        let e = object(ends, "cats.ends", &[], &["left", "right"])?;
        let mut ends_out = Map::new();
        for (side, piece) in e {
            let path = format!("cats.ends.{side}");
            let pc = object(piece, &path, &["image", "size"], &[])?;
            let wh = size(&pc["size"], &format!("{path}.size"), 64, 64)?;
            files.image(&pc["image"], wh, &path, &mut out_files)?;
            ends_out.insert(side.clone(), json!({"image": pc["image"].clone(), "size": [wh.0, wh.1]}));
        }
        cats_out.insert("ends".into(), Value::Object(ends_out));
    }

    // Sounds.
    let mut warnings = Vec::new();
    let mut sounds_out = None;
    if let Some(sounds) = m.get("sounds") {
        let s = object(sounds, "sounds", &["files", "gainDb"], &[])?;
        let gain = number(&s["gainDb"], "sounds.gainDb", -12.0, 3.0, 2)?;
        let roles = s["files"].as_object().ok_or_else(|| err("sounds.files", "not an object"))?;
        let mut roles_out = Map::new();
        let mut count = 0;
        let mut has_key = false;
        for (role, list) in roles {
            let path = format!("sounds.files.{role}");
            if !ROLES.contains(&role.as_str()) {
                return Err(err(&path, "not a sound role"));
            }
            let list = list.as_array().filter(|l| !l.is_empty()).ok_or_else(|| err(&path, "a non-empty list"))?;
            let is_key = role == "key" || (role.starts_with("key") && role.len() == 4);
            has_key |= is_key;
            for stem in list {
                count += 1;
                if count > limits::SOUND_FILES {
                    return Err(err("sounds", format!("more than {} files", limits::SOUND_FILES)));
                }
                if stem.as_str().and_then(|s| s.split_once('-')).map(|(r, _)| r) != Some(role.as_str()) {
                    return Err(err(&path, format!("{stem} must be named {role}-…")));
                }
                let (name, data) = files.take(stem, "wav", &path)?;
                let serr = |what: String| Error::new(Code::Sound, format!("{path} ({name}): {what}"));
                let samples = sound::parse(data).map_err(|e| serr(e.detail))?;
                files.pcm += samples.len() * 2;
                if files.pcm > limits::PCM_TOTAL {
                    return fail(Code::TooLarge, "sound PCM");
                }
                let lv = sound::measure(&samples);
                let max_ms = if role == "return" { 300.0 } else { 250.0 };
                if lv.duration_ms > max_ms {
                    return Err(serr(format!("{:.0} ms > {max_ms} ms", lv.duration_ms)));
                }
                if samples[0] != 0 || samples[samples.len() - 1] != 0 {
                    return Err(serr("the first and last samples must be 0".into()));
                }
                if lv.dc_db >= -80.0 {
                    return Err(serr(format!("DC {:.1} dB", lv.dc_db)));
                }
                if lv.peak_db > -6.0 {
                    return Err(serr(format!("peak {:.1} dBFS > -6", lv.peak_db)));
                }
                if is_key && (lv.la_db + 33.0).abs() > 3.0 {
                    return Err(serr(format!("A-weighted level {:.1} dB, not within 3 dB of -33", lv.la_db)));
                }
                let gone = if role == "return" { 200.0 } else { 150.0 };
                if lv.gone_ms > gone {
                    warnings.push(format!("{name}: decays at {:.0} ms (> {gone} ms)", lv.gone_ms));
                }
                out_files.push((name, sound::encode(&samples)));
            }
            roles_out.insert(role.clone(), Value::Array(list.clone()));
        }
        if !has_key {
            return Err(err("sounds.files", "needs key or key0–key5"));
        }
        sounds_out = Some(json!({"files": roles_out, "gainDb": gain}));
    }

    for name in files.entries.keys() {
        if !files.used.contains(*name) {
            return Err(Error::new(Code::Archive, format!("{name} is not used by the manifest")));
        }
        if file_name(name).is_none_or(|(_, k)| k == FileKind::Json && *name != "manifest.json") {
            return Err(Error::new(Code::Archive, name.to_string()));
        }
    }

    let mut manifest = json!({
        "base": base,
        "cats": cats_out,
        "festivals": festivals,
        "glass": glass_out,
        "id": id,
        "keys": keys_out,
        "names": names,
        "palette": palette_out,
        "rev": rev,
        "schema": 1,
        "serial": serial,
    });
    if let (Some(sounds), Some(map)) = (sounds_out, manifest.as_object_mut()) {
        map.insert("sounds".into(), sounds);
    }
    let manifest_bytes = canonical_json(&manifest);
    if manifest_bytes.len() > limits::MANIFEST {
        return fail(Code::TooLarge, "rewritten manifest.json");
    }
    out_files.push(("manifest.json".into(), manifest_bytes));
    out_files.sort_by(|a, b| a.0.cmp(&b.0));
    let total: usize = out_files.iter().map(|(_, d)| d.len()).sum();
    if total > limits::FILES_TOTAL || out_files.iter().any(|(_, d)| d.len() > limits::FILE) {
        return fail(Code::TooLarge, "installed files");
    }
    Ok(Rewritten { files: out_files, serial: serial.to_string(), base: base.to_string(), names, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 1e-9);
        assert!((contrast([255, 255, 255], [255, 255, 255]) - 1.0).abs() < 1e-9);
        assert!((contrast([0x76, 0x76, 0x76], [255, 255, 255]) - 4.54).abs() < 0.01);
    }

    #[test]
    fn numbers() {
        assert!(number(&json!(1e308), "x", 0.0, 1.0, 3).is_err());
        assert!(number(&json!(-0.5), "x", 0.0, 1.0, 3).is_err());
        assert!(number(&json!(0.1234), "x", 0.0, 1.0, 3).is_err());
        assert_eq!(number(&json!(0.125), "x", 0.0, 1.0, 3).unwrap(), 0.125);
        assert!(integer(&json!(8.5), "x", 8, 128).is_err());
        assert!(integer(&json!(0), "x", 8, 128).is_err());
        assert!(serde_json::from_str::<Value>("1e400").is_err());
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#7a4E52"), Some([0x7A, 0x4E, 0x52]));
        for bad in ["7A4E52", "#7A4E5", "#7A4E522", "#GGGGGG", "#7A4E52 "] {
            assert_eq!(parse_color(bad), None, "{bad}");
        }
    }

    #[test]
    fn whole_pose_and_key_decorations_validate() {
        fn mk_png(w: u32, h: u32, margin: u32) -> Vec<u8> {
            let mut rgba = Rgba { width: w, height: h, pixels: vec![0; (w * h * 4) as usize] };
            for y in margin..(h - margin) {
                for x in margin..(w - margin) {
                    let i = ((y * w + x) * 4) as usize;
                    rgba.pixels[i..i + 4].copy_from_slice(&[240, 180, 200, 255]);
                }
            }
            encode(&rgba).unwrap()
        }
        let manifest = serde_json::to_vec(&json!({
            "schema": 1, "id": "decoupled", "rev": 1, "serial": "0123456789abcdef",
            "names": {"hant": "測試", "hans": "测试", "ja": "テスト", "en": "Test"},
            "base": "current",
            "palette": {
                "light": {"paper": "#FBF3F5", "paper2": "#F4E5EA", "ink": "#664C55", "inkSecondary": "#80626C",
                          "pill": ["#966070", "#875362"], "pillText": "#FFFDFE", "stamp": "#E8A2B8", "accent": "#966070"},
                "dark": {"paper": "#2A2227", "paper2": "#231C20", "ink": "#DFCED4", "inkSecondary": "#B59CA5",
                         "pill": ["#CFA0B0", "#BF8FA0"], "pillText": "#2D1E24", "stamp": "#E5A8BC", "accent": "#945E6E"},
                "contrastPill": "#7E4B5A"
            },
            "glass": {"shape": "cloud", "lockShape": true, "strength": "soft", "deckTint": 0.05},
            "keys": {
                "tint": 0.04, "rim": true,
                "light": {"character": "#FFF9FB", "utility": "#F3E1E7", "text": "#634A53"},
                "dark": {"character": "#372D33", "utility": "#30262C", "text": "#E3D3D8"},
                "spaceMark": {"image": "mark", "size": [22, 22]},
                "particles": {"kind": "petal", "light": ["#FAD6E2"], "dark": ["#E5A8BC"]}
            },
            "cats": {
                "seating": "perched",
                "sprites": {
                    "momo_grab": {"image": "mgrab", "size": [64, 64], "anchor": [0.5, 0.22]}
                }
            }
        })).unwrap();
        let grab_png = mk_png(192, 192, 20);
        let mark_png = mk_png(66, 66, 4);
        let rw = validate(&[
            Entry { name: "manifest.json", data: &manifest },
            Entry { name: "mark.png", data: &mark_png },
            Entry { name: "mgrab.png", data: &grab_png },
        ], "decoupled", 1).expect("decoupled schema validates");
        let parsed: Value = serde_json::from_slice(&rw.files.iter().find(|(n, _)| n == "manifest.json").unwrap().1).unwrap();
        assert_eq!(parsed["keys"]["particles"]["kind"], "petal");
        assert_eq!(parsed["keys"]["spaceMark"]["image"], "mark");
        assert!(parsed["cats"]["sprites"]["momo_grab"]["cut"].is_null());
        assert!(parsed["cats"]["sprites"]["momo_grab"]["bounds"].is_array());
    }
}
