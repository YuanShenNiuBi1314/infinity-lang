/* infy_dyn.c — INFY v2.0 B-track dynamic object model (IFABI-2).
 *
 * Implements the `InfyDyn` reference-counted tagged box from runtime/infy_rt.h plus the
 * private constructors the IFBC interpreter (infy_ifbc.c) needs to build list/tuple/dict/
 * bytes/closure/coroutine values. Semantics: §7.7.7 (dyn boundary), §4.7.4 (drop order),
 * §4.8 (address stability: no GC, no compaction), D.6 (ref.inc/dec release at 0 per the
 * drop rules), §10.7 (std.embed.Value::as_i64 / to_json).
 *
 * All allocation goes through infy_alloc / infy_free so the B-track heap shares the single
 * arena the artifact relies on (runtime-abi.md §3). Big-endian host byte order is irrelevant
 * here: InfyDyn is an in-memory box, not serialized.
 *
 * CAPABILITY LIMITS (documented, honest):
 *   - int is a fixed 64-bit two's-complement value (DY_INT). There is NO promotion to an
 *     arbitrary-precision bignum. Python/Lua bignums that exceed i64 overflow to R-0001 at
 *     the arithmetic site; this VM does not grow the integer. (A bignum tag would require a
 *     union member that IFABI-2 does not provide.)
 *   - Reference CYCLES are not collected (§4.8 forbids a tracing GC). A cycle of DY_LIST /
 *     DY_DICT / DY_TUPLE that points back to itself leaks until process exit. The lowering
 *     front end must not create dyn cycles it expects the runtime to reclaim.
 *   - DY_INSTANCE carries no field storage in the frozen union, so an instance box holds only
 *     its tag; it is a placeholder for §4.1 A-track boxed values, not a full object model.
 *   - dict iteration is insertion-ordered (arrays scanned 0..n), which is the normative
 *     observable order for std.embed.Value (§10.13's "never by hash" rule for the dyn map).
 */
#include "infy_rt.h"

#include <stdlib.h>
#include <string.h>

/* infy_alloc / infy_free come from infy_rt.c (real) or test_ifbc_stubs.c (harness). */
extern void* infy_alloc(uint64_t size, uint64_t align);
extern void  infy_free(void* p);

/* ---- coroutine teardown hook ----------------------------------------------------------
 * The coroutine control block (locals snapshot, saved pc, nested frames) is owned by the
 * interpreter, which lives in infy_ifbc.c and whose layout infy_dyn.c must not know. The
 * interpreter installs a release callback at startup; DY_CORO free calls it. This keeps the
 * object model TU free of interpreter internals while still releasing a suspended coroutine. */
void (*infy_dyn_coro_release)(void* ctl) = 0;

/* ---- internal allocation helpers ------------------------------------------------------ */
static void* dz(uint64_t n, uint64_t align) {
    void* p = infy_alloc(n, align);
    if (p) memset(p, 0, (size_t)n);
    return p;
}

/* ============================================================================ constructors */

InfyDyn* infy_dyn_new(InfyDynTag tag) {
    InfyDyn* d = (InfyDyn*)dz(sizeof(InfyDyn), 8);
    if (!d) return 0;
    d->tag = tag;
    d->rc = 1;
    return d;
}

InfyDyn* infy_dyn_int(int64_t v) {
    InfyDyn* d = infy_dyn_new(DY_INT);
    if (d) d->v.i = v;
    return d;
}

InfyDyn* infy_dyn_float(double f) {
    InfyDyn* d = infy_dyn_new(DY_FLOAT);
    if (d) d->v.f = f;
    return d;
}

InfyDyn* infy_dyn_bool(int b) {
    InfyDyn* d = infy_dyn_new(DY_BOOL);
    if (d) d->v.i = b ? 1 : 0;
    return d;
}

