#![forbid(unsafe_code)]
//! Zero-allocation SoA beam decoder (`decode_into` and `decode_with_buffers`).
//!
//! # Examples
//!
//! ```
//! use meowpuut_core::{Lexicon, DecoderScratchSoA};
//! use meowpuut_core::decoder::{decode_into, has_exact_match};
//!
//! let tsv = concat!(
//!     "MEOWPUUT-PINYIN-1\n",
//!     "hao\thao3\t好\t好\t4800\n",
//!     "ni\tni3\t你\t你\t5000\n",
//!     "nihao\tni3hao3\t你好\t你好\t5400\n",
//!     "# context\n",
//!     "你好\t600\n",
//! );
//! let lex = Lexicon::load(tsv).unwrap();
//! assert!(has_exact_match(&lex, "nihao"));
//! let mut scratch = DecoderScratchSoA::new();
//! decode_into(&lex, "nihao", false, false, true, &mut scratch);
//! assert_eq!(scratch.decoded.len(), 1);
//! assert_eq!(scratch.decoded.text(0), "你好");
//! ```

pub use crate::fallback::{fallback_into, has_exact_match, unfinished_phrase_into};
use crate::latin;
use crate::lexicon::Lexicon;
use crate::merge;
use crate::pairs::{END, START, STEP};
use crate::reading::{Reading, BEAM, CANDIDATE_LIMIT};
use crate::workspace::{
    text_hash, DecodeOutputSoA, DecoderScratchSoA, NasalVariantsStack, PathLatticeSoA, StepPathsSoA, LATTICE_STATES,
};
use crate::{CandidateKind, CandidateScoreKind};

/// Paths kept per input position with a pair table (the research behind the
/// table lost nothing at 16 and 5 sentences at 5).
pub const PAIR_STATES: usize = LATTICE_STATES;
/// Paths per position that may start with the same word, so that different
/// neighbouring words survive for the pair score (not at the first position,
/// whose paths are the candidates).
pub const PAIR_STATES_PER_WORD: usize = 4;
/// Steps kept per word and span past the first position (see `word_floor`).
const PAIR_WORD_STEPS: usize = PAIR_STATES_PER_WORD * 2;
/// Distinct texts taken per span, heaviest first, with a pair table.
pub const PAIR_SPAN_WORDS: usize = 10;
/// Word id of the end of a decoded prefix that more keys follow: no clause-end term there.
const OPEN_END: u32 = u32::MAX;

/// Core SoA beam decoder (`decode_into` on `DecoderScratchSoA::decoded`).
pub fn decode_into(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    complete_only: bool,
    compute_metadata: bool,
    scratch: &mut DecoderScratchSoA,
) {
    decode_with_buffers(
        lexicon,
        input,
        hans,
        complete_only,
        compute_metadata,
        false,
        &mut scratch.lattice,
        &mut scratch.step_paths,
        &mut scratch.fallback_raw,
        &mut scratch.fallback_order,
        &mut scratch.fallback_out,
        &mut scratch.decoded,
    );
}

