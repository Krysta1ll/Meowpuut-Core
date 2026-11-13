#![forbid(unsafe_code)]
//! `meowpuut-lexicon` — Phonetic normalization, Structure-of-Arrays (SoA) lexicon
//! catalog & compiler, word/English evidence stores, and phrase association tables.
//!
//! # Examples
//!
//! ```
//! use meowpuut_lexicon::{
//!     compile_tsv, encode_syllable, numbered_pinyin, ContextColumnSoA, LexiconCatalogSoA,
//!     WordEvidenceSoA,
//! };
//!
//! assert_eq!(encode_syllable("ㄋㄧˇ"), "su3");
//! assert_eq!(numbered_pinyin("nǐ hǎo"), "ni3hao3");
//!
//! let mut catalog = LexiconCatalogSoA::with_capacity(4, 128);
//! catalog.push_entry("nihao", "ni3hao3", "你好", "你好", 5400, 1);
//! let ctx = ContextColumnSoA::default();
//! let tsv = compile_tsv(true, &mut catalog, &ctx);
//! let lex = meowpuut_core::Lexicon::load(&tsv).unwrap();
//! assert!(lex.pinyin);
//!
//! let ev = WordEvidenceSoA::from_pairs(&[("你好", 5400)]);
//! assert_eq!(ev.weight("你好"), Some(5400));
//! ```

pub mod compiler;
pub mod evidence;
pub mod phonetics;
pub mod soa_columns;

pub use compiler::{
    compile_binary, compile_tsv, merge_catalogs, parse_japanese_tsv, JapaneseLexiconSoA,
    MergeReport,
};
pub use evidence::{AssociationTableSoA, EnglishEvidenceSoA, WordEvidenceSoA};
pub use phonetics::{
    encode_syllable, hiragana_to_katakana, numbered_pinyin, pinyin_kana, romaji_to_hiragana,
    strip_pinyin_tones,
};
pub use soa_columns::{calibrate_weight, ContextColumnSoA, LexiconCatalogSoA};
