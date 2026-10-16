/* infy_rt_internal.h — INFY v2.0 runtime private types and entry points.
 *
 * NOT part of the frozen IFABI-2 contract (`infy_rt.h` is). Everything here is either
 *   (a) a private representation the runtime owns (cleanup deque nodes, map internals,
 *       scheduler structures, artifact `.inf meta` accessors), or
 *   (b) an entry point that `infy_rt.h` forgot to declare but which codegen/tests need
 *       (chain registration, profile setters, test hooks).
 * Group (b) is documented in runtime/README.md and reported as header gaps; adding it to
 * `infy_rt.h` is a contract change for the front end to accept, not something this file may
 * decide silently.
 *
 * Include order: this file assumes `infy_rt.h` has already been included.
 */
#ifndef INFY_RT_INTERNAL_H
#define INFY_RT_INTERNAL_H

#include "infy_rt.h"

#include <stdlib.h>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN 1
#endif
#include <windows.h>
#include <synchapi.h>
#include <process.h>
#include <io.h>
#include <direct.h>
#else
#include <pthread.h>
#include <unistd.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <fcntl.h>
#include <errno.h>
#include <time.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define INFY_UNUSED(x) ((void)(x))

/* ---------------- thread-local storage ---------------- */
#if defined(_MSC_VER) && !defined(__clang__)
#define INFY_TLS __declspec(thread)
#else
#define INFY_TLS _Thread_local
#endif

/* ---------------- platform sync typedefs ----------------
 * Windows: SRWLOCK + CONDITION_VARIABLE (the Win32 push-lock idiom; a ConditionVariable can
 * only sleep on an SRWLOCK). POSIX: pthread mutex + cond. Both are used for the ResourceLock
 * FIFO, the scheduler ready queue and the host/guard tables. */
#ifdef _WIN32
typedef SRWLOCK          InfySrw;
typedef CONDITION_VARIABLE InfyCv;
static inline void infy_mtx_init(InfySrw* m)      { InitializeSRWLock(m); }
static inline void infy_mtx_lock(InfySrw* m)      { AcquireSRWLockExclusive(m); }
static inline void infy_mtx_unlock(InfySrw* m)    { ReleaseSRWLockExclusive(m); }
static inline void infy_mtx_destroy(InfySrw* m)   { INFY_UNUSED(m); }
static inline void infy_cv_init(InfyCv* c)        { InitializeConditionVariable(c); }
static inline void infy_cv_destroy(InfyCv* c)     { INFY_UNUSED(c); }
static inline void infy_cv_wait(InfyCv* c, InfySrw* m) { SleepConditionVariableSRW(c, m, INFINITE, 0); }
static inline void infy_cv_signal(InfyCv* c)      { WakeConditionVariable(c); }
static inline void infy_cv_broadcast(InfyCv* c)   { WakeAllConditionVariable(c); }
#else
typedef pthread_mutex_t  InfySrw;
typedef pthread_cond_t   InfyCv;
static inline void infy_mtx_init(InfySrw* m)      { pthread_mutex_init(m, 0); }
static inline void infy_mtx_lock(InfySrw* m)      { pthread_mutex_lock(m); }
static inline void infy_mtx_unlock(InfySrw* m)    { pthread_mutex_unlock(m); }
static inline void infy_mtx_destroy(InfySrw* m)   { pthread_mutex_destroy(m); }
static inline void infy_cv_init(InfyCv* c)        { pthread_cond_init(c, 0); }
static inline void infy_cv_destroy(InfyCv* c)     { pthread_cond_destroy(c); }
static inline void infy_cv_wait(InfyCv* c, InfySrw* m) { pthread_cond_wait(c, m); }
static inline void infy_cv_signal(InfyCv* c)     { pthread_cond_signal(c); }
static inline void infy_cv_broadcast(InfyCv* c)  { pthread_cond_broadcast(c); }
#endif

/* ---------------- cleanup deque (§4.7.4) ----------------
 * One LIFO deque per focus, heap allocated (a focus may be dispatched on any worker thread, so
 * the deque must not live on a C stack). `is_defer` splits the drain into the two normative
 * phases: variable drops first (reverse declaration order), then defers (reverse declaration
 * order, §3.13). */
