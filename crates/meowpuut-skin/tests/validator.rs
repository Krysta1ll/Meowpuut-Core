//! The validator (§3.6) and the install-time rewrite (§5.2): names, numbers,
//! contrast, images, sounds, decompression and size bombs, odd PNG and WAV files.

mod support;

use meowpuut_skin::archive::Entry;
use meowpuut_skin::image::{chunk_types, decode, encode, Rgba, SIGNATURE};
use meowpuut_skin::manifest::{validate, Rewritten};
use meowpuut_skin::{limits, Code, Error};
use serde_json::{json, Value};
use support::*;

fn run(manifest: &Value, files: &[(String, Vec<u8>)]) -> Result<Rewritten, Error> {
    let mut all = files.to_vec();
    all.push(("manifest.json".into(), serde_json::to_vec(manifest).unwrap()));
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let entries: Vec<Entry<'_>> = all.iter().map(|(n, d)| Entry { name: n, data: d }).collect();
    validate(&entries, "sakura_tea", 3)
}

fn sample() -> Value {
    sample_manifest("sakura_tea", 3, "0123456789abcdef")
}

fn with(edit: impl Fn(&mut Value)) -> Result<Rewritten, Error> {
    let mut m = sample();
    edit(&mut m);
    run(&m, &sample_files())
}

fn code(edit: impl Fn(&mut Value)) -> Code {
    with(edit).map(|_| ()).unwrap_err().code
}

fn with_file(name: &str, data: Vec<u8>) -> Result<Rewritten, Error> {
    let mut files = sample_files();
    files.iter_mut().find(|(n, _)| n == name).unwrap().1 = data;
    run(&sample(), &files)
}

#[test]
fn the_sample_passes_and_rewrites_deterministically() {
    let a = run(&sample(), &sample_files()).unwrap();
    let b = run(&sample(), &sample_files()).unwrap();
    assert_eq!(a.files, b.files);
    assert_eq!(a.serial, "0123456789abcdef");
    assert_eq!(a.base, "current");
    assert_eq!(a.names["ja"], "桜ミルクティー");
    // Rewriting the rewritten files changes nothing (a fixed point).
    let mut again = a.files.clone();
    let manifest: Value = serde_json::from_slice(&again.iter().find(|(n, _)| n == "manifest.json").unwrap().1).unwrap();
    again.retain(|(n, _)| n != "manifest.json");
    // The measured fields are refused as input: strip them like the CLI's source form.
    let mut source = manifest.clone();
    for (_, s) in source["cats"]["sprites"].as_object_mut().unwrap() {
        for k in ["bounds", "cut", "paws"] {
            s.as_object_mut().unwrap().remove(k);
        }
    }
    source["cats"]["trim"].as_object_mut().unwrap().remove("rim");
    let c = run(&source, &again).unwrap();
    assert_eq!(c.files, a.files);
    // Measured fields in the input are unknown fields.
    assert_eq!(run(&manifest, &again).unwrap_err().code, Code::Manifest);
}

#[test]
fn structure_and_names() {
    assert_eq!(code(|m| m["schema"] = json!(2)), Code::Manifest);
    assert_eq!(code(|m| m["extra"] = json!(true)), Code::Manifest);
    assert_eq!(code(|m| m["glass"]["blur"] = json!(1)), Code::Manifest);
    assert_eq!(code(|m| { m.as_object_mut().unwrap().remove("keys"); }), Code::Manifest);
    assert_eq!(code(|m| m["id"] = json!("Sakura_tea")), Code::Manifest);
    assert_eq!(code(|m| m["rev"] = json!(4)), Code::Manifest);
    assert_eq!(code(|m| m["serial"] = json!("0123456789ABCDEF")), Code::Manifest);
    assert_eq!(code(|m| m["serial"] = json!("0123")), Code::Manifest);
    assert_eq!(code(|m| m["base"] = json!("glassier")), Code::Manifest);
    assert_eq!(code(|m| m["festivals"] = json!("always")), Code::Manifest);
    assert_eq!(code(|m| { m["base"] = json!("pixel"); m["festivals"] = json!("allow"); }), Code::Manifest);
    assert!(with(|m| { m["festivals"] = json!("allow"); }).is_ok());
    assert!(with(|m| { m.as_object_mut().unwrap().remove("festivals"); }).is_ok());
    for bad in ["", "Evil\u{202E}gnp.exe", "zero\u{200B}width", "line\nbreak", " padded", "\u{0007}bell", "喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵喵"] {
        assert_eq!(code(|m| m["names"]["en"] = json!(bad)), Code::Manifest, "{bad:?}");
    }
    assert_eq!(code(|m| m["names"]["ko"] = json!("x")), Code::Manifest);
    assert_eq!(code(|m| m["names"]["en"] = json!(5)), Code::Manifest);
}

