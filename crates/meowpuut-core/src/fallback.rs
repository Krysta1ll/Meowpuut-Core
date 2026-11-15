#![forbid(unsafe_code)]
//! Exact-match lookup, QWERTY neighbor typo correction, unfinished Pinyin phrase
//! completion, and single-character fallback candidates using preallocated SoA buffers.
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::Lexicon;
//! use meowpuut_core::fallback::{has_exact_match, unfinished_phrase_into, fallback_into};
//! use meowpuut_core::workspace::DecodeOutputSoA;
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//! );
//! let lex = Lexicon::load(tsv).unwrap();
//! assert!(has_exact_match(&lex, "nihao"));
//! let mut out = DecodeOutputSoA::with_capacity(8);
//! unfinished_phrase_into(&lex, "nihaom", false, &mut out);
//! assert_eq!(out.len(), 1);
//! assert_eq!(out.text(0), "你好");
//! ```

use std::cmp::Ordering;

use crate::decoder::decode_with_buffers;
use crate::latin;
use crate::lexicon::Lexicon;
use crate::merge;
use crate::pairs::{END, START, STEP};
use crate::reading::{strict_han_count, utf8_char_count, Reading, MAX_INPUT};
use crate::workspace::{DecodeOutputSoA, DecoderScratchSoA};
use crate::{CandidateKind, CandidateScoreKind};

/// Returns `true` if `lexicon` has an exact whole-word match for `input`.
pub fn has_exact_match(lexicon: &Lexicon, input: &str) -> bool {
    let reading = Reading::new(input, lexicon.pinyin);
    let (lo, hi) = lexicon.range(reading.sounds());
    for i in lo..hi {
        let wr = lexicon.words.reading(&lexicon.storage, i).as_bytes();
        if reading.matches(wr, 0, reading.len()) {
            return true;
        }
    }
    false
}

#[inline]
fn qwerty_neighbors(ch: u8) -> &'static [u8] {
    match ch {
        b'q' => b"wa",
        b'w' => b"qeas",
        b'e' => b"wrsd",
        b'r' => b"etdf",
        b't' => b"ryfg",
        b'y' => b"tugh",
        b'u' => b"yihj",
        b'i' => b"uojk",
        b'o' => b"ipkl",
        b'p' => b"ol",
        b'a' => b"qwsz",
        b's' => b"awedzx",
        b'd' => b"serfxc",
        b'f' => b"drtgcv",
        b'g' => b"ftyhbv",
        b'h' => b"gyujbn",
        b'j' => b"huiknm",
        b'k' => b"jiolm",
        b'l' => b"kop",
        b'z' => b"asx",
        b'x' => b"zsdc",
        b'c' => b"xdfv",
        b'v' => b"cfgb",
        b'b' => b"vghn",
        b'n' => b"bhjm",
        b'm' => b"njk",
        _ => b"",
    }
}

const FUZZY_EDITS: usize = 16;
const SYLLABLE_LENGTH: usize = 6;
const NASAL_PENALTY: f64 = 2.9;
const INITIAL_PENALTY: f64 = 3.4;
const MIN_INITIAL_WORD_WEIGHT: i32 = 4700;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SoundEdit {
    #[default]
    Remove,
    Insert(u8),
    Replace(u8),
}

impl SoundEdit {
    fn shift(self) -> isize {
        match self {
            Self::Remove => -1,
            Self::Insert(_) => 1,
            Self::Replace(_) => 0,
        }
    }

    fn write(self, input: &[u8], at: usize, output: &mut [u8]) -> usize {
        let (removed, added) = match self {
            Self::Remove => (1, 0),
            Self::Insert(byte) => { output[at] = byte; (0, 1) }
            Self::Replace(byte) => { output[at] = byte; (1, 1) }
        };
        output[..at].copy_from_slice(&input[..at]);
        let length = input.len() - removed + added;
        output[at + added..length].copy_from_slice(&input[at + removed..]);
        length
    }
}

#[derive(Clone, Copy, Default)]
struct FuzzyEdit {
    at: usize,
    start: usize,
    end: usize,
    change: SoundEdit,
}

/// Which sounds a fuzzy correction swaps.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fuzz {
    /// The finals -n and -ng.
    Final,
    /// The initials z c s and zh ch sh, n and l.
    Initial,
    /// The initials f and h, r and l: offered only when much stronger than
    /// the letters as typed (refresh.rs).
    Rare,
}

fn nasal_edit(syllable: &str) -> Option<(usize, SoundEdit)> {
    if ["ang", "eng", "ing"].iter().any(|ending| syllable.ends_with(ending)) {
        Some((syllable.len() - 1, SoundEdit::Remove))
    } else if ["an", "en", "in"].iter().any(|ending| syllable.ends_with(ending)) {
        Some((syllable.len(), SoundEdit::Insert(b'g')))
    } else {
        None
    }
}

fn initial_edit(syllable: &str) -> Option<(usize, SoundEdit)> {
    let (first, rest) = syllable.as_bytes().split_first()?;
    match first {
        b'z' | b'c' | b's' if rest.first() == Some(&b'h') => Some((1, SoundEdit::Remove)),
        b'z' | b'c' | b's' => Some((1, SoundEdit::Insert(b'h'))),
        b'n' if !rest.is_empty() => Some((0, SoundEdit::Replace(b'l'))),
        b'l' if !rest.is_empty() => Some((0, SoundEdit::Replace(b'n'))),
        _ => None,
    }
}

fn rare_initial_edit(syllable: &str) -> Option<(usize, SoundEdit)> {
    let (first, rest) = syllable.as_bytes().split_first()?;
    let other = match first {
        b'f' => b'h',
        b'h' => b'f',
        b'r' => b'l',
        b'l' => b'r',
        _ => return None,
    };
    (!rest.is_empty()).then_some((0, SoundEdit::Replace(other)))
}