InfyDyn* infy_dyn_str(InfyStr s) {
    InfyDyn* d = infy_dyn_new(DY_STR);
    if (!d) return 0;
    d->v.s.n = s.len;
    if (s.len == 0) {
        d->v.s.p = 0;
    } else {
        d->v.s.p = (uint8_t*)infy_alloc(s.len, 1);
        if (!d->v.s.p) { infy_free(d); return 0; }
        memcpy(d->v.s.p, s.ptr, (size_t)s.len);
    }
    return d;
}

/* private: DY_BYTES box from raw bytes (copies). */
InfyDyn* infy_dyn_bytes_new(const uint8_t* p, uint64_t n) {
    InfyDyn* d = infy_dyn_new(DY_BYTES);
    if (!d) return 0;
    d->v.s.n = n;
    if (n == 0) {
        d->v.s.p = 0;
    } else {
        d->v.s.p = (uint8_t*)infy_alloc(n, 1);
        if (!d->v.s.p) { infy_free(d); return 0; }
        memcpy(d->v.s.p, p, (size_t)n);
    }
    return d;
}

/* ---- list / tuple ---------------------------------------------------------------------
 * items is an array of InfyDyn* (esz = sizeof(InfyDyn*)). DY_LIST and DY_TUPLE share this
 * storage; tuple immutability is a *source* rule the verifier/lowering enforces, the box
 * itself is writable so idx.set on a tuple is a documented no-op path. push moves one owned
 * reference into the container (no extra incr). */
static int dyn_seq_reserve(InfyDyn* d, uint64_t need) {
    if (need <= d->v.list.cap) return 1;
    uint64_t ncap = d->v.list.cap ? d->v.list.cap : 4;
    while (ncap < need) ncap *= 2;
    void* items = infy_alloc(ncap * sizeof(InfyDyn*), sizeof(InfyDyn*));
    if (!items) return 0;
    if (d->v.list.items) {
        memcpy(items, d->v.list.items, (size_t)(d->v.list.n * sizeof(InfyDyn*)));
        infy_free(d->v.list.items);
    }
    d->v.list.items = items;
    d->v.list.cap = ncap;
    return 1;
}

InfyDyn* infy_dyn_list_new(void) {
    InfyDyn* d = infy_dyn_new(DY_LIST);
    if (d) d->v.list.esz = (uint32_t)sizeof(InfyDyn*);
    return d;
}

InfyDyn* infy_dyn_tuple_new(void) {
    InfyDyn* d = infy_dyn_new(DY_TUPLE);
    if (d) d->v.list.esz = (uint32_t)sizeof(InfyDyn*);
    return d;
}

int infy_dyn_seq_push(InfyDyn* d, InfyDyn* item) {
    if (!d || (d->tag != DY_LIST && d->tag != DY_TUPLE)) return 0;
    if (!dyn_seq_reserve(d, d->v.list.n + 1)) return 0;
    ((InfyDyn**)d->v.list.items)[d->v.list.n++] = item; /* takes ownership of one ref */
    return 1;
}

InfyDyn* infy_dyn_seq_get(const InfyDyn* d, uint64_t i) {
    if (!d || (d->tag != DY_LIST && d->tag != DY_TUPLE)) return 0;
    if (i >= d->v.list.n) return 0;
    return ((InfyDyn* const*)d->v.list.items)[i]; /* borrowed */
}

int infy_dyn_seq_set(InfyDyn* d, uint64_t i, InfyDyn* item) {
    if (!d || d->tag != DY_LIST) return 0; /* tuples are immutable here */
    if (i >= d->v.list.n) return 0;
    InfyDyn* old = ((InfyDyn**)d->v.list.items)[i];
    ((InfyDyn**)d->v.list.items)[i] = item; /* take new, drop old */
    if (old) infy_dyn_decr(old);
    return 1;
}

uint64_t infy_dyn_len(const InfyDyn* d) {
    if (!d) return 0;
    switch (d->tag) {
        case DY_STR: case DY_BYTES: return d->v.s.n;
        case DY_LIST: case DY_TUPLE: return d->v.list.n;
        case DY_DICT: return d->v.dict.n;
        default: return 0;
    }
}