#[test]
fn numeric_ranges() {
    let anchor = |v: Value| code(move |m| m["cats"]["sprites"]["mimimo_edge_peek"]["anchor"] = json!([0.5, v.clone()]));
    assert_eq!(anchor(json!(1e308)), Code::Manifest);
    assert_eq!(anchor(json!(-0.1)), Code::Manifest);
    assert_eq!(anchor(json!(0.7501)), Code::Manifest);
    assert_eq!(anchor(json!("0.75")), Code::Manifest);
    let size = |v: Value| code(move |m| m["cats"]["sprites"]["mimimo_edge_peek"]["size"] = v.clone());
    assert_eq!(size(json!([0, 40])), Code::Manifest);
    assert_eq!(size(json!([7, 40])), Code::Manifest);
    assert_eq!(size(json!([32.5, 40])), Code::Manifest);
    assert_eq!(size(json!([129, 40])), Code::Manifest);
    assert_eq!(size(json!([32])), Code::Manifest);
    assert_eq!(size(json!([33, 40])), Code::Image, "a size that does not match the pixels");
    assert_eq!(code(|m| m["cats"]["trim"]["size"] = json!([401, 32])), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["ends"]["right"]["size"] = json!([65, 20])), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["trim"]["caps"] = json!([0, 60, 0, 60])), Code::Manifest, "caps wider than the trim");
    assert_eq!(code(|m| m["cats"]["trim"]["caps"] = json!([16, 0, 16, 0])), Code::Manifest, "caps taller than the trim");
    assert_eq!(code(|m| m["cats"]["trim"]["caps"] = json!([0, -1, 0, 16])), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["trim"]["safeText"] = json!([2, 12, 12, 12])), Code::Manifest, "safeText over the rim");
    assert_eq!(code(|m| m["cats"]["trim"]["safeText"] = json!([12, 60, 12, 60])), Code::Manifest);
    assert_eq!(code(|m| m["keys"]["tint"] = json!(0.23)), Code::Manifest);
    assert_eq!(code(|m| m["glass"]["deckTint"] = json!(0.13)), Code::Manifest);
    assert_eq!(code(|m| m["sounds"]["gainDb"] = json!(3.5)), Code::Manifest);
    assert_eq!(code(|m| m["sounds"]["gainDb"] = json!(-12.5)), Code::Manifest);
    assert!(with(|m| m["sounds"]["gainDb"] = json!(-12)).is_ok());
    assert_eq!(code(|m| m["glass"]["shape"] = json!("star")), Code::Manifest);
    assert_eq!(code(|m| m["glass"]["lockShape"] = json!(1)), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["seating"] = json!("floating")), Code::Manifest);
}

#[test]
fn json_number_edges() {
    let mut text = serde_json::to_string(&sample()).unwrap();
    text = text.replace("\"deckTint\":0.06", "\"deckTint\":1e400");
    let mut files = sample_files();
    files.push(("manifest.json".into(), text.into_bytes()));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let entries: Vec<Entry<'_>> = files.iter().map(|(n, d)| Entry { name: n, data: d }).collect();
    assert_eq!(validate(&entries, "sakura_tea", 3).unwrap_err().code, Code::Manifest);
    // Deep nesting stops at serde_json's recursion limit instead of the stack.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let mut files = sample_files();
    files.push(("manifest.json".into(), deep.into_bytes()));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let entries: Vec<Entry<'_>> = files.iter().map(|(n, d)| Entry { name: n, data: d }).collect();
    assert!(validate(&entries, "sakura_tea", 3).is_err());
    // An oversized manifest.
    let mut files = sample_files();
    files.push(("manifest.json".into(), vec![b' '; limits::MANIFEST + 1]));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let entries: Vec<Entry<'_>> = files.iter().map(|(n, d)| Entry { name: n, data: d }).collect();
    assert_eq!(validate(&entries, "sakura_tea", 3).unwrap_err().code, Code::TooLarge);
}