typedef struct InfyCleanup {
    InfyDropFn   fn;         /* drop fn, or the defer closure */
    void*        obj;        /* object for a drop, closure env for a defer */
    InfyDeferFn  defer_fn;   /* non-NULL for defer entries (typed path) */
    uint8_t      is_defer;
    uint8_t      is_scope;   /* scope sentinel (see infy_scope_mark) */
    uint8_t      must_drop;  /* `#[must_drop]`: pop_noop is R-0730 (§9.7) */
    struct InfyCleanup* next;
} InfyCleanup;

/* Drain rule implemented by infy_cleanup_drain(): phase 1 = drops LIFO, phase 2 = defers LIFO.
 *
 * Scope markers: `infy_cleanup_pop_run()` runs one entry (as declared in infy_rt.h), which cannot
 * express §3.13 by itself — a `defer` declared between two locals must still run *after both* of
 * their drops. So codegen brackets each scope with infy_scope_mark() / infy_scope_end(), and the
 * scope end runs that scope's drops in reverse declaration order, then its defers in reverse
 * declaration order, and nothing outside the scope. */
void infy_scope_mark(void);
void infy_scope_end(void);      /* run this scope's drops then its defers, then drop the marker */
void infy_scope_end_noop(void); /* abandon the scope without running anything (§4.7.4 #[no_drop]) */
/* Drain one deque head with the two-phase rule (used by the scheduler's unwinding path). */
void infy_rt_cleanup_drain_scope(InfyCleanup** head, uint8_t run_defers);

/* ---------------- profile / policy state ----------------
 * Mirrors the `[profile]` block written into `.inf meta` (§7.2). Default = the strict reading:
 * overflow panics, radius is `focus`, checks are always on (there is no off switch, §7.2). */
typedef struct {
    InfyOverflow   overflow;      /* §4.4 */
    InfyPanicRadius panic_radius; /* §4.16 */
    uint32_t       usize_bits;    /* §2.8 */
    uint32_t       stack_bytes;
    uint32_t       focus_stack_bytes;
    uint32_t       focus_quota;   /* §8.5, default 1 */
    uint64_t       iter_guard_steps; /* §10.10, default 2^32 */
    uint8_t        fast_math;
    uint8_t        ifbc_loaded;   /* a module was accepted by the IFBC loader */
} InfyProfile;

extern InfyProfile infy_profile;

void infy_set_profile_from_meta(const InfyMetaFixed* m);

/* ---------------- panic / unwind plumbing ---------------- */
typedef struct InfyTry {
    jmp_buf          jb;
    struct InfyTry*  prev;
    uint32_t         focus_id, chain_id, node_idx;
    InfyRCode        code;
    char             detail[256];
    uint8_t          caught;
    /* §11.1: host-call state as observed when this try point was installed. A panic that
     * longjmps across an in-flight infy_host_call_event skips that frame's decrement and
     * unlock, so every catch path must roll both back to these snapshots. */
    int32_t          host_depth;
    int32_t          host_lock_depth;
} InfyTry;

extern INFY_TLS InfyTry*      infy_tls_try;
extern INFY_TLS InfyCleanup*  infy_tls_cleanup;
extern INFY_TLS uint8_t       infy_tls_unwinding;   /* drop/defer running -> double panic */

/* Test/scheduler helper: run `fn(ctx)` with a try point installed; 0 = returned normally,
 * 1 = a panic unwound into it (*out_code filled). Not part of the public contract. */
int infy_rt_catch(void (*fn)(void*), void* ctx, InfyRCode* out_code);

/* Observer notified (before rendering) for every R- report. NULL by default. */
typedef void (*InfyReportObserver)(InfyRCode code, const char* detail, uint32_t focus_id,
                                   uint32_t chain_id, const InfyNodeLoc* loc);
void infy_test_set_report_observer(InfyReportObserver obs);
/* Replaces exit(3) on the `process` radius so tests can observe it. Must not return. */
typedef void (*InfyExitHook)(int code);
void infy_test_set_exit_hook(InfyExitHook hook);
/* Drives the §11.1 nesting limit (R-0902) without a re-entrant host callback. */
void infy_test_set_host_nesting(int32_t depth);
/* Read sides of the same two counters: a catch point must leave both at its own snapshot. */
int32_t infy_test_host_nesting(void);
int32_t infy_test_host_lock_depth(void);

