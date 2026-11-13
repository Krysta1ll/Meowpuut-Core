//! Builds complete and leading candidates by joining words that match keypad spans.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;

use super::{matches_breaks, signature, KeyReading, NineKeyIndex};
use crate::lexicon::Lexicon;
use crate::merge::whole_word_score;
use crate::pairs::{Pairs, END, START, STEP};
use crate::reading::CANDIDATE_LIMIT;
use crate::soa::{CandidatePoolSoA, JapaneseTableSoA};
use crate::{CandidateKind, CandidateScoreKind};

const SPAN_WORDS: usize = 12;
const WHOLE_WORDS: usize = 160;
const PATHS: usize = 8;

fn word_score(weight: i32) -> f64 {
    f64::from(weight) / 700.0 - 19.0
}

impl NineKeyIndex {
    pub(crate) fn decode(
        &self,
        lexicon: &Lexicon,
        japanese: &JapaneseTableSoA,
        keys: &[u8],
        hans: bool,
        left: u32,
        out: &mut CandidatePoolSoA,
    ) {
        out.clear();
        if !keys.is_empty() {
            let decoder = Decoder { index: self, lexicon, japanese, keys: KeyReading::new(keys), hans };
            decoder.decode(left, out);
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
    fn decode(&self, left: u32, out: &mut CandidatePoolSoA) {
        let length = self.keys.digits.len();
        let longest = self.lexicon.longest.max(20); // Registered Japanese readings are at most 20 keys.
        let mut paths: Vec<_> = (0..=length).map(|_| Paths::default()).collect();
        paths[0].chinese.push(Path { text: String::new(), score: 0.0, last: Some(left) });
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
        choices.write(paths, &self.keys, out);
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
        self.chinese_words(span.clone(), hash, whole, at_end && keys.len() == 1, buffer);
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

    fn chinese_words(&self, span: Range<usize>, hash: u32, whole: bool, initial: bool, buffer: &mut SpanWords<'a>) {
        let lexicon = self.lexicon;
        let breaks = self.keys.internal_breaks(span.start, span.end);
        let keys = &self.keys.digits[span.clone()];
        let limit = if whole { WHOLE_WORDS } else { SPAN_WORDS };
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
    score: f64,
    id: Option<u32>,
    kind: CandidateKind,
}

impl<'a> Word<'a> {
    fn choice(self, consumed: usize, kind: CandidateKind) -> Choice<'a> {
        Choice {
            text: self.text.into(),
            consumed,
            score: self.score,
            kind,
            score_kind: CandidateScoreKind::Lexicon,
        }
    }
}

struct Path {
    text: String,
    score: f64,
    last: Option<u32>,
}

#[derive(Default)]
struct Paths {
    chinese: Vec<Path>,
    mixed: Vec<Path>,
}

impl Paths {
    // Keep an independent beam for paths with one reviewed Japanese span.
    fn extend(&mut self, previous: &Self, word: Word<'_>, pairs: Option<&Pairs>, at_end: bool) {
        let japanese = word.id.is_none();
        for (paths, already_mixed) in [(&previous.chinese, false), (&previous.mixed, true)] {
            if japanese && already_mixed {
                break;
            }
            let output = if japanese || already_mixed { &mut self.mixed } else { &mut self.chinese };
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
    output.insert(at, Path { text, score, last: word.id });
    output.truncate(PATHS);
}

struct Choice<'a> {
    text: Cow<'a, str>,
    consumed: usize,
    score: f64,
    kind: CandidateKind,
    score_kind: CandidateScoreKind,
}

#[derive(Default)]
struct Choices<'a> {
    complete: Vec<Choice<'a>>,
    partial: Vec<Choice<'a>>,
    japanese: Vec<Choice<'a>>,
}

impl Choices<'_> {
    fn write(mut self, mut paths: Vec<Paths>, keys: &KeyReading<'_>, out: &mut CandidatePoolSoA) {
        let complete = paths.pop().unwrap_or_default();
        let length = keys.consumed(keys.digits.len());
        let from_path = |path: Path, consumed, kind| Choice {
            text: path.text.into(),
            consumed,
            score: path.score,
            kind,
            score_kind: CandidateScoreKind::Path,
        };
        // Keep the decoded prefix selectable when the next digit starts an
        // unfinished syllable and leaves no complete Chinese path.
        if self.complete.is_empty() && complete.chinese.is_empty() {
            if let Some((consumed, prefix)) = paths.into_iter().enumerate().skip(1).rev()
                .find(|(_, paths)| !paths.chinese.is_empty())
            {
                self.partial.extend(prefix.chinese.into_iter().take(2)
                    .map(|path| from_path(path, keys.consumed(consumed), CandidateKind::Partial)));
            }
        }
        self.complete.extend(complete.chinese.into_iter().map(|p| from_path(p, length, CandidateKind::Path)));
        self.complete.sort_by(|a, b| b.score.total_cmp(&a.score));
        self.japanese.sort_by(|a, b| b.score.total_cmp(&a.score));
        self.japanese.extend(complete.mixed.into_iter().map(|p| from_path(p, length, CandidateKind::Japanese)));
        self.partial.sort_by(|a, b| b.consumed.cmp(&a.consumed).then_with(|| b.score.total_cmp(&a.score)));

        let mut chinese_text = HashSet::new();
        let complete: Vec<_> = self.complete.iter().filter(|c| chinese_text.insert(c.text.as_ref())).collect();
        self.japanese.retain(|c| !chinese_text.contains(c.text.as_ref()));
        // Reserve a visible slot for Japanese and keep a leading selection within reach.
        let mut complete = complete.into_iter();
        let mut japanese = self.japanese.iter();
        let mut partial = self.partial.iter();
        let mut first: Vec<_> = complete.by_ref().take(2).collect();
        first.extend(japanese.next());
        first.extend(complete.next());
        first.extend(partial.by_ref().take(2));
        let order = first.into_iter().chain(complete).chain(japanese).chain(partial);
        let mut added = HashSet::new();
        for choice in order.filter(|c| added.insert((c.text.as_ref(), c.consumed))).take(CANDIDATE_LIMIT) {
            out.push(&choice.text, choice.consumed, choice.kind as u32, choice.score_kind as u32, choice.score);
        }
    }
}
