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
// A Pinyin session whose composition keys are ASCII digits 2–9. An apostrophe
// after a digit adds a word boundary and is omitted from commits. NULL for Zhuyin.
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
// The uncommitted input as the user would write it, for committing it raw:
// Zhuyin as Bopomofo with ˊ ˇ ˋ ˙ but no ˉ or separator (ㄋㄧˇㄏㄠˇ), Pinyin
// and nine-key as the keys typed. After a partial choose, only the rest.
const char *mpn_input_text(const MPNSession *session);
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
    MPN_SCORE_FALLBACK = 3,
    MPN_SCORE_FUZZY_INITIAL = 4
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
// 1 when text is or holds a word the keyboard never volunteers (FLAG_NO_SUGGEST,
// either script; not inside an everyday word that holds it): such a word is offered
// only for its full spelling. Filter learned next words with it.
int mpn_text_no_suggest(const MPNSession *session, const char *text);

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

// English lexicon (mimo_en.bin, format MIMO-EN-1).
// Attaching it to a pinyin lexicon adds English words beyond mimo-latin to pinyin and nine-key
// typing (after Chinese; never a blocked word). Ownership as for mpn_lexicon_attach_pairs; returns
// 1 when attached, 0 for another file, a Zhuyin lexicon, or a lexicon sessions already use.
int mpn_lexicon_attach_english(MPNLexicon *lexicon, const char *data, size_t size,
                               MPNReleaseFn release, void *context);

// The English keyboard: completions, corrections, next words and keypad (nine-key) words.
// copy == 0: the bytes are used in place and must stay mapped and unchanged until
// mpn_english_close has returned; copy != 0: they are copied. NULL for a damaged image.
typedef struct MPNEnglish MPNEnglish;
typedef struct MPNEnglishQuery MPNEnglishQuery;
enum { MPN_ENGLISH_SHIFT = 1, MPN_ENGLISH_CAPS_LOCK = 2 };
enum { MPN_ENGLISH_KIND_EXACT = 1, MPN_ENGLISH_KIND_COMPLETION = 2,
       MPN_ENGLISH_KIND_CORRECTION = 3, MPN_ENGLISH_KIND_PREDICTION = 4,
       MPN_ENGLISH_KIND_LETTERS = 5 };
MPNEnglish *mpn_english_open(const void *data, size_t size, int copy);
void mpn_english_close(MPNEnglish *english);
size_t mpn_english_word_count(const MPNEnglish *english);
// A query holds the result of one call; use it on one thread. An MPNEnglish may be shared.
MPNEnglishQuery *mpn_english_query_create(void);
void mpn_english_query_destroy(MPNEnglishQuery *query);
// `before` is the text before the word (or the cursor), read during the call only; NULL for none.
// It gives the word before (pair scores) and the sentence start. Casing: a typed capital first
// letter or MPN_ENGLISH_SHIFT capitalises (acronyms and words like iPhone keep their spelling),
// two or more typed capitals or MPN_ENGLISH_CAPS_LOCK give capitals; I, proper nouns and
// acronyms are always spelled as listed. Each returns the count (at most 8); strings stay valid
// until the next call on this query. Blocked words are never offered unless typed in full.
size_t mpn_english_complete(MPNEnglishQuery *query, const MPNEnglish *english,
                            const char *prefix, const char *before, uint32_t flags);
// Only for a word not in the lexicon (dont -> don't, teh -> the); 0 for a known word.
size_t mpn_english_correct(MPNEnglishQuery *query, const MPNEnglish *english,
                           const char *word, const char *before, uint32_t flags);
// After a space: the followers of the last word of `before`; sentence starters (capitalised)
// at the start or after . ! ?.
size_t mpn_english_predict(MPNEnglishQuery *query, const MPNEnglish *english,
                           const char *before, uint32_t flags);