/// Visit only complete syllables on a valid path through the original reading.
/// This keeps edits inside their syllable and out of the following initial.
fn fuzzy_edits(
    lexicon: &Lexicon, reading: &Reading, edit: fn(&str) -> Option<(usize, SoundEdit)>,
    edits: &mut [FuzzyEdit; FUZZY_EDITS],
) -> usize {
    let sounds = reading.sounds();
    let length = sounds.len();
    let mut syllables = [0u8; MAX_INPUT + 1];
    let mut suffix = [false; MAX_INPUT + 1];
    suffix[length] = true;
    for start in (0..length).rev() {
        for end in start + 1..=length.min(start + SYLLABLE_LENGTH) {
            if reading.boundary_count[end - 1] != reading.boundary_count[start] { break; }
            if suffix[end] && merge::is_syllable(lexicon, &sounds[start..end]) {
                syllables[start] |= 1 << (end - start);
                suffix[start] = true;
            }
        }
    }
    if !suffix[0] { return 0; }
    let mut prefix = [false; MAX_INPUT + 1];
    prefix[0] = true;
    let mut count = 0;
    for start in 0..length {
        if !prefix[start] { continue; }
        for end in start + 1..=length.min(start + SYLLABLE_LENGTH) {
            if syllables[start] & (1 << (end - start)) == 0 { continue; }
            prefix[end] = true;
            let syllable = &sounds[start..end];
            let Some((offset, change)) = edit(syllable) else { continue; };
            let at = start + offset;
            if edits[..count].iter().any(|e| e.at == at && e.change == change) { continue; }
            let mut key = [0; SYLLABLE_LENGTH + 1];
            let len = change.write(syllable.as_bytes(), offset, &mut key);
            if merge::is_syllable(lexicon, std::str::from_utf8(&key[..len]).unwrap()) {
                edits[count] = FuzzyEdit { at, start, end, change };
                count += 1;
                if count == edits.len() { return count; }
            }
        }
    }
    count
}

/// Best whole dictionary word across the edited syllable and one nearby
/// syllable. Pick a couple of variants for a complete sentence decode.
fn local_word_weight(lexicon: &Lexicon, reading: &Reading, start: usize, end: usize) -> i32 {
    let word_weight = |start, end| {
        let (lo, hi) = lexicon.range(&reading.sounds()[start..end]);
        (lo..hi.min(lo + 4)).filter(|&i| {
            lexicon.words.is_fully_spelled(i)
                && !lexicon.words.is_no_suggest(i)
                && strict_han_count(lexicon.words.text(&lexicon.storage, i, false)) > 1
                && reading.matches(lexicon.words.reading(&lexicon.storage, i).as_bytes(), start, end)
        }).map(|i| lexicon.words.weight(i)).max().unwrap_or_default()
    };
    (start.saturating_sub(SYLLABLE_LENGTH)..start).map(|first| word_weight(first, end))
        .chain((end + 1..=reading.len().min(end + SYLLABLE_LENGTH)).map(|last| word_weight(start, last)))
        .max().unwrap_or_default()
}

