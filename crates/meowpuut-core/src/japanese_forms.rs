#![forbid(unsafe_code)]
//! Inflected forms and function-word tails of `JapaneseQuery::lookup_extended`
//! (`OPTION_FORMS`).
//!
//! The image holds dictionary forms only (食べる, 大きい, 勉強). A form of the input is found by
//! undoing it, as in Yomichan's de-inflector: table-driven suffix rules, applied backwards from
//! the kana key of the input, rewrite an inflected ending into the ending it comes from and say
//! what the result must be. Following the bunsetsu model (one independent word, then function
//! words), up to two particles or copula forms are first taken off the end, then up to three
//! inflection steps are undone. Every dictionary form met on the way is looked up; a word whose
//! class fits gives a form whose surface is the word without its dictionary ending plus the rest
//! of the input kana (食べ|る → 食べ + させられました). Stem rules (書き → 書く) do not count as
//! steps. Nothing is stored in the image for this: the class byte of a word decides.
//!
//! Classes are the class byte of a word record, an index into `CLASSES` of
//! `scripts/mimo_jp_binary.py`. A verb of class `v?` is read by its last kana; a final る after an
//! い- or え-row kana may be 一段 or 五段.

use super::{
    hit_text, rank, Hit, JapaneseLexicon, JapaneseMode, JapaneseQuery, JapaneseRow, Text, FLAG_FORM, FLAG_PINYIN_OK, FLAG_REVIEWED,
    KIND_READING, KIND_VARIANT, MAX_POSTINGS_PER_KEY,
};

/// Key searches for the forms of one input, at most.
pub(super) const MAX_SEARCHES: usize = 256;
/// Key searches for the forms of all the leading parts tried for segments, at most.
pub(super) const MAX_SEGMENT_SEARCHES: usize = 512;
/// Inflection steps undone, at most.
const MAX_STEPS: u8 = 3;
/// Strings derived from one input, at most.
const MAX_NODES: usize = 512;
/// Generated forms kept for one input, at most.
const MAX_FORM_HITS: usize = 64;

/// Weight deductions.
const STEP: i32 = 300;
/// Imperative: rare in typing, and its endings look like many plain words (来い and 恋, 読め and
/// 嫁).
const IMPERATIVE: i32 = 2500;
/// Potential of a 五段 verb, which is often a dictionary word of its own.
const POTENTIAL: i32 = 600;
/// Each particle or copula form after the word.
const TAIL: i32 = 500;
/// Added once to a word followed by function words when the whole input is itself a word of
/// the list: うんと stays ahead of 運と and バラスト of バラすと, while 今日は still passes 教派.
const BESIDE_WORD: i32 = 2500;
/// A noun used as a する verb: the image does not say which nouns take する.
const SURU_NOUN: i32 = 1200;
/// な after a word of class `n` (元気な), which the image does not mark as a na-adjective.
const NA_NOUN: i32 = 300;
/// A one-kana word before a function word: such splits are mostly accidents (胃って).
const SHORT_BASE: i32 = 800;
/// A noun of one or two kana before する: most such nouns do not take it (花して).
const SHORT_SURU_NOUN: i32 = 600;

// What a string met in the search may be. A rule applies to a string whose types meet its
// `when`, and gives a string of type `gives`.
/// 一段 dictionary form.
const V1: u32 = 1;
/// 五段 dictionary form; the row is the last kana.
const V5: u32 = 1 << 1;
/// 五段 dictionary form other than 行く (いて → く).
const V5X: u32 = 1 << 2;
/// 行く (って → く).
const IKU: u32 = 1 << 3;
/// 来る.
const VK: u32 = 1 << 4;
/// する or ずる.
const VS: u32 = 1 << 5;
/// i-adjective, or a form that inflects like one (食べない, 食べたい).
const ADJ: u32 = 1 << 6;
/// Noun-like word before a particle or a copula form.
const NOUN: u32 = 1 << 7;
/// Na-adjective before な.
const NA: u32 = 1 << 8;
/// Noun before する (勉強します).
const SURU: u32 = 1 << 9;
/// 連用形: the stem before ます, たい, ながら.
const STEM_I: u32 = 1 << 10;
/// 未然形 before ない.
const STEM_A: u32 = 1 << 11;
/// 未然形 before れる and せる (する gives さ here, し before ない).
const STEM_AR: u32 = 1 << 12;
/// 仮定形 before ば.
const STEM_E: u32 = 1 << 13;
/// て form.
const TE: u32 = 1 << 14;
/// Finished verb form: polite, past, volitional, imperative.
const FIN: u32 = 1 << 15;
/// Past of an adjective.
const APAST: u32 = 1 << 16;
/// A form that takes nothing after it (たら, ば, ながら).
const ALONE: u32 = 1 << 17;
/// A noun + する reached through できる (勉強できる); する itself is not looked up for it, since
/// できる is a word of its own.
const VS_NOUN: u32 = 1 << 18;
/// Types of strings; the bits above are categories of function words.
const FORM_TYPES: u32 = (1 << 20) - 1;