/// Core SoA beam decoder writing into an explicit `DecodeOutputSoA` target.
/// `open_end`: more keys follow `input` (a leading segment), so the last
/// word gets no clause-end term.
#[allow(clippy::too_many_arguments)]
pub fn decode_with_buffers(
    lexicon: &Lexicon,
    input: &str,
    hans: bool,
    complete_only: bool,
    compute_metadata: bool,
    open_end: bool,
    lattice: &mut PathLatticeSoA,
    step_paths: &mut StepPathsSoA,
    fb_raw: &mut DecodeOutputSoA,
    fb_order: &mut Vec<usize>,
    fb_out: &mut DecodeOutputSoA,
    out: &mut DecodeOutputSoA,
) {
    out.clear();
    let reading = Reading::new(input, lexicon.pinyin);
    let length = reading.len();
    if length == 0 {
        return;
    }

    let verified_word = |idx: usize| -> bool {
        if !compute_metadata || !lexicon.pinyin {
            true
        } else {
            lexicon.words.is_verified_spelling(idx)
        }
    };

    // With a pair table the spelled whole-input words compete with the paths
    // by score (merge.rs). Abbreviation rows are listed first here only when
    // no spelled word reads as the whole input (nh, xkl); otherwise they are
    // scored with the spelled words and come after them (嗎 before 默哀 for ma).
    let ranked = lexicon.pairs().is_some();
    let (whole_lo, whole_hi) = lexicon.range(reading.sounds());
    let whole = |index: usize| reading.matches(lexicon.words.reading(&lexicon.storage, index).as_bytes(), 0, length);
    let spelled_whole = ranked && (whole_lo..whole_hi).any(|i| lexicon.words.is_fully_spelled(i) && whole(i));
    for index in whole_lo..whole_hi {
        let spelled = lexicon.words.is_fully_spelled(index);
        if ranked && (spelled || !lexicon.pinyin || spelled_whole) {
            continue;
        }
        if (!complete_only || spelled) && whole(index) {
            let kind = if !reading.unambiguous {
                CandidateKind::Unknown as u32
            } else if lexicon.pinyin && !spelled {
                CandidateKind::Correction as u32
            } else if verified_word(index) {
                CandidateKind::Exact as u32
            } else {
                CandidateKind::Unknown as u32
            };
            let text = lexicon.words.text(&lexicon.storage, index, hans);
            if !out.contains_text(text) {
                out.push(
                    text,
                    input.len(),
                    kind,
                    CandidateScoreKind::Lexicon as u32,
                    f64::from(lexicon.words.weight(index)),
                );
            }
        }
        if out.len() == CANDIDATE_LIMIT {
            return;
        }
    }

    // English for the whole input keeps its fixed place (latin.rs), ahead of the
    // part of the list that is sorted by score.
    latin::whole_into(lexicon, &reading, input.len(), hans, out);

    let mut sort_from: Option<usize> = None;
    if ranked {
        sort_from = Some(out.len());
        merge::push_whole_words(lexicon, &reading, input, hans, spelled_whole && !complete_only, compute_metadata, out);
        if !complete_only && input == reading.sounds() {
            merge::push_mixed_whole(lexicon, input, hans, out);
        }
    }

    if !complete_only && lexicon.pinyin && !out.is_empty() && input == reading.sounds() {
        let mut insertion = 3.min(out.len());
        let mut added = 0usize;
        let variants = NasalVariantsStack::from_sounds(reading.sounds());
        for v_idx in 0..variants.len() {
            let (lo, hi) = lexicon.range(variants.get(v_idx));
            for index in lo..hi {
                if added >= 2 {
                    break;
                }
                let weight = lexicon.words.weight(index);
                if weight < 4000 {
                    break;
                }
                let text = lexicon.words.text(&lexicon.storage, index, hans);
                if !out.contains_text(text) {
                    out.insert_at(
                        insertion,
                        text,
                        input.len(),
                        CandidateKind::Correction as u32,
                        CandidateScoreKind::Lexicon as u32,
                        if ranked { merge::whole_word_score(lexicon, index) - merge::GUESS_PENALTY - 900.0 / 700.0 } else { f64::from(weight) },
                    );
                    insertion += 1;
                    added += 1;
                }
            }
            if added == 2 {
                break;
            }
        }
        out.truncate(CANDIDATE_LIMIT);
    }

    if out.is_empty() && lexicon.pinyin && !complete_only {
        fallback_into(lexicon, input, hans, false, fb_raw, fb_order, fb_out);
        for i in 0..fb_out.len() {
            let text = fb_out.text(i);
            if !out.contains_text(text) {
                out.push(
                    text,
                    input.len(),
                    CandidateKind::Correction as u32,
                    CandidateScoreKind::Fallback as u32,
                    if sort_from.is_some() { merge::guess_score(fb_out.score[i]) } else { fb_out.score[i] },
                );
            }
        }
    }

    lattice.reset(length);
    lattice.push_end(length, if open_end { OPEN_END } else { END });
    let pairs = lexicon.pairs();
    if let Some(p) = pairs {
        lattice.pair_cache.use_table(std::ptr::from_ref(p) as usize);
    }
    // Without a pair table the decoder keeps BEAM paths per position and
    // BEAM rows per span, as before the table existed.
    let (states, span_words) = if pairs.is_some() { (PAIR_STATES, PAIR_SPAN_WORDS) } else { (BEAM, BEAM) };
    let sounds_str = reading.sounds();
    let matches = |index: usize, start: usize, end: usize| {
        reading.matches(lexicon.words.reading(&lexicon.storage, index).as_bytes(), start, end)
    };

    let mut mixed_buf = [0u32; merge::MIXED_ROWS];
    for start in (0..length).rev() {
        step_paths.clear();
        let max_end = length.min(start + lexicon.longest);
        for end in (start + 1)..=max_end {
            let suffix_count = lattice.count_at(end);
            if suffix_count == 0 || (start == 0 && end == length) {
                continue;
            }
            let (lo, hi) = lexicon.range(&sounds_str[start..end]);
            // Full syllables mixed with initials at the end of the input (wobuzd).
            let mixed = if pairs.is_some() && !complete_only && end == length && start > 0 && reading.boundary_count[end] == reading.boundary_count[start] {
                merge::mixed_rows(lexicon, &sounds_str[start..end], &mut mixed_buf)
            } else {
                0
            };
            let english = latin::span(lexicon, reading.sounds_bytes(), start, end);
            // First-letter abbreviations only where the span is not a spelled word, or at the end of the input,
            // and never where an English word stands.
            let skip_short = english.is_some() || pairs.is_some()
                && lexicon.pinyin
                && end != length
                && (lo..hi).any(|i| lexicon.words.is_fully_spelled(i) && matches(i, start, end));
            let mut taken = [0usize; PAIR_SPAN_WORDS];
            let mut used = 0usize;
            for index in (lo..hi).chain(mixed_buf[..mixed].iter().map(|&r| r as usize)) {
                let is_mixed = index < lo || index >= hi;
                let word_spelled = lexicon.words.is_fully_spelled(index) && !is_mixed;
                if (complete_only || skip_short) && !word_spelled {
                    continue;
                }
                if !matches(index, start, end) {
                    continue;
                }
                let word_text = lexicon.words.text(&lexicon.storage, index, hans);
                if pairs.is_some() && taken[..used].iter().any(|&t| lexicon.words.text(&lexicon.storage, t, hans) == word_text) {
                    continue;
                }
                let score = f64::from(lexicon.words.weight(index)) / 700.0 - 19.0 - if is_mixed { merge::MIXED_PENALTY } else { 0.0 };
                let verified = verified_word(index);
                let traditional = lexicon.words.text(&lexicon.storage, index, false);
                let (hash, pow) = text_hash(word_text);
                let (id, start_term) = match pairs {
                    Some(p) => {
                        let id = lexicon.index.word_ids.ids.get(index);
                        (id, if start == 0 { f64::from(p.score(lattice.left, id)) / STEP } else { 0.0 })
                    }
                    None => (0, 0.0),
                };
                let mut totals = [0.0f64; LATTICE_STATES];
                for b_idx in 0..suffix_count {
                    let s_slot = PathLatticeSoA::slot_index(end, b_idx);
                    totals[b_idx] = match pairs {
                        Some(p) => {
                            let next = lattice.id[s_slot];
                            let pair = if next == OPEN_END { 0 } else { lattice.pair_cache.score(p, id, next) };
                            score + lattice.score[s_slot] + (f64::from(pair) / STEP + start_term)
                        }
                        None => score + lattice.score[s_slot] + lexicon.join_bonus(traditional, lattice.head(s_slot), false),
                    };
                }
                let floor = if pairs.is_some() && start > 0 { word_floor(&totals[..suffix_count]) } else { f64::NEG_INFINITY };
                for b_idx in 0..suffix_count {
                    let s_slot = PathLatticeSoA::slot_index(end, b_idx);
                    let total = totals[b_idx];
                    if total < floor {
                        continue;
                    }
                    step_paths.push(
                        lattice,
                        s_slot,
                        index as u32,
                        id,
                        hash,
                        pow,
                        word_text.len(),
                        total,
                        lattice.spelled[s_slot] && (!lexicon.pinyin || word_spelled),
                        lattice.verified[s_slot] && verified,
                    );
                }
                taken[used.min(PAIR_SPAN_WORDS - 1)] = index;
                used += 1;
                if used == span_words {
                    break;
                }
            }
            if let Some(rows) = english {
                latin::push_steps(lexicon, &reading, rows, start, end, hans, OPEN_END, lattice, step_paths);
            }
        }

        let mut ranked = 0usize;
        for k in 0..step_paths.order.len() {
            if k == ranked {
                ranked = rank_next(lexicon, lattice, hans, step_paths, k, states * PAIR_STATES_PER_WORD * 2);
            }
            let idx = step_paths.order[k];
            let base = PathLatticeSoA::slot_index(start, 0);
            let current = base..base + lattice.count_at(start);
            if current.clone().any(|s| lattice.hash[s] == step_paths.hash[idx] && lattice.len[s] == step_paths.len[idx]) {
                continue;
            }
            if pairs.is_some() && start > 0 && current.filter(|&s| lattice.id[s] == step_paths.id[idx]).count() == PAIR_STATES_PER_WORD {
                continue;
            }
            let traditional = lexicon.words.text(&lexicon.storage, step_paths.word[idx] as usize, false);
            lattice.push_step(start, step_paths, idx, traditional);
            if lattice.count_at(start) == states {
                break;
            }
        }
    }

    let top_count = lattice.count_at(0);
    for b_idx in 0..top_count {
        let slot = PathLatticeSoA::slot_index(0, b_idx);
        let kind = if !reading.unambiguous {
            CandidateKind::Unknown as u32
        } else if !lattice.spelled[slot] {
            CandidateKind::Correction as u32
        } else if lattice.verified[slot] {
            CandidateKind::Path as u32
        } else {
            CandidateKind::Unknown as u32
        };
        let mut text = std::mem::take(&mut lattice.text);
        text.clear();
        for s in lattice.words_from(slot) {
            text.push_str(lexicon.words.text(&lexicon.storage, lattice.word[s] as usize, hans));
        }
        if sort_from.is_some() {
            merge::push_best(out, &text, input.len(), kind, CandidateScoreKind::Path as u32, lattice.score[slot]);
        } else if !out.contains_text(&text) {
            out.push(&text, input.len(), kind, CandidateScoreKind::Path as u32, lattice.score[slot]);
        }
        lattice.text = text;
        if out.len() == CANDIDATE_LIMIT {
            break;
        }
    }
    if let Some(from) = sort_from {
        merge::sort_from(out, from);
    }
}

