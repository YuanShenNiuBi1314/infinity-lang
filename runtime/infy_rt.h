/* infy_rt.h — INFY v2.0 embedded runtime API (IFABI-2).
 *
 * This header is the CONTRACT between `codegen` (which emits C11 calling these entry points)
 * and `runtime/infy_rt.c` / `runtime/infy_ifbc.c` (which implement them). Semantics come from
 * the language spec: §4 (types, numeric rules, UB bans), §7.4 (artifact segments), §7.8 +
 * appendix D (IFBC), §8 (chains, nodes, focuses, ResourceLock), §9 (diagnostics, incl. the
 * mandatory focus/chain/node triple on every R- report), §10 (std), §11 (host protocol).
 * Representation choices (sizes, vtable order, growth factors, container layout) are fixed by
 * docs/internals/runtime-abi.md.
 *
 * C11, no dependencies beyond libc + platform threads. Compiles with gcc/clang/cl.
 */
#ifndef INFY_RT_H
#define INFY_RT_H

#include <stdarg.h>
#include <setjmp.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

#define INFY_ART_MAGIC "INFYART2"
#define INFY_IFBC_MAGIC "IFB1"
#define INFY_IFABI 2u

/* ---------- policies (§4.4, §4.16, §5.2, §7.2) ---------- */
typedef enum { INFY_OV_PANIC = 0, INFY_OV_SATURATE = 1 } InfyOverflow;
typedef enum { INFY_RADIUS_FOCUS = 0, INFY_RADIUS_CHAIN = 1, INFY_RADIUS_PROCESS = 2 } InfyPanicRadius;

/* ---------- fundamental value types ---------- */
typedef struct { const uint8_t* ptr; uint64_t len; } InfyStr;      /* str: UTF-8, not NUL-terminated */
typedef struct { uint8_t* ptr; uint64_t len; uint64_t cap; } InfyString;
typedef struct { const void* ptr; uint64_t len; } InfySlice;        /* [T] fat pointer (ptr,len) */
typedef struct { void* ptr; uint64_t len; uint64_t cap; } InfyVec;  /* Vec<T>, element size in meta */
typedef struct { uint64_t len; uint64_t cap; void* ptr; } InfyVecHdr;
typedef uint8_t InfyBool;
typedef uint32_t InfyChar;
typedef struct { uint8_t _0; } InfyUnit;

#if defined(__SIZEOF_INT128__)
typedef unsigned __int128 InfyU128;
typedef __int128 InfyI128;
#define INFY_HAS_I128 1
#else
typedef struct { uint64_t lo, hi; } InfyU128;
typedef struct { uint64_t lo, int64_t hi; } InfyI128s;
#define INFY_HAS_I128 0
#endif

/* ---------- R- codes (§4.9, §7.8.3, §8, §11) — closed set, no implementation extensions ---- */
typedef enum {
    R_INT_OVERFLOW   = 0x0001, /* R-0001 */
    R_DIV_ZERO       = 0x0002, /* R-0002 */
    R_NUL_IN_STR     = 0x0003, /* R-0003 */
    R_INDEX_OOB      = 0x0004, /* R-0004 */
    R_UNWRAP_NONE    = 0x0005, /* R-0005 */
    R_BAD_BOOL       = 0x0006, /* R-0006 */
    R_SHIFT_WIDTH    = 0x0007, /* R-0007 */
    R_INF_REM        = 0x0008, /* R-0008 */
    R_ILOG_ZERO      = 0x0009, /* R-0009 */
    R_GOTO_TARGET    = 0x0010, /* R-0010 */
    R_DOUBLE_PANIC   = 0x0011, /* R-0011 */
    R_RADIX          = 0x0012, /* R-0012 */
    R_IFBC_INVALID   = 0x0703, /* R-0703 */
    R_IFBC_NONLINEAR = 0x0704,
    R_IFBC_STACK     = 0x0705,
    R_IFBC_TYPE      = 0x0706,
    R_IFBC_FRAME     = 0x0707,
    R_IFBC_HOSTSIG   = 0x0708,
    R_IFBC_MEMBUDGET = 0x0709,
    R_IFBC_BOUNDARY  = 0x070A,
    R_DYN_TAG        = 0x0710,
    R_DYN_TARGET     = 0x0712,
    R_ATRACK_POINTER = 0x0720,
    R_ATRACK_UNWIND  = 0x0721,
    R_ATRACK_ERR_OUT = 0x0722,
    R_CB_LEAK        = 0x0723,
    R_MUST_DROP      = 0x0730,
    R_MOVED_LOCAL    = 0x0731,
    R_FOCUS_WRAP     = 0x0802,
    R_ITER_GUARD     = 0x0803,
    R_HOST_UNMAPPED  = 0x0900,
    R_HOST_QUERY_TYP = 0x0901,
    R_HOST_NESTING   = 0x0902,
    R_IFABI_MISMATCH = 0x0903,
    R_EVENT_MISSING  = 0x0904,
    R_ENV_LAYOUT     = 0x0905,
    R_DROP_RACE      = 0x1010,
    R_STACK_OVERFLOW = 0x1040
} InfyRCode;