// Keypad digits 2-9 (65 -> OK, 4663 -> good, home, gone): the words they spell, then up to three
// longer ones. Capitalised at a sentence start of `before`. When they spell no word that may be
// offered, the first result is the first letter of each key (MPN_ENGLISH_KIND_LETTERS: 7777 ->
// pppp), and a blocked word the keys spell comes after it, never first.
size_t mpn_english_nine_key(MPNEnglishQuery *query, const MPNEnglish *english,
                            const char *digits, const char *before, uint32_t flags);
// 1 when `word` (any casing) is a blocked word of the list: for filtering a platform's own
// spelling suggestions (UITextChecker), which must not offer it either. 0 otherwise.
int mpn_english_is_blocked(const MPNEnglish *english, const char *word);
const char *mpn_english_text(const MPNEnglishQuery *query, size_t index);
int mpn_english_kind(const MPNEnglishQuery *query, size_t index);

// ---------------------------------------------------------------------------
// Pets: the two cats' moments, levels and paw book (crates/meowpuut-pets).
//
// Deterministic: the engine reads no clock, randomness or file. The client
// passes a monotonic clock (systemUptime / elapsedRealtime in ms), the local
// day index (days since 1970-01-01 in the device time zone; 0 = unknown) and
// the local minute of the day, plus a seed of its choice. A day index that
// goes backwards counts as the same day; a monotonic time that goes backwards
// is clamped and blocks spontaneous moments. Any doubt plans nothing.
//
// Call mp_pets_decide only from: keyboard shown (MP_PETS_APPEAR), the commit
// callback (MP_PETS_COMMIT, arg = graphemes), the single idle timer
// (MP_PETS_IDLE), a touch on a cat or its seat (MP_PETS_TAP/HOLD/SEAT/DROP,
// arg = cat) and keyboard hidden (MP_PETS_DISAPPEAR). Never on the key path.
//
// Limits: spontaneous moments (peekaboo) need a token (bucket of 2, +1 every
// 120 s), at most 1 per session (APPEAR..DISAPPEAR) and 3 per local day, and
// never while composing, within 600 ms of a key, while a cat is lifted, while
// dismissing, with VoiceOver/Switch Control/TalkBack, with lightweight effects
// or in quiet/off mode. The greeting is once per local day. A purr earns a
// play at most once per cat per 8 s and 12 times per cat a day; 2 duets a day.
// Plays are counted but never change the level (the 0.8 level rule is kept).
//
// Persistence: mp_pets_to_bytes gives MP_PETS_STATE_BYTES (2884) bytes: fixed
// length, little endian, versioned, checksummed, no text. Flush under a file
// lock: disk = mp_pets_from_bytes(file); mp_pets_take_delta(live, d);
// mp_pets_apply_delta(disk, d); mp_pets_to_bytes(live) -> mp_pets_merge(disk);
// write mp_pets_to_bytes(disk) to a temporary file and rename it over the
// file; mp_pets_merge(live, those bytes). Merge is commutative, associative and
// idempotent; deltas add, so two processes' increments are both kept.
//
// Golden vectors for client tests: Tests/pets-moments.json.
// A handle is used on one thread. NULL handles and pointers are tolerated.
typedef struct MPPets MPPets;

#define MP_PETS_STATE_BYTES 2884
#define MP_PETS_DELTA_BYTES 84

enum { MP_PETS_MIMIMO = 1, MP_PETS_MOMO = 2 };
enum { MP_PETS_APPEAR = 1, MP_PETS_COMMIT = 2, MP_PETS_IDLE = 3, MP_PETS_TAP = 4,
       MP_PETS_HOLD = 5, MP_PETS_SEAT = 6, MP_PETS_DROP = 7, MP_PETS_DISAPPEAR = 8 };
