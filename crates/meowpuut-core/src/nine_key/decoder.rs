//! Builds complete and leading candidates by joining words that match keypad spans.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;

use super::{digit, matches_breaks, signature, KeyReading, NineKeyIndex, INITIALS};
use crate::latin::syllables;
use crate::lexicon::Lexicon;
use crate::merge::whole_word_score;
use crate::pairs::{Pairs, END, START, STEP};
use crate::reading::{is_tone, CANDIDATE_LIMIT};
use crate::soa::{CandidatePoolSoA, JapaneseTableSoA};
use crate::{CandidateKind, CandidateScoreKind};

const SPAN_WORDS: usize = 12;
const WHOLE_WORDS: usize = 160;
const PATHS: usize = 8;
/// Initials typed after the spelled syllables of one word (9426 9: xian z).
const TAIL: usize = 2;
/// Longest spelled part before such initials (two long syllables).
const TAIL_SPELLED: usize = 12;
/// Spelled words of a span tried with initials after them.
const TAIL_WORDS: usize = 24;
/// Abbreviation rows with the spelled initials searched for a word starting with a spelled one.
const TAIL_ROWS: usize = 48;
/// Abbreviated words tried after a spelled word, and on each side of two in a row (7744).
const NEXT_WORDS: usize = 4;
/// Abbreviated words offered for the whole input.
const INITIALS_WORDS: usize = 16;
/// Added to an abbreviated word's score, taken at its full weight.
const INITIALS_TERM: f64 = -3.0;
/// Lightest abbreviation row shown on the first page next to spelled words.
const FIRST_PAGE_WEIGHT: i32 = 750;
/// Fewest letters of an English word that leads over a spelled path of several words.
const LATIN_LEAD_LETTERS: usize = 4;
/// By how much it must outweigh that path.
const LATIN_LEAD_GAP: f64 = 2.0;
/// Candidates on the first page.
const PAGE: usize = 6;
/// Added to an English word whose letters are not pinyin when it is weighed against the Chinese candidates.
const LATIN_FIRST_TERM: f64 = 1.5;
/// Longest English word key (mimo_latin.py keeps up to 24 letters).
const LATIN: usize = 24;

fn word_score(weight: i32) -> f64 {
    f64::from(weight) / 700.0 - 19.0
}

/// First letters of the syllables of a toned reading (xian4zai4: x, z), or 0 when there are too many.
fn word_initials(reading: &str, letters: &mut [u8; INITIALS]) -> usize {
    let mut count = 0;
    let mut boundary = true;
    for byte in reading.bytes() {
        if is_tone(byte, true) {
            boundary = true;
        } else if boundary {
            if count == INITIALS {
                return 0;
            }
            letters[count] = byte;
            count += 1;
            boundary = false;
        }
    }
    count
}

// An abbreviation row has an eighth of its word's weight (compile-native-lexicon.py).
fn initials_score(weight: i32) -> f64 {
    word_score(weight.saturating_mul(8)) + INITIALS_TERM
}

impl NineKeyIndex {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode(
        &self,
        lexicon: &Lexicon,
        japanese: &JapaneseTableSoA,
        keys: &[u8],
        hans: bool,
        left: u32,
        out: &mut CandidatePoolSoA,
        preedit: &mut String,
    ) {
        out.clear();
        if !keys.is_empty() {
            let decoder = Decoder { index: self, lexicon, japanese, keys: KeyReading::new(keys), hans };
            decoder.decode(left, out, preedit);
        }
    }
}

struct Decoder<'a> {
    index: &'a NineKeyIndex,
    lexicon: &'a Lexicon,
    japanese: &'a JapaneseTableSoA,
    keys: KeyReading<'a>,
    hans: bool,
}