/// Lowest total worth a step for one word over one span: past the first
/// position at most `PAIR_STATES_PER_WORD` paths per word survive, so only
/// the word's best `PAIR_WORD_STEPS` steps are kept (twice that, for steps
/// lost to paths with the same text).
fn word_floor(totals: &[f64]) -> f64 {
    if totals.len() <= PAIR_WORD_STEPS {
        return f64::NEG_INFINITY;
    }
    let mut copy = [0.0f64; LATTICE_STATES];
    let copy = &mut copy[..totals.len()];
    copy.copy_from_slice(totals);
    *copy.select_nth_unstable_by(PAIR_WORD_STEPS - 1, |a, b| b.total_cmp(a)).1
}

/// Puts the best `chunk` entries of `step_paths.order[from..]` in order (score,
/// then text, then push order: the order a full stable sort would give) and
/// returns the end of the sorted part. The rest is only partitioned, so most
/// keys sort a small slice, and nothing is allocated.
fn rank_next(lexicon: &Lexicon, lattice: &PathLatticeSoA, hans: bool, step_paths: &mut StepPathsSoA, from: usize, chunk: usize) -> usize {
    let StepPathsSoA { score, word, next, order, .. } = step_paths;
    let text = |idx: usize| path_bytes(lexicon, lattice, hans, word[idx], next[idx] as usize);
    let cmp = |&a: &usize, &b: &usize| score[b].total_cmp(&score[a]).then_with(|| text(a).cmp(text(b))).then(a.cmp(&b));
    let rest = &mut order[from..];
    if rest.len() > chunk {
        rest.select_nth_unstable_by(chunk - 1, cmp);
        rest[..chunk].sort_unstable_by(cmp);
        from + chunk
    } else {
        rest.sort_unstable_by(cmp);
        order.len()
    }
}