/* ---------------- allocation header ----------------
 * §3 of runtime-abi.md: every heap block carries a 16-byte header {size, chain_id, flags}.
 * `pad` records (payload - block_base)/16 so infy_free can recover the platform pointer when a
 * requested alignment larger than 16 pushed the payload past base+16. */
#define INFY_ALLOC_F_NORMAL   0u
#define INFY_ALLOC_F_MUSTDROP 1u
typedef struct {
    uint64_t size;        /* payload size requested */
    uint32_t chain_id;    /* chain that allocated it (§12.4 triple) */
    uint16_t flags;
    uint16_t pad;         /* 16-byte units between the block base and the payload */
} InfyAllocHdr;           /* exactly 16 bytes */

/* Marks the innermost cleanup entry `#[must_drop]`: popping it without running it is R-0730. */
void infy_cleanup_mark_must_drop(void);
/* Radius bookkeeping used by infy_panic (§4.16). */
void infy_sched_mark_failed(uint32_t focus_id, uint32_t code, uint8_t whole_chain);

/* ---------------- ordered collections internals ---------------- */
typedef struct { uint8_t key[1]; /* followed by val, then the next record */ } InfyMapRecHdr;
struct InfyMap {
    uint32_t key_size, val_size;
    int (*cmp)(const void*, const void*);
    uint8_t* data;      /* n records of (key_size + val_size) bytes, ascending by cmp */
    uint64_t n, cap;    /* cap in records */
};
struct InfyVecMap {     /* insertion ordered (§10.13) */
    uint32_t key_size, val_size;
    uint8_t* data;
    uint64_t n, cap;
};
struct InfyDeque {      /* ring buffer of fixed-size items */
    uint32_t item_size;
    uint8_t* data;
    uint64_t n, cap, head;
};

void  infy_deque_init(InfyDeque*, uint32_t item_size);
void  infy_deque_free(InfyDeque*);
void  infy_deque_push_back(InfyDeque*, const void* item);
int   infy_deque_pop_front(InfyDeque*, void* item);   /* 0 when empty */
int   infy_deque_peek_front(const InfyDeque*, void** out_ref);
uint64_t infy_deque_len(const InfyDeque*);

void  infy_vecmap_init(InfyVecMap*, uint32_t key_size, uint32_t val_size);
void* infy_vecmap_get(InfyVecMap*, const void* key);
void  infy_vecmap_put(InfyVecMap*, const void* key, const void* val);
void  infy_vecmap_free(InfyVecMap*);

/* ---------------- scheduler internals (§8) ---------------- */
typedef enum {
    INFY_F_READY = 0,   /* queued for a work item */
    INFY_F_RUNNING,     /* executing a node on a worker */
    INFY_F_PAUSED,      /* `pause`: frame retained, not queued (§8.2) */
    INFY_F_THROTTLED,   /* admitted later because of host backpressure (§11.4) */
    INFY_F_DONE,        /* node_idx ran past the end, or `stop` */
    INFY_F_FAILED       /* a panic terminated it (§4.16) */
} InfyFocusState;

typedef struct InfyFocus {
    uint32_t              id;         /* env.focus_id, monotonically increasing (§8.5) */
    uint32_t              chain_id;
    uint32_t              node_idx;
    uint32_t              seq;        /* creation order, keeps iteration deterministic */
    const InfyChainDesc*  desc;
    void*                 frame;      /* heap: desc->frame_size, desc->frame_align */
    InfyCleanup*          cleanup;
    InfyFocusState        state;
    uint8_t               yield_req;  /* preemption point ③ */
    uint8_t               pause_req;  /* `pause` asked for after this node (§8.2) */
    int64_t               pending_node; /* goto target, -1 when unset (§8.2) */
    uint8_t               quota_left;
    uint64_t              started_ns;
    uint32_t              fail_code;
    struct InfyFocus*     qnext;
} InfyFocus;

typedef struct { const InfyChainDesc* desc; uint32_t n_nodes; } InfyChainSlot;

typedef struct InfyLockNode {
    struct InfyLockNode* next;
    uint64_t guard;
    uint8_t  held;      /* this node currently owns the resource */
    uint8_t  released;  /* guard dropped while still queued (should not happen) */
} InfyLockNode;