#[test]
fn colours_and_contrast() {
    assert_eq!(code(|m| m["palette"]["light"]["ink"] = json!("#3A2A2")), Code::Manifest);
    assert_eq!(code(|m| m["palette"]["light"]["ink"] = json!("red")), Code::Manifest);
    assert_eq!(code(|m| { m["palette"]["dark"].as_object_mut().unwrap().remove("stamp"); }), Code::Manifest);
    assert_eq!(code(|m| m["palette"]["light"]["tape"] = json!("#000000")), Code::Manifest);
    for (path, value) in [
        (&["palette", "light", "ink"][..], "#DDDDDD"),
        (&["palette", "light", "inkSecondary"], "#BBBBBB"),
        (&["palette", "dark", "ink"], "#444444"),
        (&["palette", "light", "pillText"], "#EBC3C8"),
        (&["palette", "light", "accent"], "#FFAACC"),
        (&["palette", "contrastPill"], "#DDDDDD"),
        (&["keys", "light", "text"], "#EEEEEE"),
        (&["keys", "dark", "text"], "#4A3A3D"),
    ] {
        let c = code(|m| {
            let mut v = &mut *m;
            for key in &path[..path.len() - 1] {
                v = &mut v[*key];
            }
            v[path[path.len() - 1]] = json!(value);
        });
        assert_eq!(c, Code::Manifest, "{path:?} = {value}");
    }
    // Pill bottom checked too.
    assert_eq!(code(|m| m["palette"]["light"]["pill"] = json!(["#F4D6D9", "#3A2A2A"])), Code::Manifest);
    // Keys under 3:1 against the keyboard need the rim.
    assert_eq!(code(|m| m["keys"]["rim"] = json!(false)), Code::Manifest);
    assert!(with(|m| {
        m["keys"]["rim"] = json!(false);
        m["keys"]["light"] = json!({"character": "#8A5A62", "utility": "#7A4E52", "text": "#FFFFFF"});
        m["keys"]["dark"] = json!({"character": "#C9A9AE", "utility": "#BFA0A5", "text": "#1A1214"});
    })
    .is_ok());
    // Lower-case input is written back in upper case.
    let r = with(|m| m["palette"]["light"]["ink"] = json!("#3a2a2a")).unwrap();
    let manifest: Value = serde_json::from_slice(&r.files.iter().find(|(n, _)| n == "manifest.json").unwrap().1).unwrap();
    assert_eq!(manifest["palette"]["light"]["ink"], "#3A2A2A");
}

#[test]
fn cat_measurements() {
    assert_eq!(code(|m| m["cats"]["sprites"]["mimimo_edge_peek"]["anchor"] = json!([0.1, 0.75])), Code::Image, "anchor off the body");
    assert_eq!(code(|m| m["cats"]["sprites"]["mimimo_edge_peek"]["anchor"] = json!([0.5, 0.5])), Code::Image, "not cut on the anchor row");
    assert_eq!(code(|m| m["cats"]["sprites"]["momo_side_hold"]["anchor"] = json!([0.95, 0.05])), Code::Image, "no paw at the anchor");
    assert_eq!(code(|m| m["cats"]["sprites"]["mimimo_edge_peek"]["bounds"] = json!([0, 0, 1, 1])), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["sprites"]["mimimo_edge_tea"] = json!({"image": "x", "size": [32, 40], "anchor": [0.5, 0.5]})), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["sprites"]["mimimo_edge_peek"]["image"] = json!("../mimimo_peek")), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["sprites"]["mimimo_edge_peek"]["image"] = json!("missing")), Code::Manifest);
    assert_eq!(code(|m| m["cats"]["sprites"]["momo_side_hold"]["image"] = json!("mimimo_peek")), Code::Manifest, "one file used twice");
    assert_eq!(code(|m| m["cats"]["trim"]["rim"] = json!([3, 3, 3, 3])), Code::Manifest, "rim is measured, not given");
    // No transparent background; touching the edge; no paws.
    let mut opaque = top_cat();
    for p in opaque.pixels.as_chunks_mut::<4>().0 {
        p[3] = p[3].max(30);
    }
    assert_eq!(with_file("mimimo_peek.png", encode(&opaque).unwrap()).unwrap_err().code, Code::Image);
    let mut touching = top_cat();
    fill(&mut touching, 0, 40, 95, 50, [1, 2, 3, 255]);
    assert_eq!(with_file("mimimo_peek.png", encode(&touching).unwrap()).unwrap_err().code, Code::Image);
    let mut pawless = blank(96, 120);
    fill(&mut pawless, 20, 20, 75, 89, [240, 200, 180, 255]);
    assert_eq!(with_file("mimimo_peek.png", encode(&pawless).unwrap()).unwrap_err().code, Code::Image);
}