// MPPetsAmbience.flags. GLASS_SEAT: the cats sit behind the glass bar with the
// cut-out mask (sinking works); otherwise peekaboo is a fade. EDGES: edge
// seating (resting poses apply). REDUCE_TRANSPARENCY also covers Increase Contrast.
enum { MP_PETS_COMPOSING = 1, MP_PETS_LIFTED = 2, MP_PETS_DISMISSING = 4, MP_PETS_ASSISTIVE = 8,
       MP_PETS_LIGHTWEIGHT = 16, MP_PETS_REDUCE_MOTION = 32, MP_PETS_REDUCE_TRANSPARENCY = 64,
       MP_PETS_GLASS_SEAT = 128, MP_PETS_EDGES = 256, MP_PETS_FESTIVAL = 512 };
// Quiet: only what the user starts, plus greeting and doze. Off: no plans
// (events still count). An unknown mode acts as off.
enum { MP_PETS_MODE_FULL = 0, MP_PETS_MODE_QUIET = 1, MP_PETS_MODE_OFF = 2 };
// MPPetsPlan.moment.
//   HELLO 0.9 s: Momo waves, Mimimo reaches a beat later.
//   PURR up to 4 s: Mimimo sleep / Momo happy (until momo_sleep exists), breathing 2.6 s.
//   PEEKABOO: sink (0.28 s; fade 0.25 s), surface after delay_ms or on SEAT.
//   FOUND 0.35 s happy. DOZE: a state until a key or touch (0.3 s stretch).
//   DUET 0.6 s: Momo jump with Mimimo happy. FLOW: rim glow 0 -> 0.12 over 3 s.
// Any moment yields within 0.18 s of a key press (client side).
enum { MP_PETS_HELLO = 1, MP_PETS_PURR = 2, MP_PETS_PEEKABOO = 3, MP_PETS_FOUND = 4,
       MP_PETS_DOZE = 5, MP_PETS_DUET = 6, MP_PETS_FLOW = 7 };
enum { MP_PETS_STYLE_MOTION = 0, MP_PETS_STYLE_FADE = 1 };
enum { MP_PETS_RIPPLE_NONE = 0, MP_PETS_RIPPLE_SOFT = 1, MP_PETS_RIPPLE_LINE = 2 };
// COMPLETED also covers the user ending it (finger lifted from a purr).
// INTERRUPTED: typing, a drag or the keyboard leaving. CANCELLED: never shown.
enum { MP_PETS_COMPLETED = 0, MP_PETS_INTERRUPTED = 1, MP_PETS_CANCELLED = 2 };
enum { MP_PETS_OK = 0, MP_PETS_FRESH = 1, MP_PETS_CORRUPT = 2, MP_PETS_UNKNOWN_VERSION = 3 };
enum { MP_PETS_REST_KEEP = 0, MP_PETS_REST_EDGE_REST = 1, MP_PETS_REST_EDGE_PEEK = 2,
       MP_PETS_REST_EDGE_LOUNGE = 3, MP_PETS_REST_SLEEP = 4 };
#define MP_PETS_NEVER UINT32_MAX

typedef struct {
    uint64_t mono_ms;
    uint32_t day;
    uint32_t minute;
} MPPetsClock;          // 16 bytes

typedef struct {
    uint32_t flags;     // MP_PETS_COMPOSING ...
    uint32_t quiet_ms;  // since the last key press; UINT32_MAX when none
    uint32_t mode;      // MP_PETS_MODE_*
    uint32_t cats;      // cats on screen, MP_PETS_MIMIMO | MP_PETS_MOMO
} MPPetsAmbience;       // 16 bytes

typedef struct {
    uint32_t token;     // for mp_pets_resolve; never 0
    uint8_t moment;     // MP_PETS_HELLO ...
    uint8_t who;        // cat bits
    uint8_t style;      // MP_PETS_STYLE_*
    uint8_t ripple;     // MP_PETS_RIPPLE_*
    uint32_t duration_ms;
    uint32_t delay_ms;  // peekaboo: surface by itself after this long
} MPPetsPlan;           // 16 bytes

typedef struct {
    uint32_t day;       // local day index
    uint8_t size;       // committed words band 0-4
    uint8_t who;        // cat bits
    uint8_t part;       // 1 << part: 0 dawn, 1 day, 2 evening, 3 night, 4 late night
    uint8_t reserved;
} MPPetsMark;           // 8 bytes

