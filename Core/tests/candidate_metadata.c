// Compile as C, then link with the C++ core to exercise the exported ABI.
// cc -std=c11 -c Core/tests/candidate_metadata.c -o /tmp/mpn-candidate-test.o
// c++ -std=c++17 /tmp/mpn-candidate-test.o Core/MeowpuutCore.cpp -o /tmp/mpn-candidate-test && /tmp/mpn-candidate-test
#include "../MeowpuutCore.h"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static void type(MPNSession *s, const char *keys) {
    while (*keys) assert(mpn_key(s, (unsigned char)*keys++));
}
static MPNSession *session(const char *data, MPNLexicon **owner) {
    *owner = mpn_lexicon_create(data, strlen(data));
    assert(*owner);
    MPNSession *s = mpn_session_create(*owner);
    assert(s);
    return s;
}
static size_t find(MPNSession *s, const char *text, uint32_t kind, size_t consumed) {
    for (size_t i = 0; i < mpn_candidate_count(s); ++i) {
        MPNCandidateInfo info;
        assert(mpn_candidate_info(s, i, &info, sizeof(info)));
        assert(info.consumed_bytes == mpn_candidate_consumed(s, i));
        assert(info.remaining_offset_bytes == info.consumed_bytes);
        if (!strcmp(mpn_candidate(s, i), text) && info.kind == kind && info.consumed_bytes == consumed) return i;
    }
    fprintf(stderr, "candidate missing: %s kind %u consumed %zu for %s\n", text, kind, consumed, mpn_input(s));
    for (size_t i = 0; i < mpn_candidate_count(s); ++i) {
        MPNCandidateInfo x;
        assert(mpn_candidate_info(s, i, &x, sizeof(x)));
        fprintf(stderr, "  %s kind %u consumed %llu\n", mpn_candidate(s, i), x.kind, (unsigned long long)x.consumed_bytes);
    }
    assert(0);
    return 0;
}
int main(void) {
    const char *pinyin = "MEOWPUUT-PINYIN-1\n"
        "hao\thao3\t好\t好\t15000\n"
        "ni\tni3\t你\t你\t15000\n"
        "ni\tni3\t𠮷\t𠮷\t12000\n"
        "nihao\tni3hao3\t你好\t你好\t16000\n"
        "nixua\tni3xua1\t你花\t你花\t16000\n"
        "wo\two3\t我\t我\t15000\n"
        "wox\two3x1\t我想\t我想\t16000\n"
        "xihuan\txi3huan1\t喜歡\t喜欢\t15000\n";
    MPNLexicon *lex;
    MPNSession *s = session(pinyin, &lex);
    type(s, "nihao");
    size_t index = find(s, "你好", MPN_CANDIDATE_EXACT, 5);
    MPNCandidateInfo info;
    assert(mpn_candidate_info(s, index, &info, sizeof(info)));
    assert(info.score_kind == MPN_SCORE_LEXICON && info.score == 16000);
    assert(!mpn_candidate_info(NULL, 0, &info, sizeof(info)));
    assert(!mpn_candidate_info(s, index, NULL, sizeof(info)));
    assert(!mpn_candidate_info(s, mpn_candidate_count(s), &info, sizeof(info)));
    assert(!mpn_candidate_info(s, index, &info, 0));
    struct { uint32_t kind; uint32_t guard; } small = {99, 0x12345678};
    assert(mpn_candidate_info(s, index, (MPNCandidateInfo *)&small, sizeof(small.kind)));
    assert(small.kind == MPN_CANDIDATE_EXACT && small.guard == 0x12345678);
    mpn_clear(s); type(s, "nihaoni");
    find(s, "你好你", MPN_CANDIDATE_PATH, 7);
    mpn_clear(s); type(s, "wox");
    find(s, "我想", MPN_CANDIDATE_CORRECTION, 3);
    // A vowel is not enough evidence that xua is a complete syllable.
    mpn_clear(s); type(s, "nixua");
    find(s, "你花", MPN_CANDIDATE_UNKNOWN, 5);
    mpn_clear(s); type(s, "niaho");
    find(s, "你好", MPN_CANDIDATE_CORRECTION, 5);
    mpn_clear(s); type(s, "ni3hao3x");
    index = find(s, "你好", MPN_CANDIDATE_PARTIAL, 7);
    assert(mpn_choose(s, index));
    assert(!strcmp(mpn_committed(s), "你好") && !strcmp(mpn_input(s), "x"));
    mpn_clear(s); type(s, "ni3hao3");
    find(s, "你好", MPN_CANDIDATE_EXACT, 7);
    mpn_clear(s); type(s, "ni2hao3");
    for (size_t i = 0; i < mpn_candidate_count(s); ++i) {
        assert(mpn_candidate_info(s, i, &info, sizeof(info)));
        assert(strcmp(mpn_candidate(s, i), "你好") || info.kind != MPN_CANDIDATE_EXACT);
    }
    mpn_clear(s); type(s, "ni'hao");
    find(s, "你好", MPN_CANDIDATE_EXACT, 6);
    mpn_clear(s); type(s, "ni");
    find(s, "𠮷", MPN_CANDIDATE_EXACT, 2);
    mpn_session_destroy(s); mpn_lexicon_destroy(lex);

    const char *zhuyin = "MEOWPUUT-ZHUYIN-1\n"
        "sucl\tsu3cl3\t你好\t你好\t15000\n"
        "suclv\tsu3cl3v \t你好\t你好\t16000\n";
    s = session(zhuyin, &lex);
    type(s, "su3cl3v");
    size_t full = find(s, "你好", MPN_CANDIDATE_EXACT, 7);
    size_t partial = find(s, "你好", MPN_CANDIDATE_PARTIAL, 6);
    assert(full != partial);
    assert(mpn_choose(s, partial));
    assert(!strcmp(mpn_committed(s), "你好") && !strcmp(mpn_input(s), "v"));
    mpn_clear(s); type(s, "su3cl3v");
    full = find(s, "你好", MPN_CANDIDATE_EXACT, 7);
    assert(mpn_choose(s, full) && !strcmp(mpn_input(s), ""));
    mpn_session_destroy(s); mpn_lexicon_destroy(lex);
    puts("PASS: C candidate provenance, ABI bounds, tones and distinct consumed lengths");
}
