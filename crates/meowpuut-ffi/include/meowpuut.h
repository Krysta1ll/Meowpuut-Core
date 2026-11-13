// C ABI of the Rust engine (crates/meowpuut-ffi). Drop-in replacement for the
// former C++ Core/MeowpuutCore.h; existing declarations are unchanged.
#ifndef MEOWPUUT_CORE_H
#define MEOWPUUT_CORE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MPNLexicon MPNLexicon;
typedef struct MPNSession MPNSession;

// The loader accepts either a MIMO-BIN-1 binary image or a UTF-8 TSV dictionary.
// Sessions retain the loaded lexicon independently.
MPNLexicon *mpn_lexicon_create(const char *data, size_t size);
// Extracts and loads either Pinyin (pinyin != 0) or Zhuyin (pinyin == 0) from a
// MIMO-BASE-1 (mimo_base.bin) container. The engine copies what it reads
// to the heap (about 44 MB for pinyin, 26 MB for zhuyin with the lexicon of
// 2026-11-11), so data may be freed when this returns.
MPNLexicon *mpn_lexicon_create_from_base(const char *data, size_t size, int pinyin);

// Same, but reads the container in place instead of copying it, so the
// lexicon costs file pages (from a mapped file) instead of heap.
//
// Ownership: the engine takes the bytes over on every call. release(context)
// is called exactly once: when mpn_lexicon_destroy has been called and every
// session created from the lexicon has been destroyed (whichever happens
// last, on that thread), or before this function returns NULL on failure.
// Until then data[0..size) must stay valid and unchanged; unmap or free it in
// release. release must be callable from any thread. With a NULL release the
// caller keeps the bytes alive until the lexicon and all its sessions are
// destroyed.
//
// A version 2 file has index sections and nothing large is built on the heap;
// with a version 1 file the hash indexes (about 6 MiB) are built at load.
typedef void (*MPNReleaseFn)(void *context);
MPNLexicon *mpn_lexicon_create_from_base_in_place(const char *data, size_t size, int pinyin,
                                                  MPNReleaseFn release, void *context);
// Attaches the word-pair table (mimo_pairs.bin) to a lexicon, read in place;
// the decoder then scores each word with the word after it and with the start
// and end of the input. Call it before creating sessions. Returns 1 when
// attached, 0 when the bytes are not a pair table for this lexicon (made for
// another word list) or sessions already exist; the lexicon then ranks as
// without a table. Ownership as for mpn_lexicon_create_from_base_in_place:
// release(context) is called exactly once, when the lexicon and all its
// sessions are destroyed, or before this function returns 0.
int mpn_lexicon_attach_pairs(MPNLexicon *lexicon, const char *data, size_t size,
                             MPNReleaseFn release, void *context);
void mpn_lexicon_destroy(MPNLexicon *lexicon);
// Heap bytes held by the lexicon tables (memory diagnostics); 0 for NULL.
// Bytes read in place are not counted.
size_t mpn_lexicon_heap_bytes(const MPNLexicon *lexicon);
MPNSession *mpn_session_create(const MPNLexicon *lexicon);
// A Pinyin session whose composition keys are ASCII digits 2–9. NULL for Zhuyin.
MPNSession *mpn_nine_key_session_create(const MPNLexicon *lexicon);
void mpn_session_destroy(MPNSession *session);