typedef struct {
    char        name[128];
    InfySrw     m;
    InfyCv      cv;
    InfyLockNode* head, *tail;
    uint32_t    waiting;   /* queued but not served — used by tests to observe FIFO */
} InfyLockEntry;

typedef struct {
    InfySrw       m;
    InfyCv        cv;
    InfyFocus**   focuses;      /* creation order array; deterministic iteration */
    uint32_t      n_focuses, cap_focuses;
    InfyFocus*    ready_head, *ready_tail;   /* FIFO over ready focuses (§8.5) */
    uint32_t      n_workers, worker_idx, n_workers_live;
    uint32_t      n_runnable;                /* READY entries in the FIFO */
    uint32_t      n_alive;                   /* READY|RUNNING|PAUSED|THROTTLED */
    uint32_t      n_throttled;               /* backpressure queue (§11.4) */
    uint32_t      focus_cap_bytes, chain_cap_bytes;
    uint8_t       stop_requested;            /* §8.2 stop */
    uint8_t       shutdown;
    uint8_t       inited;
    uint32_t      next_focus_id;             /* R-0802 on wraparound */
    float         pressure;                  /* §11.4 */
    uint64_t      dispatch_count;
    uint32_t      n_failed;
} InfySched;

extern InfySched infy_sched;

/* Chain registration. `infy_rt.h` has no entry point for this, yet infy_focus_spawn() and
 * infy_runtime_start() cannot work without the chain mirror table — see README ("header gaps").
 * Chains must be registered in ascending chain_id order: the index in the table *is* the chain id. */
void infy_chain_register(const InfyChainDesc* desc);
void infy_chain_register_table(const InfyChainDesc* const* descs, uint32_t n);
uint32_t infy_chain_count(void);
const InfyChainDesc* infy_chain_desc(uint32_t chain_id);
void infy_sched_reset(void);
void infy_unlock_all_locks_for_reset(void);
void init_mono_base_once(void);
/* host-driven `stop` (§11.1 request_stop) shares the graceful-stop path with std.chain.stop(). */
void infy_host_mark_stop(void);
uint8_t infy_host_stop_requested(void);
/* Test hook: put the focus-id counter anywhere in the u32 range so R-0802 is reachable (§8.5). */
void infy_test_set_focus_seq(uint32_t next_id);
/* ResourceLock table accessors (tests observe queue depth for the FIFO assertion). */
uint32_t infy_lock_waiting(const char* resource);

/* ---------------- artifact container (§7.4) ----------------
 * runtime-abi.md §6 sketches a trailer whose listed fields need ~148 bytes while the same
 * sentence says "fixed 64-byte trailer". The 64-byte size is normative for tooling, so the
 * offsets are u32 (blob-relative for the four inner regions) and the 32-byte build-id lives in
 * `.inf meta`, with its first four bytes mirrored at trailer offset 60. */
#define INFY_ART_TRAILER_SIZE 64u
#define INFY_ART_HDR_VER      2u
/*  0  magic[8] "INFYART2"                                       */
/*  8  hdr_ver u16 | ifabi u16 | seg_count u16 | flags u16       */
/* 16  blob_off u32  | blob_len u32                              */
/* 24  meta_off u32  | meta_len u32  | ifbc_off u32| ifbc_len u32*/
/* 40  sum_off u32   | sum_len u32   | rodata_off  | rodata_len  */
/* 56  trailer_crc u32 (CRC-32 over trailer[0..56))              */
/* 60  build_id_low u32                                          */
typedef struct {
    uint32_t blob_off, blob_len;
    uint32_t meta_off, meta_len;     /* relative to blob_off */
    uint32_t ifbc_off, ifbc_len;
    uint32_t sum_off,  sum_len;
    uint32_t rodata_off, rodata_len;
    uint32_t trailer_crc, build_id_low;
    uint16_t hdr_ver, ifabi, seg_count, flags;
} InfyArtTrailer;

/* `.inf meta` fixed record — written by the front end, parsed by infy_art_read_file().
 * Little-endian, INFY_META_FIXED_BYTES long; the five string fields carry (u32 pool index,
 * u32 reserved) and are resolved against the blob's string pool by meta_resolve_strings(). */