// Categories of function words, for which may follow which.
const CASE: u32 = 1 << 20;
const TOPIC: u32 = 1 << 21;
const NO: u32 = 1 << 22;
const ADVERBIAL: u32 = 1 << 23;
const COPULA: u32 = 1 << 24;
const ATTRIBUTIVE: u32 = 1 << 25;
const FINAL: u32 = 1 << 26;

/// Finished predicates: what a sentence-final particle may follow.
const PREDICATE: u32 = V1 | V5 | VK | VS | ADJ | FIN | APAST;
/// Types of the whole input: anything but a bare stem.
const WHOLE: u32 = V1 | V5 | VK | VS | ADJ | NOUN | NA | TE | FIN | APAST | ALONE;
/// Types looked up in the image, in the order they are tried on a word.
const WORD_TYPES: [u32; 10] = [V1, V5, V5X, IKU, VK, VS, ADJ, NOUN, NA, SURU];
/// The same as one bit set.
const LOOKED_UP: u32 = V1 | V5 | V5X | IKU | VK | VS | ADJ | NOUN | NA | SURU;

// Class bytes of word records (`CLASSES` of scripts/mimo_jp_binary.py).
const CLASS_N: u8 = 0;
const CLASS_PN: u8 = 1;
const CLASS_V1: u8 = 2;
const CLASS_V5K: u8 = 3;
const CLASS_V5G: u8 = 4;
const CLASS_V5S: u8 = 5;
const CLASS_V5T: u8 = 6;
const CLASS_V5N: u8 = 7;
const CLASS_V5B: u8 = 8;
const CLASS_V5M: u8 = 9;
const CLASS_V5R: u8 = 10;
const CLASS_V5W: u8 = 11;
const CLASS_VS: u8 = 12;
const CLASS_VK: u8 = 13;
const CLASS_VERB: u8 = 14;
const CLASS_ADJ_I: u8 = 15;
const CLASS_ADJ_NA: u8 = 16;
const CLASS_ADV: u8 = 17;
const CLASS_PLACE: u8 = 21;
const CLASS_ADJ: u8 = 22;

/// A suffix rewrite, read backwards: a string of a type in `when` that ends with `from` comes
/// from the string that ends with `to` instead, which has type `gives`.
struct Rule {
    from: &'static str,
    to: &'static str,
    when: u32,
    gives: u32,
    /// Weight deduction; 0 for a stem rule, which is not an inflection step.
    cost: i32,
}

const fn stem(from: &'static str, to: &'static str, when: u32, gives: u32) -> Rule {
    Rule { from, to, when, gives, cost: 0 }
}

const fn step(from: &'static str, to: &'static str, when: u32, gives: u32, cost: i32) -> Rule {
    Rule { from, to, when, gives, cost }
}

const A_STEMS: u32 = STEM_A | STEM_AR;