/* ---------- diagnostics (§9.2 byte layout, §12.4 mandatory triple) ---------- */
typedef struct { const char* file; uint32_t line; uint32_t col; uint32_t node_idx; uint32_t linkage_count; } InfyNodeLoc;

void infy_report(InfyRCode code, const char* detail, uint32_t focus_id, uint32_t chain_id, const InfyNodeLoc* loc);
/* Never returns; applies the panic radius from .inf meta (§4.16). */
void infy_panic(InfyRCode code, const char* detail, const InfyNodeLoc* loc);

/* ---------- allocation + drop bookkeeping (§4.7.4, §4.8) ---------- */
void*  infy_alloc(uint64_t size, uint64_t align);
void*  infy_realloc(void* p, uint64_t old_size, uint64_t new_size, uint64_t align);
void   infy_free(void* p);
typedef void (*InfyDropFn)(void* p);
typedef void (*InfyDeferFn)(void* env);
/* Push a cleanup entry (drop or defer) on the current focus's LIFO deque. */
void   infy_cleanup_push(InfyDropFn fn, void* obj, uint8_t is_defer);
void   infy_cleanup_pop_run(void);    /* normal scope exit: run the top entry, then pop   */
void   infy_cleanup_pop_noop(void);   /* #[no_drop]: pop without running (§4.7.4)          */
void   infy_cleanup_drain(void);      /* unwind: drain LIFO (§4.7.4 drop order)            */
typedef struct { uint64_t live_bytes; uint64_t live_objs; uint64_t total_allocs; uint64_t arena_bytes; } InfyMemStats;
InfyMemStats infy_mem_stats(void);
/* Leak check at `stop` / unload: nonzero outstanding callback Arc count => R-0723 (§7.6.5). */
void infy_leak_check(void);

/* ---------- checked arithmetic: boundary checks are never optimised out (§4.9) ---------- */
#define INFY_CHECK(cond, code, detail, loc) do { if (__builtin_expect(!(cond), 0)) infy_panic((code), (detail), (loc)); } while (0)

uint8_t  infy_add_u8 (uint8_t a, uint8_t b, const InfyNodeLoc* l);
uint16_t infy_add_u16(uint16_t a, uint16_t b, const InfyNodeLoc* l);
uint32_t infy_add_u32(uint32_t a, uint32_t b, const InfyNodeLoc* l);
uint64_t infy_add_u64(uint64_t a, uint64_t b, const InfyNodeLoc* l);
int8_t   infy_add_i8 (int8_t a, int8_t b, const InfyNodeLoc* l);
int16_t  infy_add_i16(int16_t a, int16_t b, const InfyNodeLoc* l);
int32_t  infy_add_i32(int32_t a, int32_t b, const InfyNodeLoc* l);
int64_t  infy_add_i64(int64_t a, int64_t b, const InfyNodeLoc* l);
/* sub/mul/div/rem follow the same per-width pattern; div/rem check R-0002 first (§4.12:
 * division truncates toward zero, % takes the sign of the dividend). */