/// Word id of the last word of `text`, picked for the keys `input`: the
/// longest lexicon word that spells the end of `input` and ends `text`.
/// `START` when there is no pair table or no such word.
pub fn last_word_id(lexicon: &Lexicon, input: &str, text: &str, hans: bool) -> u32 {
    if lexicon.pairs().is_none() {
        return START;
    }
    let reading = Reading::new(input, lexicon.pinyin);
    let length = reading.len();
    for start in length.saturating_sub(lexicon.longest)..length {
        let (lo, hi) = lexicon.range(&reading.sounds()[start..]);
        for index in lo..hi {
            let word = lexicon.words.text(&lexicon.storage, index, hans);
            if !word.is_empty() && text.ends_with(word) && reading.matches(lexicon.words.reading(&lexicon.storage, index).as_bytes(), start, length) {
                return lexicon.index.word_ids.ids.get(index);
            }
        }
    }
    START
}

/// Output text of word `word` followed by the lattice path in slot `next`, byte by byte.
fn path_bytes<'a>(lexicon: &'a Lexicon, lattice: &'a PathLatticeSoA, hans: bool, word: u32, next: usize) -> impl Iterator<Item = u8> + 'a {
    std::iter::once(word as usize)
        .chain(lattice.words_from(next).map(move |s| lattice.word[s] as usize))
        .flat_map(move |w| lexicon.words.text(&lexicon.storage, w, hans).bytes())
}