#[test]
fn safe_text_band_must_be_clear() {
    let mut img = trim();
    fill(&mut img, 150, 45, 160, 50, [0, 0, 0, 25]);
    assert_eq!(with_file("frame.png", encode(&img).unwrap()).unwrap_err().code, Code::Image);
    let mut faint = trim();
    fill(&mut faint, 150, 45, 160, 50, [0, 0, 0, 24]);
    assert!(with_file("frame.png", encode(&faint).unwrap()).is_ok());
    let mut one_stroke = blank(360, 96);
    fill(&mut one_stroke, 6, 6, 353, 11, [255, 255, 255, 255]);
    assert_eq!(with_file("frame.png", encode(&one_stroke).unwrap()).unwrap_err().code, Code::Image);
}

#[test]
fn image_format_rules() {
    let good = encode(&top_cat()).unwrap();
    // Not RGBA / 16-bit / interlaced: IHDR bytes 24..29 (depth, colour, compression, filter, interlace).
    for (at, value) in [(24usize, 16u8), (25, 2), (25, 3), (26, 1), (27, 1), (28, 1)] {
        let mut p = good.clone();
        p[at] = value;
        let crc = crc32(&p[12..29]);
        p[29..33].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(decode(&p, 96, 120).unwrap_err().code, Code::Image, "byte {at} = {value}");
    }
    // A bad CRC, a truncated stream, a missing IEND, no IDAT.
    let mut p = good.clone();
    p[40] ^= 0xFF;
    assert!(decode(&p, 96, 120).is_err());
    assert!(decode(&good[..good.len() - 12], 96, 120).is_err());
    let mut no_idat = good[..33].to_vec();
    no_idat.extend(chunk(b"IEND", &[]));
    assert!(decode(&no_idat, 96, 120).is_err());
    // A second IHDR, a PLTE, an unknown critical chunk, CgBI before IHDR: refused.
    for (kind, data) in [(b"IHDR", &good[16..29]), (b"PLTE", &[0u8, 0, 0][..]), (b"ZZZZ", &[][..])] {
        let mut p = good[..33].to_vec();
        p.extend(chunk(kind, data));
        p.extend_from_slice(&good[33..]);
        assert!(decode(&p, 96, 120).is_err(), "{}", String::from_utf8_lossy(kind));
    }
    let mut cgbi = SIGNATURE.to_vec();
    cgbi.extend(chunk(b"CgBI", &[0x50, 0, 0x20, 2]));
    cgbi.extend_from_slice(&good[8..]);
    assert!(decode(&cgbi, 96, 120).is_err());
    // Ancillary chunks (text, iCCP, APNG acTL/fcTL) and bytes after IEND: decoded, then dropped.
    let mut extras = good[..33].to_vec();
    extras.extend(chunk(b"iCCP", b"x\0\0\x78\x9c\x03\0\0\0\0\x01"));
    extras.extend(chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]));
    extras.extend(chunk(b"zTXt", b"k\0\0\x78\x9c\x03\0\0\0\0\x01"));
    extras.extend_from_slice(&good[33..]);
    extras.extend_from_slice(b"\0\0\0\x05tail!");
    let decoded = decode(&extras, 96, 120).unwrap();
    assert_eq!(decoded, top_cat());
    let rewritten = encode(&decoded).unwrap();
    assert_eq!(rewritten, good);
    assert!(chunk_types(&rewritten).iter().all(|t| matches!(t, b"IHDR" | b"IDAT" | b"IEND")));
    let r = with_file("mimimo_peek.png", extras).unwrap();
    assert_eq!(r.files.iter().find(|(n, _)| n == "mimimo_peek.png").unwrap().1, good);
}