// Printable ASCII keys and the same X11 editing keys as the Rime adapter.
// A session is used on one thread. Copy returned strings before the next mutation.
// At the limit, the next valid key preserves the old preedit as a commit and
// begins a new composition; boundary tone marks complete the old reading.
size_t mpn_max_input_bytes(void);
int mpn_key(MPNSession *session, int32_t key);
void mpn_set_simplified(MPNSession *session, int simplified);
void mpn_clear(MPNSession *session);
// Restore a Chinese prefix only while rebuilding the same partial composition.
// NULL/empty clears it. The getter points into the session until its mutation.
void mpn_set_segment_context(MPNSession *session, const char *text);
const char *mpn_segment_context(const MPNSession *session);
void mpn_commit(MPNSession *session);
int mpn_choose(MPNSession *session, size_t index);
const char *mpn_input(const MPNSession *session);
const char *mpn_preedit(const MPNSession *session);
const char *mpn_committed(const MPNSession *session);
void mpn_clear_commit(MPNSession *session);
size_t mpn_candidate_count(const MPNSession *session);
const char *mpn_candidate(const MPNSession *session, size_t index);
// Raw input bytes consumed when selecting this candidate; 0 for invalid input.
size_t mpn_candidate_consumed(const MPNSession *session, size_t index);
// Provenance describes how a candidate was generated, not its confidence.
// A fully spelled path is an exact reading, but may not be a plausible phrase.
typedef enum {
    MPN_CANDIDATE_UNKNOWN = 0,
    MPN_CANDIDATE_EXACT = 1,
    MPN_CANDIDATE_PATH = 2,
    MPN_CANDIDATE_PARTIAL = 3,
    MPN_CANDIDATE_CORRECTION = 4,
    MPN_CANDIDATE_JAPANESE = 5
} MPNCandidateKind;
typedef enum {
    MPN_SCORE_NONE = 0,
    MPN_SCORE_LEXICON = 1,
    MPN_SCORE_PATH = 2,
    MPN_SCORE_FALLBACK = 3
} MPNCandidateScoreKind;
typedef struct {
    uint32_t kind;
    uint32_t score_kind;
    uint64_t consumed_bytes;
    // First raw ASCII input byte retained after selection. This may be a
    // valid next word as well as an unfinished or unrecognised suffix.
    uint64_t remaining_offset_bytes;
    // Generation score before session-context reordering. Interpret only
    // within score_kind; it is not a probability or a calibrated confidence.
    double score;
} MPNCandidateInfo;
// Writes the fields fitting in info_size; returns 0 for invalid arguments.
// The info and string describe the same candidate until the next mutation.
int mpn_candidate_info(const MPNSession *session, size_t index, MPNCandidateInfo *info, size_t info_size);
size_t mpn_page(const MPNSession *session);

// Optional Japanese words for mixed pinyin/romaji. Registration and decoding
// are local to the session; a returned string remains valid until the next call.
// Weight 3500 denotes an ambiguous all-kanji pinyin alias: it participates in
// a mixed sentence only when another, higher-confidence Japanese word is present.
// Two-letter readings are rejected except the high-confidence reading "au";
// mixed decoding of that short reading also requires independent evidence.
int mpn_add_japanese_word(MPNSession *session, const char *reading, const char *text, int weight);
const char *mpn_mixed_pinyin(MPNSession *session, const char *reading, int simplified);
// A complete leading segment and its ASCII reading length, leaving the suffix untouched.
const char *mpn_mixed_pinyin_prefix(MPNSession *session, const char *reading, int simplified, size_t *consumed);

// Next-word associations for already committed text (trailing Han characters
// are used; anything else yields 0). Output follows mpn_set_simplified. Returns
// the count (at most min(limit, 64)); strings stay valid until the next
// mpn_predict on this session.
size_t mpn_predict(MPNSession *session, const char *context, size_t limit);
const char *mpn_prediction(const MPNSession *session, size_t index);

// Japanese lexicon image (mimo_jp.bin, format MIMO-JP-1) and its lookup. One romaji string or
// ASCII alias gives the words stored for exactly that input; there is no prefix search.
typedef struct MPNJapanese MPNJapanese;
typedef struct MPNJapaneseQuery MPNJapaneseQuery;

enum { MPN_JP_MODE_JAPANESE = 0, MPN_JP_MODE_PINYIN = 1 };
enum { MPN_JP_KIND_READING = 0, MPN_JP_KIND_VARIANT = 1, MPN_JP_KIND_ALIAS_KANA = 2,
       MPN_JP_KIND_ALIAS_KANJI = 3, MPN_JP_KIND_AUTHORED = 4 };