impl<'a> Decoder<'a> {
    fn decode(&self, left: u32, out: &mut CandidatePoolSoA, preedit: &mut String) {
        let length = self.keys.digits.len();
        let longest = self.lexicon.longest.max(20); // Registered Japanese readings are at most 20 keys.
        let mut paths: Vec<_> = (0..=length).map(|_| Paths::default()).collect();
        paths[0].chinese.push(Path { text: String::new(), reading: String::new(), score: 0.0, last: Some(left), common: true });
        let mut choices = Choices::default();
        let mut words = SpanWords::default();
        for start in 0..length {
            let (visited, later) = paths.split_at_mut(start + 1);
            let previous = &visited[start];
            if previous.chinese.is_empty() && previous.mixed.is_empty() {
                continue;
            }
            for (offset, next) in later.iter_mut().take(longest).enumerate() {
                let end = start + offset + 1;
                self.advance(start..end, previous, next, &mut words, &mut choices);
            }
        }
        self.trailing_initials(&mut paths, &mut words);
        self.whole_initials(&mut words, &mut choices);
        self.whole_latin(&mut choices);
        self.two_initials(&paths, &mut words, &mut choices);
        // Initials are never the full spelling of a never-volunteer word.
        choices.initials.retain(|c| !self.lexicon.is_no_suggest_text(c.text.as_ref()));
        choices.write(paths, &self.keys, out, preedit, |text, consumed| self.volunteers(text, consumed));
    }

    /// Whether `text` for the keys up to raw byte `consumed` would volunteer
    /// a word of the never-volunteer list (`Lexicon::volunteers_by`): the
    /// keys are not the digits of its full spelling.
    fn volunteers(&self, text: &str, consumed: usize) -> bool {
        let lexicon = self.lexicon;
        if !lexicon.pinyin || lexicon.no_suggest().is_empty() {
            return false;
        }
        let digits = &self.keys.digits;
        let typed = &digits[..(0..=digits.len()).find(|&d| self.keys.consumed(d) >= consumed).unwrap_or(digits.len())];
        let keypad = |letters: &[u8], at: &[u8]| letters.len() == at.len()
            && letters.iter().zip(at).all(|(&l, &k)| l.is_ascii_lowercase() && digit(l) == k);
        lexicon.volunteers_by(
            text,
            |letters| keypad(letters, typed),
            |letters| typed.windows(letters.len()).any(|w| keypad(letters, w)),
        )
    }

    fn advance(
        &self,
        span: Range<usize>,
        previous: &Paths,
        next: &mut Paths,
        buffer: &mut SpanWords<'a>,
        choices: &mut Choices<'a>,
    ) {
        let keys = &self.keys.digits[span.clone()];
        let hash = signature(keys.iter().copied());
        let at_end = span.end == self.keys.digits.len();
        let whole = span.start == 0 && at_end;
        let consumed = self.keys.consumed(span.end);
        let breaks = self.keys.internal_breaks(span.start, span.end);
        // A final single key can be an initial while the user is still typing.
        let limit = if whole { WHOLE_WORDS } else { SPAN_WORDS };
        self.chinese_words(span.clone(), hash, whole, at_end && keys.len() == 1, limit, buffer);
        let pairs = self.lexicon.pairs();
        for &word in &buffer.words {
            if whole {
                choices.complete.push(word.choice(consumed, word.kind));
            } else {
                if span.start == 0 {
                    choices.partial.push(word.choice(consumed, CandidateKind::Partial));
                }
                next.extend(previous, word, pairs, at_end);
            }
        }
        // Require more evidence before treating a substring as Japanese.
        let minimum = if whole { 3 } else { 4 };
        if keys.len() < minimum || !breaks.is_empty() {
            return;
        }
        for row in self.index.japanese_rows(self.japanese, keys, hash) {
            let word = Word {
                text: self.japanese.text(row),
                reading: self.japanese.reading(row),
                tail: "",
                initials: false,
                common: false,
                score: word_score(self.japanese.weight[row]),
                id: None,
                kind: CandidateKind::Japanese,
            };
            if whole {
                choices.japanese.push(word.choice(consumed, word.kind));
            } else {
                next.extend(previous, word, pairs, at_end);
            }
        }
    }

    /// Ends paths with a word whose last syllables are typed as initials: all
    /// of them after spelled words (你好 + 99, 現在), or the last one or two
    /// after its spelled syllables (9426 9: xian z, 現在).
    fn trailing_initials(&self, paths: &mut [Paths], buffer: &mut SpanWords<'a>) {
        let length = self.keys.digits.len();
        let lexicon = self.lexicon;
        let pairs = lexicon.pairs();
        let (paths, last) = paths.split_at_mut(length);
        let last = &mut last[0];
        // The spelled part is two keys or more: a, e and o before initials are mostly noise.
        let first = length.saturating_sub(INITIALS).max(2);
        for (start, previous) in paths.iter().enumerate().take(length.saturating_sub(1)).skip(first) {
            if previous.is_empty() {
                continue;
            }
            let breaks = self.keys.internal_breaks(start, length);
            self.initials_words(start..length, breaks, false, buffer);
            for &word in &buffer.words {
                last.extend(previous, word, pairs, true);
            }
        }
        for tail in 1..=TAIL {
            let Some(end) = length.checked_sub(tail).filter(|&end| end > 1) else { break };
            // The spelled part is two keys or more, as above.
            for (start, previous) in paths.iter().enumerate().take(end - 1).skip(end.saturating_sub(TAIL_SPELLED)) {
                if previous.is_empty() || !self.keys.internal_breaks(start, length).is_empty() {
                    continue;
                }
                let span = start..end;
                let hash = signature(self.keys.digits[span.clone()].iter().copied());
                self.chinese_words(span, hash, false, false, TAIL_WORDS, buffer);
                self.complete_words(&buffer.words, end, previous, last);
            }
        }
    }