const RULES: &[Rule] = &[
    // 連用形: 食べ, 書き, 来(き), し.
    stem("", "る", STEM_I, V1),
    stem("き", "く", STEM_I, V5),
    stem("ぎ", "ぐ", STEM_I, V5),
    stem("し", "す", STEM_I, V5),
    stem("ち", "つ", STEM_I, V5),
    stem("に", "ぬ", STEM_I, V5),
    stem("び", "ぶ", STEM_I, V5),
    stem("み", "む", STEM_I, V5),
    stem("り", "る", STEM_I, V5),
    stem("い", "う", STEM_I, V5),
    stem("き", "くる", STEM_I, VK),
    stem("し", "する", STEM_I, VS),
    stem("じ", "ずる", STEM_I, VS),
    // 未然形: 食べ, 書か (う → わ), 来(こ); する gives し before ない, さ before れる and せる.
    stem("", "る", A_STEMS, V1),
    stem("か", "く", A_STEMS, V5),
    stem("が", "ぐ", A_STEMS, V5),
    stem("さ", "す", A_STEMS, V5),
    stem("た", "つ", A_STEMS, V5),
    stem("な", "ぬ", A_STEMS, V5),
    stem("ば", "ぶ", A_STEMS, V5),
    stem("ま", "む", A_STEMS, V5),
    stem("ら", "る", A_STEMS, V5),
    stem("わ", "う", A_STEMS, V5),
    stem("こ", "くる", A_STEMS, VK),
    stem("し", "する", STEM_A, VS),
    stem("じ", "ずる", STEM_A, VS),
    stem("さ", "する", STEM_AR, VS),
    stem("ざ", "ずる", STEM_AR, VS),
    // 仮定形 before ば: 食べれ, 書け, 来(くれ), すれ.
    stem("れ", "る", STEM_E, V1),
    stem("け", "く", STEM_E, V5),
    stem("げ", "ぐ", STEM_E, V5),
    stem("せ", "す", STEM_E, V5),
    stem("て", "つ", STEM_E, V5),
    stem("ね", "ぬ", STEM_E, V5),
    stem("べ", "ぶ", STEM_E, V5),
    stem("め", "む", STEM_E, V5),
    stem("れ", "る", STEM_E, V5),
    stem("え", "う", STEM_E, V5),
    stem("くれ", "くる", STEM_E, VK),
    stem("すれ", "する", STEM_E, VS),
    stem("ずれ", "ずる", STEM_E, VS),
    // て form with the 音便 of 五段 verbs; 行く has 行って, never 行いて.
    stem("て", "る", TE, V1),
    stem("いて", "く", TE, V5X),
    stem("いで", "ぐ", TE, V5),
    stem("して", "す", TE, V5),
    stem("って", "つ", TE, V5),
    stem("って", "る", TE, V5),
    stem("って", "う", TE, V5),
    stem("って", "く", TE, IKU),
    stem("んで", "ぬ", TE, V5),
    stem("んで", "ぶ", TE, V5),
    stem("んで", "む", TE, V5),
    stem("きて", "くる", TE, VK),
    stem("して", "する", TE, VS),
    stem("じて", "ずる", TE, VS),
    // Polite forms on the 連用形.
    step("ます", "", FIN, STEM_I, STEP),
    step("ました", "", FIN, STEM_I, STEP),
    step("ません", "", FIN, STEM_I, STEP),
    step("ませんでした", "", FIN, STEM_I, STEP),
    step("ましょう", "", FIN, STEM_I, STEP),
    step("まして", "", TE, STEM_I, STEP),
    // Past, conditional and listing forms are built on the て form.
    step("た", "て", FIN, TE, STEP),
    step("だ", "で", FIN, TE, STEP),
    step("たら", "て", ALONE, TE, STEP),
    step("だら", "で", ALONE, TE, STEP),
    step("たり", "て", ALONE, TE, STEP),
    step("だり", "で", ALONE, TE, STEP),
    // Verbs after the て form.
    step("ている", "て", V1, TE, STEP),
    step("でいる", "で", V1, TE, STEP),
    step("てる", "て", V1, TE, STEP),
    step("でる", "で", V1, TE, STEP),
    step("てみる", "て", V1, TE, STEP),
    step("でみる", "で", V1, TE, STEP),
    step("てしまう", "て", V5, TE, STEP),
    step("でしまう", "で", V5, TE, STEP),
    step("ちゃう", "て", V5, TE, STEP),
    step("じゃう", "で", V5, TE, STEP),
    step("ておく", "て", V5, TE, STEP),
    step("でおく", "で", V5, TE, STEP),
    step("ください", "", FIN, TE, STEP),
    // Negative and wishes inflect as i-adjectives.
    step("ない", "", ADJ, STEM_A, STEP),
    step("ないで", "", TE, STEM_A, STEP),
    step("たい", "", ADJ, STEM_I, STEP),
    step("やすい", "", ADJ, STEM_I, STEP),
    step("にくい", "", ADJ, STEM_I, STEP),
    // Passive, causative and すぎる inflect as 一段 verbs.
    step("られる", "", V1, STEM_AR, STEP),
    step("れる", "", V1, STEM_AR, STEP),
    step("させる", "", V1, STEM_AR, STEP),
    step("せる", "", V1, STEM_AR, STEP),
    step("すぎる", "", V1, STEM_I, STEP),
    // Potential of a noun + する.
    step("できる", "する", V1, VS_NOUN, POTENTIAL),
    // Potential of a 五段 verb: an 一段 verb on the え row.
    step("ける", "く", V1, V5, POTENTIAL),
    step("げる", "ぐ", V1, V5, POTENTIAL),
    step("せる", "す", V1, V5, POTENTIAL),
    step("てる", "つ", V1, V5, POTENTIAL),
    step("ねる", "ぬ", V1, V5, POTENTIAL),
    step("べる", "ぶ", V1, V5, POTENTIAL),
    step("める", "む", V1, V5, POTENTIAL),
    step("れる", "る", V1, V5, POTENTIAL),
    step("える", "う", V1, V5, POTENTIAL),
    // Other forms on a stem.
    step("ながら", "", ALONE, STEM_I, STEP),
    step("なさい", "", FIN, STEM_I, STEP),
    step("そう", "", NOUN | NA, STEM_I, STEP),
    step("ば", "", ALONE, STEM_E, STEP),
    // Volitional.
    step("よう", "る", FIN, V1, STEP),
    step("こう", "く", FIN, V5, STEP),
    step("ごう", "ぐ", FIN, V5, STEP),
    step("そう", "す", FIN, V5, STEP),
    step("とう", "つ", FIN, V5, STEP),
    step("のう", "ぬ", FIN, V5, STEP),
    step("ぼう", "ぶ", FIN, V5, STEP),
    step("もう", "む", FIN, V5, STEP),
    step("ろう", "る", FIN, V5, STEP),
    step("おう", "う", FIN, V5, STEP),
    step("こよう", "くる", FIN, VK, STEP),
    step("しよう", "する", FIN, VS, STEP),
    step("じよう", "ずる", FIN, VS, STEP),
    // Imperative.
    step("ろ", "る", FIN, V1, IMPERATIVE),
    step("け", "く", FIN, V5, IMPERATIVE),
    step("げ", "ぐ", FIN, V5, IMPERATIVE),
    step("せ", "す", FIN, V5, IMPERATIVE),
    step("て", "つ", FIN, V5, IMPERATIVE),
    step("ね", "ぬ", FIN, V5, IMPERATIVE),
    step("べ", "ぶ", FIN, V5, IMPERATIVE),
    step("め", "む", FIN, V5, IMPERATIVE),
    step("れ", "る", FIN, V5, IMPERATIVE),
    step("え", "う", FIN, V5, IMPERATIVE),
    step("こい", "くる", FIN, VK, IMPERATIVE),
    step("しろ", "する", FIN, VS, IMPERATIVE),
    // i-adjectives (いい inflects through よい, a word of its own).
    step("く", "い", ALONE, ADJ, STEP),
    step("くて", "い", TE, ADJ, STEP),
    step("かった", "い", APAST, ADJ, STEP),
    step("くない", "い", ADJ, ADJ, STEP),
    step("ければ", "い", ALONE, ADJ, STEP),
    step("かったら", "い", ALONE, ADJ, STEP),
    step("さ", "い", NOUN, ADJ, STEP),
    step("そう", "い", NOUN | NA, ADJ, STEP),
    step("すぎる", "い", V1, ADJ, STEP),
];