#[test]
fn image_bombs() {
    // A huge IHDR is refused from its header, before any allocation.
    let mut huge = encode(&Rgba { width: 1, height: 1, pixels: vec![0; 4] }).unwrap();
    huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    let crc = crc32(&huge[12..29]);
    huge[29..33].copy_from_slice(&crc.to_be_bytes());
    assert_eq!(decode(&huge, 96, 120).unwrap_err().code, Code::Image);
    assert_eq!(decode(&huge, 100_000, 100_000).unwrap_err().code, Code::TooLarge);
    // A deflate stream that inflates far beyond the image (64 MiB of zeros) is cut off.
    let mut bomb = encode(&top_cat()).unwrap()[..33].to_vec();
    bomb.extend(chunk(b"IDAT", &zlib_zeros(64 << 20)));
    bomb.extend(chunk(b"IEND", &[]));
    let start = std::time::Instant::now();
    let result = decode(&bomb, 96, 120);
    assert!(start.elapsed().as_secs() < 5);
    // Either refused or the image alone (the extra data is never kept).
    if let Ok(img) = result {
        assert_eq!(img.pixels.len(), 96 * 120 * 4);
    }
    // Images whose decoded total passes 4 MiB are refused.
    let mut m = sample();
    let mut files = sample_files();
    let poses = ["mimimo_edge_rest", "mimimo_edge_lounge", "mimimo_side_hold", "momo_edge_rest", "momo_edge_peek", "momo_edge_lounge"];
    for (k, pose) in poses.iter().enumerate() {
        let name = format!("big{k}");
        let mut img = blank(384, 384);
        fill(&mut img, 20, 20, 360, 287, [9, 9, 9, 255]);
        fill(&mut img, 100, 288, 120, 310, [9, 9, 9, 255]);
        files.push((format!("{name}.png"), encode(&img).unwrap()));
        m["cats"]["sprites"][pose] = json!({"image": name, "size": [128, 128], "anchor": [0.5, 0.75]});
    }
    assert!(run(&m, &files).is_ok(), "six large cats fit");
    files.retain(|(n, _)| n != "frame.png");
    files.push(("frame.png".into(), encode(&blank(1200, 288)).unwrap()));
    m["cats"]["trim"]["size"] = json!([400, 96]);
    assert_eq!(run(&m, &files).unwrap_err().code, Code::TooLarge);
}

/// The zlib stream of `n` zero bytes (taken from the PNG encoder's IDAT).
fn zlib_zeros(n: usize) -> Vec<u8> {
    let png = encode(&Rgba { width: (n / 4) as u32, height: 1, pixels: vec![0; n / 4 * 4] }).unwrap();
    let mut out = Vec::new();
    let mut at = 8;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        if &png[at + 4..at + 8] == b"IDAT" {
            out.extend_from_slice(&png[at + 8..at + 8 + len]);
        }
        at += 12 + len;
    }
    out
}

#[test]
fn image_decoder_survives_random_input() {
    let good = encode(&top_cat()).unwrap();
    let mut rng = Rng(4242);
    for _ in 0..1500 {
        let mut p = good.clone();
        for _ in 0..1 + rng.below(6) {
            let i = 8 + rng.below(p.len() - 8);
            p[i] = rng.next() as u8;
        }
        let _ = decode(&p, 96, 120);
    }
    for n in 0..good.len() {
        assert!(decode(&good[..n], 96, 120).is_err() || n == good.len());
    }
}