typedef struct {
    uint32_t level;     // 0-100
    uint32_t graphemes_to_next;
    uint32_t contacts_to_next;
    uint32_t reserved;
    uint64_t pets;
    uint64_t falls;
    uint64_t plays;     // rewarded moments; not part of the level
} MPPetsProgress;       // 40 bytes

// Always returns a handle: NULL, empty, damaged or unknown-version data start
// fresh; *status (may be NULL) is MP_PETS_OK, _FRESH, _CORRUPT or _UNKNOWN_VERSION.
MPPets *mp_pets_from_bytes(const uint8_t *data, size_t size, int *status);
void mp_pets_free(MPPets *pets);
// Returns 1 and writes *plan, or 0 for no plan (also for invalid arguments).
int mp_pets_decide(MPPets *pets, uint32_t trigger, uint32_t arg, const MPPetsAmbience *ambience,
                   const MPPetsClock *clock, uint64_t seed, MPPetsPlan *plan);
// Returns the cat bits that earned a play; 0 for unknown or stale tokens.
uint32_t mp_pets_resolve(MPPets *pets, uint32_t token, uint32_t outcome, const MPPetsClock *clock);
// Delay for the single idle timer (0: fire now), or MP_PETS_NEVER.
uint32_t mp_pets_idle_delay_ms(const MPPets *pets, const MPPetsAmbience *ambience, const MPPetsClock *clock);
// Resting pose on appearance (MP_PETS_REST_*): edge seating only, not during festivals.
uint32_t mp_pets_rest_pose(uint32_t pet, const MPPetsAmbience *ambience, uint32_t minute);
// Writes MP_PETS_STATE_BYTES when capacity allows; returns MP_PETS_STATE_BYTES (0 for NULL).
size_t mp_pets_to_bytes(const MPPets *pets, uint8_t *out, size_t capacity);
// Moves the unflushed increments into out and clears them, only when capacity
// allows; returns MP_PETS_DELTA_BYTES (0 for NULL).
size_t mp_pets_take_delta(MPPets *pets, uint8_t *out, size_t capacity);
// 1 when applied/merged; 0 (state unchanged) for invalid bytes.
int mp_pets_apply_delta(MPPets *pets, const uint8_t *data, size_t size);
int mp_pets_merge(MPPets *pets, const uint8_t *data, size_t size);
// Old per-platform counters (iOS petAffinity.v1, Android committed_graphemes/pets_*/falls_*),
// merged with max so importing twice changes nothing.
int mp_pets_import_legacy(MPPets *pets, uint64_t committed_graphemes, uint64_t mimimo_pets,
                          uint64_t mimimo_falls, uint64_t momo_pets, uint64_t momo_falls);
// Clears the records and raises the epoch; flush afterwards instead of only deleting the file.
void mp_pets_clear(MPPets *pets);
// Writes the fields fitting in info_size; 0 for invalid arguments.
int mp_pets_progress(const MPPets *pets, uint32_t pet, MPPetsProgress *info, size_t info_size);
// Copies up to capacity marks, oldest day first; returns the number of marks (at most 400).
size_t mp_pets_marks(const MPPets *pets, MPPetsMark *out, size_t capacity);