/* ---- dict (insertion ordered) ---------------------------------------------------------
 * keys/vals are parallel InfyDyn* arrays. set takes ownership of both; on key hit it drops
 * the old value and the redundant new key. get/eq scan linearly (dyn maps are small in B-track). */
static int dyn_dict_reserve(InfyDyn* d, uint64_t need) {
    if (need <= d->v.dict.cap) return 1;
    uint64_t ncap = d->v.dict.cap ? d->v.dict.cap : 4;
    while (ncap < need) ncap *= 2;
    InfyDyn** ks = (InfyDyn**)infy_alloc(ncap * sizeof(InfyDyn*), sizeof(InfyDyn*));
    InfyDyn** vs = (InfyDyn**)infy_alloc(ncap * sizeof(InfyDyn*), sizeof(InfyDyn*));
    if (!ks || !vs) { if (ks) infy_free(ks); if (vs) infy_free(vs); return 0; }
    if (d->v.dict.keys) {
        memcpy(ks, d->v.dict.keys, (size_t)(d->v.dict.n * sizeof(InfyDyn*)));
        memcpy(vs, d->v.dict.vals, (size_t)(d->v.dict.n * sizeof(InfyDyn*)));
        infy_free(d->v.dict.keys);
        infy_free(d->v.dict.vals);
    }
    d->v.dict.keys = ks;
    d->v.dict.vals = vs;
    d->v.dict.cap = ncap;
    return 1;
}

InfyDyn* infy_dyn_dict_new(void) { return infy_dyn_new(DY_DICT); }

int infy_dyn_eq(const InfyDyn* a, const InfyDyn* b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    if (a->tag != b->tag) return 0;
    switch (a->tag) {
        case DY_INT: case DY_BOOL: return a->v.i == b->v.i;
        case DY_FLOAT: return a->v.f == b->v.f;
        case DY_STR: case DY_BYTES:
            if (a->v.s.n != b->v.s.n) return 0;
            if (a->v.s.n == 0) return 1;
            return memcmp(a->v.s.p, b->v.s.p, (size_t)a->v.s.n) == 0;
        default:
            /* lists/dicts/closures: identity compare only (no structural hash defined). */
            return 0;
    }
}

int infy_dyn_dict_set(InfyDyn* d, InfyDyn* k, InfyDyn* v) {
    if (!d || d->tag != DY_DICT) { if (k) infy_dyn_decr(k); if (v) infy_dyn_decr(v); return 0; }
    for (uint64_t i = 0; i < d->v.dict.n; i++) {
        if (infy_dyn_eq(d->v.dict.keys[i], k)) {
            InfyDyn* old = d->v.dict.vals[i];
            d->v.dict.vals[i] = v;            /* replace value in place */
            if (old) infy_dyn_decr(old);
            infy_dyn_decr(k);                 /* drop the redundant key */
            return 1;
        }
    }
    if (!dyn_dict_reserve(d, d->v.dict.n + 1)) { if (k) infy_dyn_decr(k); if (v) infy_dyn_decr(v); return 0; }
    d->v.dict.keys[d->v.dict.n] = k;          /* append at end: preserves insertion order */
    d->v.dict.vals[d->v.dict.n] = v;
    d->v.dict.n++;
    return 1;
}

InfyDyn* infy_dyn_dict_get(InfyDyn* d, InfyDyn* k) {
    if (!d || d->tag != DY_DICT) return 0;
    for (uint64_t i = 0; i < d->v.dict.n; i++)
        if (infy_dyn_eq(d->v.dict.keys[i], k)) return d->v.dict.vals[i]; /* borrowed */
    return 0;
}

/* ---- closure (DY_FUNC) ---------------------------------------------------------------- */
InfyDyn* infy_dyn_func(const struct InfyIfbcFunc* fn, InfyDyn** cap, uint32_t ncap) {
    InfyDyn* d = infy_dyn_new(DY_FUNC);
    if (!d) { if (cap) infy_free(cap); return 0; }
    d->v.clo.fn = fn;
    d->v.clo.ncap = ncap;
    d->v.clo.cap = cap;   /* the caller already holds one ref per entry and hands them over */
    return d;
}