#[test]
fn sound_rules() {
    let wav = |s: &[i16]| meowpuut_skin::sound::encode(s);
    let key = |data: Vec<u8>| with_file("key-1.wav", data).map(|_| ()).unwrap_err().code;
    assert_eq!(key(wav(&tone(0.0731, 11_100))), Code::Sound, "longer than 250 ms");
    assert_eq!(key(wav(&tone(0.5, 2205))), Code::Sound, "too loud");
    assert_eq!(key(wav(&tone(0.02, 2205))), Code::Sound, "too quiet");
    let mut edge = tone(0.0731, 2205);
    edge[0] = 5;
    assert_eq!(key(wav(&edge)), Code::Sound, "first sample");
    let mut dc = tone(0.0731, 2205);
    for s in dc.iter_mut().skip(1).take(2203) {
        *s += 40;
    }
    assert_eq!(key(wav(&dc)), Code::Sound, "DC");
    let mut peak = tone(0.0731, 2205);
    peak[1000] = 20_000;
    assert_eq!(key(wav(&peak)), Code::Sound, "peak above -6 dBFS");
    // Format: stereo, 48 kHz, 8-bit, float, no data.
    let good = wav(&tone(0.0731, 2205));
    for (at, value) in [(22usize, 2u8), (24, 0x80), (34, 8), (20, 3)] {
        let mut w = good.clone();
        w[at] = value;
        assert_eq!(key(w), Code::Sound, "byte {at}");
    }
    assert_eq!(key(good[..44].to_vec()), Code::Sound);
    // return may be up to 300 ms; others not.
    assert!(with_file("return-1.wav", wav(&tone(0.06, 13_000))).is_ok());
    assert_eq!(with_file("return-1.wav", wav(&tone(0.06, 13_300))).unwrap_err().code, Code::Sound);
    assert_eq!(with_file("space-1.wav", wav(&tone(0.06, 11_100))).unwrap_err().code, Code::Sound);
    // Manifest rules for sounds.
    assert_eq!(code(|m| m["sounds"]["files"]["meow"] = json!(["meow-1"])), Code::Manifest);
    assert_eq!(code(|m| m["sounds"]["files"]["space"] = json!(["key-1"])), Code::Manifest, "name must start with the role");
    assert_eq!(code(|m| m["sounds"]["files"] = json!({"space": ["space-1"], "return": ["return-1"]})), Code::Manifest, "no key");
    assert_eq!(code(|m| m["sounds"]["files"]["key"] = json!([])), Code::Manifest);
    assert_eq!(code(|m| m["sounds"]["files"]["key"] = json!(["key-1", "key-1"])), Code::Manifest);
    // Without sounds at all (and without their files) the skin is fine.
    let mut m = sample();
    m.as_object_mut().unwrap().remove("sounds");
    let files: Vec<_> = sample_files().into_iter().filter(|(n, _)| !n.ends_with(".wav")).collect();
    assert!(run(&m, &files).is_ok());
    // More than 20 files.
    let mut m = sample();
    let mut files = sample_files();
    let mut list = vec![json!("key-1")];
    for k in 2..=21 {
        files.push((format!("key-{k}.wav"), wav(&tone(0.0731, 2205))));
        list.push(json!(format!("key-{k}")));
    }
    m["sounds"]["files"]["key"] = Value::Array(list);
    assert_eq!(run(&m, &files).unwrap_err().code, Code::Manifest);
}

#[test]
fn wav_parser_survives_random_input() {
    let good = meowpuut_skin::sound::encode(&tone(0.0731, 300));
    let mut rng = Rng(31337);
    for _ in 0..3000 {
        let mut w = good.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(w.len());
            w[i] = rng.next() as u8;
        }
        let _ = meowpuut_skin::sound::parse(&w);
        let n = rng.below(80);
        let _ = meowpuut_skin::sound::parse(&rng.bytes(n));
    }
    for n in 0..good.len() {
        let _ = meowpuut_skin::sound::parse(&good[..n]);
    }
    // A chunk claiming 4 GiB is refused from its length.
    let mut w = good.clone();
    w.extend_from_slice(b"junk\xff\xff\xff\xff");
    let riff = (w.len() - 8) as u32;
    w[4..8].copy_from_slice(&riff.to_le_bytes());
    assert!(meowpuut_skin::sound::parse(&w).is_err());
}

#[test]
fn manifest_fuzz_does_not_panic() {
    let base = serde_json::to_vec(&sample()).unwrap();
    let mut rng = Rng(2024);
    let files = sample_files();
    for _ in 0..400 {
        let mut text = base.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(text.len());
            text[i] = b"{}[],:\"0123456789.-e #tfn"[rng.below(25)];
        }
        let mut all = files.clone();
        all.push(("manifest.json".into(), text));
        all.sort_by(|a, b| a.0.cmp(&b.0));
        let entries: Vec<Entry<'_>> = all.iter().map(|(n, d)| Entry { name: n, data: d }).collect();
        let _ = validate(&entries, "sakura_tea", 3);
    }
}