#ifdef MEOWPUUT_SKIN
// ---------------------------------------------------------------------------
// Custom skins (cargo feature `skin` of meowpuut-ffi; plans/custom-skins.md).
// For the Android app and IME and the iOS containing app only: never link
// these into a keyboard extension (the iOS keyboard verifies installed skins
// with CryptoKit, crates/meowpuut-skin/SEAL.md).
//
// Two package kinds, both signed by a root-certified `skin` subkey:
//   .mpskin   recipient: encrypted for one request on one device; redeemed
//             with mps_skin_redeem (Android only: iOS has no redemption).
//   .mpskinb  bundled: plaintext, not device-bound, shipped inside an app
//             update; installed with mps_skin_install_bundled (iOS, Android).
// Feeding one kind to the other's entry point fails with MPS_E_KIND.
//
// Every int32_t function returns MPS_OK (0) or one of the MPS_E_* codes below
// (text: mps_skin_error). JSON results are NUL-terminated UTF-8 written into
// the caller's buffer; MPS_RESULT_CAP bytes are always enough. On an error the
// buffer receives {"detail":"…","error":<code>} when it fits. Nothing is
// installed when an error (MPS_E_BUFFER included) is returned.
//
// Trust follows the build: with the cargo feature `skin-test-keys` (Debug and
// Lab builds only) the published test root and `test-` kids are accepted;
// without it every `test-` kid is refused (MPS_E_TEST_KEY).
// mps_skin_accepts_test_keys() says which; a Release build must see 0.
//
// state_json is the device's monotonic state kept by the platform (Android:
// a Keystore AES-GCM encrypted private file); NULL or "" for a new device:
//   {"highest":{"<id>":<rev>},"revoked":{"kid":[],"serial":[],"skin":[]},"seq":<n>,"v":1}
// Store the "state" a call returns; never lower or drop entries.
//
// Secrets passed in (request_secrets) are copied and wiped inside; wipe the
// caller's own copies right after the call.
enum {
    MPS_OK = 0,
    MPS_E_TOO_LARGE = 1,          // an input is over its size limit
    MPS_E_MALFORMED = 2,          // container or seal structure broken
    MPS_E_HEADER = 3,             // header JSON invalid or not canonical
    MPS_E_CERTIFICATE = 4,        // subkey certificate invalid or from an unknown root
    MPS_E_KEY_USE = 5,            // subkey not certified for skins
    MPS_E_TEST_KEY = 6,           // a test- kid in a build without test keys
    MPS_E_REVOKED = 7,            // subkey, skin revision or serial revoked
    MPS_E_SIGNATURE = 8,          // header signature does not verify (strict)
    MPS_E_PAYLOAD = 9,            // ciphertext length or hash differs from the header
    MPS_E_NOT_FOR_THIS_DEVICE = 10, // issued for another request
    MPS_E_KEY_AGREEMENT = 11,     // small-order key
    MPS_E_DECRYPT = 12,           // the code does not match the package
    MPS_E_ARCHIVE = 13,           // MPAR archive or file name invalid
    MPS_E_MANIFEST = 14,          // manifest failed validation
    MPS_E_IMAGE = 15,             // an image failed validation
    MPS_E_SOUND = 16,             // a sound failed validation
    MPS_E_FILES = 17,             // rewritten files differ from the signed list
    MPS_E_DOWNGRADE = 18,         // older than the highest revision seen
    MPS_E_TOO_MANY_SKINS = 19,    // 12 skins installed already
    MPS_E_CLIENT = 20,            // not made for this platform or needs a newer client
    MPS_E_STATE = 21,             // state_json invalid
    MPS_E_STORAGE = 22,           // reading or writing the skins folder failed
    MPS_E_NOT_INSTALLED = 23,     // no such installed skin
    MPS_E_TAMPERED = 24,          // installed files differ from the seal
    MPS_E_BINDING = 25,           // installed skin belongs to another device
    MPS_E_ARGUMENT = 26,          // NULL, not UTF-8 or malformed argument
    MPS_E_BUFFER = 27,            // output buffer too small; nothing changed
    MPS_E_REVOCATION = 28,        // revocation file invalid or not root-signed
    MPS_E_INTERNAL = 29,
    MPS_E_REQUEST_ID = 30,
    MPS_E_RANDOM = 31,            // the system random source failed
    MPS_E_KIND = 32               // the package is of the other kind (bundled vs recipient)
};
#define MPS_RESULT_CAP (256 * 1024)

// Largest .mpskin or .mpskinb: check the file size (or bound the content:// read loop)
// before reading it, and refuse a larger file without reading it.
size_t  mps_skin_max_package_len(void);
// 1 in a Debug/Lab build (test keys accepted), 0 in a release build.
int32_t mps_skin_accepts_test_keys(void);