/// A function word that may end the input: its kana, its category, and the types and
/// categories it may follow.
struct Tail {
    kana: &'static str,
    is: u32,
    after: u32,
}

const fn tail(kana: &'static str, is: u32, after: u32) -> Tail {
    Tail { kana, is, after }
}

const TAILS: &[Tail] = &[
    // Case particles.
    tail("が", CASE, NOUN | NO | ADVERBIAL | PREDICATE | COPULA),
    tail("を", CASE, NOUN | NO | ADVERBIAL),
    tail("に", CASE, NOUN | NO | ADVERBIAL),
    tail("で", CASE, NOUN | NO | ADVERBIAL),
    tail("と", CASE, NOUN | PREDICATE | COPULA),
    tail("へ", CASE, NOUN),
    tail("から", CASE, NOUN | NO | PREDICATE | TE | COPULA),
    tail("まで", CASE, NOUN | NO | PREDICATE),
    tail("より", CASE, NOUN | NO | PREDICATE),
    // Topic and focus.
    tail("は", TOPIC, NOUN | NO | ADVERBIAL | CASE | TE),
    tail("も", TOPIC, NOUN | NO | ADVERBIAL | CASE | TE),
    // Nominaliser, listing, limits.
    tail("の", NO, NOUN | PREDICATE | CASE | ADVERBIAL),
    tail("や", FINAL, NOUN),
    tail("だけ", ADVERBIAL, NOUN | PREDICATE),
    tail("など", ADVERBIAL, NOUN | PREDICATE),
    // Sentence end and clause links.
    tail("か", FINAL, NOUN | PREDICATE | COPULA | CASE),
    tail("ね", FINAL, NOUN | PREDICATE | COPULA | TE | CASE | TOPIC),
    tail("よ", FINAL, NOUN | PREDICATE | COPULA | TE),
    tail("な", FINAL | ATTRIBUTIVE, NA | PREDICATE),
    tail("よね", FINAL, NOUN | PREDICATE | COPULA | TE),
    tail("かな", FINAL, NOUN | PREDICATE),
    tail("って", FINAL, NOUN | PREDICATE | COPULA),
    tail("けど", FINAL, PREDICATE | COPULA),
    tail("ので", FINAL, PREDICATE | ATTRIBUTIVE),
    tail("のに", FINAL, PREDICATE | ATTRIBUTIVE),
    tail("し", FINAL, PREDICATE | COPULA),
    // Copula.
    tail("だ", COPULA, NOUN),
    tail("です", COPULA, NOUN | ADJ | APAST),
    tail("でした", COPULA, NOUN),
    tail("だった", COPULA, NOUN),
    tail("じゃない", COPULA, NOUN),
    tail("ではない", COPULA, NOUN),
    tail("じゃなかった", COPULA, NOUN),
    tail("ではなかった", COPULA, NOUN),
    tail("じゃありません", COPULA, NOUN),
    tail("ではありません", COPULA, NOUN),
    tail("でしょう", COPULA, NOUN | PREDICATE),
    tail("だろう", COPULA, NOUN | PREDICATE),
    tail("なら", COPULA, NOUN | PREDICATE),
    tail("なんです", COPULA, NOUN),
    tail("なんだ", COPULA, NOUN),
    tail("んです", COPULA, PREDICATE),
    tail("んだ", COPULA, PREDICATE),
    tail("のです", COPULA, PREDICATE),
];

