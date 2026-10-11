/* test_ifbc_stubs.c — TEST-ONLY stand-ins for the subset of runtime/infy_rt.c that
 * infy_ifbc.c + infy_dyn.c call. The real implementations are owned by another engineer and
 * live in infy_rt.c; this file exists ONLY so the IFBC harness can be built and run before
 * infy_rt.c is ready.
 *
 * DO NOT SHIP THIS FILE. It is compiled into ifbc_tests alongside the test harness and is a
 * thin, obviously-incomplete mirror: no arena, no chain_id/must_drop header, no panic radius,
 * no UTF-8 NFC, no real clock backends beyond the platform default. Anything that links the
 * production runtime must exclude it (it would collide with infy_rt.c's strong symbols).
 *
 * Symbols provided (all declared in runtime/infy_rt.h):
 *   infy_alloc, infy_free, infy_panic, infy_mem_stats, infy_monotonic_ms,
 *   infy_str_of, infy_string_new, infy_string_push, infy_string_push_str,
 *   infy_string_as_str, infy_string_drop.
 */
#include "infy_rt.h"

#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN 1
#endif
#include <windows.h>
#endif

/* ---- allocation counters (a 16-byte header per block, per runtime-abi.md §3) ---------- */
typedef struct { uint64_t size; uint64_t magic; } StubHdr;
#define STUB_HDR_MAGIC 0x53744B5Bu /* "STK" */

static uint64_t g_live_bytes = 0;
static uint64_t g_live_objs = 0;
static uint64_t g_total_allocs = 0;

void* infy_alloc(uint64_t size, uint64_t align) {
    (void)align;
    uint64_t need = size + sizeof(StubHdr);
    StubHdr* h = (StubHdr*)malloc((size_t)(need ? need : sizeof(StubHdr) + 1));
    if (!h) return 0;
    h->size = size;
    h->magic = STUB_HDR_MAGIC;
    g_live_bytes += size;
    g_live_objs++;
    g_total_allocs++;
    return (void*)(h + 1);
}

void infy_free(void* p) {
    if (!p) return;
    StubHdr* h = (StubHdr*)p - 1;
    if (h->magic == STUB_HDR_MAGIC) {
        /* keep counters from going negative under a double-free; tests assert == 0 anyway */
        if (g_live_bytes >= h->size) g_live_bytes -= h->size;
        if (g_live_objs > 0) g_live_objs--;
    }
    free(h);
}

InfyMemStats infy_mem_stats(void) {
    InfyMemStats s;
    s.live_bytes = g_live_bytes;
    s.live_objs = g_live_objs;
    s.total_allocs = g_total_allocs;
    s.arena_bytes = g_live_bytes;
    return s;
}

/* harness-visible leak probe (not in the header) */
uint64_t infy_stub_live_objs(void) { return g_live_objs; }
uint64_t infy_stub_live_bytes(void) { return g_live_bytes; }

/* ---- panic (never used by the IFBC interpreter; defined so the file links) ------------- */
void infy_panic(InfyRCode code, const char* detail, const InfyNodeLoc* loc) {
    fprintf(stderr, "UNEXPECTED infy_panic R-%04X: %s\n", (unsigned)code, detail ? detail : "");
    (void)loc;
    abort();
}
void infy_report(InfyRCode code, const char* detail, uint32_t focus_id, uint32_t chain_id, const InfyNodeLoc* loc) {
    fprintf(stderr, "infy_report R-%04X f=%u c=%u: %s\n", (unsigned)code, focus_id, chain_id, detail ? detail : "");
    (void)loc;
}

/* ---- environment / location ----------------------------------------------------------- */
static InfyEnvSnapshot g_env;
const InfyEnvSnapshot* infy_env(void) { return &g_env; }

static InfyNodeLoc g_loc = { "test_ifbc.c", 0, 0, 0, 0 };
const InfyNodeLoc* infy_current_node_loc(void) { return &g_loc; }

/* ---- monotonic clock ------------------------------------------------------------------ */
uint64_t infy_monotonic_ms(void) {
#ifdef _WIN32
    LARGE_INTEGER freq, t;
    QueryPerformanceFrequency(&freq);
    QueryPerformanceCounter(&t);
    if (freq.QuadPart > 0) return (uint64_t)((double)t.QuadPart * 1000.0 / (double)freq.QuadPart);
    return 0;
#else
    struct timespec ts;
#if defined(CLOCK_MONOTONIC)
    if (clock_gettime(CLOCK_MONOTONIC, &ts) == 0) return (uint64_t)ts.tv_sec * 1000u + (uint64_t)(ts.tv_nsec / 1000000);
#endif
    if (clock_gettime(CLOCK_REALTIME, &ts) == 0) return (uint64_t)ts.tv_sec * 1000u + (uint64_t)(ts.tv_nsec / 1000000);
    return 0;
#endif
}

/* ---- str / String (minimal) ----------------------------------------------------------- */
InfyStr infy_str_of(const char* utf8_literal) {
    InfyStr s; s.ptr = (const uint8_t*)utf8_literal; s.len = utf8_literal ? strlen(utf8_literal) : 0; return s;
}

InfyString infy_string_new(void) {
    InfyString s; s.ptr = 0; s.len = 0; s.cap = 0; return s;
}

static void str_reserve(InfyString* s, uint64_t need) {
    if (need <= s->cap) return;
    uint64_t nc = s->cap ? s->cap : 4;
    while (nc < need) nc *= 2;
    uint8_t* np = (uint8_t*)infy_alloc(nc, 1);
    if (!np) return;
    if (s->ptr) { memcpy(np, s->ptr, (size_t)s->len); infy_free(s->ptr); }
    s->ptr = np; s->cap = nc;
}

void infy_string_push(InfyString* s, uint32_t cp) {
    /* encode a Unicode code point as UTF-8 */
    uint8_t buf[4]; int n = 0;
    if (cp < 0x80) { buf[n++] = (uint8_t)cp; }
    else if (cp < 0x800) { buf[n++] = (uint8_t)(0xC0 | (cp >> 6)); buf[n++] = (uint8_t)(0x80 | (cp & 0x3F)); }
    else if (cp < 0x10000) { buf[n++] = (uint8_t)(0xE0 | (cp >> 12)); buf[n++] = (uint8_t)(0x80 | ((cp >> 6) & 0x3F)); buf[n++] = (uint8_t)(0x80 | (cp & 0x3F)); }
    else { buf[n++] = (uint8_t)(0xF0 | (cp >> 18)); buf[n++] = (uint8_t)(0x80 | ((cp >> 12) & 0x3F)); buf[n++] = (uint8_t)(0x80 | ((cp >> 6) & 0x3F)); buf[n++] = (uint8_t)(0x80 | (cp & 0x3F)); }
    str_reserve(s, s->len + (uint64_t)n);
    for (int i = 0; i < n; i++) s->ptr[s->len++] = buf[i];
}

void infy_string_push_str(InfyString* s, InfyStr b) {
    if (!b.ptr || b.len == 0) return;
    str_reserve(s, s->len + b.len);
    memcpy(s->ptr + s->len, b.ptr, (size_t)b.len);
    s->len += b.len;
}

InfyStr infy_string_as_str(const InfyString* s) {
    InfyStr r; r.ptr = s->ptr; r.len = s->len; return r;
}

void infy_string_drop(InfyString* s) {
    if (!s) return;
    if (s->ptr) infy_free(s->ptr);
    s->ptr = 0; s->len = 0; s->cap = 0;
}