/// Returns `true` when the corrections are whole dictionary words.
pub(crate) fn fuzzy_corrections_into(
    lexicon: &Lexicon, input: &str, hans: bool, scratch: &mut DecoderScratchSoA, out: &mut DecodeOutputSoA, fuzz: Fuzz,
) -> bool {
    let initials = fuzz != Fuzz::Final;
    out.clear();
    if !lexicon.pinyin || !(2..=MAX_INPUT).contains(&input.len())
        || !input.bytes().all(|b| b.is_ascii_lowercase() || (b'1'..=b'5').contains(&b) || b == b'\'')
    { return false; }
    let plausible = if initials {
        let letters: &[u8] = if fuzz == Fuzz::Rare { b"fhrl" } else { b"zcsnl" };
        input.len() >= 4 && input.bytes().any(|byte| letters.contains(&byte))
    } else {
        input.as_bytes().windows(2).any(|w| b"aei".contains(&w[0]) && w[1] == b'n')
    };
    if !plausible { return false; }
    let reading = Reading::new(input, true);
    if !reading.unambiguous { return false; }
    let edit = match fuzz { Fuzz::Final => nasal_edit, Fuzz::Initial => initial_edit, Fuzz::Rare => rare_initial_edit };
    let penalty = if initials { INITIAL_PENALTY } else { NASAL_PENALTY };
    let score_kind = if initials { CandidateScoreKind::FuzzyInitial } else { CandidateScoreKind::Fallback } as u32;
    let mut edits = [FuzzyEdit::default(); FUZZY_EDITS];
    let count = fuzzy_edits(lexicon, &reading, edit, &mut edits);
    if count == 0 { return false; }
    // The byte offset just after a sound is before any explicit tone or
    // separator. Inserting here leaves both the separator and the tone intact.
    let mut offsets = [0usize; MAX_INPUT + 1];
    let mut sounds = 0;
    for (i, byte) in input.bytes().enumerate() {
        if byte.is_ascii_lowercase() {
            sounds += 1;
            offsets[sounds] = i + 1;
        }
    }
    let raw_at = |edit: FuzzyEdit| match edit.change {
        SoundEdit::Insert(_) => offsets[edit.at],
        SoundEdit::Remove | SoundEdit::Replace(_) => offsets[edit.at + 1] - 1,
    };
    let mut best = [(i32::MIN, FuzzyEdit::default()); 2];
    let mut buffer = [0; MAX_INPUT + 1];
    for &edit in &edits[..count] {
        if matches!(edit.change, SoundEdit::Insert(_)) && input.len() == MAX_INPUT { continue; }
        let len = edit.change.write(input.as_bytes(), raw_at(edit), &mut buffer);
        let revised = Reading::new(std::str::from_utf8(&buffer[..len]).unwrap(), true);
        let (lo, hi) = lexicon.range(revised.sounds());
        let mut found = false;
        for i in lo..hi.min(lo + 7) {
            let wr = lexicon.words.reading(&lexicon.storage, i);
            let text = lexicon.words.text(&lexicon.storage, i, hans);
            // A correction is never a never-volunteer word.
            if !lexicon.words.is_fully_spelled(i) || lexicon.words.is_no_suggest(i) || !revised.matches(wr.as_bytes(), 0, revised.len()) { continue; }
            let han_count = strict_han_count(text);
            // A one-syllable reading already has enough homophones.
            if han_count == 0 || (initials && (han_count == 1 || lexicon.words.weight(i) < MIN_INITIAL_WORD_WEIGHT)) { continue; }
            merge::push_best(out, text, input.len(), CandidateKind::Correction as u32, score_kind,
                             merge::whole_word_score(lexicon, i) - penalty);
            found = true;
        }
        if !found && reading.len() >= 7 {
            let weight = local_word_weight(lexicon, &revised, edit.start, edit.end.saturating_add_signed(edit.change.shift()));
            let typed_weight = local_word_weight(lexicon, &reading, edit.start, edit.end);
            // A change inside a sentence is worth a second decode only when
            // it makes a stronger word next to the edited syllable.
            if weight <= typed_weight || (initials && weight < MIN_INITIAL_WORD_WEIGHT) { continue; }
            if weight > best[0].0 { best[1] = best[0]; best[0] = (weight, edit); }
            else if weight > best[1].0 { best[1] = (weight, edit); }
        }
    }

    // Direct whole-word corrections are already useful. Only decode longer
    // variants when they still need to be composed from dictionary words.
    let words = !out.is_empty();
    if !words && fuzz != Fuzz::Rare {
        for &(weight, edit) in &best {
            if weight == i32::MIN { break; }
            let len = edit.change.write(input.as_bytes(), raw_at(edit), &mut buffer);
            let revised = std::str::from_utf8(&buffer[..len]).unwrap();
            scratch.lattice.skip_no_suggest = true;
            decode_with_buffers(lexicon, revised, hans, true, false, false, &mut scratch.lattice, &mut scratch.step_paths,
                                &mut scratch.fallback_raw, &mut scratch.fallback_order, &mut scratch.fallback_out, &mut scratch.aux_decoded);
            scratch.lattice.skip_no_suggest = false;
            for i in 0..scratch.aux_decoded.len().min(3) {
                let text = scratch.aux_decoded.text(i);
                if strict_han_count(text) > 1 {
                    merge::push_best(out, text, input.len(), CandidateKind::Correction as u32, score_kind,
                                     scratch.aux_decoded.score[i] - penalty);
                }
            }
        }
    }
    merge::sort_from(out, 0);
    out.truncate(6);
    words
}

/// A dropped letter can make a very different word. Only offer the best
/// common dictionary match, after the caller has kept the normal ranking.
pub(crate) fn dropped_letter_candidate<'a>(lexicon: &'a Lexicon, input: &str, hans: bool) -> Option<(&'a str, f64)> {
    let bytes = input.as_bytes();
    if !lexicon.pinyin || !(4..=8).contains(&bytes.len()) || !bytes.iter().all(u8::is_ascii_lowercase) {
        return None;
    }
    let mut best = None;
    let mut key = [0u8; 9];
    for at in 0..=bytes.len() {
        key[..at].copy_from_slice(&bytes[..at]);
        key[at + 1..=bytes.len()].copy_from_slice(&bytes[at..]);
        for letter in b'a'..=b'z' {
            key[at] = letter;
            let (lo, hi) = lexicon.range(std::str::from_utf8(&key[..=bytes.len()]).unwrap());
            for i in lo..hi.min(lo + 7) {
                if lexicon.words.weight(i) < 4000 { break; }
                let text = lexicon.words.text(&lexicon.storage, i, hans);
                if !lexicon.words.is_fully_spelled(i) || lexicon.words.is_no_suggest(i) || strict_han_count(text) < 2 { continue; }
                let score = merge::whole_word_score(lexicon, i) - merge::GUESS_PENALTY - 2.0;
                if best.is_none_or(|(_, prior)| score > prior) { best = Some((text, score)); }
            }
        }
    }
    best
}

/// Longest span, in letters, that the edit search looks up as one word.
const EDIT_SPAN: usize = 12;
/// Longest input, in letters, that the edit search tries.
const EDIT_INPUT: usize = 40;
/// Edits decoded in full, the best word covers first.
const EDIT_DECODES: usize = 2;
/// Word lookups one search may make, nearest places first.
const EDIT_LOOKUPS: usize = 600;
/// Letters before the first uncovered letter that a typo may be.
const EDIT_REACH: usize = 6;
/// Nats of word cost within which a second fix is decoded too.
const SECOND_FIX: f64 = 3.0;
/// Cost in nats of a letter that no word covers, for the typed letters.
const LOOSE_LETTER: f64 = -14.0;
/// Nats a one-key edit costs.
const EDIT_PENALTY: f64 = 4.0;
/// Nats more when the typed letters are all words: the filler may be meant.
const COVERED_PENALTY: f64 = 3.0;

/// One key typed wrong: `removed` letters at `at` become `added[..count]`.
#[derive(Clone, Copy, Default)]
struct KeyEdit {
    at: usize,
    removed: usize,
    added: [u8; 2],
    count: usize,
}