/// A string met in the search: `arena[start..start + len]`.
#[derive(Clone, Copy)]
struct Node {
    start: usize,
    len: usize,
    types: u32,
    steps: u8,
    tails: u8,
    cost: i32,
}

/// A key search already made for a string of the arena.
#[derive(Clone, Copy)]
struct Searched {
    hash: u32,
    start: usize,
    len: usize,
    key: Option<usize>,
}

/// Buffers of the search, kept by the query between lookups.
pub(super) struct FormScratch {
    /// The input key first, then every derived string, back to back.
    arena: Vec<u8>,
    nodes: Vec<Node>,
    searched: Vec<Searched>,
}

impl FormScratch {
    pub(super) fn new() -> Self {
        Self {
            arena: Vec::with_capacity(16 * 1024),
            nodes: Vec::with_capacity(64),
            searched: Vec::with_capacity(MAX_SEARCHES),
        }
    }
}

impl JapaneseQuery {
    /// Adds behind `hits[group..]` the forms whose kana is the normalised key in `key`, each
    /// converting `consumed` input bytes. Makes at most `budget` key searches and returns how
    /// many it made.
    pub(super) fn add_forms(
        &mut self,
        lexicon: &JapaneseLexicon,
        mode: JapaneseMode,
        group: usize,
        consumed: usize,
        budget: usize,
    ) -> usize {
        let JapaneseQuery { forms, hits, built, key, .. } = self;
        let FormScratch { arena, nodes, searched } = forms;
        arena.clear();
        nodes.clear();
        searched.clear();
        arena.extend_from_slice(key.as_bytes());
        let beside = if hits.len() > group { BESIDE_WORD } else { 0 };
        let mut search = Search {
            lexicon,
            pinyin: mode == JapaneseMode::Pinyin,
            arena,
            nodes,
            searched,
            hits,
            built,
            group,
            consumed,
            budget,
            used: 0,
            pushed: 0,
            whole: key.len(),
            beside,
        };
        search.run();
        search.used
    }
}