/* ---- coroutine (DY_CORO) --------------------------------------------------------------
 * v.list.items = (CoroCtl*) owned by the interpreter; v.list.esz (u32) = coroutine func idx. */
InfyDyn* infy_dyn_coro(uint32_t func_idx) {
    InfyDyn* d = infy_dyn_new(DY_CORO);
    if (!d) return 0;
    d->v.list.items = 0;
    d->v.list.n = 0;
    d->v.list.cap = 0;
    d->v.list.esz = func_idx;
    return d;
}

uint32_t infy_dyn_coro_func(const InfyDyn* d) {
    return (d && d->tag == DY_CORO) ? d->v.list.esz : 0u;
}
void* infy_dyn_coro_ctl(const InfyDyn* d) {
    return (d && d->tag == DY_CORO) ? d->v.list.items : 0;
}
void infy_dyn_coro_set_ctl(InfyDyn* d, void* ctl) {
    if (d && d->tag == DY_CORO) d->v.list.items = ctl;
}

/* ============================================================================ refcounting */

InfyDyn* infy_dyn_incr(InfyDyn* d) {
    if (d && d->rc != 0xFFFFFFFFu) d->rc++;
    return d;
}

void infy_dyn_decr(InfyDyn* d) {
    if (!d) return;
    if (d->rc > 1) { d->rc--; return; }
    /* rc == 1 -> this is the last strong ref; drop per §4.7.4 order then free. */
    switch (d->tag) {
        case DY_STR:
        case DY_BYTES:
            if (d->v.s.p) infy_free(d->v.s.p);
            break;
        case DY_LIST:
        case DY_TUPLE: {
            InfyDyn** items = (InfyDyn**)d->v.list.items;
            for (uint64_t i = 0; i < d->v.list.n; i++) infy_dyn_decr(items[i]);
            if (items) infy_free(items);
            break;
        }
        case DY_DICT: {
            for (uint64_t i = 0; i < d->v.dict.n; i++) {
                infy_dyn_decr(d->v.dict.keys[i]);
                infy_dyn_decr(d->v.dict.vals[i]);
            }
            if (d->v.dict.keys) infy_free(d->v.dict.keys);
            if (d->v.dict.vals) infy_free(d->v.dict.vals);
            break;
        }
        case DY_FUNC:
            for (uint32_t i = 0; i < d->v.clo.ncap; i++) infy_dyn_decr(d->v.clo.cap[i]);
            if (d->v.clo.cap) infy_free(d->v.clo.cap);
            break;
        case DY_CORO:
            if (d->v.list.items && infy_dyn_coro_release) infy_dyn_coro_release(d->v.list.items);
            break;
        default:
            break; /* DY_NONE/INT/FLOAT/BOOL/INSTANCE hold no owned sub-allocations */
    }
    infy_free(d);
}

/* ============================================================================ predicates */

/* Non-normative predicate: 1 when the tag matches, else 0. R-0710 is raised by the *callers*
 * (the interpreter's cast.dyn / unbox.dyn, and the load-time tag-checkpoint validator), not
 * here, so this function stays total and side-effect free. */
int infy_dyn_is(InfyDyn* d, InfyDynTag tag) {
    return (d && d->tag == tag) ? 1 : 0;
}

int64_t infy_dyn_as_i64(InfyDyn* d, int* ok) {
    if (ok) *ok = 0;
    if (!d) return 0;
    if (d->tag == DY_INT || d->tag == DY_BOOL) { if (ok) *ok = 1; return d->v.i; }
    if (d->tag == DY_FLOAT) { /* explicit lossy narrowing is the caller's choice; report not-ok */
        if (ok) *ok = 0;
        return 0;
    }
    return 0;
}

/* ============================================================================ JSON */