#define INFY_ARITH_SET(op) \
  uint8_t  infy_##op##_u8 (uint8_t a,  uint8_t b,  const InfyNodeLoc* l); \
  uint16_t infy_##op##_u16(uint16_t a, uint16_t b, const InfyNodeLoc* l); \
  uint32_t infy_##op##_u32(uint32_t a, uint32_t b, const InfyNodeLoc* l); \
  uint64_t infy_##op##_u64(uint64_t a, uint64_t b, const InfyNodeLoc* l); \
  int8_t   infy_##op##_i8 (int8_t a,   int8_t b,   const InfyNodeLoc* l); \
  int16_t  infy_##op##_i16(int16_t a,  int16_t b,  const InfyNodeLoc* l); \
  int32_t  infy_##op##_i32(int32_t a,  int32_t b,  const InfyNodeLoc* l); \
  int64_t  infy_##op##_i64(int64_t a,  int64_t b,  const InfyNodeLoc* l);
INFY_ARITH_SET(sub) INFY_ARITH_SET(mul) INFY_ARITH_SET(div) INFY_ARITH_SET(rem)

/* shifts: shift amount >= width is R-0007, not a no-op (§4.12) */
uint64_t infy_shl_u64(uint64_t v, uint32_t n, const InfyNodeLoc* l);
uint64_t infy_shr_u64(uint64_t v, uint32_t n, const InfyNodeLoc* l);
int64_t  infy_shr_i64(int64_t v, uint32_t n, const InfyNodeLoc* l); /* arithmetic (§4.12) */
/* float remainder: x % 0.0 => R-0002, inf % x => R-0008 (§4.12) */
double infy_remf64(double a, double b, const InfyNodeLoc* l);
float  infy_remf32(float a, float b, const InfyNodeLoc* l);
/* explicit non-panicking entries — the only place wrap is allowed (§4.4) */
uint64_t infy_wrapping_add_u64(uint64_t a, uint64_t b);
int64_t  infy_wrapping_add_i64(int64_t a, int64_t b);
uint64_t infy_saturating_add_u64(uint64_t a, uint64_t b);
int64_t  infy_saturating_add_i64(int64_t a, int64_t b);
uint64_t infy_wrapping_shl_u64(uint64_t v, uint32_t n);

/* ---------- str / String / Vec / slice (§4.1, §10.13) ---------- */
int      infy_str_eq_nfc(InfyStr a, InfyStr b);      /* NFC content compare (§4.4) */
int      infy_str_cmp(InfyStr a, InfyStr b);
InfyStr  infy_str_of(const char* utf8_literal);      /* .inf rodata backed, no copy */
InfyString infy_string_new(void);
InfyString infy_string_from(InfyStr s);
InfyString infy_string_concat(InfyString a, InfyStr b);
void     infy_string_push(InfyString* s, uint32_t cp);
void     infy_string_push_str(InfyString* s, InfyStr b);
InfyStr  infy_string_as_str(const InfyString* s);
void     infy_string_drop(InfyString* s);
/* NUL detection before an A-track boundary: R-0003 (§4.9 UB-007) */
void     infy_str_check_nul(InfyStr s, const InfyNodeLoc* l);
/* C string conversion is explicit in source: str.to_cstr() (§7.6.4) */
char*    infy_to_cstr(InfyStr s, const InfyNodeLoc* l);