struct Search<'a> {
    lexicon: &'a JapaneseLexicon,
    pinyin: bool,
    arena: &'a mut Vec<u8>,
    nodes: &'a mut Vec<Node>,
    searched: &'a mut Vec<Searched>,
    hits: &'a mut Vec<Hit>,
    built: &'a mut String,
    group: usize,
    consumed: usize,
    budget: usize,
    used: usize,
    pushed: usize,
    /// Length of the input key at the start of the arena.
    whole: usize,
    /// `BESIDE_WORD` when the whole input already has a word of the list, else 0.
    beside: i32,
}

impl Search<'_> {
    fn run(&mut self) {
        if self.whole == 0 {
            return;
        }
        // The key without one or two function words; in pinyin mode none is taken off.
        if !self.pinyin {
            for outer in TAILS {
                let Some(rest) = self.strip(self.whole, outer.kana) else { continue };
                let types = outer.after & FORM_TYPES;
                self.push(Node { start: 0, len: rest, types, steps: 0, tails: 1, cost: TAIL + self.beside });
                for inner in TAILS.iter().filter(|inner| outer.after & inner.is != 0) {
                    let Some(base) = self.strip(rest, inner.kana) else { continue };
                    let types = inner.after & FORM_TYPES;
                    self.push(Node { start: 0, len: base, types, steps: 0, tails: 2, cost: 2 * TAIL + self.beside });
                }
            }
        }
        // The whole key goes last onto the stack, so that it is searched first.
        self.push(Node { start: 0, len: self.whole, types: WHOLE, steps: 0, tails: 0, cost: 0 });
        while let Some(node) = self.nodes.pop() {
            if self.used >= self.budget {
                break;
            }
            // The whole key as it is was the exact lookup; every other string starts later in
            // the arena or is shorter.
            if node.start != 0 || node.len != self.whole {
                self.words(node.start, node.len, node.types, node.cost);
            }
            if !self.pinyin && node.types & (VS | VS_NOUN) != 0 && node.len > "する".len() {
                let text = self.arena.get(node.start..node.start + node.len);
                if text.is_some_and(|text| text.ends_with("する".as_bytes())) {
                    self.words(node.start, node.len - "する".len(), SURU, node.cost + SURU_NOUN);
                }
            }
            self.expand(node);
        }
    }

    /// Length of `arena[..len]` without `ending`, when it ends so and something is left.
    fn strip(&self, len: usize, ending: &str) -> Option<usize> {
        let text = self.arena.get(..len)?;
        (text.len() > ending.len() && text.ends_with(ending.as_bytes())).then(|| len - ending.len())
    }

    /// Pushes a node. The roots are few (a key ends with at most a handful of function words);
    /// `expand` keeps the total under `MAX_NODES`.
    fn push(&mut self, node: Node) {
        self.pushed += 1;
        self.nodes.push(node);
    }

    /// Pushes the strings that the rules give for `node`.
    fn expand(&mut self, node: Node) {
        let end = node.start + node.len;
        for rule in RULES {
            if rule.when & node.types == 0 || (rule.cost > 0 && node.steps >= MAX_STEPS) {
                continue;
            }
            // Pinyin mode keeps to the common forms: no imperative, no potential.
            if self.pinyin && rule.cost > STEP {
                continue;
            }
            if self.pushed >= MAX_NODES {
                return;
            }
            let Some(text) = self.arena.get(node.start..end) else { return };
            if !text.ends_with(rule.from.as_bytes()) {
                continue;
            }
            let stem_end = end - rule.from.len();
            if stem_end == node.start && rule.to.is_empty() {
                continue;
            }
            // A stem rule on the input itself undoes a て form that was typed as such: that is an
            // inflection, though not one more step of depth.
            let cost = if rule.cost == 0 && node.start == 0 { STEP } else { rule.cost };
            let start = self.arena.len();
            self.arena.extend_from_within(node.start..stem_end);
            self.arena.extend_from_slice(rule.to.as_bytes());
            self.push(Node {
                start,
                len: self.arena.len() - start,
                types: rule.gives,
                steps: node.steps + u8::from(rule.cost > 0),
                tails: node.tails,
                cost: node.cost + cost,
            });
        }
    }

    /// Key index of `arena[start..start + len]`; each string is searched once.
    fn search(&mut self, start: usize, len: usize) -> Option<usize> {
        let bytes = self.arena.get(start..start.checked_add(len)?)?;
        let hash = bytes.iter().fold(0x811C_9DC5u32, |hash, &byte| (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193));
        for seen in self.searched.iter() {
            if seen.hash == hash && self.arena.get(seen.start..seen.start + seen.len) == Some(bytes) {
                return seen.key;
            }
        }
        if self.used >= self.budget {
            return None;
        }
        self.used += 1;
        let key = self.lexicon.find_key(bytes);
        self.searched.push(Searched { hash, start, len, key });
        key
    }

    /// Looks up `arena[start..start + len]` and adds a form for each posting whose word fits one
    /// of `types`, with weight less `cost`.
    fn words(&mut self, start: usize, len: usize, types: u32, cost: i32) {
        if types & LOOKED_UP == 0 {
            return;
        }
        let Some(index) = self.search(start, len) else { return };
        // A string that a rule wrote (not the input less its function words) is inflected.
        let inflected = start != 0;
        let Search { lexicon, pinyin, arena, hits, built, group, consumed, whole, .. } = self;
        let lexicon: &JapaneseLexicon = lexicon;
        let reading = arena.get(start..start + len).and_then(|bytes| std::str::from_utf8(bytes).ok());
        let input = arena.get(..*whole).and_then(|bytes| std::str::from_utf8(bytes).ok());
        let (Some(reading), Some(input), Some(record)) = (reading, input, lexicon.key_record(index)) else { return };
        for slot in 0..record.posting_count.min(MAX_POSTINGS_PER_KEY) {
            let Some(posting) = record.first_posting.checked_add(slot).and_then(|at| lexicon.posting(at)) else {
                return;
            };
            if (posting.kind != KIND_READING && posting.kind != KIND_VARIANT)
                // A reviewed word is typed in pinyin through its own key (taberu), which carries the
                // permission; its kana posting only carries the review.
                || (*pinyin && posting.flags & (FLAG_PINYIN_OK | FLAG_REVIEWED) == 0)
            {
                continue;
            }
            let Some(word) = lexicon.word_record(posting.word) else { continue };
            let Some(surface) = lexicon.blob_str(word.surface_offset, word.surface_len) else { continue };
            let fitting = |bit: u32| types & bit != 0 && fits(bit, word.class, reading, inflected);
            let Some(bit) = WORD_TYPES.into_iter().find(|&bit| fitting(bit)) else { continue };
            let mut deduction = cost;
            let kana = reading.chars().count();
            if bit & (NOUN | NA | SURU) != 0 && kana < 2 {
                deduction += SHORT_BASE;
            }
            if bit == SURU && kana < 3 {
                deduction += SHORT_SURU_NOUN;
            }
            if bit == NA && word.class != CLASS_ADJ_NA {
                deduction += NA_NOUN;
            }
            let row = JapaneseRow {
                word: posting.word,
                weight: (i32::from(posting.weight) - deduction).max(1),
                kind: posting.kind,
                flags: FLAG_FORM | (posting.flags & FLAG_PINYIN_OK),
            };
            let start = built.len();
            if build_surface(built, surface, reading, input, bit == VK) {
                let text = Text::Built { start, len: built.len() - start };
                keep_form(hits, built, lexicon, *group, Hit { row, text, consumed: *consumed }, start);
            }
        }
    }
}