    /// Extends `previous` into `last` with the words that start with one of
    /// the spelled `words` and go on with the initials typed from `end`.
    /// Words with the same initials share one pass over the abbreviation rows.
    fn complete_words(&self, words: &[Word<'a>], end: usize, previous: &Paths, last: &mut Paths) {
        let lexicon = self.lexicon;
        let tail = &self.keys.digits[end..];
        let mut initials = [([0u8; INITIALS], 0usize); TAIL_WORDS];
        for (word, (letters, count)) in words.iter().zip(initials.iter_mut()) {
            *count = word_initials(word.reading, letters);
        }
        let initials = &initials[..words.len().min(TAIL_WORDS)];
        for (i, &(letters, count)) in initials.iter().enumerate() {
            if count == 0 || count + tail.len() > INITIALS || initials[..i].contains(&(letters, count)) {
                continue;
            }
            let mut keys = [0u8; INITIALS];
            for (key, &letter) in keys.iter_mut().zip(&letters[..count]) {
                *key = digit(letter);
            }
            keys[count..count + tail.len()].copy_from_slice(tail);
            let rows = self.index.initials_rows(lexicon, &keys[..count + tail.len()]).filter(|&row| {
                let reading = lexicon.words.reading(&lexicon.storage, row);
                reading.bytes().step_by(2).take(count).eq(letters[..count].iter().copied())
            });
            for row in rows.take(TAIL_ROWS) {
                let weight = lexicon.words.weight(row);
                let reading = lexicon.words.reading(&lexicon.storage, row);
                let text = lexicon.words.text(&lexicon.storage, row, self.hans);
                let spelled = words.iter().zip(initials).skip(i).find(|(word, &same)| {
                    same == (letters, count) && text.len() > word.text.len() && text.starts_with(word.text)
                });
                let Some((word, _)) = spelled else { continue };
                let id = lexicon.pairs().map_or(START, |_| lexicon.index.word_ids.ids.get(row));
                let completed = Word {
                    text,
                    reading: word.reading,
                    tail: &reading[2 * count..],
                    initials: true,
                    common: weight >= FIRST_PAGE_WEIGHT,
                    score: initials_score(weight),
                    id: Some(id),
                    kind: CandidateKind::Correction,
                };
                last.extend(previous, completed, lexicon.pairs(), true);
            }
        }
    }

    /// Fills `buffer` with the abbreviated words whose initials are the keys of `span`.
    fn initials_words(&self, span: Range<usize>, breaks: &[usize], whole: bool, buffer: &mut SpanWords<'a>) {
        let lexicon = self.lexicon;
        let start = span.start;
        let keys = &self.keys.digits[span];
        buffer.words.clear();
        buffer.seen.clear();
        if !(2..=INITIALS).contains(&keys.len()) {
            return;
        }
        let limit = if whole { INITIALS_WORDS } else { NEXT_WORDS };
        // Twice the words kept, for the same word in both scripts and readings (下載 x4z3, x4z4).
        for row in self.index.initials_rows(lexicon, keys).take(2 * limit) {
            let reading = lexicon.words.reading(&lexicon.storage, row);
            let weight = lexicon.words.weight(row);
            if !breaks.is_empty() && !matches_breaks(reading, start, breaks) {
                continue;
            }
            // No clause start and end terms, as in merge::push_whole_words.
            let score = initials_score(weight);
            buffer.words.push(Word {
                text: lexicon.words.text(&lexicon.storage, row, self.hans),
                reading,
                tail: "",
                initials: true,
                common: whole && weight >= FIRST_PAGE_WEIGHT,
                score,
                id: Some(lexicon.pairs().map_or(START, |_| lexicon.index.word_ids.ids.get(row))),
                kind: CandidateKind::Correction,
            });
        }
        buffer.words.sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.text.cmp(b.text)));
        buffer.words.retain(|word| buffer.seen.insert(word.text));
        buffer.words.truncate(limit);
    }

    /// Abbreviated words for the whole input (99: 現在, 下載).
    fn whole_initials(&self, buffer: &mut SpanWords<'a>, choices: &mut Choices<'a>) {
        let length = self.keys.digits.len();
        self.initials_words(0..length, self.keys.internal_breaks(0, length), true, buffer);
        let consumed = self.keys.consumed(length);
        choices.initials.extend(buffer.words.iter().map(|word| word.choice(consumed, word.kind)));
    }

    /// Two abbreviated words in a row for initials that make no single one
    /// (7744: 搜索 + 好好), when nothing else reads the input, English included.
    fn two_initials(&self, paths: &[Paths], buffer: &mut SpanWords<'a>, choices: &mut Choices<'a>) {
        let length = self.keys.digits.len();
        if !choices.initials.is_empty() || !choices.latin.is_empty() || !(4..=2 * INITIALS).contains(&length)
            || paths[1..].iter().any(|p| !p.is_empty())
        {
            return;
        }
        let consumed = self.keys.consumed(length);
        let pairs = self.lexicon.pairs();
        let breaks = self.keys.internal_breaks(0, length);
        // A typed word break can only be the split between the two words.
        for split in (2..=length - 2).filter(|&split| breaks.iter().all(|&b| b == split)) {
            let mut firsts = [None; NEXT_WORDS];
            self.initials_words(0..split, &[], false, buffer);
            for (slot, &word) in firsts.iter_mut().zip(&buffer.words) {
                *slot = Some(word);
            }
            self.initials_words(split..length, &[], false, buffer);
            for first in firsts.iter().flatten() {
                for second in buffer.words.iter().take(NEXT_WORDS) {
                    let mut score = first.score + second.score;
                    if let (Some(pairs), Some(a), Some(b)) = (pairs, first.id, second.id) {
                        score += f64::from(pairs.score(a, b) + pairs.score(b, END)) / STEP;
                    }
                    choices.initials.push(Choice {
                        text: [first.text, second.text].concat().into(),
                        reading: [first.reading, " ", second.reading].concat().into(),
                        consumed,
                        score,
                        kind: CandidateKind::Correction,
                        score_kind: CandidateScoreKind::Path,
                        common: false,
                    });
                }
            }
        }
    }

    /// English words spelled by the whole input (65: OK), except where their
    /// letters are one pinyin syllable (an, she). `latin_first` is set when
    /// the letters are not pinyin at all and have a vowel.
    fn whole_latin(&self, choices: &mut Choices<'a>) {
        let lexicon = self.lexicon;
        let keys = &self.keys.digits;
        if keys.len() > LATIN || !self.keys.breaks.is_empty() {
            return;
        }
        let consumed = self.keys.consumed(keys.len());
        let mut letters = [0u8; LATIN];
        for (lo, hi) in self.index.latin_rows(lexicon, keys) {
            for row in lo..hi {
                let reading = lexicon.words.reading(&lexicon.storage, row);
                for (letter, byte) in letters.iter_mut().zip(reading.bytes()) {
                    *letter = byte.to_ascii_lowercase();
                }
                let letters = &letters[..keys.len()];
                let first = match syllables(lexicon, letters) {
                    1 => continue,
                    0 => letters.iter().any(|b| b"aeiou".contains(b)),
                    _ => false,
                };
                choices.latin_first |= first;
                choices.latin.push(Choice {
                    text: lexicon.words.text(&lexicon.storage, row, self.hans).into(),
                    reading: reading.into(),
                    consumed,
                    score: word_score(lexicon.words.weight(row)) + if first { LATIN_FIRST_TERM } else { 0.0 },
                    kind: CandidateKind::Exact,
                    score_kind: CandidateScoreKind::Lexicon,
                    common: true,
                });
            }
        }
        choices.latin.sort_by(|a, b| b.score.total_cmp(&a.score));
    }

    #[allow(clippy::too_many_arguments)]
    fn chinese_words(&self, span: Range<usize>, hash: u32, whole: bool, initial: bool, limit: usize, buffer: &mut SpanWords<'a>) {
        let lexicon = self.lexicon;
        let breaks = self.keys.internal_breaks(span.start, span.end);
        let keys = &self.keys.digits[span.clone()];
        buffer.words.clear();
        buffer.seen.clear();
        for (lo, hi) in self.index.chinese_rows(lexicon, keys, hash) {
            let rows = (lo..hi).filter(|&row| {
                (initial || lexicon.words.is_fully_spelled(row))
                    && (breaks.is_empty() || matches_breaks(lexicon.words.reading(&lexicon.storage, row), span.start, breaks))
            });
            for row in rows.take(limit) {
                let spelled = lexicon.words.is_fully_spelled(row);
                let mut score = if whole {
                    whole_word_score(lexicon, row)
                } else {
                    word_score(lexicon.words.weight(row))
                };
                if !spelled {
                    score -= 5.0;
                }
                let id = lexicon.pairs().map_or(START, |_| lexicon.index.word_ids.ids.get(row));
                buffer.words.push(Word {
                    text: lexicon.words.text(&lexicon.storage, row, self.hans),
                    reading: lexicon.words.reading(&lexicon.storage, row),
                    tail: "",
                    initials: false,
                    common: false,
                    score,
                    id: Some(id),
                    kind: if spelled { CandidateKind::Exact } else { CandidateKind::Correction },
                });
            }
        }
        buffer.words.sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.text.cmp(b.text)));
        buffer.words.retain(|word| buffer.seen.insert(word.text));
        buffer.words.truncate(limit);
    }
}