// A new X25519 request key pair, one per "ask the developer for a skin".
// Keep the secret wrapped by the Keystore; keep the public key for
// mps_skin_verify_installed. 32 bytes each.
int32_t mps_request_key_generate(uint8_t secret_out[32], uint8_t public_out[32]);
int32_t mps_request_public_key(const uint8_t secret[32], uint8_t public_out[32]);
// The redemption ID to send to the developer ("MPR2-XXXX-…", 74 characters).
// Returns its length, or 0 for a NULL key or a buffer under 75 bytes.
size_t  mps_redemption_id(const uint8_t public_key[32], char *out, size_t cap);

// Verifies, decrypts, validates, rewrites and installs a .mpskin into
// <skins_dir>/<id>/<rev>/ (atomic rename; idempotent for the same package).
// request_secrets: secret_count keys of 32 bytes (this device's open requests).
// client: "android/<major>.<minor>" (checked against the header's minClient).
// result_json: {"base","dir","id","kind":"recipient","names":{…},"rev",
//               "skin":"<id>@<rev>","state":{…new state…},"warnings":[…]}
int32_t mps_skin_redeem(const uint8_t *package, size_t package_len,
                        const uint8_t *request_secrets, size_t secret_count,
                        const char *state_json, const char *skins_dir, const char *client,
                        char *result_json, size_t cap);
// Verifies, validates, rewrites and installs a bundled .mpskinb read from the
// app bundle into <skins_dir>/<id>/<rev>/ (files + seal.bin, no binding.bin;
// atomic rename; idempotent: call it for every bundled package after each app
// update, and ignore MPS_E_DOWNGRADE when the device already has a newer rev).
// Same checks as mps_skin_redeem minus the request keys: certificate, kind,
// strict signature, revocations (kid, id@rev, serial), minClient, highest
// revision, payload hash, validator, rewrite == signed files.
// client: "ios/<major>.<minor>" or "android/<major>.<minor>".
// result_json: as mps_skin_redeem, with "kind":"bundled".
int32_t mps_skin_install_bundled(const uint8_t *package, size_t package_len,
                                 const char *state_json, const char *skins_dir, const char *client,
                                 char *result_json, size_t cap);
// Verify-on-load: call before reading any image or sound of the skin named by
// the customSkin setting ("<id>@<rev>", re-parsed here). Checks the seal,
// revocations, highest revision, every file's length and SHA-256, extra files
// and, for a recipient skin, binding.bin and the device binding (a bundled
// skin is not device-bound). request_publics: this device's request public
// keys (public_count × 32 bytes); NULL skips only the binding check, a
// non-NULL pointer with count 0 means the device has none (the check fails).
// result_json: {"base","dir","id","kind":"recipient"|"bundled","names":{…},"rev","skin"}
int32_t mps_skin_verify_installed(const char *skins_dir, const char *custom_skin,
                                  const char *state_json,
                                  const uint8_t *request_publics, size_t public_count,
                                  char *result_json, size_t cap);
// Every installed skin, each verified as above:
// {"skins":[{…as verify…,"ok":true} | {"error":<code>,"ok":false,"skin":"<id>@<rev>"}]}
int32_t mps_skin_list_installed(const char *skins_dir, const char *state_json,
                                const uint8_t *request_publics, size_t public_count,
                                char *result_json, size_t cap);
// Removes <skins_dir>/<id>/<rev>/. The state keeps the highest revision.
int32_t mps_skin_remove(const char *skins_dir, const char *custom_skin);
// Merges a root-signed .mprevoke (union, never removes): {"added":<n>,"state":{…}}.
int32_t mps_revocation_merge(const uint8_t *file, size_t len, const char *state_json,
                             char *out_json, size_t cap);
// Static English text of a code ("" for MPS_OK).
const char *mps_skin_error(int32_t code);
#endif // MEOWPUUT_SKIN

#ifdef __cplusplus
}
#endif

#endif