void     infy_vec_reserve(InfyVec* v, uint64_t n, uint32_t elem_size, uint32_t elem_align);
void     infy_vec_push(InfyVec* v, const void* item, uint32_t elem_size, uint32_t elem_align);
void*    infy_vec_index(const InfyVec* v, uint64_t i, uint32_t elem_size, const InfyNodeLoc* l); /* R-0004 */
const void* infy_slice_index(const InfySlice* s, uint64_t i, uint32_t elem_size, const InfyNodeLoc* l);
void     infy_vec_drop(InfyVec* v, uint32_t elem_size, InfyDropFn elem_drop);
/* §10.13 growth factors are ABI: 4, x2 to 1024, x1.5 to 1 MiB, x1.25 above */
uint64_t infy_vec_new_cap(uint64_t cur, uint64_t need);

/* ---------- ordered collections (§10.13: Map/Set iterate by Ord, never by hash) ---------- */
typedef struct InfyMap InfyMap;      /* sorted by key Ord; iteration order is observable */
typedef struct InfyVecMap InfyVecMap; /* insertion ordered */
typedef struct InfyDeque InfyDeque;
InfyMap* infy_map_new(uint32_t key_size, uint32_t val_size, int (*cmp)(const void*, const void*));
void*    infy_map_get(InfyMap*, const void* key);
void     infy_map_put(InfyMap*, const void* key, const void* val); /* replaces in place */
int      infy_map_len(const InfyMap*);
void     infy_map_iter(const InfyMap*, void (*f)(const void* k, const void* v, void* ctx), void* ctx);
void     infy_map_drop(InfyMap*);

/* ---------- std.fmt (§10.11): the format string is parsed at compile time ---------- */
typedef enum {
    FY_AUTO = 0, FY_DISPLAY, FY_DEBUG, FY_DEBUG_PRETTY, FY_LOWER_HEX, FY_UPPER_HEX,
    FY_BINARY, FY_OCTAL, FY_POINTER, FY_EXP
} InfyFmtWhich;
typedef struct {
    /* literal run between placeholders */
    const char* lit; uint32_t lit_len;
    uint32_t arg_index;              /* "{}"/"{0}" positional; "{name}" resolved by codegen */
    InfyFmtWhich which;
    int32_t width;                   /* -1 = unset */
    int32_t precision;               /* -1 = unset */
    uint8_t fill;                    /* 0 = space */
    uint8_t align;                   /* 0 default, '<' '>' '^' */
    uint8_t alt;                     /* {:#?} / {:#x} */
    uint8_t zero_pad;                /* {:0>10} */
} InfyFmtSpec;
typedef enum { FA_I64, FA_U64, FA_F64, FA_STR, FA_BOOL, FA_CHAR, FA_DEBUG_PTR, FA_I128, FA_U128 } InfyFmtKind;
typedef struct { InfyFmtKind kind; union { int64_t i; uint64_t u; double f; InfyStr s; uint8_t b; uint32_t c; struct { const void* obj; const struct InfyDebugVt* vt; } d; } v; } InfyFmtArg;
typedef struct InfyDebugVt { void (*write)(struct InfyString* out, const void* obj); } InfyDebugVt;
InfyString infy_fmt_run(const InfyFmtSpec* specs, uint32_t n_specs, const InfyFmtArg* args, uint32_t n_args);
/* §6.7 Debug: `Name { f1: v1, f2: v2 }` in declaration order; repr(C) prints `Name(f1, f2)` */
void infy_debug_write_struct(InfyString* out, const char* name, const char* const* fields, const InfyFmtArg* vals, uint32_t n, int tuple_form, int pretty);
/* concrete value writers used by generated Debug/Display impls */
void infy_fmt_write_int(InfyString*, int64_t, const InfyFmtSpec*);
void infy_fmt_write_uint(InfyString*, uint64_t, const InfyFmtSpec*);
void infy_fmt_write_float(InfyString*, double, const InfyFmtSpec*);
void infy_fmt_write_str(InfyString*, InfyStr, const InfyFmtSpec*);
void infy_fmt_write_bool(InfyString*, uint8_t);
void infy_fmt_write_char(InfyString*, uint32_t);
/* xxHash64 — §6.7 fixes the algorithm for Hash derive (changing it would be a MAJOR) */
uint64_t infy_xxhash64(const void* data, uint64_t len, uint64_t seed);
void     infy_hash_feed_u64(uint64_t* state, uint64_t v);