#[derive(Default)]
struct SpanWords<'a> {
    words: Vec<Word<'a>>,
    seen: HashSet<&'a str>,
}

#[derive(Clone, Copy)]
struct Word<'a> {
    text: &'a str,
    reading: &'a str,
    /// Initials after `reading` when the word was typed partly spelled (z4 after xian4).
    tail: &'a str,
    /// The word was typed as initials, wholly or in part.
    initials: bool,
    /// Typed as initials and common enough for the first page next to spelled
    /// words: a whole-input abbreviation or a word completed from its spelled
    /// syllables, not an abbreviated word after spelled ones.
    common: bool,
    score: f64,
    id: Option<u32>,
    kind: CandidateKind,
}

impl<'a> Word<'a> {
    fn choice(self, consumed: usize, kind: CandidateKind) -> Choice<'a> {
        Choice {
            text: self.text.into(),
            reading: self.reading.into(),
            consumed,
            score: self.score,
            kind,
            score_kind: CandidateScoreKind::Lexicon,
            common: self.common || !self.initials,
        }
    }
}

struct Path {
    text: String,
    reading: String,
    score: f64,
    last: Option<u32>,
    /// Spelled, or one `common` word typed as initials from the start of the input.
    common: bool,
}

#[derive(Default)]
struct Paths {
    chinese: Vec<Path>,
    mixed: Vec<Path>,
    /// Paths ending in a word typed as initials; their own beam, so they
    /// never push spelled paths out.
    initials: Vec<Path>,
}