#define INFY_META_MAGIC "INFYMETA"
#define INFY_META_FIXED_BYTES 208u
/* Byte offsets inside the fixed `.inf meta` record (little-endian). This is the layout the
 * front end must emit; runtime/test/tooling share it from here so they can never drift. */
enum {
    INFY_META_OFF_IFABI = 8, INFY_META_OFF_OPT = 12,
    INFY_META_OFF_OVERFLOW = 16, INFY_META_OFF_RADIUS = 17, INFY_META_OFF_USIZE = 18,
    INFY_META_OFF_FASTMATH = 19, INFY_META_OFF_LTO = 20,
    INFY_META_OFF_STACK = 24, INFY_META_OFF_FOCUS_STACK = 28, INFY_META_OFF_QUOTA = 32,
    INFY_META_OFF_ITER_GUARD = 40,
    INFY_META_OFF_N_CHAINS = 48, INFY_META_OFF_N_EVENTS = 52, INFY_META_OFF_N_NODES = 56,
    INFY_META_OFF_N_IFBC = 60, INFY_META_OFF_N_RESKEYS = 64,
    INFY_META_OFF_CHAINS = 72, INFY_META_OFF_EVENTS = 80, INFY_META_OFF_NODES = 88,
    INFY_META_OFF_STRINGS = 96, INFY_META_OFF_IFBC = 104, INFY_META_OFF_SUM = 112,
    INFY_META_OFF_RODATA = 120, INFY_META_OFF_EXPORT = 128,
    INFY_META_OFF_S_TARGET = 136, INFY_META_OFF_S_BUILDID = 144, INFY_META_OFF_S_PLATFORM = 152,
    INFY_META_OFF_S_LANGVER = 160, INFY_META_OFF_S_TOOLCHAIN = 168,
    INFY_META_OFF_BUILDID32 = 176
};

/* host-driven env refresh, called by the scheduler between node dispatches (§4.6) */
void infy_rt_host_env_refresh(void);

uint32_t infy_art_crc32(const uint8_t* p, uint64_t n);
uint32_t infy_art_rd_u32(const uint8_t* p);
uint64_t infy_art_rd_u64(const uint8_t* p);
void     infy_art_wr_u32(uint8_t* p, uint32_t v);
void     infy_art_wr_u64(uint8_t* p, uint64_t v);
int      infy_art_parse_trailer(const uint8_t* file, uint64_t file_len, InfyArtTrailer* out,
                               const char** why);

/* ---------------- host protocol internals (§11) ---------------- */
uint64_t infy_env_layout_hash(void);
void     infy_host_lock(void);
void     infy_host_unlock(void);
int8_t   infy_host_nesting_push(void);   /* returns 0 when the >8 limit is hit -> R-0902 */
void     infy_host_nesting_pop(void);

/* std.host.query / set have no callback slot in `InfyHostApi` (header gap, see README); the
 * runtime keeps a key table that the host fills through this internal entry point. */
typedef enum { INFY_HQ_INT = 0, INFY_HQ_STR = 1 } InfyHostQueryKind;
void infy_host_set_query(const char* key, InfyHostQueryKind kind, int64_t i, InfyStr s);
void infy_host_clear_query(void);
void infy_host_register_events(const uint32_t* ids, uint32_t n, const uint64_t* sigs);
uint8_t infy_host_is_registered(void);
uint8_t infy_host_handshake_done(void);
void infy_host_mark_chains_started(void);
/* host resource callbacks, called outside the serialising host lock (§11.1) */
uint64_t infy_lock_acquire_host_resource(uint32_t res, uint64_t ts);
void infy_release_host_resource(uint64_t guard);

/* Debug vtable writers cannot receive the spec (§10.11 `{:#?}`): generated impls may consult
 * this hint. */
uint8_t infy_debug_pretty_hint(void);

/* ---------------- misc internal helpers ---------------- */
void* infy_rt_zalloc(uint64_t n);
void  infy_rt_write_str(FILE* f, const char* s, uint64_t n);
uint64_t infy_rt_utf8_decode(const uint8_t* p, uint64_t len, uint32_t* cp);
int   infy_rt_nfc_covered(void);   /* 1 while every normalised input stayed in the documented subset */
void  infy_rt_append_cp(InfyString* s, uint32_t cp);

#ifdef __cplusplus
}
#endif
#endif /* INFY_RT_INTERNAL_H */