impl KeyEdit {
    fn write(self, input: &[u8], output: &mut [u8]) -> usize {
        let tail = self.at + self.removed;
        output[..self.at].copy_from_slice(&input[..self.at]);
        output[self.at..self.at + self.count].copy_from_slice(&self.added[..self.count]);
        let length = input.len() - self.removed + self.count;
        output[self.at + self.count..length].copy_from_slice(&input[tail..]);
        length
    }
}

/// Decoder cost of the heaviest fully spelled word read as `key`, or
/// negative infinity when there is none.
fn span_cost(lexicon: &Lexicon, key: &[u8]) -> f64 {
    let (lo, hi) = lexicon.range(std::str::from_utf8(key).unwrap_or(""));
    (lo..hi.min(lo + 8)).filter(|&i| lexicon.words.is_fully_spelled(i))
        .map(|i| f64::from(lexicon.words.weight(i)) / 700.0 - 19.0)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// `span_cost` for letters that are not the typed ones (a key edit): a
/// never-volunteer word is not a fix.
fn edited_span_cost(lexicon: &Lexicon, key: &[u8]) -> f64 {
    let (lo, hi) = lexicon.range(std::str::from_utf8(key).unwrap_or(""));
    (lo..hi.min(lo + 8)).filter(|&i| lexicon.words.is_fully_spelled(i) && !lexicon.words.is_no_suggest(i))
        .map(|i| f64::from(lexicon.words.weight(i)) / 700.0 - 19.0)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Best decoder cost of a word for any key of letters (any capitalization
/// of English). The lexicon's heaviest words weigh about 10000.
const BEST_WORD: f64 = 10_500.0 / 700.0 - 19.0;

/// Word costs of the spans of the letters last searched for key edits,
/// kept between keys: one more key looks up only the spans that end at it.
#[derive(Debug, Clone)]
pub(crate) struct SpanCosts {
    lexicon: usize,
    letters: [u8; EDIT_INPUT],
    len: usize,
    cost: [[f32; EDIT_SPAN]; EDIT_INPUT],
}

impl Default for SpanCosts {
    fn default() -> Self {
        Self { lexicon: 0, letters: [0; EDIT_INPUT], len: 0, cost: [[f32::NEG_INFINITY; EDIT_SPAN]; EDIT_INPUT] }
    }
}

impl SpanCosts {
    fn update(&mut self, lexicon: &Lexicon, letters: &[u8]) {
        let id = std::ptr::from_ref(lexicon) as usize;
        let same = if id == self.lexicon { self.letters[..self.len].iter().zip(letters).take_while(|(a, b)| a == b).count() } else { 0 };
        let n = letters.len();
        for start in 0..n {
            for len in 1..=EDIT_SPAN.min(n - start) {
                if start + len <= same { continue; }
                let span = &letters[start..start + len];
                let mut cost = span_cost(lexicon, span);
                // English typed in the letters is meant (deadline, KPI).
                if let Some((lo, hi, term)) = latin::span(lexicon, letters, start, start + len) {
                    let weight = (lo..hi).map(|i| lexicon.words.weight(i)).max().unwrap_or_default();
                    cost = cost.max(f64::from(weight) / 700.0 - 19.0 + term);
                }
                self.cost[start][len - 1] = cost as f32;
            }
        }
        self.lexicon = id;
        self.letters[..n].copy_from_slice(letters);
        self.len = n;
    }

    #[inline]
    fn get(&self, start: usize, end: usize) -> f64 {
        f64::from(self.cost[start][end - start - 1])
    }
}

/// Best cover of plain letters with dictionary words, scored like the
/// decoder's word costs without the pair terms: `prefix[i]` for the first
/// `i` letters, `suffix[i]` for the letters from `i`. `loose` also lets
/// single letters stand uncovered, so that typed letters always get a score.
struct WordCover {
    prefix: [f64; EDIT_INPUT + 1],
    suffix: [f64; EDIT_INPUT + 1],
    loose: f64,
    /// Letters of the best cover's syllables without an initial (o, e, ng),
    /// one bit per letter: an extra or doubled key often makes one.
    fillers: u64,
    /// First letter that the loose cover leaves uncovered or the first
    /// filler: the likely place of the typo, searched first.
    trouble: usize,
}

/// Syllables without an initial that a stray key often makes.
const FILLERS: [&[u8]; 15] = [b"a", b"ai", b"an", b"ang", b"ao", b"e", b"ei", b"en", b"eng", b"er", b"o", b"ou", b"n", b"ng", b"m"];

impl WordCover {
    fn new(costs: &SpanCosts, letters: &[u8]) -> Self {
        let n = letters.len();
        let mut cover = Self { prefix: [f64::NEG_INFINITY; EDIT_INPUT + 1], suffix: [f64::NEG_INFINITY; EDIT_INPUT + 1], loose: 0.0, fillers: 0, trouble: 0 };
        let mut from = [0usize; EDIT_INPUT + 1];
        let mut loose = [f64::NEG_INFINITY; EDIT_INPUT + 1];
        let mut loose_from = [0usize; EDIT_INPUT + 1];
        cover.prefix[0] = 0.0;
        loose[0] = 0.0;
        cover.suffix[n] = 0.0;
        for end in 1..=n {
            loose[end] = loose[end - 1] + LOOSE_LETTER;
            loose_from[end] = usize::MAX;
            for start in end.saturating_sub(EDIT_SPAN)..end {
                let word = costs.get(start, end);
                if cover.prefix[start] + word > cover.prefix[end] {
                    cover.prefix[end] = cover.prefix[start] + word;
                    from[end] = start;
                }
                if loose[start] + word > loose[end] {
                    loose[end] = loose[start] + word;
                    loose_from[end] = start;
                }
            }
        }
        for start in (0..n).rev() {
            for end in start + 1..=n.min(start + EDIT_SPAN) {
                cover.suffix[start] = cover.suffix[start].max(costs.get(start, end) + cover.suffix[end]);
            }
        }
        cover.loose = loose[n];
        let mut end = n;
        while end > 0 {
            if loose_from[end] == usize::MAX { cover.trouble = end - 1; }
            end = if loose_from[end] == usize::MAX { end - 1 } else { loose_from[end] };
        }
        end = n;
        while end > 0 && cover.prefix[n] > f64::NEG_INFINITY {
            let start = from[end];
            if FILLERS.contains(&&letters[start..end]) {
                cover.fillers |= ((1u64 << (end - start)) - 1) << start;
                cover.trouble = start;
            }
            end = start;
        }
        cover
    }

    fn covered(&self, letters: usize) -> bool {
        self.prefix[letters] > f64::NEG_INFINITY
    }

    /// Start of the longest tail after covered letters that one more key
    /// makes a word of (cantin for 餐廳, bangongsh, nira), or an English
    /// word of at least three letters may go on from (dingleube for Uber):
    /// the user may be typing it, so no edit there or after it is a fix.
    fn open_tail(&self, lexicon: &Lexicon, letters: &[u8]) -> usize {
        let n = letters.len();
        let mut key = [0u8; EDIT_SPAN + 1];
        (n.saturating_sub(EDIT_SPAN)..n).find(|&start| {
            if self.prefix[start] == f64::NEG_INFINITY { return false; }
            let tail = &letters[start..];
            key[..tail.len()].copy_from_slice(tail);
            let spelled = b"aeiouvngrh".iter().any(|&next| {
                key[tail.len()] = next;
                span_cost(lexicon, &key[..=tail.len()]) > f64::NEG_INFINITY
            });
            spelled || tail.len() >= 3 && {
                key[..tail.len()].make_ascii_uppercase();
                let upper = std::str::from_utf8(&key[..tail.len()]).unwrap_or("");
                let bucket = lexicon.lower_bound_bucket(upper);
                bucket < lexicon.buckets.len() && {
                    let found = lexicon.buckets.key(&lexicon.storage, bucket);
                    found.len() > upper.len() && found.starts_with(upper)
                }
            }
        }).unwrap_or(n)
    }

    /// Best cover of `edited` (the typed letters with `edit` applied): the
    /// typed covers before and after the edit, joined by one word over it.
    /// Spans that cannot beat `need` are not looked up, and each lookup
    /// spends one of `budget`.
    fn edited(&self, lexicon: &Lexicon, letters: usize, edit: KeyEdit, edited: &[u8], need: f64, budget: &mut usize) -> f64 {
        let (at, added) = (edit.at, edit.count);
        // The last letter may start a syllable not typed yet, and a typed
        // syllable there may be meant (nihaoa). Change it only when the
        // letters have no cover, and only into one whole word.
        if at + edit.removed.max(1) >= letters {
            if self.covered(letters) || edited.len() > EDIT_SPAN || *budget == 0 { return f64::NEG_INFINITY; }
            *budget -= 1;
            return edited_span_cost(lexicon, edited);
        }
        let mut best = if added == 0 { self.prefix[at] + self.suffix[at + edit.removed] } else { f64::NEG_INFINITY };
        if added == 0 && at == 0 { return best; }
        // A word over a removed letter starts before it and ends after it.
        let (first, last) = if added == 0 { (at + 1, at - 1) } else { (at + added, at) };
        let starts = first.saturating_sub(EDIT_SPAN)..=last.min(at);
        let ends = first..=edited.len().min(last.min(at) + EDIT_SPAN);
        let before = starts.clone().map(|start| self.prefix[start]).fold(f64::NEG_INFINITY, f64::max);
        let after = ends.clone().map(|end| self.suffix[end + edit.removed - added]).fold(f64::NEG_INFINITY, f64::max);
        if before + BEST_WORD + after <= need.max(best) { return best; }
        for start in starts {
            if self.prefix[start] + BEST_WORD + after <= need.max(best) { continue; }
            for end in first.max(start + 1)..=edited.len().min(start + EDIT_SPAN) {
                let after = self.suffix[end + edit.removed - added];
                if self.prefix[start] + BEST_WORD + after > need.max(best) {
                    if *budget == 0 { return best; }
                    *budget -= 1;
                    best = best.max(self.prefix[start] + edited_span_cost(lexicon, &edited[start..end]) + after);
                }
            }
        }
        best
    }
}

/// Best cover of `input` (plain letters) with dictionary words, in the
/// decoder's word costs without pair terms; negative infinity when the
/// letters are not all words or are not plain letters.
pub(crate) fn typed_cover_score(lexicon: &Lexicon, input: &str, scratch: &mut DecoderScratchSoA) -> f64 {
    let letters = input.as_bytes();
    if !(1..=EDIT_INPUT).contains(&letters.len()) || !letters.iter().all(u8::is_ascii_lowercase) {
        return f64::NEG_INFINITY;
    }
    scratch.span_costs.update(lexicon, letters);
    WordCover::new(&scratch.span_costs, letters).prefix[letters.len()]
}

/// Visits the one-key edits of `letters`: a key typed in excess, two keys
/// swapped, a neighbouring key, and, when `missing`, a key left out (last,
/// as there are many). Places nearest `trouble` come first: a good fix found
/// early rules most of the others out without a lookup.
fn each_key_edit(letters: &[u8], trouble: usize, missing: bool, mut visit: impl FnMut(KeyEdit)) {
    let n = letters.len();
    let places = (0..2 * n).filter_map(|d| if d % 2 == 0 { Some(trouble + d / 2) } else { trouble.checked_sub(d / 2 + 1) })
        .filter(move |&at| at < n);
    for at in places.clone() {
        visit(KeyEdit { at, removed: 1, ..KeyEdit::default() });
        if at + 1 < n && letters[at] != letters[at + 1] {
            visit(KeyEdit { at, removed: 2, added: [letters[at + 1], letters[at]], count: 2 });
        }
        for &near in qwerty_neighbors(letters[at]) {
            visit(KeyEdit { at, removed: 1, added: [near, 0], count: 1 });
        }
    }
    for at in places.filter(|&at| missing && at > 0) {
        for letter in b'a'..=b'z' {
            visit(KeyEdit { at, removed: 0, added: [letter, 0], count: 1 });
        }
    }
}

/// Decodes of the input with one key fixed, when the fixed letters are all
/// dictionary words and read much better than the letters as typed
/// (gongtonfg, haioyou, woxianzaikeyiuuijiama). The score is how much
/// better, in nats of word cost, less `EDIT_PENALTY`.
pub(crate) fn key_edit_corrections_into(
    lexicon: &Lexicon, input: &str, hans: bool, scratch: &mut DecoderScratchSoA, out: &mut DecodeOutputSoA,
) {
    out.clear();
    let letters = input.as_bytes();
    if !lexicon.pinyin || !(4..=EDIT_INPUT).contains(&letters.len()) || !letters.iter().all(u8::is_ascii_lowercase) {
        return;
    }
    scratch.span_costs.update(lexicon, letters);
    let cover = WordCover::new(&scratch.span_costs, letters);
    let typed = cover.loose;
    let covered = cover.covered(letters.len());
    if covered && cover.fillers == 0 { return; }
    let open = cover.open_tail(lexicon, letters);
    // Keys still being typed that make no word yet: nothing to fix before them.
    if open < letters.len() && cover.suffix[open] == f64::NEG_INFINITY { return; }
    // Only fixes worth offering are decoded.
    let floor = typed + EDIT_PENALTY + if covered { COVERED_PENALTY } else { 0.0 };
    let mut best = [f64::NEG_INFINITY; EDIT_DECODES];
    let mut buffer = [0u8; EDIT_INPUT + 1];
    let mut kept = [[0u8; EDIT_INPUT + 1]; EDIT_DECODES];
    let mut kept_len = [0usize; EDIT_DECODES];
    let mut budget = EDIT_LOOKUPS;
    // Letters that are all words lose a key only to a typo that leaves
    // fragments, so look for missing keys only where some letter is loose.
    each_key_edit(letters, cover.trouble, !covered, |edit| {
        if edit.at + edit.removed.max(1) > open || budget == 0 { return; }
        // The typo is at most a syllable or so before the first letter it
        // leaves uncovered.
        if !covered && (edit.at + EDIT_REACH < cover.trouble || edit.at > cover.trouble + 3) { return; }
        // Letters that are all words are fixed only next to a filler.
        let near = ((1u64 << (edit.removed.max(1) + 2)) - 1) << edit.at >> 1;
        if covered && cover.fillers & near == 0 { return; }
        let len = edit.write(letters, &mut buffer);
        // A second fix is decoded only when it is nearly as good as the first.
        let need = floor.max(best[EDIT_DECODES - 1]).max(best[0] - SECOND_FIX);
        let score = cover.edited(lexicon, letters.len(), edit, &buffer[..len], need, &mut budget);
        if score <= need { return; }
        if (0..EDIT_DECODES).any(|k| kept[k][..kept_len[k]] == buffer[..len]) { return; }
        let mut slot = EDIT_DECODES - 1;
        while slot > 0 && best[slot - 1] < score {
            best[slot] = best[slot - 1];
            kept[slot] = kept[slot - 1];
            kept_len[slot] = kept_len[slot - 1];
            slot -= 1;
        }
        best[slot] = score;
        kept[slot][..len].copy_from_slice(&buffer[..len]);
        kept_len[slot] = len;
    });
    if best[1] < best[0] - SECOND_FIX { best[1] = f64::NEG_INFINITY; }
    for k in 0..EDIT_DECODES {
        if best[k] == f64::NEG_INFINITY { break; }
        let revised = std::str::from_utf8(&kept[k][..kept_len[k]]).unwrap_or("");
        scratch.lattice.skip_no_suggest = true;
        decode_with_buffers(lexicon, revised, hans, true, false, false, &mut scratch.lattice, &mut scratch.step_paths,
                            &mut scratch.fallback_raw, &mut scratch.fallback_order, &mut scratch.fallback_out, &mut scratch.aux_decoded);
        scratch.lattice.skip_no_suggest = false;
        // The decoder's first reading of the fixed letters, all Han.
        let Some(i) = (0..scratch.aux_decoded.len().min(3)).find(|&i| {
            let text = scratch.aux_decoded.text(i);
            strict_han_count(text) > 1 && strict_han_count(text) == utf8_char_count(text)
        }) else { continue; };
        merge::push_best(out, scratch.aux_decoded.text(i), input.len(), CandidateKind::Correction as u32,
                         CandidateScoreKind::Fallback as u32, best[k] - floor);
    }
    merge::sort_from(out, 0);
}

/// Finds completed phrases when the final Pinyin syllable (`1..=3` letters) is still unfinished.
pub fn unfinished_phrase_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    if !lexicon.pinyin || input.len() < 3 || input.len() > 64 || has_exact_match(lexicon, input) {
        return;
    }
    let max_tail = 3.min(input.len() - 1);
    for tail in 1..=max_tail {
        let suffix = &input[input.len() - tail..];
        if !suffix.bytes().all(|c| (b'a'..=b'z').contains(&c)) {
            continue;
        }
        let complete_suffix = lexicon.range(suffix);
        if complete_suffix.0 != complete_suffix.1 {
            continue;
        }
        let count = input.len() - tail;
        let prefix = Reading::new(&input[..count], true);
        let (lo, hi) = lexicon.range(prefix.sounds());
        for i in lo..hi {
            if out.len() >= 3 {
                break;
            }
            let text = lexicon.words.text(&lexicon.storage, i, hans);
            let wr = lexicon.words.reading(&lexicon.storage, i).as_bytes();
            if lexicon.words.is_fully_spelled(i)
                && !lexicon.words.is_no_suggest(i)
                && utf8_char_count(text) > 1
                && prefix.matches(wr, 0, prefix.len())
            {
                out.push(
                    text,
                    count,
                    CandidateKind::Partial as u32,
                    CandidateScoreKind::None as u32,
                    f64::from(lexicon.words.weight(i)),
                );
            }
        }
        if !out.is_empty() {
            return;
        }
    }
}

/// Generates bounded prefix completions, single-edit typo corrections, and single-char fallbacks
/// into preallocated SoA buffers without heap allocation.
pub fn fallback_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    allow_single: bool,
    raw: &mut DecodeOutputSoA,
    order: &mut Vec<usize>,
    out: &mut DecodeOutputSoA,
) {
    raw.clear();
    order.clear();
    out.clear();
    let reading = Reading::new(input, lexicon.pinyin);
    let sounds = reading.sounds();
    let sounds_bytes = reading.sounds_bytes();
    // With a pair table a word that fills the whole input also gets the
    // clause start and end terms, in weight units (700 per nat), as in the lattice.
    let edges = |i: usize, whole: bool| -> f64 {
        match lexicon.pairs() {
            Some(p) => {
                let id = lexicon.index.word_ids.ids.get(i);
                let end = if whole { p.score(id, END) } else { 0 };
                700.0 * f64::from(p.score(START, id) + end) / STEP
            }
            None => 0.0,
        }
    };

    let collect = |key: &str, penalty: f64, raw_buf: &mut DecodeOutputSoA| {
        let (lo, hi) = lexicon.range(key);
        for i in lo..hi.min(lo + 7) {
            // Completions and typo fixes: never a never-volunteer word.
            if lexicon.words.is_no_suggest(i) && key != sounds {
                continue;
            }
            let text = lexicon.words.text(&lexicon.storage, i, hans);
            raw_buf.push(
                text,
                input.len(),
                CandidateKind::Correction as u32,
                CandidateScoreKind::Fallback as u32,
                f64::from(lexicon.words.weight(i)) - penalty + edges(i, true),
            );
        }
    };

    if sounds_bytes.len() == 1 {
        for idx in lexicon.initials(sounds_bytes[0]) {
            raw.push(
                lexicon.words.text(&lexicon.storage, idx, hans),
                input.len(),
                CandidateKind::Correction as u32,
                CandidateScoreKind::Fallback as u32,
                f64::from(lexicon.words.weight(idx)) + edges(idx, true),
            );
        }
    }

    if lexicon.pinyin && (2..=32).contains(&sounds.len()) && input == sounds {
        let mut scanned = 0usize;
        let mut bucket_idx = lexicon.lower_bound_bucket(sounds);
        while bucket_idx < lexicon.buckets.len() && scanned < 256 {
            scanned += 1;
            let bkey = lexicon.buckets.key(&lexicon.storage, bucket_idx);
            if !bkey.starts_with(sounds) {
                break;
            }
            let extra = bkey.len() - sounds.len();
            if extra <= 6 {
                collect(bkey, 1200.0 * (extra as f64), raw);
            }
            bucket_idx += 1;
        }

        if sounds_bytes.len() >= 4 {
            let mut key_buf = [0u8; 36];
            let slen = sounds_bytes.len();
            for i in 0..slen {
                if i + 1 < slen && sounds_bytes[i] != sounds_bytes[i + 1] {
                    key_buf[..slen].copy_from_slice(sounds_bytes);
                    key_buf.swap(i, i + 1);
                    if let Ok(s) = std::str::from_utf8(&key_buf[..slen]) {
                        collect(s, 800.0, raw);
                    }
                }
                if i > 0 && sounds_bytes[i] == sounds_bytes[i - 1] {
                    key_buf[..i].copy_from_slice(&sounds_bytes[..i]);
                    key_buf[i..slen - 1].copy_from_slice(&sounds_bytes[i + 1..slen]);
                    if let Ok(s) = std::str::from_utf8(&key_buf[..slen - 1]) {
                        collect(s, 800.0, raw);
                    }
                }
                for &ch in qwerty_neighbors(sounds_bytes[i]) {
                    key_buf[..slen].copy_from_slice(sounds_bytes);
                    key_buf[i] = ch;
                    if let Ok(s) = std::str::from_utf8(&key_buf[..slen]) {
                        collect(s, 1200.0, raw);
                    }
                }
            }
        }

    }

    if raw.is_empty() && allow_single {
        let mut count = input.len().min(7);
        while count > 0 {
            let prefix = Reading::new(&input[..count], lexicon.pinyin);
            if !prefix.is_empty() {
                let (lo, hi) = lexicon.range(prefix.sounds());
                for i in lo..hi {
                    if raw.len() >= 28 {
                        break;
                    }
                    let text = lexicon.words.text(&lexicon.storage, i, hans);
                    let wr = lexicon.words.reading(&lexicon.storage, i).as_bytes();
                    if utf8_char_count(text) == 1 && prefix.matches(wr, 0, prefix.len())
                        && !(lexicon.words.is_no_suggest(i) && count != input.len())
                    {
                        raw.push(
                            text,
                            count,
                            CandidateKind::Partial as u32,
                            CandidateScoreKind::Fallback as u32,
                            f64::from(lexicon.words.weight(i)) + edges(i, count == input.len()),
                        );
                    }
                }
                if !raw.is_empty() {
                    break;
                }
            }
            count -= 1;
        }
    }

    for i in 0..raw.len() {
        order.push(i);
    }
    order.sort_by(|&a, &b| {
        if raw.score[a] > raw.score[b] {
            Ordering::Less
        } else if raw.score[a] < raw.score[b] {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });

    for &idx in order.iter() {
        let text = raw.text(idx);
        if !out.contains_text(text) {
            out.push(text, raw.consumed[idx], raw.kind[idx], raw.score_kind[idx], raw.score[idx]);
            if out.len() == 28 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexicon(rows: &[(&str, &str, &str, i32)]) -> Lexicon {
        let mut rows = rows.to_vec();
        rows.sort_unstable();
        let mut tsv = String::from("MEOWPUUT-PINYIN-1\n");
        for (bare, toned, text, weight) in rows {
            tsv.push_str(&format!("{bare}\t{toned}\t{text}\t{text}\t{weight}\n"));
        }
        Lexicon::load(&tsv).unwrap()
    }

    fn fixes(lex: &Lexicon, input: &str) -> Vec<String> {
        let mut scratch = DecoderScratchSoA::new();
        let mut out = DecodeOutputSoA::with_capacity(8);
        key_edit_corrections_into(lex, input, false, &mut scratch, &mut out);
        (0..out.len()).map(|i| out.text(i).to_owned()).collect()
    }

    const WORDS: [(&str, &str, &str, i32); 14] = [
        ("gong", "gong1", "工", 6000), ("tong", "tong2", "同", 6000), ("gongtong", "gong4tong2", "共同", 7000),
        ("hai", "hai2", "還", 8000), ("o", "o1", "喔", 7000), ("you", "you3", "有", 9000),
        ("haiyou", "hai2you3", "還有", 8000), ("can", "can1", "餐", 6000), ("ting", "ting1", "聽", 7000),
        ("canting", "can1ting1", "餐廳", 6000), ("ji", "ji1", "機", 7000), ("chang", "chang3", "場", 7000),
        ("jichang", "ji1chang3", "機場", 7000), ("ta", "ta1", "他", 9000),
    ];

    #[test]
    fn key_edits_write_the_fixed_letters() {
        let mut out = [0u8; 16];
        let input = b"gongtonfg";
        let len = KeyEdit { at: 7, removed: 1, ..KeyEdit::default() }.write(input, &mut out);
        assert_eq!(&out[..len], b"gongtong");
        let len = KeyEdit { at: 5, removed: 0, added: [b'x', 0], count: 1 }.write(input, &mut out);
        assert_eq!(&out[..len], b"gongtxonfg");
        let len = KeyEdit { at: 7, removed: 2, added: *b"gf", count: 2 }.write(input, &mut out);
        assert_eq!(&out[..len], b"gongtongf");
        let len = KeyEdit { at: 0, removed: 1, added: [b'h', 0], count: 1 }.write(input, &mut out);
        assert_eq!(&out[..len], b"hongtonfg");
    }

    #[test]
    fn an_extra_or_missing_key_reads_as_the_word() {
        let lex = lexicon(&WORDS);
        assert_eq!(fixes(&lex, "gongtonfg"), ["共同"]);
        assert_eq!(fixes(&lex, "jicfhang"), ["機場"]);
        assert_eq!(fixes(&lex, "gongtng"), ["共同"]);
        assert_eq!(fixes(&lex, "tajicahng"), ["他機場"]);
    }

    #[test]
    fn a_filler_syllable_made_by_a_stray_key_is_dropped() {
        let lex = lexicon(&WORDS);
        assert_eq!(fixes(&lex, "haioyou"), ["還有"]);
    }

    #[test]
    fn spelled_words_and_letters_still_being_typed_are_left_alone() {
        let lex = lexicon(&WORDS);
        for input in ["haiyou", "gongtong", "tajichang", "cantin", "jichan", "gongtongj"] {
            assert!(fixes(&lex, input).is_empty(), "{input}");
        }
    }

    #[test]
    fn span_costs_kept_between_keys_match_a_fresh_lookup() {
        let lex = lexicon(&WORDS);
        let mut kept = SpanCosts::default();
        for typed in ["gong", "gongton", "gongtong", "gongt", "haioyou"] {
            kept.update(&lex, typed.as_bytes());
            let mut fresh = SpanCosts::default();
            fresh.update(&lex, typed.as_bytes());
            for start in 0..typed.len() {
                for end in start + 1..=typed.len().min(start + EDIT_SPAN) {
                    assert_eq!(kept.get(start, end), fresh.get(start, end), "{typed} {start}..{end}");
                }
            }
        }
    }

    #[test]
    fn rare_initials_swap_f_with_h_and_r_with_l() {
        assert!(rare_initial_edit("fen") == Some((0, SoundEdit::Replace(b'h'))));
        assert!(rare_initial_edit("hao") == Some((0, SoundEdit::Replace(b'f'))));
        assert!(rare_initial_edit("le") == Some((0, SoundEdit::Replace(b'r'))));
        assert!(rare_initial_edit("ren") == Some((0, SoundEdit::Replace(b'l'))));
        assert!(rare_initial_edit("f").is_none() && rare_initial_edit("zhi").is_none());
    }
}