impl Paths {
    fn is_empty(&self) -> bool {
        self.chinese.is_empty() && self.mixed.is_empty()
    }

    // Keep an independent beam for paths with one reviewed Japanese span.
    fn extend(&mut self, previous: &Self, word: Word<'_>, pairs: Option<&Pairs>, at_end: bool) {
        let japanese = word.id.is_none();
        for (paths, already_mixed) in [(&previous.chinese, false), (&previous.mixed, true)] {
            if japanese && already_mixed {
                break;
            }
            let output = if word.initials {
                &mut self.initials
            } else if japanese || already_mixed {
                &mut self.mixed
            } else {
                &mut self.chinese
            };
            for path in paths {
                let mut score = path.score + word.score;
                if let (Some(pairs), Some(id)) = (pairs, word.id) {
                    if let Some(last) = path.last {
                        score += f64::from(pairs.score(last, id)) / STEP;
                    }
                    if at_end {
                        score += f64::from(pairs.score(id, END)) / STEP;
                    }
                }
                push_path(output, path, word, score);
            }
        }
    }
}

fn push_path(output: &mut Vec<Path>, previous: &Path, word: Word<'_>, score: f64) {
    let at = output.partition_point(|path| path.score >= score);
    if at >= PATHS {
        return;
    }
    let mut text = String::with_capacity(previous.text.len() + word.text.len());
    text.push_str(&previous.text);
    text.push_str(word.text);
    if let Some(same) = output.iter().position(|path| path.text == text && path.last == word.id) {
        if same < at {
            return;
        }
        output.remove(same);
    }
    let mut reading = String::with_capacity(previous.reading.len() + word.reading.len() + word.tail.len() + 2);
    reading.push_str(&previous.reading);
    if !reading.is_empty() { reading.push(' '); }
    reading.push_str(word.reading);
    if !word.tail.is_empty() {
        reading.push(' ');
        reading.push_str(word.tail);
    }
    // Only a word typed as initials at the start of the input counts as common.
    let common = if word.initials { word.common && previous.text.is_empty() } else { previous.common };
    output.insert(at, Path { text, reading, score, last: word.id, common });
    output.truncate(PATHS);
}