/* ---------- time, rand, env (§4.6, §10.12) ---------- */
uint64_t infy_monotonic_ns(void);
uint64_t infy_monotonic_ms(void);       /* std.env.tick(): ms since process start (§4.6) */
uint64_t infy_unix_seconds(void);
void     infy_sleep_ms(uint64_t ms);
typedef struct { uint64_t state, inc; } InfyRng;   /* SplitMix64 seeding -> PCG32 (§10.12) */
InfyRng  infy_rng_seeded(uint64_t seed);
uint64_t infy_rng_next_u64(InfyRng*);
uint64_t infy_rng_range(InfyRng*, uint64_t lo, uint64_t hi);
/* unbiased Fisher-Yates, tail to head, range(0..=i) — direction is normative (§10.12) */
void     infy_rng_shuffle(InfyRng*, void* base, uint64_t n, uint32_t elem_size);
uint64_t infy_rng_choice_index(InfyRng*, uint64_t n);

/* EnvSnapshot: fixed layout, host refreshes between node dispatches; env.x compiles to an
 * offset load (§4.6). Offsets are published in .inf meta's EnvOffsets table. */
typedef struct {
    uint32_t iter; uint32_t runtime_s; uint64_t realtime; uint32_t ram_mb;
    float cpu_use; float gpu_use; uint32_t chain_id; uint32_t node_idx; uint32_t focus_id;
    InfyStr platform; InfyStr build_id;
} InfyEnvSnapshot;
const InfyEnvSnapshot* infy_env(void);
void infy_env_update(const InfyEnvSnapshot* s);

/* ---------- chains, nodes, focuses, ResourceLock (§8) ---------- */
typedef struct { uint32_t chain_id; uint32_t node_idx; uint32_t focus_id; } InfyFocusId3;
typedef void (*InfyNodeFn)(void* frame);
typedef struct { const char* name; uint32_t n_nodes; uint32_t entry_node; uint32_t frame_size; uint32_t frame_align; const InfyNodeFn* nodes; const InfyNodeLoc* locs; const uint32_t* linkage_counts; } InfyChainDesc;

void     infy_sched_init(uint32_t threads, uint32_t focus_stack_bytes, uint32_t chain_stack_bytes);
uint32_t infy_focus_spawn(uint32_t chain_id, uint32_t start_node, const InfyNodeLoc* at); /* R-0802 on wraparound */
void     infy_focus_pause(uint32_t focus_id);
void     infy_focus_resume(uint32_t focus_id);
void     infy_request_stop(void);            /* graceful: current node's plain+linkage finish (§8.2) */
uint32_t infy_focus_count(uint32_t chain_id);
uint32_t infy_current_focus(void);
uint32_t infy_current_chain(void);
uint32_t infy_current_node(void);
const InfyNodeLoc* infy_current_node_loc(void);
void     infy_yield_now(void);               /* preemption point ③ (§8.5) */
int      infy_sched_run_until_idle(void);    /* returns process exit code */
void     infy_set_pressure(float p);         /* host backpressure (§11.4) */
/* goto: resolved at compile time; the runtime path is only the R-0010 fallback (§4.9 UB-013) */
void     infy_goto_checked(uint32_t chain_id, uint32_t target, const InfyNodeLoc* at);
/* FIFO resource lock (§8.3): Guard drop releases; fairness must not starve */
uint64_t infy_lock_acquire(const char* resource, const InfyNodeLoc* at);
void     infy_lock_release(uint64_t guard);

/* ---------- A-track boundary thunks (§7.6.4) ---------- */
void*    infy_atrack_ptr_in(const void* p, uint64_t len, const InfyNodeLoc* l);   /* R-0720 */
void     infy_atrack_err_out(int32_t err_written, const InfyNodeLoc* l);          /* R-0722 */
void*    infy_arc_cb_box(void* obj, uint64_t* count);
void     infy_arc_cb_free(void* boxed, const InfyNodeLoc* l);                     /* R-0723 */