/// Whether a word of `class` read `reading` is a dictionary form of type `bit`. `inflected`: the
/// reading was reached through a rule; いい then does not fit, since it inflects through よい
/// (よかった, never いかった).
fn fits(bit: u32, class: u8, reading: &str, inflected: bool) -> bool {
    let verb = class == CLASS_VERB;
    let iku = reading.ends_with("いく") || reading.ends_with("ゆく");
    match bit {
        V1 => {
            let mut chars = reading.chars().rev();
            let (last, before) = (chars.next(), chars.next());
            // A v? verb is 一段 only when an い- or え-row kana stands before る.
            let i_or_e_row = before.is_some_and(|ch| "いきしちにひみりぎじぢびぴえけせてねへめれげぜでべぺ".contains(ch));
            last == Some('る') && (class == CLASS_V1 || verb && i_or_e_row)
        }
        V5 | V5X => {
            let row = match reading.chars().next_back() {
                Some('く') => CLASS_V5K,
                Some('ぐ') => CLASS_V5G,
                Some('す') => CLASS_V5S,
                Some('つ') => CLASS_V5T,
                Some('ぬ') => CLASS_V5N,
                Some('ぶ') => CLASS_V5B,
                Some('む') => CLASS_V5M,
                Some('る') => CLASS_V5R,
                Some('う') => CLASS_V5W,
                _ => return false,
            };
            let irregular = reading.ends_with("する") || reading.ends_with("ずる") || reading.ends_with("くる");
            (class == row || verb && !irregular) && !(bit == V5X && iku)
        }
        IKU => iku && (class == CLASS_V5K || verb),
        VK => reading.ends_with("くる") && (class == CLASS_VK || verb),
        VS => (reading.ends_with("する") || reading.ends_with("ずる")) && (class == CLASS_VS || verb),
        ADJ => {
            let adjective = class == CLASS_ADJ_I || class == CLASS_ADJ;
            adjective && reading.ends_with('い') && !(inflected && reading == "いい")
        }
        NOUN => matches!(class, CLASS_N | CLASS_PN | CLASS_PLACE | CLASS_ADJ_NA | CLASS_ADV),
        NA => class == CLASS_ADJ_NA || class == CLASS_N,
        SURU => class == CLASS_N,
        _ => false,
    }
}