struct Choice<'a> {
    text: Cow<'a, str>,
    reading: Cow<'a, str>,
    consumed: usize,
    score: f64,
    kind: CandidateKind,
    score_kind: CandidateScoreKind,
    /// See `Word::common`; true for the other kinds.
    common: bool,
}

#[derive(Default)]
struct Choices<'a> {
    complete: Vec<Choice<'a>>,
    partial: Vec<Choice<'a>>,
    japanese: Vec<Choice<'a>>,
    /// Abbreviated words for the whole input and complete paths ending in one.
    initials: Vec<Choice<'a>>,
    /// English words for the whole input.
    latin: Vec<Choice<'a>>,
    latin_first: bool,
}

impl Choices<'_> {
    fn write(mut self, mut paths: Vec<Paths>, keys: &KeyReading<'_>, out: &mut CandidatePoolSoA, preedit: &mut String,
             volunteers: impl Fn(&str, usize) -> bool) {
        let complete = paths.pop().unwrap_or_default();
        let length = keys.consumed(keys.digits.len());
        let from_path = |path: Path, consumed, kind| Choice {
            text: path.text.into(),
            reading: path.reading.into(),
            consumed,
            score: path.score,
            kind,
            score_kind: CandidateScoreKind::Path,
            common: path.common,
        };
        self.complete.extend(complete.chinese.into_iter().map(|p| from_path(p, length, CandidateKind::Path)));
        // Paths with initials rank with the abbreviated words, after the spelled ones.
        self.initials.extend(complete.initials.into_iter().map(|p| from_path(p, length, CandidateKind::Correction)));
        // Keep the decoded prefix selectable when the next digit starts an
        // unfinished syllable and leaves no complete spelled Chinese path.
        if self.complete.is_empty() {
            if let Some((consumed, prefix)) = paths.into_iter().enumerate().skip(1).rev()
                .find(|(_, paths)| !paths.chinese.is_empty())
            {
                self.partial.extend(prefix.chinese.into_iter().take(2)
                    .map(|path| from_path(path, keys.consumed(consumed), CandidateKind::Partial)));
            }
        }
        self.complete.sort_by(|a, b| b.score.total_cmp(&a.score));
        self.initials.sort_by(|a, b| b.score.total_cmp(&a.score));
        self.japanese.sort_by(|a, b| b.score.total_cmp(&a.score));
        self.japanese.extend(complete.mixed.into_iter().map(|p| from_path(p, length, CandidateKind::Japanese)));
        self.partial.sort_by(|a, b| b.consumed.cmp(&a.consumed).then_with(|| b.score.total_cmp(&a.score)));

        let mut chinese_text = HashSet::new();
        let complete: Vec<_> = self.complete.iter().filter(|c| chinese_text.insert(c.text.as_ref())).collect();
        self.japanese.retain(|c| !chinese_text.contains(c.text.as_ref()));
        // Reserve a visible slot for Japanese and keep a leading selection within reach.
        // Spelled words lead, and so does the decoded prefix of two keys or
        // more while the next syllable is being typed (ta men 9). Abbreviated
        // words and words completed from that prefix (现在 for xian z) come
        // next, two of them on the first page when they are common, and only
        // after a full page when the input spells one syllable (yu). English
        // whose letters are not pinyin leads input that decodes to nothing, or
        // to a path of several words it outweighs by LATIN_LEAD_GAP (alfred
        // over 啊垃圾饿) when it has LATIN_LEAD_LETTERS letters or more; it
        // must also outweigh the first abbreviated word. Otherwise it follows
        // the first Chinese candidate or two.
        let spelled = !complete.is_empty();
        // A complete reading through Japanese (莫莫可愛い) counts as spelled here.
        let typing = !spelled && self.japanese.is_empty() && self.partial.first().is_some_and(|p| p.consumed >= 2);
        let decoded = spelled || typing;
        let lead = self.latin_first
            && !typing
            && self.latin.first().is_some_and(|l| {
                complete.first().is_none_or(|c| {
                    c.score_kind == CandidateScoreKind::Path
                        && keys.digits.len() >= LATIN_LEAD_LETTERS
                        && l.score >= c.score + LATIN_LEAD_GAP
                }) && self.initials.first().is_none_or(|i| l.score >= i.score)
            });
        let one_syllable = complete.first().is_some_and(|c| c.text.chars().nth(1).is_none());
        let mut complete = complete.into_iter();
        let mut japanese = self.japanese.iter();
        let mut partial = self.partial.iter();
        let mut initials = self.initials.iter();
        let mut latin = self.latin.iter();
        let mut first: Vec<_> = Vec::new();
        if lead {
            first.extend(latin.next());
        }
        if typing {
            first.extend(partial.by_ref().take(2));
        }
        first.extend(complete.by_ref().take(2));
        if self.latin_first && decoded && !lead {
            first.extend(latin.next());
        }
        first.extend(japanese.next());
        first.extend(complete.next());
        if one_syllable {
            first.extend(complete.by_ref().take(PAGE.saturating_sub(first.len())));
        }
        let strong = self.initials.iter().take(2).take_while(|c| !decoded || c.common).count();
        let mut common = initials.by_ref().take(strong);
        first.extend(common.next());
        if !decoded {
            first.extend(latin.next());
        }
        first.extend(common);
        first.extend(partial.by_ref().take(2));
        // While typing, other readings of the prefix fill the first page before the guesses.
        if typing {
            first.extend(partial.by_ref().take(PAGE.saturating_sub(first.len())));
        }
        let order = first.into_iter().chain(latin).chain(complete).chain(initials).chain(japanese).chain(partial);
        let mut added = HashSet::new();
        // Never a word of the never-volunteer list for keys that do not spell it in full.
        let order = order.filter(|c| !volunteers(c.text.as_ref(), c.consumed));
        for (rank, choice) in order.filter(|c| added.insert((c.text.as_ref(), c.consumed))).take(CANDIDATE_LIMIT).enumerate() {
            // The preedit spells the leading candidate, abbreviated words included.
            if rank == 0 {
                show_reading(preedit, &choice.reading, choice.consumed, keys);
            }
            out.push(&choice.text, choice.consumed, choice.kind as u32, choice.score_kind as u32, choice.score);
        }
    }
}