/* ---------- host protocol (§11) ---------- */
typedef struct InfyArgFrame { const uint8_t* raw; uint32_t len; const struct InfyArgLayout* layout; } InfyArgFrame;
typedef struct InfyOutVal { uint8_t tag; union { int64_t i; uint64_t u; double d; InfyStr s; } v; } InfyOutVal;
typedef struct InfyHostApi {
    void* self;
    const InfyEnvSnapshot* (*env_snapshot)(void*);
    int8_t (*call_native_event)(void*, uint32_t id, const InfyArgFrame*, InfyOutVal*);
    uint64_t (*acquire_resource)(void*, uint32_t res, uint64_t ts);
    void (*release_resource)(void*, uint64_t guard);
    uint32_t (*spawn_focus)(void*, uint32_t chain, uint32_t node);
    void (*request_pause)(void*, uint32_t focus);
    void (*request_stop)(void*);
    void (*set_pressure)(void*, float);
} InfyHostApi;
void infy_host_register(const InfyHostApi* host);           /* before any chain starts (§11.1) */
/* three-step handshake (§11.3): IFABI, event table + signature digests, EnvSnapshot layout */
int  infy_host_handshake(uint32_t ifabi, const uint32_t* event_ids, uint32_t n_events, uint64_t env_layout_hash);
InfyOutVal infy_host_call_event(uint32_t id, const InfyArgFrame* args, const InfyNodeLoc* at); /* R-0900/R-0904 */
/* std.host.* (§10.8): the only entry point for domain-specific data */
uint8_t infy_host_query(InfyStr key, int64_t* out);
uint8_t infy_host_query_str(InfyStr key, InfyString* out);
int8_t  infy_host_set(InfyStr key, InfyStr val);

/* ---------- artifact container (§7.4) ---------- */
typedef struct {
    uint32_t ifabi, opt_level;
    uint8_t overflow_policy, panic_radius, usize_bits, fast_math, lto;
    uint32_t stack_bytes, focus_stack_bytes, focus_quota, iter_guard_steps;
    InfyStr target_triple, build_id, platform, lang_version, source_toolchain;
    uint32_t n_chains, n_events, n_nodes, n_ifbc_modules, n_resource_keys;
    uint32_t off_chains, len_chains, off_events, len_events, off_nodes, len_nodes,
             off_strings, len_strings, off_ifbc, len_ifbc, off_sum, len_sum, off_export, len_export;
} InfyMetaFixed;
/* Locate and validate the trailer of the running executable; used at startup and by
 * `infy artifact info|sections` (which call infy_art_read_file instead). */
int  infy_art_self(InfyMetaFixed* out_meta, uint8_t** blob, uint64_t* blob_len);
int  infy_art_read_file(const char* path, InfyMetaFixed* out_meta, uint8_t** blob, uint64_t* blob_len);
/* Section accessors: string pool entries are (u32 off, u32 len) pairs against the blob. */
InfyStr infy_art_string(const InfyMetaFixed* m, const uint8_t* blob, uint32_t idx);
/* Dump .inf meta as text — the same rendering `infy artifact info` prints. */
void infy_art_dump_meta(const InfyMetaFixed* m, const uint8_t* blob, FILE* out);