/// Appends to `built` the surface of a form: `surface` (read `reading`) without the part of its
/// reading that differs from `input`, then the rest of `input`. 来る keeps 来 for the き, こ or く
/// of the input. False, writing nothing, when the surface does not end with that part of its
/// reading.
fn build_surface(built: &mut String, surface: &str, reading: &str, input: &str, vk: bool) -> bool {
    if vk {
        if let (Some(head), Some(stem)) = (surface.strip_suffix("来る"), reading.strip_suffix("くる")) {
            let Some(rest) = input.strip_prefix(stem) else { return false };
            let mut chars = rest.chars();
            if !matches!(chars.next(), Some('き' | 'こ' | 'く')) {
                return false;
            }
            built.push_str(head);
            built.push('来');
            built.push_str(chars.as_str());
            return true;
        }
    }
    let shared = reading
        .char_indices()
        .zip(input.chars())
        .find(|&((_, left), right)| left != right)
        .map_or(reading.len().min(input.len()), |((at, _), _)| at);
    let (Some(ending), Some(rest)) = (reading.get(shared..), input.get(shared..)) else { return false };
    let Some(head) = surface.strip_suffix(ending) else { return false };
    built.push_str(head);
    built.push_str(rest);
    true
}

/// Adds a generated form, whose text was just written to `built[start..]`, behind
/// `hits[group..]`. A surface already there keeps the better ranked of the two; past
/// `MAX_FORM_HITS` forms, the new one replaces the worst when better. A text that is not kept
/// is taken off `built` again.
fn keep_form(
    hits: &mut Vec<Hit>,
    built: &mut String,
    lexicon: &JapaneseLexicon,
    group: usize,
    hit: Hit,
    start: usize,
) {
    let text = hit_text(lexicon, built, hit.text);
    let same = hits.iter().enumerate().skip(group).find(|(_, other)| hit_text(lexicon, built, other.text) == text);
    let replace = match same {
        Some((at, other)) => (rank(&hit.row) < rank(&other.row)).then_some(at),
        None => {
            let generated = |other: &&Hit| matches!(other.text, Text::Built { .. });
            if hits.iter().skip(group).filter(generated).count() < MAX_FORM_HITS {
                hits.push(hit);
                return;
            }
            let worst = hits
                .iter()
                .enumerate()
                .skip(group)
                .filter(|(_, other)| generated(other))
                .max_by_key(|(_, other)| rank(&other.row));
            worst.and_then(|(at, other)| (rank(&hit.row) < rank(&other.row)).then_some(at))
        }
    };
    match replace.and_then(|at| hits.get_mut(at)) {
        Some(slot) => *slot = hit,
        None => built.truncate(start),
    }
}