/// Replaces the raw keys in `preedit` with `reading` for the first `consumed`
/// input bytes, keeping the rest as typed. The text is built after the raw
/// keys and the keys are then dropped, so the session buffer is reused.
fn show_reading(preedit: &mut String, reading: &str, consumed: usize, keys: &KeyReading<'_>) {
    let raw = preedit.len();
    if !push_reading(preedit, reading, consumed, keys) {
        preedit.truncate(raw);
        return;
    }
    if consumed < raw {
        if !preedit.ends_with('\'') { preedit.push(' '); }
        preedit.extend_from_within(consumed..raw);
    }
    preedit.drain(..raw);
}

fn push_reading(out: &mut String, reading: &str, consumed: usize, keys: &KeyReading<'_>) -> bool {
    let mut position = 0;
    let mut boundary = false;
    let mut breaks = keys.breaks.iter().copied().peekable();
    for byte in reading.bytes() {
        // English readings are upper case (IPHONE5) and show in lower case.
        let byte = byte.to_ascii_lowercase();
        if byte.is_ascii_lowercase() {
            if keys.digits.get(position) != Some(&digit(byte)) { return false; }
            if breaks.peek() == Some(&position) {
                breaks.next();
                out.push('\'');
            } else if boundary {
                out.push(' ');
            }
            boundary = false;
            out.push(char::from(byte));
            position += 1;
        } else if is_tone(byte, true) || byte == b' ' || byte == b'\'' {
            boundary = true;
        }
    }
    // Never replace digits with a reading that does not match their length.
    if position == 0 || keys.consumed(position) != consumed { return false; }
    if breaks.peek() == Some(&position) { out.push('\''); }
    true
}