/* ---------- IFBC virtual machine (§7.8, appendix D) ---------- */
typedef enum {
    DY_NONE = 0, DY_INT, DY_FLOAT, DY_STR, DY_BYTES, DY_LIST, DY_TUPLE, DY_DICT, DY_BOOL,
    DY_FUNC, DY_CORO, DY_INSTANCE
} InfyDynTag;
typedef struct InfyDyn { InfyDynTag tag; uint32_t rc; union { int64_t i; double f; struct { uint8_t* p; uint64_t n; } s; struct { void* items; uint64_t n, cap; uint32_t esz; } list; struct { struct InfyDyn** keys; struct InfyDyn** vals; uint64_t n, cap; } dict; struct { const struct InfyIfbcFunc* fn; struct InfyDyn** cap; uint32_t ncap; } clo; } v; } InfyDyn;
typedef struct InfyIfbcModule InfyIfbcModule;
/* §7.8.3: all six validations per function, wholly before any instruction executes; the first
 * failure rejects the whole module (appendix D.10). */
int  infy_ifbc_load(const uint8_t* image, uint64_t len, InfyIfbcModule** out, uint8_t all_errors, FILE* report);
int  infy_ifbc_invoke(InfyIfbcModule*, uint32_t func_idx, InfyDyn** args, uint32_t nargs, InfyDyn** ret, uint64_t timeout_ms, const InfyNodeLoc* at); /* R-0701 */
void infy_ifbc_dump(const uint8_t* image, uint64_t len, FILE* out);   /* `infy ifbc dump` */
InfyDyn* infy_dyn_new(InfyDynTag tag);
InfyDyn* infy_dyn_incr(InfyDyn*);
void     infy_dyn_decr(InfyDyn*);
InfyDyn* infy_dyn_int(int64_t); InfyDyn* infy_dyn_float(double); InfyDyn* infy_dyn_str(InfyStr); InfyDyn* infy_dyn_bool(int);
int      infy_dyn_is(InfyDyn*, InfyDynTag);           /* R-0710 on mismatch where normative */
int64_t  infy_dyn_as_i64(InfyDyn*, int* ok);          /* std.embed.Value::as_i64 (§10.7) */
void     infy_dyn_to_json(InfyDyn*, InfyString* out); /* std.embed.Value::to_json */

/* ---------- std.fs / std.process (§10.3, §10.5) ---------- */
typedef enum { IE_NONE = 0, IE_NOTFOUND, IE_DENIED, IE_UNSUPPORTED, IE_PARTIAL, IE_OTHER } InfyIoErrKind;
typedef struct { InfyIoErrKind kind; InfyString message; uint32_t raw_errno; } InfyIoErr;
int        infy_fs_read_all(const char* path, uint8_t** out, uint64_t* len, InfyIoErr* err);
int        infy_fs_write_all(const char* path, const uint8_t* data, uint64_t len, InfyIoErr* err);
int        infy_fs_exists(const char* path);
int        infy_fs_mkdir_p(const char* path, InfyIoErr* err);
typedef struct { uint8_t* buf; uint64_t len, cap; uint8_t readonly; } InfyFileHandle;
int        infy_fs_open(const char* path, const char* mode, InfyFileHandle* out, InfyIoErr* err); /* Drop closes (§10.3) */
void       infy_fs_close(InfyFileHandle* f);
typedef struct { InfyString stdout_bytes, stderr_bytes; int32_t code; } InfyExecResult;
int        infy_process_exec(const char* cmd, const char* const* argv, uint32_t n_argv, uint32_t timeout_ms, InfyExecResult* out, InfyIoErr* err);

/* ---------- module entry / startup order ---------- */
/* Packages initialise in reverse-topological order, files by bytewise name order, within a
 * package in bytewise file order (§5.4): the compiler emits infy_pkg_init_<n>() calls in exactly
 * that sequence from this function's callee list. */
typedef void (*InfyInitFn)(void);
void infy_run_package_inits(const InfyInitFn* fns, uint32_t n);
/* Start the entry chain (§7.4 entry rules) and run the scheduler; returns the process exit code. */
int  infy_runtime_start(uint32_t entry_chain, uint32_t entry_node);
/* the log sink that B-track print() maps to (§7.7.1 item 4) */
void infy_log(uint8_t level, InfyStr msg, const char* origin_lang, uint32_t origin_line);

#ifdef __cplusplus
}
#endif
#endif /* INFY_RT_H */