enum { MPN_JP_FLAG_PINYIN_OK = 1, MPN_JP_FLAG_REVIEWED = 2, MPN_JP_FLAG_FORM = 4 };
enum { MPN_JP_REVIEWED_POOL = 1 };

typedef struct {
    uint32_t kind;
    uint32_t flags;
    int32_t  weight;
    uint32_t word;      // index of the word in the image; equal for the same word under two keys
} MPNJapaneseInfo;      // 16 bytes

// Opens a MIMO-JP-1 image. size must be the length of the whole file.
// copy == 0: the bytes are used in place. They must stay mapped and unchanged until
// mpn_japanese_close has returned: unmap after closing, never before; do not close while a
// lookup on this handle is running; do not rewrite the file in place while it is mapped
// (write a new file and rename it). Nothing is read from the bytes after close; a query
// keeps its own copy of the strings it returns.
// copy != 0: the bytes are copied and may be released at once.
// Returns NULL for NULL, a short buffer, a wrong magic or version, a size other than the file_len
// field, or sections outside the buffer.
MPNJapanese *mpn_japanese_open(const void *data, size_t size, int copy);
void mpn_japanese_close(MPNJapanese *japanese);
size_t mpn_japanese_word_count(const MPNJapanese *japanese);
// First four bytes of the sha256 of the Chinese lexicon the image was compiled with.
uint32_t mpn_japanese_chinese_build(const MPNJapanese *japanese);

// Reviewed rows: one per reviewed reading, sorted by reading (at most 4096, at most 512 with
// MPN_JP_REVIEWED_POOL).
size_t mpn_japanese_reviewed_count(const MPNJapanese *japanese);
// Copies row `index` into the caller's buffers (NUL-terminated). Returns 0 and writes nothing
// when the index is out of range or a buffer is NULL or too small; 97 and 129 bytes are always
// enough. weight and flags may be NULL.
int mpn_japanese_reviewed_get(const MPNJapanese *japanese, size_t index,
                              char *reading, size_t reading_size, char *text, size_t text_size,
                              int32_t *weight, uint32_t *flags);
// Index of the reviewed row with exactly this reading, or -1.
int32_t mpn_japanese_reviewed_find(const MPNJapanese *japanese, const char *reading);
// Registers every MPN_JP_REVIEWED_POOL row with the session through the rule of
// mpn_add_japanese_word. Returns the number accepted.
size_t mpn_session_add_japanese_mixed(MPNSession *session, const MPNJapanese *japanese);

// A query holds the result of one lookup. Use one query on one thread. An MPNJapanese may be shared
// by any number of threads and queries.
MPNJapaneseQuery *mpn_japanese_query_create(void);
void mpn_japanese_query_destroy(MPNJapaneseQuery *query);
// Looks up the whole input (a-z, ' and -, at most 80 bytes; capitals are read as lower case).
// Returns the number of words (at most `limit`); 0 for other input or an unknown mode. Strings
// and info stay valid until the next lookup on this query.
size_t mpn_japanese_lookup(MPNJapaneseQuery *query, const MPNJapanese *japanese,
                           const char *input, int mode, size_t limit);
const char *mpn_japanese_text(const MPNJapaneseQuery *query, size_t index);
// Writes the fields fitting in info_size; returns 0 for invalid arguments.
int mpn_japanese_info(const MPNJapaneseQuery *query, size_t index, MPNJapaneseInfo *info, size_t info_size);
// Hiragana and katakana of the whole input; "" when the input is not a complete romaji string.
const char *mpn_japanese_kana(const MPNJapaneseQuery *query);
const char *mpn_japanese_katakana(const MPNJapaneseQuery *query);

#ifdef __cplusplus
}
#endif

#endif
