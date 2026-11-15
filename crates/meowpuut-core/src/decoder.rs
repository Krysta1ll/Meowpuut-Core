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
use crate::abbrev;
use crate::latin;
use crate::lexicon::Lexicon;
use crate::merge;
use crate::pairs::{END, START, STEP};
use crate::reading::{Reading, BEAM, CANDIDATE_LIMIT};
use crate::workspace::{
    text_hash, DecodeOutputSoA, DecoderScratchSoA, PathLatticeSoA, StepPathsSoA, LATTICE_STATES,
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

    // Zhuyin words typed with first symbols (ㄋㄏ, ㄨㄛˇㄒㄏ) after the words
    // typed in full; with a pair table they are sorted with the paths below.
    let mut whole_spans = abbrev::ZhuyinSpans::default();
    let mut zhuyin_spans = abbrev::ZhuyinSpans::default();
    let mut abbrev_budget = abbrev::SCAN_BUDGET;
    let zhuyin_abbreviated = !complete_only && !open_end && abbrev::abbreviated(lexicon, &reading);
    if zhuyin_abbreviated {
        abbrev::zhuyin_from(lexicon, &reading, 0, &mut abbrev_budget, &mut whole_spans);
        if !ranked {
            merge::push_abbreviated_whole(lexicon, whole_spans.words(length), input.len(), hans, false, out);
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
        if zhuyin_abbreviated {
            merge::push_abbreviated_whole(lexicon, whole_spans.words(length), input.len(), hans, true, out);
        }
    }

    // One-edit guesses for the whole input, listed before the paths without
    // a pair table (with one they are sorted with them by `guess_score`).
    let mut guesses = 0..0;
    if out.is_empty() && lexicon.pinyin && !complete_only {
        guesses.start = out.len();
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
        guesses.end = out.len();
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

    // An English word that ends the input (bug, zhegebug, html) keeps its
    // letters: a bare initial in it is not read as a character (不乾 for bug).
    // Only a word that starts the input or starts with a consonant counts;
    // the id of nid is the end of ni.
    let latin_tail = if lexicon.pinyin && !complete_only {
        let sounds = reading.sounds_bytes();
        (length.saturating_sub(latin::LONGEST)..length)
            .find(|&s| (s == 0 || !b"aeiouv".contains(&sounds[s])) && latin::span(lexicon, sounds, s, length).is_some())
    } else {
        None
    };
    let mut mixed_buf = [0u32; merge::MIXED_ROWS];
    for start in (0..length).rev() {
        step_paths.clear();
        if zhuyin_abbreviated {
            if start == 0 {
                zhuyin_spans = whole_spans;
            } else {
                abbrev::zhuyin_from(lexicon, &reading, start, &mut abbrev_budget, &mut zhuyin_spans);
            }
        }
        let max_end = length.min(start + lexicon.longest);
        for end in (start + 1)..=max_end {
            let suffix_count = lattice.count_at(end);
            if suffix_count == 0 || (start == 0 && end == length) {
                continue;
            }
            let (lo, hi) = lexicon.range(&sounds_str[start..end]);
            // Words typed with initials that no lexicon row spells: Zhuyin first
            // symbols (ㄋㄏ), a bare Pinyin initial at the end of the input (the d
            // of wod) or among other initials (the q of jtqkdy; after a syllable a
            // lone consonant is more often English, bug or mail), and full
            // syllables mixed with initials at the end of the input (wobuzd).
            let mixed = if complete_only {
                0
            } else if !lexicon.pinyin {
                let rows = zhuyin_spans.rows(end);
                mixed_buf[..rows.len()].copy_from_slice(rows);
                rows.len()
            } else if abbrev::pinyin_initial(reading.sounds_bytes(), start, end)
                && (end == length || abbrev::after_initial(reading.sounds_bytes(), start))
                && latin_tail.is_none_or(|s| s > start)
            {
                abbrev::pinyin_initial_rows(lexicon, &reading.sounds_bytes()[start..end], reading.boundaries[end], &mut mixed_buf)
            } else if pairs.is_some() && end == length && start > 0 && reading.boundary_count[end] == reading.boundary_count[start] {
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
            // Where the span can only be initials (no spelled word reads it, and
            // it is not the end of a syllable before it), an abbreviation row is
            // scored as its word less the mixed penalty (merge::abbreviation_score).
            let abbreviation_only = lexicon.pinyin
                && lo < hi
                && abbrev::pinyin_initials(reading.sounds_bytes(), start, end)
                && !(lo..hi).any(|i| lexicon.words.is_fully_spelled(i) && matches(i, start, end));
            let mut taken = [0usize; PAIR_SPAN_WORDS];
            let mut used = 0usize;
            for index in (lo..hi).chain(mixed_buf[..mixed].iter().map(|&r| r as usize)) {
                if lattice.skip_no_suggest && lexicon.words.is_no_suggest(index) {
                    continue;
                }
                let is_mixed = index < lo || index >= hi;
                let word_spelled = lexicon.words.is_fully_spelled(index) && !is_mixed;
                if (complete_only || skip_short) && !word_spelled {
                    continue;
                }
                // Abbreviated rows were matched syllable by syllable, tones included.
                if !is_mixed && !matches(index, start, end) {
                    continue;
                }
                let word_text = lexicon.words.text(&lexicon.storage, index, hans);
                if pairs.is_some() && taken[..used].iter().any(|&t| lexicon.words.text(&lexicon.storage, t, hans) == word_text) {
                    continue;
                }
                let score = if abbreviation_only && !is_mixed {
                    merge::abbreviation_score(lexicon.words.weight(index))
                } else {
                    f64::from(lexicon.words.weight(index)) / 700.0 - 19.0 - if is_mixed { merge::MIXED_PENALTY } else { 0.0 }
                };
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
                        lattice.spelled[s_slot] && (word_spelled || !lexicon.pinyin && !is_mixed),
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
    } else if !guesses.is_empty() {
        // Without a pair table a spelled path that reads better than the best
        // guess, scored as the pair table would score it, comes before the
        // guesses (woxiawu: 我下午, not 我下去 for woxiaqu), unless that guess
        // only goes on from the letters typed (doujian: 豆浆 being typed).
        let best_guess = merge::guess_score(out.score[guesses.start]);
        let completion = completes(lexicon, reading.sounds(), out.text(guesses.start), hans);
        if let Some(i) = (guesses.end..out.len()).find(|&i| !completion && out.kind[i] == CandidateKind::Path as u32) {
            if out.score[i] > best_guess {
                let text = out.text(i).to_owned();
                let (kind, score) = (out.kind[i], out.score[i]);
                out.remove_at(i);
                out.insert_at(guesses.start, &text, input.len(), kind, CandidateScoreKind::Path as u32, score);
            }
        }
    }
    if lattice.skip_no_suggest {
        let mut i = 0;
        while i < out.len() {
            if lexicon.is_no_suggest_text(out.text(i)) {
                out.remove_at(i);
            } else {
                i += 1;
            }
        }
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

/// Whether a lexicon word whose bare reading goes on from `sounds` reads `text`.
fn completes(lexicon: &Lexicon, sounds: &str, text: &str, hans: bool) -> bool {
    (lexicon.lower_bound_bucket(sounds)..lexicon.buckets.len())
        .take(64)
        .take_while(|&bucket| lexicon.buckets.key(&lexicon.storage, bucket).starts_with(sounds))
        .any(|bucket| {
            let (lo, hi) = lexicon.buckets.range(bucket);
            (lo..hi).any(|i| lexicon.words.text(&lexicon.storage, i, hans) == text)
        })
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