static void j_putc(InfyString* out, char c) { uint32_t cp = (uint32_t)(unsigned char)c; infy_string_push(out, cp); }
static void j_puts(InfyString* out, const char* s) {
    InfyStr t; t.ptr = (const uint8_t*)s; t.len = strlen(s);
    infy_string_push_str(out, t);
}
static void j_quote(InfyString* out, const uint8_t* p, uint64_t n) {
    j_putc(out, '"');
    for (uint64_t i = 0; i < n; i++) {
        unsigned char c = p[i];
        switch (c) {
            case '"':  j_puts(out, "\\\""); break;
            case '\\': j_puts(out, "\\\\"); break;
            case '\b': j_puts(out, "\\b"); break;
            case '\f': j_puts(out, "\\f"); break;
            case '\n': j_puts(out, "\\n"); break;
            case '\r': j_puts(out, "\\r"); break;
            case '\t': j_puts(out, "\\t"); break;
            default:
                if (c < 0x20) {
                    char buf[8];
                    /* deterministic \u00XX escape for control bytes */
                    buf[0] = '\\'; buf[1] = 'u'; buf[2] = '0'; buf[3] = '0';
                    static const char hx[] = "0123456789abcdef";
                    buf[4] = hx[(c >> 4) & 0xF];
                    buf[5] = hx[c & 0xF];
                    buf[6] = 0;
                    j_puts(out, buf);
                } else {
                    j_putc(out, (char)c); /* UTF-8 bytes pass through verbatim */
                }
        }
    }
    j_putc(out, '"');
}
static void j_num_i(InfyString* out, int64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%lld", (long long)v);
    if (n > 0) { InfyStr t; t.ptr = (const uint8_t*)buf; t.len = (uint64_t)n; infy_string_push_str(out, t); }
}
static void j_num_f(InfyString* out, double f) {
    char buf[64];
    int n = snprintf(buf, sizeof buf, "%.17g", f); /* IEEE-754 shortest-repr surrogate */
    if (n > 0) { InfyStr t; t.ptr = (const uint8_t*)buf; t.len = (uint64_t)n; infy_string_push_str(out, t); }
}

void infy_dyn_to_json(InfyDyn* d, InfyString* out) {
    if (!d || !out) return;
    switch (d->tag) {
        case DY_NONE:     j_puts(out, "null"); break;
        case DY_INT:      j_num_i(out, d->v.i); break;
        case DY_BOOL:     j_puts(out, d->v.i ? "true" : "false"); break;
        case DY_FLOAT:    j_num_f(out, d->v.f); break;
        case DY_STR: case DY_BYTES: j_quote(out, d->v.s.p, d->v.s.n); break;
        case DY_LIST: case DY_TUPLE: {
            InfyDyn** items = (InfyDyn**)d->v.list.items;
            j_putc(out, '[');
            for (uint64_t i = 0; i < d->v.list.n; i++) {
                if (i) j_putc(out, ',');
                infy_dyn_to_json(items[i], out);
            }
            j_putc(out, ']');
            break;
        }
        case DY_DICT: {
            j_putc(out, '{');
            for (uint64_t i = 0; i < d->v.dict.n; i++) {
                if (i) j_putc(out, ',');
                /* keys are stringified: strings quoted, scalars rendered then quoted */
                InfyDyn* k = d->v.dict.keys[i];
                if (k && (k->tag == DY_STR || k->tag == DY_BYTES)) j_quote(out, k->v.s.p, k->v.s.n);
                else { j_putc(out, '"'); if (k) { InfyString tmp = infy_string_new(); infy_dyn_to_json(k, &tmp);
                        InfyStr s = infy_string_as_str(&tmp); j_puts(out, ""); infy_string_push_str(out, s); infy_string_drop(&tmp); }
                       j_putc(out, '"'); }
                j_putc(out, ':');
                infy_dyn_to_json(d->v.dict.vals[i], out);
            }
            j_putc(out, '}');
            break;
        }
        case DY_FUNC: case DY_CORO: case DY_INSTANCE: j_puts(out, "null"); break;
        default: j_puts(out, "null"); break;
    }
}
