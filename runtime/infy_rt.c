/* infy_rt.c — INFY v2.0 embedded runtime (IFABI-2).
 *
 * Implementation of the frozen contract in `infy_rt.h` plus the private plumbing declared in
 * `infy_rt_internal.h`. The IFBC VM and the `InfyDyn` helpers (declarations under "IFBC virtual
 * machine" in infy_rt.h) are owned by runtime/infy_ifbc.c and runtime/infy_dyn.c and are NOT
 * implemented here; nothing in this file calls them.
 *
 * Normative sources: spec §4.4/§4.9/§4.12 (numeric semantics, UB ban list), §4.7.4 (drop order),
 * §4.16 (panic radius), §4.6 (env), §7.2 (checks are never off), §7.3/§7.4 (build-id, container),
 * §8.2–§8.5 (chain/goto/lock/focus), §9.2 (byte-exact diagnostics), §10 (std), §11 (host),
 * §12.4 (focus/chain/node triple on every R- report), §13.4 (exit codes),
 * docs/internals/runtime-abi.md (representation choices).
 *
 * C11, libc + platform APIs only. Deterministic: no libc rand(), no time-seeded state, no
 * hash-order-dependent iteration.
 */
#define _GNU_SOURCE 1
#define _POSIX_C_SOURCE 200809L

#include "infy_rt.h"
#include "infy_rt_internal.h"

#include <ctype.h>
#include <math.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#ifndef _WIN32
#include <signal.h>
#include <sys/select.h>
#include <sys/wait.h>
#include <wchar.h>
#endif

/* ======================= global state ======================= */

InfyProfile infy_profile = {
    INFY_OV_PANIC,          /* §4.4 default overflow = "panic" */
    INFY_RADIUS_FOCUS,      /* §4.16 default radius */
    64,                     /* usize width (§2.8), replaced from .inf meta */
    8u * 1024u * 1024u,     /* chain stack 8 MiB (§4.8) */
    256u * 1024u,           /* focus stack 256 KiB (§8.5) */
    1u,                     /* focus_quota, one work item per node (§8.5) */
    0x100000000ULL,         /* iter_guard_steps = 2^32 (§10.10) */
    0, 0
};

INFY_TLS InfyTry*     infy_tls_try     = 0;
INFY_TLS InfyCleanup* infy_tls_cleanup = 0;
INFY_TLS uint8_t      infy_tls_unwinding = 0;

/* Allocation counters (§10.14 std.rt.mem_stats). */
static _Atomic uint64_t g_live_bytes = 0, g_live_objs = 0, g_total_allocs = 0, g_arena_bytes = 0;

/* Current dispatch context; 0 means "outside the scheduler" (test harness, package inits). */
static INFY_TLS uint32_t tls_focus_id = 0;
static INFY_TLS uint32_t tls_chain_id = 0;
static INFY_TLS uint32_t tls_node_idx = 0;
static INFY_TLS const InfyNodeLoc* tls_node_loc = 0;
static INFY_TLS uint8_t tls_has_focus = 0;

/* §4.6: the EnvSnapshot is a fixed-layout struct whose last three fields are per-focus, so
 * `infy_env()` hands out the address of a thread-local copy refreshed at every node dispatch
 * (codegen compiles env.x to an offset load — the address must stay stable within a dispatch). */
static INFY_TLS InfyEnvSnapshot tls_env;
static InfyEnvSnapshot          g_env_host;     /* host-published part */
static uint8_t                  g_env_has_host = 0;
static InfyStr                  g_env_platform = { 0, 0 };
static InfyStr                  g_env_build_id = { 0, 0 };

static InfyReportObserver g_report_obs = 0;
static InfyExitHook       g_exit_hook = 0;

/* rodata-backed literal pool for infy_str_of (§4.1: `str` is a view, not a copy). */
static uint64_t g_monotonic_start_ns = 0;

void infy_test_set_report_observer(InfyReportObserver obs) { g_report_obs = obs; }
void infy_test_set_exit_hook(InfyExitHook hook) { g_exit_hook = hook; }

/* ======================= small utilities ======================= */

static void rt_fputs(FILE* f, const char* s) { if (s) fputs(s, f); }
static void rt_spaces(FILE* f, uint32_t n) { for (uint32_t i = 0; i < n; i++) fputc(' ', f); }
static uint32_t digits_of(uint32_t n) { uint32_t d = 1; while (n >= 10u) { n /= 10u; d++; } return d; }

void* infy_rt_zalloc(uint64_t n) {
    void* p = (n == 0) ? malloc(1) : malloc((size_t)n);
    if (p) memset(p, 0, (size_t)n);
    return p;
}

void infy_rt_write_str(FILE* f, const char* s, uint64_t n) {
    if (!s) return;
    uint64_t off = 0;
    while (off < n) {
        size_t chunk = (n - off) > 0x40000000ull ? 0x40000000ull : (size_t)(n - off);
        size_t w = fwrite(s + off, 1, chunk, f);
        if (w == 0) break;
        off += w;
    }
}

uint32_t infy_art_rd_u32(const uint8_t* p) {
    return (uint32_t)((uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) |
                      ((uint32_t)p[3] << 24));
}
uint64_t infy_art_rd_u64(const uint8_t* p) {
    return (uint64_t)infy_art_rd_u32(p) | ((uint64_t)infy_art_rd_u32(p + 4) << 32);
}
void infy_art_wr_u32(uint8_t* p, uint32_t v) {
    p[0] = (uint8_t)(v); p[1] = (uint8_t)(v >> 8); p[2] = (uint8_t)(v >> 16); p[3] = (uint8_t)(v >> 24);
}
void infy_art_wr_u64(uint8_t* p, uint64_t v) {
    infy_art_wr_u32(p, (uint32_t)v); infy_art_wr_u32(p + 4, (uint32_t)(v >> 32));
}

/* CRC-32 (IEEE 802.3, reflected) — used for the artifact trailer, §7.4. Deterministic, table
 * built once at first use (the table itself is a fixed polynomial expansion, not entropy). */
static uint32_t g_crc_table[256];
static int g_crc_ready = 0;
static void crc_ready(void) {
    for (uint32_t i = 0; i < 256; i++) {
        uint32_t c = i;
        for (int k = 0; k < 8; k++) c = (c & 1u) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
        g_crc_table[i] = c;
    }
    g_crc_ready = 1;
}
uint32_t infy_art_crc32(const uint8_t* p, uint64_t n) {
    if (!g_crc_ready) crc_ready();
    uint32_t c = 0xFFFFFFFFu;
    for (uint64_t i = 0; i < n; i++) c = g_crc_table[(c ^ p[i]) & 0xFFu] ^ (c >> 8);
    return c ^ 0xFFFFFFFFu;
}

/* UTF-8 decode of one code point; returns bytes consumed (0 on a truncated/invalid run, and
 * then *cp is U+FFFD so callers can keep going deterministically). */
uint64_t infy_rt_utf8_decode(const uint8_t* p, uint64_t len, uint32_t* cp) {
    if (len == 0) { *cp = 0xFFFD; return 0; }
    uint8_t b0 = p[0];
    if (b0 < 0x80u) { *cp = b0; return 1; }
    uint32_t need, val;
    if ((b0 & 0xE0u) == 0xC0u) { need = 1; val = b0 & 0x1Fu; }
    else if ((b0 & 0xF0u) == 0xE0u) { need = 2; val = b0 & 0x0Fu; }
    else if ((b0 & 0xF8u) == 0xF0u) { need = 3; val = b0 & 0x07u; }
    else { *cp = 0xFFFD; return 1; }
    if (len < need + 1) { *cp = 0xFFFD; return 1; }
    for (uint32_t i = 0; i < need; i++) {
        uint8_t c = p[1 + i];
        if ((c & 0xC0u) != 0x80u) { *cp = 0xFFFD; return 1 + i; }
        val = (val << 6) | (uint32_t)(c & 0x3Fu);
    }
    static const uint32_t mins[4] = { 0, 0x80u, 0x800u, 0x10000u };
    if (val < mins[need] || val > 0x10FFFFu || (val >= 0xD800u && val <= 0xDFFFu)) { *cp = 0xFFFD; return 1; }
    *cp = val;
    return need + 1;
}

void infy_rt_append_cp(InfyString* s, uint32_t cp) { infy_string_push(s, cp); }

static void append_cp_raw(InfyString* s, uint32_t cp) {
    uint8_t b[4]; uint32_t n;
    if (cp < 0x80u) { b[0] = (uint8_t)cp; n = 1; }
    else if (cp < 0x800u) { b[0] = (uint8_t)(0xC0u | (cp >> 6)); b[1] = (uint8_t)(0x80u | (cp & 0x3Fu)); n = 2; }
    else if (cp < 0x10000u) { b[0] = (uint8_t)(0xE0u | (cp >> 12)); b[1] = (uint8_t)(0x80u | ((cp >> 6) & 0x3Fu));
                              b[2] = (uint8_t)(0x80u | (cp & 0x3Fu)); n = 3; }
    else { b[0] = (uint8_t)(0xF0u | (cp >> 18)); b[1] = (uint8_t)(0x80u | ((cp >> 12) & 0x3Fu));
           b[2] = (uint8_t)(0x80u | ((cp >> 6) & 0x3Fu)); b[3] = (uint8_t)(0x80u | (cp & 0x3Fu)); n = 4; }
    infy_string_push_str(s, (InfyStr){ b, n });
}

/* ======================= R- code table (§9.3, §9.7) =======================
 * Titles are the normative table rows, verbatim; the caret label is the code's short trigger.
 * §4.9/UB-012 forbid inventing codes, and §9.1 forbids inventing levels. */
typedef struct { uint32_t code; const char* title; const char* label; } InfyRRow;
static const InfyRRow r_table[] = {
    { 0x0001, "整数溢出（`overflow=panic`）", "此运算溢出" },
    { 0x0002, "除 0", "除数为 0" },
    { 0x0003, "`str` 运行时检出 NUL", "含 \\0" },
    { 0x0004, "数组越界", "下标越界" },
    { 0x0005, "`Option/Result::unwrap()` 于 None/Err", "unwrap 于 None/Err" },
    { 0x0006, "`bool` 非法位模式", "非法位模式" },
    { 0x0007, "移位量 ≥ 类型宽度", "移位量非法" },
    { 0x0008, "`inf % x`", "无穷取模" },
    { 0x0009, "`ilog2/ilog10` 于 0", "定义域为 0" },
    { 0x0010, "`goto` 运行时目标不存在", "目标节点不存在" },
    { 0x0011, "drop 中 panic（双 panic）", "drop 内 panic" },
    { 0x0012, "运行时 radix 越界", "radix 越界" },
    { 0x0100, "`goto` 运行时目标不存在", "目标节点不存在" },
    { 0x0703, "IFBC 装载校验失败（总类）", "字节码校验失败" },
    { 0x0704, "IFBC 非线性指令流", "指令流非线性" },
    { 0x0705, "IFBC 栈高度不一致", "栈高度不一致" },
    { 0x0706, "IFBC 操作数类型与后缀不符", "操作数类型不符" },
    { 0x0707, "协程指令出现在非协程帧", "帧种类不符" },
    { 0x0708, "`call.host` 签名与 `IFABI` 不符", "宿主签名不符" },
    { 0x0709, "嵌入单元超出 `max_mem_bytes`", "内存预算超限" },
    { 0x070A, "IFBC 装载校验失败（边界）", "边界校验失败" },
    { 0x0710, "`dyn` 标签与声明类型不符（UB-020）", "动态标签不符" },
    { 0x0712, "动态调用目标越界", "目标越界" },
    { 0x0713, "非 `repr(C)` 求字段偏移", "布局不可查询" },
    { 0x0720, "A 轨跨边界指针调用期无效", "指针无效" },
    { 0x0721, "异常跨 `\"C\"` ABI 边界展开", "外部异常" },
    { 0x0722, "A 轨 `Result` 出参未写", "err 出参未写" },
    { 0x0723, "回调 `Arc` 卸载时计数非 0（泄漏）", "回调仍被持有" },
    { 0x0730, "丢弃必释放值", "`#[must_drop]` 被 pop" },
    { 0x0731, "读取已移出的局部", "已移出" },
    { 0x0802, "焦点 ID 回绕", "focus_id 超过 u32" },
    { 0x0803, "无限迭代器消费", "迭代步数超限" },
    { 0x0900, "宿主事件返回未映射错误", "unmapped host error" },
    { 0x0901, "`std.host.query` 键类型与宿主定义不符", "键类型不符" },
    { 0x0902, "宿主回调嵌套过深", "嵌套 > 8 层" },
    { 0x0903, "`IFABI` 不匹配", "产物与宿主 ABI 版本不同" },
    { 0x0904, "宿主事件缺失/签名不符", "宿主未注册该事件" },
    { 0x0905, "`EnvSnapshot` 布局不符", "字段偏移表不一致" },
    { 0x1010, "drop 期间被跨链访问", "drop 中被跨链访问" },
    { 0x1040, "链栈溢出", "栈溢出" }
};

static const InfyRRow* r_lookup(uint32_t code) {
    /* closed set: linear scan over the normative table, deterministic order */
    for (size_t i = 0; i < sizeof(r_table) / sizeof(r_table[0]); i++)
        if (r_table[i].code == code) return &r_table[i];
    return 0;
}

/* ======================= diagnostics (§9.2 byte layout) =======================
 * Geometry is the Rust `src/diag.rs` rule, reproduced exactly:
 *   gw = digits(widest line number) + 1; a numbered gutter is `<line>│`, an unnumbered one is
 *   `spaces(gw-1)│`; content rows are `gutter + ' ' + text`; caret rows are
 *   `spaces(gw-1)│ + spaces(col) + '^' * width + "  " + label`.
 * The runtime usually has no source text: it *tries* to read `<line>` from `loc->file` and only
 * prints `<source unavailable>` when the file cannot be read (§9.2 makes the bytes normative,
 * so the placeholder is an explicitly documented deviation for that case). */

static char* read_source_line(const char* path, uint32_t line) {
    if (!path || !path[0] || line == 0) return 0;
    FILE* f = fopen(path, "rb");
    if (!f) return 0;
    uint32_t cur = 1;
    uint64_t cap = 256, n = 0;
    char* buf = (char*)malloc((size_t)cap);
    if (!buf) { fclose(f); return 0; }
    int c;
    while ((c = fgetc(f)) != EOF) {
        if (c == '\n') {
            if (cur == line) break;
            cur++; n = 0;
            continue;
        }
        if (c == '\r') continue;
        if (cur == line) {
            if (n + 1 >= cap) { cap *= 2; char* nb = (char*)realloc(buf, (size_t)cap); if (!nb) break; buf = nb; }
            buf[n++] = (char)c;
        }
    }
    fclose(f);
    if (cur != line) { free(buf); return 0; }
    buf[n] = 0;
    return buf;
}

void infy_report(InfyRCode code, const char* detail, uint32_t focus_id, uint32_t chain_id,
                 const InfyNodeLoc* loc) {
    InfyNodeLoc zero = { "<unknown>", 0, 0, 0, 0 };
    const InfyNodeLoc l0 = { "<unknown>", 0, 0, 0, 0 };
    if (!loc) loc = &l0;
    if (!loc->file) loc = &zero;

    const InfyRRow* row = r_lookup((uint32_t)code);
    char fallback[64];
    const char* title = row ? row->title : 0;
    const char* label = row ? row->label : "runtime panic";
    if (!title) {
        snprintf(fallback, sizeof fallback, "runtime panic R-%04X", (unsigned)code);
        title = fallback;
    }

    uint32_t line = loc->line ? loc->line : 0;
    uint32_t col = loc->col ? loc->col : 1;
    uint32_t gw = digits_of(line ? line : 1) + 1;
    FILE* out = stderr;

    if (g_report_obs) g_report_obs(code, detail, focus_id, chain_id, loc);

    fprintf(out, "runtime[R-%04X]: %s\n", (unsigned)code, title);
    rt_spaces(out, gw - 1);
    fprintf(out, "\xE2\x94\x8C\xE2\x94\x80 %s:%u:%u\n", loc->file, (unsigned)loc->line, (unsigned)loc->col);
    rt_spaces(out, gw - 1);
    fprintf(out, "\xE2\x94\x82\n");

    char* src = read_source_line(loc->file, line);
    fprintf(out, "%u\xE2\x94\x82 %s\n", (unsigned)(line ? line : 0), src ? src : "<source unavailable>");
    free(src);

    rt_spaces(out, gw - 1);
    fprintf(out, "\xE2\x94\x82");
    rt_spaces(out, col);
    for (int i = 0; i < 1; i++) fputc('^', out);
    fprintf(out, "  %s\n", label);

    /* §12.4: the focus/chain/node triple is mandatory on every R- report. */
    fprintf(out, "  = focus: %u, chain: %u, node: %s:%u:%u (node_idx %u)\n",
            (unsigned)focus_id, (unsigned)chain_id, loc->file, (unsigned)loc->line,
            (unsigned)loc->col, (unsigned)loc->node_idx);
    if (detail && detail[0]) fprintf(out, "  = note: %s\n", detail);
    fflush(out);
    INFY_UNUSED(rt_fputs);
}

/* ======================= cleanup deque (§4.7.4) ======================= */

void infy_cleanup_push(InfyDropFn fn, void* obj, uint8_t is_defer) {
    InfyCleanup* e = (InfyCleanup*)calloc(1, sizeof(InfyCleanup));
    if (!e) { infy_panic(R_IFBC_MEMBUDGET, "cleanup node allocation failed", 0); return; }
    e->fn = fn; e->obj = obj; e->is_defer = is_defer ? 1 : 0;
    e->next = infy_tls_cleanup;
    infy_tls_cleanup = e;
}

/* `#[must_drop]` marking lives on the cleanup entry (there is no separate public API). */
void infy_cleanup_mark_must_drop(void) { if (infy_tls_cleanup) infy_tls_cleanup->must_drop = 1; }

static void run_entry(InfyCleanup* e) {
    if (e->is_defer) {
        if (e->defer_fn) e->defer_fn(e->obj);
        else if (e->fn) e->fn(e->obj);
    } else if (e->fn) {
        e->fn(e->obj);
    }
}

void infy_cleanup_pop_run(void) {
    InfyCleanup* e = infy_tls_cleanup;
    if (!e) return;
    infy_tls_cleanup = e->next;
    /* codegen pops the innermost entry at scope end; drops and defers are already in the exact
     * declaration order it emitted, so the single-entry run needs no reordering (§4.7.4). */
    run_entry(e);
    free(e);
}

void infy_cleanup_pop_noop(void) {
    InfyCleanup* e = infy_tls_cleanup;
    if (!e) return;
    if (e->must_drop) {
        /* §9.7 R-0730: popping a must-drop value without dropping it. */
        e->must_drop = 0;
        infy_panic(R_MUST_DROP, "pop of a #[must_drop] value without drop", infy_current_node_loc());
    }
    infy_tls_cleanup = e->next;
    free(e);   /* ownership moved out (§4.7.4) — deliberately not run */
}

void infy_rt_cleanup_drain_scope(InfyCleanup** head, uint8_t run_defers) {
    InfyCleanup* e = *head;
    *head = 0;
    if (!e) return;
    /* Two normative phases: locals/fields drop in reverse declaration order (LIFO), then the
     * `defer` closures in reverse declaration order (§3.13, §4.7.4). */
    InfyCleanup* drops = 0; InfyCleanup* defers = 0;
    while (e) {
        InfyCleanup* nx = e->next;
        if (e->is_defer && run_defers) { e->next = defers; defers = e; }
        else { e->next = drops; drops = e; }
        e = nx;
    }
    infy_tls_unwinding = 1;
    /* the partition above already reversed push order; running `drops` then `defers` gives
     * declaration-reverse order inside each phase, which is what §4.7.4 requires. */
    while (drops) {
        InfyCleanup* cur = drops; drops = cur->next;
        run_entry(cur); free(cur);
    }
    while (defers) {
        InfyCleanup* cur = defers; defers = cur->next;
        run_entry(cur); free(cur);
    }
    infy_tls_unwinding = 0;
}

void infy_cleanup_drain(void) { infy_rt_cleanup_drain_scope(&infy_tls_cleanup, 1); }

/* ======================= panic + radius (§4.16) ======================= */

static void process_radius_exit(void) {
    /* §4.16 `process`: run every deferred drop, then exit with code 3 (§13.4). */
    infy_cleanup_drain();
    if (g_exit_hook) g_exit_hook(3);
    fflush(stderr); fflush(stdout);
    exit(3);
}

void infy_panic(InfyRCode code, const char* detail, const InfyNodeLoc* loc) {
    InfyNodeLoc tmp = { "<unknown>", 0, 0, 0, 0 };
    if (!loc) loc = &tmp;
    uint32_t f = tls_has_focus ? tls_focus_id : (infy_tls_try ? infy_tls_try->focus_id : 0);
    uint32_t c = tls_has_focus ? tls_chain_id : (infy_tls_try ? infy_tls_try->chain_id : 0);

    if (infy_tls_unwinding) {
        /* §4.16: a panic while unwinding cannot be expressed; switch to the `process` radius
         * and report R-0011. */
        infy_report(R_DOUBLE_PANIC, detail, f, c, loc);
        process_radius_exit();
        return; /* not reached */
    }

    infy_report(code, detail, f, c, loc);

    if (infy_profile.panic_radius == INFY_RADIUS_PROCESS) { process_radius_exit(); return; }

    /* focus / chain radius (§4.16): this focus dies, others keep running. */
    infy_sched_mark_failed(f, (uint32_t)code, infy_profile.panic_radius == INFY_RADIUS_CHAIN);
    infy_cleanup_drain();

    InfyTry* t = infy_tls_try;
    if (t) {
        infy_tls_try = t->prev;
        t->caught = 1;
        t->code = code;
        longjmp(t->jb, 1);
    }
    /* No unwind target: the focus was never dispatched by the scheduler (direct runtime call).
     * Falling through would violate "never silent", so terminate per §13.4 exit code 3. */
    process_radius_exit();
}

int infy_rt_catch(void (*fn)(void*), void* ctx, InfyRCode* out_code) {
    InfyTry t;
    memset(&t, 0, sizeof t);
    t.prev = infy_tls_try;
    t.focus_id = tls_has_focus ? tls_focus_id : 0;
    t.chain_id = tls_has_focus ? tls_chain_id : 0;
    t.node_idx = tls_has_focus ? tls_node_idx : 0;
    infy_tls_try = &t;
    if (setjmp(t.jb) == 0) {
        fn(ctx);
        infy_tls_try = t.prev;
        if (out_code) *out_code = (InfyRCode)0;
        return 0;
    }
    infy_tls_try = t.prev;
    if (out_code) *out_code = t.code;
    return 1;
}

/* ======================= allocation (§4.7.4, §4.8, §10.14) ======================= */

#ifdef _WIN32
static void* plat_aligned_alloc(size_t align, size_t n) { return _aligned_malloc(n, align); }
static void  plat_aligned_free(void* p) { _aligned_free(p); }
#else
static void* plat_aligned_alloc(size_t align, size_t n) {
    void* p = 0;
    if (align < sizeof(void*)) align = sizeof(void*);
    if ((align & (align - 1)) != 0) align = (align + 1) & ~(align - 1);
    if (posix_memalign(&p, align, n) != 0) return 0;
    return p;
}
static void plat_aligned_free(void* p) { free(p); }
#endif

void* infy_alloc(uint64_t size, uint64_t align) {
    if (size == 0) size = 1;                 /* ZSTs are InfyUnit (§4.1): never a zero-sized block */
    if (align == 0 || (align & (align - 1)) != 0) align = 16;
    if (align < 16) align = 16;               /* the 16-byte header sits in front of the payload */
    if (align > 4096 || size > (uint64_t)SIZE_MAX - (16 + align)) {
        infy_panic(R_INT_OVERFLOW, "allocation size overflowed", infy_current_node_loc());
        return 0;
    }
    uint64_t total = 16 + align + size;
    void* base = plat_aligned_alloc((size_t)align, (size_t)total);
    if (!base) { infy_panic(R_IFBC_MEMBUDGET, "out of memory", infy_current_node_loc()); return 0; }
    uint8_t* payload = (uint8_t*)base + 16;
    if (((uintptr_t)payload % (uintptr_t)align) != 0)
        payload = (uint8_t*)(((uintptr_t)payload + (uintptr_t)align - 1) & ~(uintptr_t)(align - 1));
    InfyAllocHdr* h = (InfyAllocHdr*)(payload - 16);
    h->size = size;
    h->chain_id = tls_has_focus ? tls_chain_id : 0;
    h->flags = INFY_ALLOC_F_NORMAL;
    h->pad = (uint16_t)(((uint32_t)(payload - (uint8_t*)base)) / 16u);
    atomic_fetch_add_explicit(&g_live_bytes, size, memory_order_relaxed);
    atomic_fetch_add_explicit(&g_live_objs, 1, memory_order_relaxed);
    atomic_fetch_add_explicit(&g_total_allocs, 1, memory_order_relaxed);
    /* arena_bytes accounts payload + the 16-byte header; alignment slack is not accounted. */
    atomic_fetch_add_explicit(&g_arena_bytes, 16 + size, memory_order_relaxed);
    return payload;
}

void* infy_realloc(void* p, uint64_t old_size, uint64_t new_size, uint64_t align) {
    if (!p) return infy_alloc(new_size, align);
    if (new_size == 0) new_size = 1;
    void* nq = infy_alloc(new_size, align);
    uint64_t copy = old_size < new_size ? old_size : new_size;
    memcpy(nq, p, (size_t)copy);
    infy_free(p);
    return nq;
}

void infy_free(void* p) {
    if (!p) return;
    InfyAllocHdr* h = (InfyAllocHdr*)((uint8_t*)p - 16);
    if (h->size == 0xFFFFFFFFFFFFFFFFull || h->pad > 4096u) return;  /* not a runtime block */
    uint64_t sz = h->size;
    uint64_t total = 16 + sz;   /* must mirror the accounting in infy_alloc (slack excluded) */
    void* base = (void*)((uint8_t*)p - (size_t)h->pad * 16u);
    atomic_fetch_sub_explicit(&g_live_bytes, sz, memory_order_relaxed);
    atomic_fetch_sub_explicit(&g_live_objs, 1, memory_order_relaxed);
    atomic_fetch_sub_explicit(&g_arena_bytes, total, memory_order_relaxed);
    h->size = 0xFFFFFFFFFFFFFFFFull;   /* poison: a second infy_free becomes a no-op, never UB */
    plat_aligned_free(base);
}

InfyMemStats infy_mem_stats(void) {
    InfyMemStats s;
    s.live_bytes = atomic_load_explicit(&g_live_bytes, memory_order_relaxed);
    s.live_objs = atomic_load_explicit(&g_live_objs, memory_order_relaxed);
    s.total_allocs = atomic_load_explicit(&g_total_allocs, memory_order_relaxed);
    s.arena_bytes = atomic_load_explicit(&g_arena_bytes, memory_order_relaxed);
    return s;
}

/* ======================= checked arithmetic (§4.4, §4.9, §4.12) =======================
 * Boundary checks are unconditional — there is no "release mode disables checks" switch (§7.2),
 * and UB-003 forbids silent wraparound. On overflow the module profile decides between
 * R-0001 (default) and saturation (§4.4 `overflow = "saturate"`); "wrap" is not a profile value,
 * wraparound is only reachable through the explicit infy_wrapping_* entries at the bottom. */

static const uint64_t umax_tab[4] = {
    0xFFull, 0xFFFFull, 0xFFFFFFFFull, 0xFFFFFFFFFFFFFFFFull
};
#define MIN64 (-(int64_t)9223372036854775807LL - 1)
#define MAX64 ((int64_t)9223372036854775807LL)

#define SEM_ADD 1
#define SEM_SUB 2
#define SEM_MUL 3
#define SEM_DIV 4
#define SEM_REM 5

static void overflow_report(const char* what, const InfyNodeLoc* l) {
    char buf[128];
    snprintf(buf, sizeof buf, "%s overflowed (§4.4: overflow=\"panic\", §4.9 UB-003)", what);
    infy_panic(R_INT_OVERFLOW, buf, l);
}

/* All unsigned widths funnel through the u64 ring: every intermediate is representable, so no
 * step here is ever UB (§4.9). */
static uint64_t uop(int sem, uint64_t a, uint64_t b, uint64_t mx, const char* what,
                    const InfyNodeLoc* l) {
    switch (sem) {
    case SEM_ADD:
        if (b > mx - a) {
            if (infy_profile.overflow == INFY_OV_SATURATE) return mx;
            overflow_report(what, l);
            return mx;
        }
        return a + b;
    case SEM_SUB:
        if (a < b) {
            if (infy_profile.overflow == INFY_OV_SATURATE) return 0;
            overflow_report(what, l);
            return 0;
        }
        return a - b;
    case SEM_MUL:
        if (a != 0 && b > mx / a) {
            if (infy_profile.overflow == INFY_OV_SATURATE) return mx;
            overflow_report(what, l);
            return mx;
        }
        return a * b;
    case SEM_DIV:
        if (b == 0) { infy_panic(R_DIV_ZERO, "division by zero (§4.12: UB-004)", l); return 0; }
        return a / b;
    default: /* SEM_REM — unsigned remainder is never negative (§4.12) */
        if (b == 0) { infy_panic(R_DIV_ZERO, "remainder by zero (§4.12: UB-004)", l); return 0; }
        return a % b;
    }
}

/* All signed widths funnel through the i64 ring; mn/mx are the concrete type's own bounds, so an
 * i8 result is checked against [-128,127] and not against i64. */
static int64_t iop(int sem, int64_t a, int64_t b, int64_t mn, int64_t mx, const char* what,
                   const InfyNodeLoc* l) {
    switch (sem) {
    case SEM_ADD:
        if ((b > 0 && a > mx - b) || (b < 0 && a < mn - b)) {
            if (infy_profile.overflow == INFY_OV_SATURATE) return (a > 0) ? mx : mn;
            overflow_report(what, l);
            return (a > 0) ? mx : mn;
        }
        return a + b;
    case SEM_SUB:
        if ((b < 0 && a > mx + b) || (b > 0 && a < mn + b)) {
            if (infy_profile.overflow == INFY_OV_SATURATE) return (a > 0) ? mx : mn;
            overflow_report(what, l);
            return (a > 0) ? mx : mn;
        }
        return a - b;
    case SEM_MUL: {
        int over;
        if (a > 0)      over = (b > 0) ? (a > mx / b) : (b != 0 && b < mn / a);
        else if (a < 0) over = (b > 0) ? (a < mn / b) : (b < mx / a);
        else            over = 0;
        if (over) {
            int64_t sat = ((a > 0) == (b > 0)) ? mx : mn;
            if (infy_profile.overflow == INFY_OV_SATURATE) return sat;
            overflow_report(what, l);
            return sat;
        }
        return a * b;
    }
    case SEM_DIV:
    case SEM_REM:
        if (b == 0) {
            infy_panic(R_DIV_ZERO, sem == SEM_DIV ? "division by zero (§4.12: UB-004)"
                                                  : "remainder by zero (§4.12: UB-004)", l);
            return 0;
        }
        if (a == mn && b == -1) {
            /* C leaves this undefined; §4.9 demands a deterministic answer, so this is a
             * normative panic (or saturation), never the two's-complement wrap. */
            if (infy_profile.overflow == INFY_OV_SATURATE) return (sem == SEM_DIV) ? mx : 0;
            overflow_report(what, l);
            return (sem == SEM_DIV) ? mx : 0;
        }
        /* C99 truncation toward zero is exactly §4.12 ("/ 向零截断", `(-7)/2 == -3`), and C99 %
         * then yields the sign of the dividend (`(-7)%2 == -1`). */
        return (sem == SEM_DIV) ? (a / b) : (a % b);
    }
    return 0;
}

#define GEN_U(NAME, TY, IDX, SEM, WHAT)                                                       \
    TY NAME(TY a, TY b, const InfyNodeLoc* l) {                                              \
        return (TY)uop(SEM, (uint64_t)a, (uint64_t)b, umax_tab[IDX], WHAT, l);               \
    }
#define GEN_I(NAME, TY, IDX, SEM, WHAT)                                                       \
    TY NAME(TY a, TY b, const InfyNodeLoc* l) {                                               \
        int64_t mn = -(int64_t)(umax_tab[IDX] / 2u + 1u);                                     \
        int64_t mx = (int64_t)(umax_tab[IDX] / 2u);                                           \
        return (TY)iop(SEM, (int64_t)a, (int64_t)b, mn, mx, WHAT, l);                          \
    }

GEN_U(infy_add_u8, uint8_t, 0, SEM_ADD, "infy_add_u8")     GEN_U(infy_add_u16, uint16_t, 1, SEM_ADD, "infy_add_u16")
GEN_U(infy_add_u32, uint32_t, 2, SEM_ADD, "infy_add_u32")  GEN_U(infy_add_u64, uint64_t, 3, SEM_ADD, "infy_add_u64")
GEN_U(infy_sub_u8, uint8_t, 0, SEM_SUB, "infy_sub_u8")     GEN_U(infy_sub_u16, uint16_t, 1, SEM_SUB, "infy_sub_u16")
GEN_U(infy_sub_u32, uint32_t, 2, SEM_SUB, "infy_sub_u32")  GEN_U(infy_sub_u64, uint64_t, 3, SEM_SUB, "infy_sub_u64")
GEN_U(infy_mul_u8, uint8_t, 0, SEM_MUL, "infy_mul_u8")     GEN_U(infy_mul_u16, uint16_t, 1, SEM_MUL, "infy_mul_u16")
GEN_U(infy_mul_u32, uint32_t, 2, SEM_MUL, "infy_mul_u32")  GEN_U(infy_mul_u64, uint64_t, 3, SEM_MUL, "infy_mul_u64")
GEN_U(infy_div_u8, uint8_t, 0, SEM_DIV, "infy_div_u8")     GEN_U(infy_div_u16, uint16_t, 1, SEM_DIV, "infy_div_u16")
GEN_U(infy_div_u32, uint32_t, 2, SEM_DIV, "infy_div_u32")  GEN_U(infy_div_u64, uint64_t, 3, SEM_DIV, "infy_div_u64")
GEN_U(infy_rem_u8, uint8_t, 0, SEM_REM, "infy_rem_u8")     GEN_U(infy_rem_u16, uint16_t, 1, SEM_REM, "infy_rem_u16")
GEN_U(infy_rem_u32, uint32_t, 2, SEM_REM, "infy_rem_u32")  GEN_U(infy_rem_u64, uint64_t, 3, SEM_REM, "infy_rem_u64")

GEN_I(infy_add_i8, int8_t, 0, SEM_ADD, "infy_add_i8")      GEN_I(infy_add_i16, int16_t, 1, SEM_ADD, "infy_add_i16")
GEN_I(infy_add_i32, int32_t, 2, SEM_ADD, "infy_add_i32")   GEN_I(infy_add_i64, int64_t, 3, SEM_ADD, "infy_add_i64")
GEN_I(infy_sub_i8, int8_t, 0, SEM_SUB, "infy_sub_i8")      GEN_I(infy_sub_i16, int16_t, 1, SEM_SUB, "infy_sub_i16")
GEN_I(infy_sub_i32, int32_t, 2, SEM_SUB, "infy_sub_i32")   GEN_I(infy_sub_i64, int64_t, 3, SEM_SUB, "infy_sub_i64")
GEN_I(infy_mul_i8, int8_t, 0, SEM_MUL, "infy_mul_i8")      GEN_I(infy_mul_i16, int16_t, 1, SEM_MUL, "infy_mul_i16")
GEN_I(infy_mul_i32, int32_t, 2, SEM_MUL, "infy_mul_i32")   GEN_I(infy_mul_i64, int64_t, 3, SEM_MUL, "infy_mul_i64")
GEN_I(infy_div_i8, int8_t, 0, SEM_DIV, "infy_div_i8")      GEN_I(infy_div_i16, int16_t, 1, SEM_DIV, "infy_div_i16")
GEN_I(infy_div_i32, int32_t, 2, SEM_DIV, "infy_div_i32")   GEN_I(infy_div_i64, int64_t, 3, SEM_DIV, "infy_div_i64")
GEN_I(infy_rem_i8, int8_t, 0, SEM_REM, "infy_rem_i8")      GEN_I(infy_rem_i16, int16_t, 1, SEM_REM, "infy_rem_i16")
GEN_I(infy_rem_i32, int32_t, 2, SEM_REM, "infy_rem_i32")   GEN_I(infy_rem_i64, int64_t, 3, SEM_REM, "infy_rem_i64")

/* shifts (§4.12: shift amount >= the type width is R-0007, never a no-op and never UB) */
uint64_t infy_shl_u64(uint64_t v, uint32_t n, const InfyNodeLoc* l) {
    if (n >= 64u) { infy_panic(R_SHIFT_WIDTH, "shift amount >= width (§4.12)", l); return 0; }
    return v << n;
}
uint64_t infy_shr_u64(uint64_t v, uint32_t n, const InfyNodeLoc* l) {
    if (n >= 64u) { infy_panic(R_SHIFT_WIDTH, "shift amount >= width (§4.12)", l); return 0; }
    return v >> n;
}
int64_t infy_shr_i64(int64_t v, uint32_t n, const InfyNodeLoc* l) {
    if (n >= 64u) { infy_panic(R_SHIFT_WIDTH, "shift amount >= width (§4.12)", l); return 0; }
    uint64_t r = (uint64_t)v >> n;                       /* logical on the unsigned view: defined */
    if (v < 0 && n > 0) r |= ~(uint64_t)0 << (64u - n); /* arithmetic / sign-extending (§4.12) */
    return (int64_t)r;
}

/* float remainder (§4.12: `f % 0.0` -> R-0002, `inf % x` -> R-0008, never a silent NaN) */
double infy_remf64(double a, double b, const InfyNodeLoc* l) {
    if (b == 0.0) { infy_panic(R_DIV_ZERO, "float remainder by 0.0 (§4.12)", l); return 0.0; }
    if (isinf(a)) { infy_panic(R_INF_REM, "infinite left operand of % (§4.12)", l); return 0.0; }
    return fmod(a, b);
}
float infy_remf32(float a, float b, const InfyNodeLoc* l) {
    if (b == 0.0f) { infy_panic(R_DIV_ZERO, "float remainder by 0.0 (§4.12)", l); return 0.0f; }
    if (isinf(a)) { infy_panic(R_INF_REM, "infinite left operand of % (§4.12)", l); return 0.0f; }
    return fmodf(a, b);
}

/* the only places wraparound is legal (§4.4: explicit std.num.wrapping_*) */
uint64_t infy_wrapping_add_u64(uint64_t a, uint64_t b) { return a + b; }
int64_t  infy_wrapping_add_i64(int64_t a, int64_t b) {
    return (int64_t)((uint64_t)a + (uint64_t)b);   /* wrap on the unsigned ring, never UB */
}
uint64_t infy_saturating_add_u64(uint64_t a, uint64_t b) {
    return (b > (uint64_t)-1 - a) ? (uint64_t)-1 : a + b;
}
int64_t  infy_saturating_add_i64(int64_t a, int64_t b) {
    if ((b > 0 && a > MAX64 - b) || (b < 0 && a < MIN64 - b)) return (a > 0) ? MAX64 : MIN64;
    return a + b;
}
uint64_t infy_wrapping_shl_u64(uint64_t v, uint32_t n) { return v << (n & 63u); }
/* ======================= str / String / Vec (§4.1, §10.13) ======================= */

InfyStr infy_str_of(const char* utf8_literal) {
    InfyStr s;
    s.ptr = (const uint8_t*)(utf8_literal ? utf8_literal : "");
    s.len = utf8_literal ? (uint64_t)strlen(utf8_literal) : 0;
    return s;
}

/* Ord for `str` is code-point order, which for valid UTF-8 is byte order (§10.13: Map iterates by
 * Ord; the comparison must not depend on hash order or on normalisation). */
int infy_str_cmp(InfyStr a, InfyStr b) {
    uint64_t n = a.len < b.len ? a.len : b.len;
    if (n > 0) {
        int c = memcmp(a.ptr ? a.ptr : (const uint8_t*)"", b.ptr ? b.ptr : (const uint8_t*)"", (size_t)n);
        if (c < 0) return -1;
        if (c > 0) return 1;
    }
    if (a.len == b.len) return 0;
    return a.len < b.len ? -1 : 1;
}

/* ---- Unicode normalisation, NFC (§2.10, §4.4 `==` on `str`) ----
 * A libc-only runtime cannot carry the full Unicode Composition Exclusion + Canonical Decomposition
 * tables, and the spec bans external dependencies. What is implemented here is the algorithmic core
 * (canonical decomposition -> canonical ordering -> canonical composition, UAX#15) with a
 * *documented covered subset*:
 *   - Hangul syllables: fully algorithmic (no table needed), so every modern syllable composes;
 *   - Latin-1 supplement U+00C0..U+00FF (minus U+00D7/U+00F7), Latin Extended-A U+0100..U+017E,
 *     Cyrillic U+0401/U+0451: explicit pair table;
 *   - canonical combining class for U+0300..U+036F (202 for cedilla/ogonek, the "below" band 220,
 *     everything else in the block 230).
 * Outside that subset a code point is passed through unchanged, and infy_rt_nfc_covered() reports
 * whether the input was fully inside the subset, so callers/tests can tell. Two strings that are
 * byte-identical are always equal without any table lookup (§4.4's fast path). */

static const struct { uint32_t composed; uint32_t a, b; } nfc_pairs[] = {
    {0x00C0,0x0041,0x0300},{0x00C1,0x0041,0x0301},{0x00C2,0x0041,0x0302},{0x00C3,0x0041,0x0303},
    {0x00C4,0x0041,0x0308},{0x00C5,0x0041,0x030A},{0x00C7,0x0043,0x0327},
    {0x00C8,0x0045,0x0300},{0x00C9,0x0045,0x0301},{0x00CA,0x0045,0x0302},{0x00CB,0x0045,0x0308},
    {0x00CC,0x0049,0x0300},{0x00CD,0x0049,0x0301},{0x00CE,0x0049,0x0302},{0x00CF,0x0049,0x0308},
    {0x00D1,0x004E,0x0303},{0x00D2,0x004F,0x0300},{0x00D3,0x004F,0x0301},{0x00D4,0x004F,0x0302},
    {0x00D5,0x004F,0x0303},{0x00D6,0x004F,0x0308},{0x00D9,0x0055,0x0300},{0x00DA,0x0055,0x0301},
    {0x00DB,0x0055,0x0302},{0x00DC,0x0055,0x0308},{0x00DD,0x0059,0x0301},
    {0x00E0,0x0061,0x0300},{0x00E1,0x0061,0x0301},{0x00E2,0x0061,0x0302},{0x00E3,0x0061,0x0303},
    {0x00E4,0x0061,0x0308},{0x00E5,0x0061,0x030A},{0x00E7,0x0063,0x0327},
    {0x00E8,0x0065,0x0300},{0x00E9,0x0065,0x0301},{0x00EA,0x0065,0x0302},{0x00EB,0x0065,0x0308},
    {0x00EC,0x0069,0x0300},{0x00ED,0x0069,0x0301},{0x00EE,0x0069,0x0302},{0x00EF,0x0069,0x0308},
    {0x00F1,0x006E,0x0303},{0x00F2,0x006F,0x0300},{0x00F3,0x006F,0x0301},{0x00F4,0x006F,0x0302},
    {0x00F5,0x006F,0x0303},{0x00F6,0x006F,0x0308},{0x00F9,0x0075,0x0300},{0x00FA,0x0075,0x0301},
    {0x00FB,0x0075,0x0302},{0x00FC,0x0075,0x0308},{0x00FD,0x0079,0x0301},{0x00FE,0x006F,0x0303},
    {0x0100,0x0041,0x0304},{0x0101,0x0061,0x0304},{0x0102,0x0041,0x0306},{0x0103,0x0061,0x0306},
    {0x0104,0x0041,0x0315},{0x0105,0x0061,0x0315},{0x0106,0x0043,0x0301},{0x0107,0x0063,0x0301},
    {0x0108,0x0043,0x0302},{0x0109,0x0063,0x0302},{0x010A,0x0043,0x0307},{0x010B,0x0063,0x0307},
    {0x010C,0x0043,0x030C},{0x010D,0x0063,0x030C},{0x010E,0x0044,0x030C},{0x010F,0x0064,0x030C},
    {0x0112,0x0045,0x0304},{0x0113,0x0065,0x0304},{0x0114,0x0045,0x0306},{0x0115,0x0065,0x0306},
    {0x0116,0x0045,0x0307},{0x0117,0x0065,0x0307},{0x0118,0x0045,0x0328},{0x0119,0x0065,0x0328},
    {0x011A,0x0045,0x030C},{0x011B,0x0065,0x030C},{0x011C,0x0047,0x0302},{0x011D,0x0067,0x0302},
    {0x011E,0x0047,0x0306},{0x011F,0x0067,0x0306},{0x0120,0x0047,0x0307},{0x0121,0x0067,0x0307},
    {0x0122,0x0047,0x0327},{0x0123,0x0067,0x0327},{0x0124,0x0048,0x0302},{0x0125,0x0068,0x0302},
    {0x0128,0x0049,0x0303},{0x0129,0x0069,0x0303},{0x012A,0x0049,0x0304},{0x012B,0x0069,0x0304},
    {0x012C,0x0049,0x0306},{0x012D,0x0069,0x0306},{0x012E,0x0049,0x0328},{0x012F,0x0069,0x0328},
    {0x0130,0x0049,0x0307},{0x0134,0x004A,0x0302},{0x0135,0x006A,0x0302},
    {0x0136,0x004B,0x0327},{0x0137,0x006B,0x0327},{0x0139,0x004C,0x0301},{0x013A,0x006C,0x0301},
    {0x013B,0x004C,0x0327},{0x013C,0x006C,0x0327},{0x013D,0x004C,0x030C},{0x013E,0x006C,0x030C},
    {0x0143,0x004E,0x0301},{0x0144,0x006E,0x0301},{0x0145,0x004E,0x0327},{0x0146,0x006E,0x0327},
    {0x0147,0x004E,0x030C},{0x0148,0x006E,0x030C},{0x014C,0x004F,0x0304},{0x014D,0x006F,0x0304},
    {0x0154,0x0052,0x0301},{0x0155,0x0072,0x0301},{0x0156,0x0052,0x0327},{0x0157,0x0072,0x0327},
    {0x0158,0x0052,0x030C},{0x0159,0x0072,0x030C},{0x015A,0x0053,0x0301},{0x015B,0x0073,0x0301},
    {0x015C,0x0053,0x0302},{0x015D,0x0073,0x0302},{0x015E,0x0053,0x0327},{0x015F,0x0073,0x0327},
    {0x0160,0x0053,0x030C},{0x0161,0x0073,0x030C},{0x0162,0x0054,0x0327},{0x0163,0x0074,0x0327},
    {0x0164,0x0054,0x030C},{0x0165,0x0074,0x030C},{0x0168,0x0055,0x0303},{0x0169,0x0075,0x0303},
    {0x016A,0x0055,0x0304},{0x016B,0x0075,0x0304},{0x016C,0x0055,0x0306},{0x016D,0x0075,0x0306},
    {0x016E,0x0055,0x030A},{0x016F,0x0075,0x030A},{0x0172,0x0055,0x0328},{0x0173,0x0075,0x0328},
    {0x0176,0x0059,0x0302},{0x0177,0x0079,0x0302},{0x0178,0x0059,0x0308},{0x0179,0x005A,0x0301},
    {0x017A,0x007A,0x0301},{0x017B,0x005A,0x0307},{0x017C,0x007A,0x0307},{0x017D,0x005A,0x030C},
    {0x017E,0x007A,0x030C},{0x0401,0x0415,0x0308},{0x0451,0x0435,0x0308}
};

static uint32_t nfc_ccc(uint32_t cp) {
    if (cp < 0x0300u || cp > 0x036Fu) return 0;
    if (cp == 0x0327u || cp == 0x0328u) return 202;        /* cedilla, ogonek */
    if (cp >= 0x0316u && cp <= 0x0333u) return 220;        /* the "attached below" band */
    return 230;                                            /* attached above */
}

/* one-step canonical decomposition; returns the number of code points written (1 = unchanged) */
static uint32_t nfc_decomp_one(uint32_t cp, uint32_t* out) {
    if (cp >= 0xAC00u && cp <= 0xD7A3u) {                  /* Hangul: algorithmic (§2.10) */
        uint32_t s = cp - 0xAC00u;
        uint32_t l = 0x1100u + s / (21u * 28u);
        uint32_t v = 0x1161u + (s % (21u * 28u)) / 28u;
        uint32_t t = s % 28u;
        out[0] = l; out[1] = v;
        if (t == 0) return 2;
        out[2] = 0x11A7u + t;
        return 3;
    }
    for (size_t i = 0; i < sizeof(nfc_pairs)/sizeof(nfc_pairs[0]); i++) {
        if (nfc_pairs[i].composed == cp) { out[0] = nfc_pairs[i].a; out[1] = nfc_pairs[i].b; return 2; }
    }
    out[0] = cp;
    return 1;
}

static uint32_t nfc_compose(uint32_t a, uint32_t b) {
    /* Hangul composition first (algorithmic), then the pair table. */
    if (a >= 0x1100u && a <= 0x11FFu && b >= 0x1161u && b <= 0x11A5u) {
        if (a <= 0x1112u) {
            uint32_t li = a - 0x1100u, vi = b - 0x1161u;
            if (li < 21u && vi < 21u) return 0xAC00u + (li * 21u + vi) * 28u;
        }
        return 0;
    }
    if (a >= 0xAC00u && a <= 0xD7A3u && b >= 0x11A8u && b <= 0x11C2u) {
        uint32_t s = a - 0xAC00u;
        if (s % 28u == 0) return a + (b - 0x11A7u);
        return 0;
    }
    for (size_t i = 0; i < sizeof(nfc_pairs)/sizeof(nfc_pairs[0]); i++)
        if (nfc_pairs[i].a == a && nfc_pairs[i].b == b) return nfc_pairs[i].composed;
    return 0;
}

static int g_nfc_covered = 1;   /* set by the last normalisation: 0 = outside the documented subset */
int infy_rt_nfc_covered(void) { return g_nfc_covered; }

/* Returns a malloc'd code-point array; *covered is 0 when some code point fell outside the
 * documented subset (it is still normalised as far as the tables allow, deterministically). */
static uint32_t* nfc_normalize(InfyStr in, uint64_t* out_len, int* covered) {
    uint64_t cap = in.len + 8, n = 0;
    uint32_t* buf = (uint32_t*)malloc((size_t)cap * sizeof(uint32_t));
    if (!buf) { *out_len = 0; if (covered) *covered = 0; return 0; }
    uint64_t off = 0;
    int cov = 1;
    while (off < in.len) {
        uint32_t cp;
        uint64_t adv = infy_rt_utf8_decode(in.ptr + off, in.len - off, &cp);
        if (adv == 0) { cov = 0; break; }
        off += adv;
        uint32_t tmp[3];
        uint32_t k = nfc_decomp_one(cp, tmp);
        if (cp > 0x10FFFFu) cov = 0;
        for (uint32_t i = 0; i < k; i++) {
            if (n + 1 >= cap) { cap *= 2; uint32_t* nb = (uint32_t*)realloc(buf, (size_t)cap * sizeof(uint32_t)); if (!nb) { free(buf); *out_len = 0; if (covered) *covered = cov; return 0; } buf = nb; }
            buf[n++] = tmp[i];
        }
    }
    /* canonical ordering: stable sort of combining-mark runs by ccc */
    for (uint64_t i = 1; i < n; i++) {
        uint32_t c = buf[i], cc = nfc_ccc(c);
        if (cc == 0) continue;
        uint64_t j = i;
        while (j > 0) {
            uint32_t pc = nfc_ccc(buf[j - 1]);
            if (pc == 0 || pc <= cc) break;
            buf[j] = buf[j - 1];
            j--;
        }
        buf[j] = c;
    }
    /* canonical composition */
    uint64_t m = 0;
    for (uint64_t i = 0; i < n; i++) {
        uint32_t c = buf[i];
        if (m > 0) {
            uint32_t prev = buf[m - 1];
            if (nfc_ccc(prev) == 0 && nfc_ccc(c) != 0) {
                uint32_t comp = nfc_compose(prev, c);
                if (comp) { buf[m - 1] = comp; continue; }
            }
        }
        buf[m++] = c;
    }
    *out_len = m;
    if (covered) *covered = cov;
    return buf;
}

int infy_str_eq_nfc(InfyStr a, InfyStr b) {
    if (a.len == b.len && (a.len == 0 || (a.ptr && b.ptr && memcmp(a.ptr, b.ptr, (size_t)a.len) == 0)))
        return 1;                        /* §4.4 fast path: identical bytes are NFC-equal */
    uint64_t la = 0, lb = 0;
    int ca = 1, cb = 1;
    uint32_t* na = nfc_normalize(a, &la, &ca);
    uint32_t* nb = nfc_normalize(b, &lb, &cb);
    int eq = 0;
    if (na && nb && la == lb) {
        eq = (la == 0) || (memcmp(na, nb, (size_t)la * sizeof(uint32_t)) == 0);
    }
    g_nfc_covered = ca && cb;
    free(na); free(nb);
    return eq ? 1 : 0;
}

void infy_str_check_nul(InfyStr s, const InfyNodeLoc* l) {
    for (uint64_t i = 0; i < s.len; i++) {
        if (s.ptr[i] == 0u) {
            /* §4.9 UB-007: NUL crossing an A-track boundary is a normative panic. */
            infy_panic(R_NUL_IN_STR, "str contains \\0 at an A-track boundary (§4.9 UB-007)", l);
            return;
        }
    }
}

char* infy_to_cstr(InfyStr s, const InfyNodeLoc* l) {
    infy_str_check_nul(s, l);
    char* buf = (char*)infy_alloc(s.len + 1, 1);
    if (!buf) return 0;
    if (s.len) memcpy(buf, s.ptr, (size_t)s.len);
    buf[s.len] = 0;
    return buf;
}

InfyString infy_string_new(void) {
    InfyString s = { 0, 0, 0 };
    return s;
}

InfyStr infy_string_as_str(const InfyString* s) {
    InfyStr r;
    r.ptr = s->ptr;
    r.len = s->len;
    return r;
}

void infy_string_drop(InfyString* s) {
    if (!s) return;
    infy_free(s->ptr);
    s->ptr = 0; s->len = 0; s->cap = 0;
}

InfyString infy_string_from(InfyStr src) {
    InfyString s = infy_string_new();
    if (src.len) {
        uint64_t cap = infy_vec_new_cap(0, src.len);
        s.ptr = (uint8_t*)infy_alloc(cap, 1);
        s.cap = cap; s.len = src.len;
        memcpy(s.ptr, src.ptr, (size_t)src.len);
    }
    return s;
}

InfyString infy_string_concat(InfyString a, InfyStr b) {
    InfyString r = a;
    infy_string_push_str(&r, b);
    return r;
}

static void string_reserve(InfyString* s, uint64_t extra) {
    uint64_t need = s->len + extra;
    if (need <= s->cap) return;
    if (need < 4) need = 4;
    uint64_t nc = infy_vec_new_cap(s->cap, need);
    uint8_t* np;
    if (s->ptr == 0) { np = (uint8_t*)infy_alloc(nc, 1); np[0] = 0; }
    else np = (uint8_t*)infy_realloc(s->ptr, s->cap, nc, 1);
    s->ptr = np;
    s->cap = nc;
}

void infy_string_push(InfyString* s, uint32_t cp) {
    if (cp == 0u) {
        /* A U+0000 inside a `str`/`String` is R-0003 (§4.9 UB-007). */
        infy_panic(R_NUL_IN_STR, "String::push of U+0000 (§4.9 UB-007)", infy_current_node_loc());
        return;
    }
    if (cp > 0x10FFFFu || (cp >= 0xD800u && cp <= 0xDFFFu)) {
        /* §4.1 defines `char` as a Unicode scalar value; any other bit pattern is an illegal
         * representation, and R-0006 is the normative "illegal representation" panic. */
        infy_panic(R_BAD_BOOL, "char value is not a Unicode scalar value (§4.1)",
                   infy_current_node_loc());
        return;
    }
    string_reserve(s, 4);
    append_cp_raw(s, cp);
}

void infy_string_push_str(InfyString* s, InfyStr b) {
    if (b.len == 0) return;
    string_reserve(s, b.len);
    memcpy(s->ptr + s->len, b.ptr, (size_t)b.len);
    s->len += b.len;
}

/* ---- Vec / slice (§10.13, §4.9 UB-008) ----
 * Growth factors are ABI: start at 4, x2 while the new capacity is <= 1024, x3/2 while <= 1 MiB,
 * x5/4 above (docs/internals/runtime-abi.md §1, spec §10.13). Growth never shrinks. */
uint64_t infy_vec_new_cap(uint64_t cur, uint64_t need) {
    static const uint64_t kStep2Limit = 1024ull;
    static const uint64_t kStep32Limit = 1024ull * 1024ull;
    if (need <= cur) return cur;
    uint64_t c = cur;
    if (c == 0) c = 4;                       /* first allocation starts at 4 */
    while (c < need) {
        uint64_t next;
        uint64_t c2 = (c > (uint64_t)-1 / 2u) ? (uint64_t)-1 : c * 2u;
        uint64_t c32 = (c > (uint64_t)-1 / 3u) ? (uint64_t)-1 : (c * 3u) / 2u;
        uint64_t c54 = (c > (uint64_t)-1 / 5u) ? (uint64_t)-1 : (c * 5u) / 4u;
        if (c2 <= kStep2Limit && c2 > c) next = c2;                 /* x2 band */
        else if (c32 <= kStep32Limit && c32 > c) next = c32;         /* x3/2 band */
        else if (c54 > c) next = c54;                                /* x5/4 band */
        else if (c < need) next = c + 1;                              /* ensure progress */
        else return c;
        if (next <= c) return c;
        c = next;
        if (c == (uint64_t)-1) break;
    }
    return c;
}

void infy_vec_reserve(InfyVec* v, uint64_t n, uint32_t elem_size, uint32_t elem_align) {
    if (elem_size == 0) elem_size = 1;
    uint64_t need = v->len + n;
    if (need <= v->cap) return;
    uint64_t nc = infy_vec_new_cap(v->cap, need);
    uint64_t bytes;
    if (elem_size && nc > (uint64_t)-1 / elem_size) {
        infy_panic(R_INT_OVERFLOW, "Vec capacity byte size overflowed", infy_current_node_loc());
        return;
    }
    bytes = nc * elem_size;
    if (v->ptr == 0) {
        v->ptr = infy_alloc(bytes, elem_align ? elem_align : 1);
    } else {
        v->ptr = infy_realloc(v->ptr, v->cap * elem_size, bytes, elem_align ? elem_align : 1);
    }
    memset((uint8_t*)v->ptr + (size_t)(v->cap * elem_size), 0, (size_t)(bytes - v->cap * elem_size));
    v->cap = nc;
}

void infy_vec_push(InfyVec* v, const void* item, uint32_t elem_size, uint32_t elem_align) {
    if (v->len + 1 > v->cap) infy_vec_reserve(v, 1, elem_size, elem_align);
    if (item && elem_size) memcpy((uint8_t*)v->ptr + (size_t)v->len * elem_size, item, (size_t)elem_size);
    v->len++;
}

void* infy_vec_index(const InfyVec* v, uint64_t i, uint32_t elem_size, const InfyNodeLoc* l) {
    if (i >= v->len) {
        char buf[96];
        snprintf(buf, sizeof buf, "index %llu out of bounds for length %llu (§4.9 UB-008)",
                 (unsigned long long)i, (unsigned long long)v->len);
        infy_panic(R_INDEX_OOB, buf, l);
        return 0;
    }
    return (uint8_t*)v->ptr + (size_t)i * elem_size;
}

const void* infy_slice_index(const InfySlice* s, uint64_t i, uint32_t elem_size, const InfyNodeLoc* l) {
    if (i >= s->len) {
        char buf[96];
        snprintf(buf, sizeof buf, "index %llu out of bounds for slice length %llu (§4.9 UB-008)",
                 (unsigned long long)i, (unsigned long long)s->len);
        infy_panic(R_INDEX_OOB, buf, l);
        return 0;
    }
    return (const uint8_t*)s->ptr + (size_t)i * elem_size;
}

void infy_vec_drop(InfyVec* v, uint32_t elem_size, InfyDropFn elem_drop) {
    if (elem_drop && elem_size) {
        /* elements drop in ascending index order (declaration order of the sequence) */
        for (uint64_t i = 0; i < v->len; i++) elem_drop((uint8_t*)v->ptr + (size_t)i * elem_size);
    }
    infy_free(v->ptr);
    v->ptr = 0; v->len = 0; v->cap = 0;
}

/* ======================= ordered collections (§10.13) =======================
 * `Map` is a sorted dense array: iteration is strictly by `Ord` (never hash order), insertion is
 * O(n) but the observable order is exactly the key order. `VecMap` keeps insertion order. */

InfyMap* infy_map_new(uint32_t key_size, uint32_t val_size, int (*cmp)(const void*, const void*)) {
    InfyMap* m = (InfyMap*)calloc(1, sizeof(InfyMap));
    if (!m) return 0;
    m->key_size = key_size ? key_size : 1;
    m->val_size = val_size ? val_size : 1;
    m->cmp = cmp;
    return m;
}

static uint8_t* map_key(InfyMap* m, uint64_t i) { return m->data + i * (m->key_size + m->val_size); }
static uint8_t* map_val(InfyMap* m, uint64_t i) { return map_key(m, i) + m->key_size; }

static int64_t map_find(const InfyMap* m, const void* key) {
    int64_t lo = 0, hi = (int64_t)m->n - 1;
    while (lo <= hi) {
        int64_t mid = lo + (hi - lo) / 2;
        int c = m->cmp(map_key((InfyMap*)m, (uint64_t)mid), key);
        if (c == 0) return mid;
        if (c < 0) lo = mid + 1; else hi = mid - 1;
    }
    return -(lo + 1);
}

void* infy_map_get(InfyMap* m, const void* key) {
    if (!m || !m->n) return 0;
    int64_t i = map_find(m, key);
    if (i < 0) return 0;
    return map_val(m, (uint64_t)i);
}

void infy_map_put(InfyMap* m, const void* key, const void* val) {
    if (!m) return;
    int64_t i = map_find(m, key);
    if (i >= 0) {                                /* replaces in place, order untouched */
        memcpy(map_val(m, (uint64_t)i), val, (size_t)m->val_size);
        return;
    }
    uint64_t at = (uint64_t)(-(i + 1));
    if (m->n + 1 > m->cap) {
        uint64_t nc = infy_vec_new_cap(m->cap, m->n + 1);
        size_t rec = (size_t)(m->key_size + m->val_size);
        uint8_t* nd = (uint8_t*)infy_realloc(m->data, m->cap * rec, nc * rec, 8);
        if (m->data == 0) memset(nd, 0, (size_t)nc * rec);
        m->data = nd; m->cap = nc;
    }
    memmove(map_key(m, at + 1), map_key(m, at), (size_t)(m->n - at) * (m->key_size + m->val_size));
    memcpy(map_key(m, at), key, (size_t)m->key_size);
    memcpy(map_val(m, at), val, (size_t)m->val_size);
    m->n++;
}

int infy_map_len(const InfyMap* m) { return m ? (int)(uint64_t)m->n : 0; }

void infy_map_iter(const InfyMap* m, void (*f)(const void* k, const void* v, void* ctx), void* ctx) {
    if (!m || !f) return;
    for (uint64_t i = 0; i < m->n; i++) f(map_key((InfyMap*)m, i), map_val((InfyMap*)m, i), ctx);
}

void infy_map_drop(InfyMap* m) {
    if (!m) return;
    infy_free(m->data);
    free(m);
}

void infy_vecmap_init(InfyVecMap* m, uint32_t key_size, uint32_t val_size) {
    m->key_size = key_size ? key_size : 1;
    m->val_size = val_size ? val_size : 1;
    m->data = 0; m->n = 0; m->cap = 0;
}
static uint8_t* vm_rec(InfyVecMap* m, uint64_t i) {
    return m->data + i * (m->key_size + m->val_size);
}
void* infy_vecmap_get(InfyVecMap* m, const void* key) {
    for (uint64_t i = 0; i < m->n; i++)
        if (memcmp(vm_rec(m, i), key, (size_t)m->key_size) == 0) return vm_rec(m, i) + m->key_size;
    return 0;
}
void infy_vecmap_put(InfyVecMap* m, const void* key, const void* val) {
    void* ex = infy_vecmap_get(m, key);
    if (ex) { memcpy(ex, val, (size_t)m->val_size); return; }
    if (m->n + 1 > m->cap) {
        uint64_t nc = infy_vec_new_cap(m->cap, m->n + 1);
        size_t rec = (size_t)(m->key_size + m->val_size);
        m->data = (uint8_t*)infy_realloc(m->data, m->cap * rec, nc * rec, 8);
        m->cap = nc;
    }
    memcpy(vm_rec(m, m->n), key, (size_t)m->key_size);
    memcpy(vm_rec(m, m->n) + m->key_size, val, (size_t)m->val_size);
    m->n++;
}
void infy_vecmap_free(InfyVecMap* m) { infy_free(m->data); m->data = 0; m->n = m->cap = 0; }

void infy_deque_init(InfyDeque* d, uint32_t item_size) {
    d->item_size = item_size ? item_size : 1; d->data = 0; d->n = d->cap = d->head = 0;
}
static void deque_grow(InfyDeque* d) {
    uint64_t nc = infy_vec_new_cap(d->cap, d->cap + 1);
    uint8_t* nd = (uint8_t*)infy_alloc(nc * d->item_size, 8);
    for (uint64_t i = 0; i < d->n; i++)
        memcpy(nd + i * d->item_size,
               d->data + ((d->head + i) % d->cap) * d->item_size, d->item_size);
    infy_free(d->data);
    d->data = nd; d->cap = nc; d->head = 0;
}
void infy_deque_push_back(InfyDeque* d, const void* item) {
    if (d->n + 1 > d->cap) deque_grow(d);
    memcpy(d->data + ((d->head + d->n) % d->cap) * d->item_size, item, d->item_size);
    d->n++;
}
int infy_deque_pop_front(InfyDeque* d, void* item) {
    if (d->n == 0) return 0;
    if (item) memcpy(item, d->data + d->head * d->item_size, d->item_size);
    d->head = (d->head + 1) % d->cap;
    d->n--;
    return 1;
}
int infy_deque_peek_front(const InfyDeque* d, void** out_ref) {
    if (d->n == 0) return 0;
    *out_ref = d->data + (d->head % d->cap) * d->item_size;
    return 1;
}
uint64_t infy_deque_len(const InfyDeque* d) { return d ? d->n : 0; }
void infy_deque_free(InfyDeque* d) { infy_free(d->data); d->data = 0; d->n = d->cap = d->head = 0; }
/* ======================= xxHash64 (§6.7 Hash derive) =======================
 * Algorithm fixed by §6.7 (changing it would be a MAJOR). Always little-endian reads, so the
 * digest is target-independent (§12.3 golden files). Verified against the published vectors
 * xxh64("")=0xEF46DB3751D8F993, xxh64("xxhash")=0x32DD38952C4BC720 and
 * xxh64_seed(20141025,"xxhash")=0xB559B98D844E0635 (see runtime/test_rt.c). */
#define XXH_P1 0x9E3779B185EBCA87ull
#define XXH_P2 0xC2B2AE3D27D4EB4Full
#define XXH_P3 0x165667B19E3779F9ull
#define XXH_P4 0x85EBCA77C2B2AE63ull
#define XXH_P5 0x27D4EB2F165667C5ull

static uint64_t rol64(uint64_t x, int n) { return (x << n) | (x >> (64 - n)); }
static uint64_t xxh_round(uint64_t acc, uint64_t lane) {
    acc += lane * XXH_P2; acc = rol64(acc, 31); acc *= XXH_P1; return acc;
}
static uint64_t xxh_merge(uint64_t acc, uint64_t val) {
    acc ^= xxh_round(0, val); acc = acc * XXH_P1 + XXH_P4; return acc;
}
static uint64_t rd_le64(const uint8_t* p) {
    return (uint64_t)p[0] | ((uint64_t)p[1] << 8) | ((uint64_t)p[2] << 16) | ((uint64_t)p[3] << 24) |
           ((uint64_t)p[4] << 32) | ((uint64_t)p[5] << 40) | ((uint64_t)p[6] << 48) | ((uint64_t)p[7] << 56);
}
static uint32_t rd_le32(const uint8_t* p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

uint64_t infy_xxhash64(const void* data, uint64_t len, uint64_t seed) {
    const uint8_t* p = (const uint8_t*)data;
    const uint8_t* end = p + len;
    uint64_t h;
    if (len >= 32u) {
        const uint8_t* limit = end - 32u;
        uint64_t v1 = seed + XXH_P1 + XXH_P2, v2 = seed + XXH_P2, v3 = seed, v4 = seed - XXH_P1;
        do {
            v1 = xxh_round(v1, rd_le64(p)); p += 8;
            v2 = xxh_round(v2, rd_le64(p)); p += 8;
            v3 = xxh_round(v3, rd_le64(p)); p += 8;
            v4 = xxh_round(v4, rd_le64(p)); p += 8;
        } while (p <= limit);
        h = rol64(v1, 1) + rol64(v2, 7) + rol64(v3, 12) + rol64(v4, 18);
        h = xxh_merge(h, v1); h = xxh_merge(h, v2); h = xxh_merge(h, v3); h = xxh_merge(h, v4);
    } else {
        h = seed + XXH_P5;
    }
    h += len;
    while (p + 8 <= end) { h ^= xxh_round(0, rd_le64(p)); h = rol64(h, 27) * XXH_P1 + XXH_P4; p += 8; }
    if (p + 4 <= end) { h ^= (uint64_t)rd_le32(p) * XXH_P1; h = rol64(h, 23) * XXH_P2 + XXH_P3; p += 4; }
    while (p < end) { h ^= (uint64_t)(*p) * XXH_P5; h = rol64(h, 11) * XXH_P1; p++; }
    h ^= h >> 33; h *= XXH_P2;
    h ^= h >> 29; h *= XXH_P3;
    h ^= h >> 32;
    return h;
}

/* Streaming fold for `#[derive(Hash)]`: the fixed algorithm is xxHash64, so feeding a field is
 * "hash the 8 little-endian bytes of the value with the running state as seed". Order-sensitive
 * (§6.7 feeds field_discriminant then the fields in declaration order). */
void infy_hash_feed_u64(uint64_t* state, uint64_t v) {
    uint8_t buf[8];
    infy_art_wr_u64(buf, v);
    *state = infy_xxhash64(buf, 8, *state);
}

/* ======================= time (§4.5, §10.12) ======================= */

#ifdef _WIN32
static uint64_t qpc_freq(void) {
    LARGE_INTEGER f; QueryPerformanceFrequency(&f);
    return (uint64_t)f.QuadPart;
}
static uint64_t mono_ns_platform(void) {
    LARGE_INTEGER t; QueryPerformanceCounter(&t);
    uint64_t f = qpc_freq();
    if (f == 0) return (uint64_t)t.QuadPart;
    return (uint64_t)((double)t.QuadPart * 1e9 / (double)f);
}
static void init_mono_base(void) { g_monotonic_start_ns = mono_ns_platform(); }
uint64_t infy_monotonic_ns(void) { return mono_ns_platform() - g_monotonic_start_ns; }
uint64_t infy_unix_seconds(void) {
    FILETIME ft; GetSystemTimePreciseAsFileTime(&ft);
    ULARGE_INTEGER u; u.LowPart = ft.dwLowDateTime; u.HighPart = ft.dwHighDateTime;
    return (uint64_t)(u.QuadPart / 10000000ull - 11644473600ull);
}
void infy_sleep_ms(uint64_t ms) { Sleep((DWORD)(ms > 0xFFFFFFFF ? 0xFFFFFFFF : ms)); }
#else
static uint64_t mono_ns_platform(void) {
    struct timespec ts;
#if defined(CLOCK_MONOTONIC_RAW)
    if (clock_gettime(CLOCK_MONOTONIC_RAW, &ts) != 0)
#endif
        clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}
static void init_mono_base(void) { g_monotonic_start_ns = mono_ns_platform(); }
uint64_t infy_monotonic_ns(void) { return mono_ns_platform() - g_monotonic_start_ns; }
uint64_t infy_unix_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (uint64_t)ts.tv_sec;
}
void infy_sleep_ms(uint64_t ms) {
    struct timespec ts;
    ts.tv_sec = (time_t)(ms / 1000u);
    ts.tv_nsec = (long)((ms % 1000u) * 1000000ull);
    while (nanosleep(&ts, &ts) == -1 && errno == EINTR) { }
}
#endif

uint64_t infy_monotonic_ms(void) { return infy_monotonic_ns() / 1000000ull; }  /* std.env.tick() */

static uint8_t g_mono_base_ready = 0;
void init_mono_base_once(void) {
    if (g_mono_base_ready) return;
    g_mono_base_ready = 1;
    init_mono_base();
}

/* ======================= rand (§10.12: SplitMix64 seeding -> PCG32) =======================
 * No global random source, no time seeding: an InfyRng only ever holds state derived from the
 * explicit seed passed to infy_rng_seeded. */

static uint64_t splitmix64(uint64_t* x) {
    uint64_t z = (*x += 0x9E3779B97F4A7C15ull);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    return z ^ (z >> 31);
}

InfyRng infy_rng_seeded(uint64_t seed) {
    /* PCG's canonical reseed sequence: state=0, inc=(seq<<1)|1, one advance, state+=stream, one
     * advance. `stream`/`seq` are the first two SplitMix64 words of the seed (§10.12). */
    uint64_t s = seed;
    uint64_t stream = splitmix64(&s);
    uint64_t seq = splitmix64(&s);
    InfyRng r;
    r.state = 0u;
    r.inc = (seq << 1u) | 1u;
    r.state = r.state * 6364136223846793005ull + r.inc;   /* advance */
    r.state += stream;
    r.state = r.state * 6364136223846793005ull + r.inc;   /* advance */
    return r;
}

static uint32_t pcg32_u32(InfyRng* r) {
    uint64_t old = r->state;
    r->state = old * 6364136223846793005ull + r->inc;
    uint32_t xorshifted = (uint32_t)(((old >> 18u) ^ old) >> 27u);
    uint32_t rot = (uint32_t)(old >> 59u);
    return (xorshifted >> rot) | (xorshifted << ((0u - rot) & 31u));
}

uint64_t infy_rng_next_u64(InfyRng* r) {
    uint64_t hi = pcg32_u32(r), lo = pcg32_u32(r);
    return (hi << 32) | lo;
}

/* 64x64 -> 128 multiply (high word), portable: no __int128 dependency (§10.12 unbiasedness). */
static uint64_t mulhi64(uint64_t a, uint64_t b) {
    uint64_t a0 = a & 0xFFFFFFFFu, a1 = a >> 32;
    uint64_t b0 = b & 0xFFFFFFFFu, b1 = b >> 32;
    uint64_t p11 = a1 * b1, p01 = a0 * b1, p10 = a1 * b0, p00 = a0 * b0;
    uint64_t middle = (p00 >> 32) + (p10 & 0xFFFFFFFFu) + (p01 & 0xFFFFFFFFu);
    return p11 + (p10 >> 32) + (p01 >> 32) + (middle >> 32);
}

/* Lemire's bounded random with rejection: exactly uniform over [0, range), no modulo bias,
 * no floating point (deterministic bit for bit on every target). range must be > 0. */
static uint64_t rng_below(InfyRng* r, uint64_t range) {
    uint64_t x = infy_rng_next_u64(r);
    uint64_t hi = mulhi64(x, range);
    uint64_t lo = x * range;
    if (lo < range) {
        uint64_t t = (0ull - range) % range;
        while (lo < t) {
            x = infy_rng_next_u64(r);
            hi = mulhi64(x, range);
            lo = x * range;
        }
    }
    return hi;
}

uint64_t infy_rng_range(InfyRng* r, uint64_t lo, uint64_t hi) {
    /* Half-open [lo, hi): the INFY source form `range(lo, hi)` is a half-open range; the inclusive
     * form `0..=i` used by shuffle is handled inside infy_rng_shuffle (§10.12). */
    if (hi <= lo) {
        infy_panic(R_INDEX_OOB, "rng range is empty (hi <= lo, §10.12)", infy_current_node_loc());
        return lo;
    }
    if (hi == (uint64_t)-1 && lo == 0) return infy_rng_next_u64(r);
    return lo + rng_below(r, hi - lo);
}

uint64_t infy_rng_choice_index(InfyRng* r, uint64_t n) {
    if (n == 0) {
        infy_panic(R_INDEX_OOB, "choice() on an empty collection (§10.12)", infy_current_node_loc());
        return 0;
    }
    return rng_below(r, n);
}

/* §10.12: Fisher-Yates, tail to head, drawing from `0..=i`. The direction is normative — a fixed
 * seed must give a byte-identical permutation on every implementation (§12.4 matrix). */
void infy_rng_shuffle(InfyRng* r, void* base, uint64_t n, uint32_t elem_size) {
    if (elem_size == 0 || n < 2) return;
    uint8_t* p = (uint8_t*)base;
    uint8_t stackbuf[2][256];
    uint8_t* tmp[2] = { stackbuf[0], stackbuf[1] };
    uint8_t* heap[2] = { 0, 0 };
    if (elem_size > 256) {
        heap[0] = (uint8_t*)infy_alloc(elem_size, 1);
        heap[1] = (uint8_t*)infy_alloc(elem_size, 1);
        tmp[0] = heap[0]; tmp[1] = heap[1];
    }
    for (uint64_t i = n; i-- > 1;) {
        uint64_t j = rng_below(r, i + 1u);            /* range(0..=i), inclusive (§10.12) */
        memcpy(tmp[0], p + (size_t)i * elem_size, elem_size);
        memcpy(tmp[1], p + (size_t)j * elem_size, elem_size);
        memcpy(p + (size_t)i * elem_size, tmp[1], elem_size);
        memcpy(p + (size_t)j * elem_size, tmp[0], elem_size);
    }
    if (heap[0]) infy_free(heap[0]);
    if (heap[1]) infy_free(heap[1]);
}

/* ======================= env (§4.6) ======================= */

uint64_t infy_env_layout_hash(void) {
    /* Field count + offsets + sizes, hashed with the fixed xxHash64 (§11.3 step 3). The layout is
     * IFABI-fixed; any change here must be an ABI bump. */
    uint64_t h = 0x9E3779B97F4A7C15ull;
    uint64_t fields[11][2] = {
        { offsetof(InfyEnvSnapshot, iter),                 4 },
        { offsetof(InfyEnvSnapshot, runtime_s),            4 },
        { offsetof(InfyEnvSnapshot, realtime),             8 },
        { offsetof(InfyEnvSnapshot, ram_mb),               4 },
        { offsetof(InfyEnvSnapshot, cpu_use),              4 },
        { offsetof(InfyEnvSnapshot, gpu_use),              4 },
        { offsetof(InfyEnvSnapshot, chain_id),             4 },
        { offsetof(InfyEnvSnapshot, node_idx),             4 },
        { offsetof(InfyEnvSnapshot, focus_id),             4 },
        { offsetof(InfyEnvSnapshot, platform),            16 },
        { offsetof(InfyEnvSnapshot, build_id),            16 }
    };
    infy_hash_feed_u64(&h, sizeof(InfyEnvSnapshot));
    infy_hash_feed_u64(&h, 11);                       /* the closed field set (§4.6) */
    for (int i = 0; i < 11; i++) {
        infy_hash_feed_u64(&h, fields[i][0]);
        infy_hash_feed_u64(&h, fields[i][1]);
    }
    return h;
}

static uint32_t g_entry_iterations = 0;   /* §4.6: with no host, env.iter = entry-chain start count */
static void bump_entry_iterations(void) { g_entry_iterations++; }

static void env_refresh_tls(void) {
    uint32_t f = tls_has_focus ? tls_focus_id : g_env_host.focus_id;
    uint32_t c = tls_has_focus ? tls_chain_id : g_env_host.chain_id;
    uint32_t n = tls_has_focus ? tls_node_idx : g_env_host.node_idx;
    tls_env = g_env_host;
    tls_env.focus_id = f; tls_env.chain_id = c; tls_env.node_idx = n;
    if (!tls_env.platform.ptr) { tls_env.platform = g_env_platform; }
    if (!tls_env.build_id.ptr) { tls_env.build_id = g_env_build_id; }
    if (!g_env_has_host) {
        /* No host registered: the runtime self-fills the fields it owns (§4.6). This is not a
         * source of nondeterminism for any hash or iteration order — a program can only observe
         * it by explicitly reading env.realtime / env.runtime_s / env.iter. */
        tls_env.realtime = infy_unix_seconds();
        tls_env.runtime_s = (uint32_t)(infy_monotonic_ms() / 1000u);
        tls_env.iter = g_entry_iterations;
    }
}

const InfyEnvSnapshot* infy_env(void) {
    env_refresh_tls();
    return &tls_env;
}

void infy_env_update(const InfyEnvSnapshot* s) {
    if (!s) return;
    g_env_host = *s;
    g_env_has_host = 1;
    if (!g_env_host.platform.ptr) g_env_host.platform = g_env_platform;
    if (!g_env_host.build_id.ptr) g_env_host.build_id = g_env_build_id;
    env_refresh_tls();
}
/* ======================= std.fmt (§10.11) + Debug (§6.7) =======================
 * The format string is parsed at compile time; codegen hands us an InfyFmtSpec per placeholder.
 * `lit` is the literal run *before* the placeholder. A format string can end with literal text and
 * InfyFmtSpec has no field for a trailing run, so a spec with arg_index == UINT32_MAX is accepted
 * as "literal only" (documented in runtime/README.md). */

#define INFY_FMT_TAIL_ONLY 0xFFFFFFFFu

static void str_push_bytes(InfyString* s, const char* p, uint64_t n) {
    InfyStr b = { (const uint8_t*)p, n };
    infy_string_push_str(s, b);
}
static void str_push_cstr(InfyString* s, const char* p) { str_push_bytes(s, p, (uint64_t)strlen(p)); }

/* shortest decimal form that round-trips (Rust's Display behaviour for f32/f64) */
static void fmt_double_core(InfyString* out, double v, const InfyFmtSpec* sp) {
    char buf[512];
    int prec = sp->precision;
    if (isnan(v)) { str_push_cstr(out, "NaN"); return; }
    if (isinf(v)) { str_push_cstr(out, v < 0 ? "-inf" : "inf"); return; }

    if (sp->which == FY_EXP) {
        int p = (prec >= 0) ? prec : 17;
        snprintf(buf, sizeof buf, "%.*e", p, v);
        if (prec < 0) {
            /* shrink to the shortest representation that still reads back as the same value */
            for (int q = 0; q <= 17; q++) {
                snprintf(buf, sizeof buf, "%.*e", q, v);
                if (strtod(buf, 0) == v) break;
            }
        }
        char* e = strchr(buf, 'e');
        if (!e) { str_push_cstr(out, buf); return; }
        char mant[512];
        size_t mlen = (size_t)(e - buf);
        if (mlen >= sizeof mant) mlen = sizeof mant - 1;
        memcpy(mant, buf, mlen);
        mant[mlen] = 0;
        if (prec < 0) {
            char* dot = strchr(mant, '.');
            if (dot) {
                char* q = mant + strlen(mant) - 1;
                while (q > dot && *q == '0') { *q = 0; q--; }
                if (q == dot) *q = 0;
            }
        }
        int en = atoi(e + 1);
        char expart[16];
        snprintf(expart, sizeof expart, "%s%d", en < 0 ? "-" : "", en < 0 ? -en : en);
        /* no '+' sign and no leading zeros in the exponent (§10.11 byte-exact golden output) */
        str_push_bytes(out, mant, strlen(mant));
        str_push_cstr(out, "e");
        str_push_cstr(out, expart);
        return;
    }

    if (prec >= 0) {
        snprintf(buf, sizeof buf, "%.*f", prec, v);
        str_push_cstr(out, buf);
        return;
    }
    for (int q = 0; q <= 17; q++) {
        snprintf(buf, sizeof buf, "%.*f", q, v);
        if (strtod(buf, 0) == v) break;
    }
    /* Rust prints integral floats without a ".0" tail */
    str_push_cstr(out, buf);
}

static void fmt_uint_core(InfyString* out, uint64_t v, const InfyFmtSpec* sp) {
    char buf[128];
    switch (sp->which) {
    case FY_LOWER_HEX: snprintf(buf, sizeof buf, "%s%llx", sp->alt ? "0x" : "", (unsigned long long)v); break;
    case FY_UPPER_HEX: snprintf(buf, sizeof buf, "%s%llX", sp->alt ? "0X" : "", (unsigned long long)v); break;
    case FY_OCTAL:     snprintf(buf, sizeof buf, "%s%llo", sp->alt ? "0o" : "", (unsigned long long)v); break;
    case FY_BINARY: {
        char rev[65], bits[66];
        uint32_t n = 0, k = 0;
        uint64_t x = v;
        if (x == 0) rev[n++] = '0';
        while (x) { rev[n++] = (char)('0' + (int)(x & 1u)); x >>= 1; }
        for (uint32_t i = 0; i < n; i++) bits[k++] = rev[n - 1 - i];
        bits[k] = 0;
        snprintf(buf, sizeof buf, "%s%s", sp->alt ? "0b" : "", bits);
        break;
    }
    case FY_EXP:       snprintf(buf, sizeof buf, "%llu", (unsigned long long)v); break;
    default:           snprintf(buf, sizeof buf, "%llu", (unsigned long long)v); break;
    }
    str_push_cstr(out, buf);
}

static void fmt_int_core(InfyString* out, int64_t v, const InfyFmtSpec* sp) {
    if (v < 0) {
        /* negate on the unsigned ring: -INT64_MIN is UB in C (§4.9) */
        str_push_cstr(out, "-");
        fmt_uint_core(out, (uint64_t)0 - (uint64_t)v, sp);
        return;
    }
    fmt_uint_core(out, (uint64_t)v, sp);
}

static void utf8_truncate(InfyString* out, InfyStr s, int32_t precision) {
    if (precision < 0) { str_push_bytes(out, (const char*)s.ptr, s.len); return; }
    uint64_t limit = (uint64_t)precision, off = 0;
    while (off < s.len && off < limit) {
        uint32_t cp;
        uint64_t adv = infy_rt_utf8_decode(s.ptr + off, s.len - off, &cp);
        if (adv == 0) break;
        if (off + adv > limit) break;           /* never split a code point (§4.1: str is UTF-8) */
        off += adv;
    }
    str_push_bytes(out, (const char*)s.ptr, off);
}

static void fmt_str_core(InfyString* out, InfyStr s, const InfyFmtSpec* sp) {
    if (sp->which != FY_DEBUG && sp->which != FY_DEBUG_PRETTY) {
        utf8_truncate(out, s, sp->precision);
        return;
    }
    str_push_cstr(out, "\"");
    uint64_t off = 0;
    while (off < s.len) {
        uint32_t cp;
        uint64_t adv = infy_rt_utf8_decode(s.ptr + off, s.len - off, &cp);
        if (adv == 0) adv = 1;
        const uint8_t* raw = s.ptr + off;
        if (cp == '"') str_push_cstr(out, "\\\"");
        else if (cp == '\\') str_push_cstr(out, "\\\\");
        else if (cp == '\n') str_push_cstr(out, "\\n");
        else if (cp == '\r') str_push_cstr(out, "\\r");
        else if (cp == '\t') str_push_cstr(out, "\\t");
        else if (cp == 0) str_push_cstr(out, "\\0");
        else if (cp < 0x20u || cp == 0x7Fu) { char b[16]; snprintf(b, sizeof b, "\\u{%X}", (unsigned)cp); str_push_cstr(out, b); }
        else str_push_bytes(out, (const char*)raw, adv);
        off += adv;
    }
    str_push_cstr(out, "\"");
}

static void fmt_char_core(InfyString* out, uint32_t c, const InfyFmtSpec* sp) {
    if (sp->which == FY_DEBUG || sp->which == FY_DEBUG_PRETTY) {
        char b[32];
        if (c == '\'') str_push_cstr(out, "'\\''");
        else if (c == '\\') str_push_cstr(out, "'\\\\'");
        else if (c == '\n') str_push_cstr(out, "'\\n'");
        else if (c == '\t') str_push_cstr(out, "'\\t'");
        else if (c == '\r') str_push_cstr(out, "'\\r'");
        else if (c < 0x20u || c == 0x7Fu) { snprintf(b, sizeof b, "'\\u{%X}'", (unsigned)c); str_push_cstr(out, b); }
        else { str_push_cstr(out, "'"); append_cp_raw(out, c); str_push_cstr(out, "'"); }
        return;
    }
    append_cp_raw(out, c);
}

/* Apply {:>10} / {:0>10} / {:^10} / fill around an already-rendered core. */
static void emit_padded(InfyString* out, const InfyString* core, const InfyFmtSpec* sp) {
    int32_t width = sp->width;
    uint64_t len = core->len;
    if (width <= 0 || (uint64_t)width <= len) {
        str_push_bytes(out, (const char*)core->ptr, core->len);
        return;
    }
    uint32_t pad = (uint32_t)((uint64_t)width - len);
    uint8_t fillb = sp->fill ? sp->fill : (uint8_t)' ';
    char fillseq[8]; uint32_t flen = 0;
    if (fillb >= 0x80u) { /* a multi-byte fill is not representable in one byte; use space */ fillb = ' '; }
    fillseq[0] = (char)fillb; flen = 1;
    uint8_t align = sp->align ? sp->align : '<';
    if (sp->zero_pad) {
        /* zero padding puts the sign first, then the zeros (Rust's {:0>10} for negatives) */
        uint32_t lead = 0;
        if (core->len && (core->ptr[0] == '-' || core->ptr[0] == '+')) lead = 1;
        str_push_bytes(out, (const char*)core->ptr, lead);
        for (uint32_t i = 0; i < pad; i++) str_push_bytes(out, fillseq, flen);
        str_push_bytes(out, (const char*)core->ptr + lead, core->len - lead);
        return;
    }
    if (align == '<') {
        str_push_bytes(out, (const char*)core->ptr, core->len);
        for (uint32_t i = 0; i < pad; i++) str_push_bytes(out, fillseq, flen);
    } else if (align == '>') {
        for (uint32_t i = 0; i < pad; i++) str_push_bytes(out, fillseq, flen);
        str_push_bytes(out, (const char*)core->ptr, core->len);
    } else { /* '^' : extra pad goes to the right */
        uint32_t left = pad / 2u, right = pad - left;
        for (uint32_t i = 0; i < left; i++) str_push_bytes(out, fillseq, flen);
        str_push_bytes(out, (const char*)core->ptr, core->len);
        for (uint32_t i = 0; i < right; i++) str_push_bytes(out, fillseq, flen);
    }
}

static uint8_t g_debug_pretty = 0;
uint8_t infy_debug_pretty_hint(void) { return g_debug_pretty; }
static void set_pretty_hint(const InfyFmtSpec* sp) {
    g_debug_pretty = (sp && (sp->which == FY_DEBUG_PRETTY || sp->alt)) ? 1 : 0;
}

/* The Debug vtable writer is declared in the frozen header as taking `struct InfyString*`, which is
 * a different (incomplete) type from the `InfyString` typedef; the cast below is the only way to
 * call it without editing the header (reported as a header bug). */
typedef void (*InfyDebugWriteFn)(InfyString*, const void*);

static void fmt_arg_into(InfyString* out, const InfyFmtArg* a, const InfyFmtSpec* sp) {
    InfyString core = infy_string_new();
    InfyFmtSpec local = *sp;
    switch (a->kind) {
    case FA_I64:  fmt_int_core(&core, a->v.i, &local); break;
    case FA_U64:  fmt_uint_core(&core, a->v.u, &local); break;
    case FA_F64:  fmt_double_core(&core, a->v.f, &local); break;
    case FA_STR:  fmt_str_core(&core, a->v.s, &local); break;
    case FA_CHAR: fmt_char_core(&core, a->v.c, &local); break;
    case FA_BOOL: str_push_cstr(&core, a->v.b ? "true" : "false"); break;
    /* FA_I128/FA_U128: InfyFmtArg's union has no 128-bit slot (header gap, see README), so codegen
     * passes the low 64 bits here; the high half must then be rendered by a generated Debug impl. */
    case FA_I128: fmt_int_core(&core, a->v.i, &local); break;
    case FA_U128: fmt_uint_core(&core, a->v.u, &local); break;
    case FA_DEBUG_PTR: default: {
        if (local.which == FY_AUTO) local.which = FY_DEBUG;
        if (local.which == FY_POINTER) {
            char b[32];
            snprintf(b, sizeof b, "0x%llx", (unsigned long long)(uintptr_t)a->v.d.obj);
            str_push_cstr(&core, b);
        } else if (a->kind == FA_DEBUG_PTR && a->v.d.obj) {
            set_pretty_hint(&local);
            if (a->v.d.vt && a->v.d.vt->write)
                ((InfyDebugWriteFn)a->v.d.vt->write)(&core, a->v.d.obj);
            else {
                char b[32];
                snprintf(b, sizeof b, "0x%llx", (unsigned long long)(uintptr_t)a->v.d.obj);
                str_push_cstr(&core, b);
            }
            set_pretty_hint(sp);
        } else {
            str_push_cstr(&core, "?");
        }
        break;
    }
    }
    emit_padded(out, &core, sp);
    infy_string_drop(&core);
}

void infy_fmt_write_int(InfyString* out, int64_t v, const InfyFmtSpec* sp) {
    InfyString core = infy_string_new();
    fmt_int_core(&core, v, sp);
    emit_padded(out, &core, sp);
    infy_string_drop(&core);
}
void infy_fmt_write_uint(InfyString* out, uint64_t v, const InfyFmtSpec* sp) {
    InfyString core = infy_string_new();
    fmt_uint_core(&core, v, sp);
    emit_padded(out, &core, sp);
    infy_string_drop(&core);
}
void infy_fmt_write_float(InfyString* out, double v, const InfyFmtSpec* sp) {
    InfyString core = infy_string_new();
    fmt_double_core(&core, v, sp);
    emit_padded(out, &core, sp);
    infy_string_drop(&core);
}
void infy_fmt_write_str(InfyString* out, InfyStr v, const InfyFmtSpec* sp) {
    InfyString core = infy_string_new();
    fmt_str_core(&core, v, sp);
    emit_padded(out, &core, sp);
    infy_string_drop(&core);
}
void infy_fmt_write_bool(InfyString* out, uint8_t v) {
    InfyFmtSpec sp; memset(&sp, 0, sizeof sp);
    sp.width = -1; sp.precision = -1;
    InfyString core = infy_string_new();
    str_push_cstr(&core, v ? "true" : "false");
    emit_padded(out, &core, &sp);
    infy_string_drop(&core);
}
void infy_fmt_write_char(InfyString* out, uint32_t v) {
    InfyFmtSpec sp; memset(&sp, 0, sizeof sp);
    sp.width = -1; sp.precision = -1;
    InfyString core = infy_string_new();
    fmt_char_core(&core, v, &sp);
    emit_padded(out, &core, &sp);
    infy_string_drop(&core);
}

InfyString infy_fmt_run(const InfyFmtSpec* specs, uint32_t n_specs, const InfyFmtArg* args,
                        uint32_t n_args) {
    InfyString out = infy_string_new();
    for (uint32_t i = 0; i < n_specs; i++) {
        const InfyFmtSpec* sp = &specs[i];
        if (sp->lit && sp->lit_len) str_push_bytes(&out, sp->lit, sp->lit_len);
        if (sp->arg_index == INFY_FMT_TAIL_ONLY) continue;   /* literal-only tail spec */
        if (sp->arg_index >= n_args) {
            /* codegen resolves positions at compile time (E-0180); a runtime miss is an internal
             * contract break, and §4.9 forbids silently printing nothing. */
            char b[96];
            snprintf(b, sizeof b, "format argument %u out of range for %u args (§10.11)",
                     (unsigned)sp->arg_index, (unsigned)n_args);
            infy_panic(R_INDEX_OOB, b, infy_current_node_loc());
            continue;
        }
        fmt_arg_into(&out, &args[sp->arg_index], sp);
    }
    return out;
}

/* §6.7 Debug: `Name { f1: v1, f2: v2 }` in declaration order; `#[repr(C)]` prints `Name(f1, f2)`.
 * `{:#?}` = one field per line, 4-space indent, trailing comma (§10.11). */
void infy_debug_write_struct(InfyString* out, const char* name, const char* const* fields,
                             const InfyFmtArg* vals, uint32_t n, int tuple_form, int pretty) {
    InfyFmtSpec sp; memset(&sp, 0, sizeof sp);
    sp.width = -1; sp.precision = -1; sp.which = pretty ? FY_DEBUG_PRETTY : FY_DEBUG;
    if (pretty) sp.alt = 1;
    str_push_cstr(out, name ? name : "?");
    set_pretty_hint(&sp);
    if (n == 0) {
        str_push_cstr(out, tuple_form ? "()" : " {}");
        g_debug_pretty = 0;
        return;
    }
    if (pretty) {
        str_push_cstr(out, tuple_form ? "(" : " {");
        for (uint32_t i = 0; i < n; i++) {
            str_push_cstr(out, "\n    ");
            if (!tuple_form) { str_push_cstr(out, fields && fields[i] ? fields[i] : "?"); str_push_cstr(out, ": "); }
            fmt_arg_into(out, &vals[i], &sp);
            str_push_cstr(out, ",");                       /* trailing comma, 4-space indent (§10.11) */
        }
        str_push_cstr(out, tuple_form ? "\n)" : "\n}");
    } else {
        str_push_cstr(out, tuple_form ? "(" : " {");
        for (uint32_t i = 0; i < n; i++) {
            if (!tuple_form) {
                if (i) str_push_cstr(out, ",");
                str_push_cstr(out, " ");
                str_push_cstr(out, fields && fields[i] ? fields[i] : "?");
                str_push_cstr(out, ": ");
            } else if (i) {
                str_push_cstr(out, ", ");
            }
            fmt_arg_into(out, &vals[i], &sp);
        }
        str_push_cstr(out, tuple_form ? ")" : " }");
    }
    g_debug_pretty = 0;
}
/* ======================= chains, focuses, scheduler (§8) =======================
 * Cooperative work-item pool. A work item is "run one node of focus F"; the item returns to the
 * pool at the node boundary, which is preemption point ① (§8.5). ② is served by the boundary
 * thunk re-entering the scheduler (infy_yield_now() is what the A/B-track thunks call), ③ is
 * std.chain.yield_now(), ⑤ is a real blocking wait on a ResourceLock. ④ belongs to IFBC's tickloop
 * (owned by infy_ifbc.c).
 *
 * Documented deviation (runtime-abi.md §5): §8.5 describes a focus as a user-space coroutine stack;
 * portable C has no coroutine primitive, so a focus's *locals* live in a heap frame and each node
 * invocation is a fresh C call. The observable semantics (advance position, independent ownership
 * scope, closed preemption set, fairness, failure radius) are unchanged. Because the cleanup deque
 * is heap-resident, a focus may be dispatched on any worker thread. */

InfySched infy_sched;

#define INFY_MAX_CHAINS 4096u
static const InfyChainDesc* g_chains[INFY_MAX_CHAINS];
static uint32_t g_n_chains = 0;

void infy_chain_register(const InfyChainDesc* desc) {
    if (!desc) return;
    if (g_n_chains >= INFY_MAX_CHAINS) {
        infy_panic(R_IFBC_MEMBUDGET, "chain table full (§8.4)", 0);
        return;
    }
    g_chains[g_n_chains++] = desc;
}
void infy_chain_register_table(const InfyChainDesc* const* descs, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) infy_chain_register(descs[i]);
}
uint32_t infy_chain_count(void) { return g_n_chains; }
const InfyChainDesc* infy_chain_desc(uint32_t chain_id) {
    return chain_id < g_n_chains ? g_chains[chain_id] : 0;
}

void infy_set_profile_from_meta(const InfyMetaFixed* m) {
    if (!m) return;
    infy_profile.overflow = (InfyOverflow)m->overflow_policy;
    infy_profile.panic_radius = (InfyPanicRadius)m->panic_radius;
    infy_profile.usize_bits = m->usize_bits ? m->usize_bits : 64;
    infy_profile.stack_bytes = m->stack_bytes;
    infy_profile.focus_stack_bytes = m->focus_stack_bytes;
    infy_profile.focus_quota = m->focus_quota ? m->focus_quota : 1;
    infy_profile.iter_guard_steps = m->iter_guard_steps ? m->iter_guard_steps : 0x100000000ULL;
    infy_profile.fast_math = m->fast_math;
    g_env_platform = m->platform;
    g_env_build_id = m->build_id;
}

static uint32_t worker_threads_to_use(void) {
    const char* e = getenv("INFY_THREADS");            /* §8.5 override */
    if (e && *e) {
        long v = strtol(e, 0, 10);
        if (v >= 1 && v <= 4096) return (uint32_t)v;
    }
#ifdef _WIN32
    SYSTEM_INFO si; GetSystemInfo(&si);
    uint32_t n = (uint32_t)si.dwNumberOfProcessors;
#else
    long n = (long)sysconf(_SC_NPROCESSORS_ONLN);
#endif
    if (n < 1) n = 1;
    return (uint32_t)(n > 8 ? 8 : n);                  /* min(logical_cpus, 8) */
}

static void sched_lock(void)   { infy_mtx_lock(&infy_sched.m); }
static void sched_unlock(void) { infy_mtx_unlock(&infy_sched.m); }

void infy_sched_init(uint32_t threads, uint32_t focus_stack_bytes, uint32_t chain_stack_bytes) {
    memset(&infy_sched, 0, sizeof infy_sched);
    infy_mtx_init(&infy_sched.m);
    infy_cv_init(&infy_sched.cv);
    infy_sched.next_focus_id = 0;
    infy_sched.focus_cap_bytes = focus_stack_bytes ? focus_stack_bytes : 256u * 1024u;
    infy_sched.chain_cap_bytes = chain_stack_bytes ? chain_stack_bytes : 8u * 1024u * 1024u;
    infy_sched.n_workers = threads ? threads : worker_threads_to_use();
    infy_profile.focus_stack_bytes = infy_sched.focus_cap_bytes;
    infy_profile.stack_bytes = infy_sched.chain_cap_bytes;
    infy_sched.inited = 1;
}

void infy_sched_reset(void) {
    /* frees every focus that is not live any more; used by the test harness between cases */
    sched_lock();
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
        InfyFocus* f = infy_sched.focuses[i];
        if (!f) continue;
        free(f);
    }
    free(infy_sched.focuses);
    memset(&infy_sched, 0, sizeof infy_sched);
    infy_sched.focus_cap_bytes = 256u * 1024u;
    infy_sched.chain_cap_bytes = 8u * 1024u * 1024u;
    infy_mtx_init(&infy_sched.m);
    infy_cv_init(&infy_sched.cv);
    infy_sched.inited = 1;
    infy_unlock_all_locks_for_reset();
}

void infy_test_set_focus_seq(uint32_t next_id) {
    if (!infy_sched.inited) infy_sched_init(1, 0, 0);
    sched_lock();
    infy_sched.next_focus_id = next_id;
    sched_unlock();
}

static void focus_table_push(InfyFocus* f) {
    if (infy_sched.n_focuses >= infy_sched.cap_focuses) {
        uint32_t nc = infy_sched.cap_focuses ? infy_sched.cap_focuses * 2u : 16u;
        InfyFocus** nb = (InfyFocus**)realloc(infy_sched.focuses, (size_t)nc * sizeof(InfyFocus*));
        if (!nb) return;
        memset(nb + infy_sched.cap_focuses, 0, (size_t)(nc - infy_sched.cap_focuses) * sizeof(InfyFocus*));
        infy_sched.focuses = nb;
        infy_sched.cap_focuses = nc;
    }
    infy_sched.focuses[infy_sched.n_focuses++] = f;
}

static void ready_enqueue_locked(InfyFocus* f) {
    f->qnext = 0;
    if (infy_sched.ready_tail) infy_sched.ready_tail->qnext = f;
    else infy_sched.ready_head = f;
    infy_sched.ready_tail = f;
    f->state = INFY_F_READY;
    infy_sched.n_runnable++;
}

uint32_t infy_focus_spawn(uint32_t chain_id, uint32_t start_node, const InfyNodeLoc* at) {
    if (!infy_sched.inited) infy_sched_init(1, 0, 0);
    const InfyChainDesc* d = infy_chain_desc(chain_id);
    if (!d) {
        char b[128];
        snprintf(b, sizeof b, "start(): chain %u is not in the chain mirror table (§8.2: E-0800 is "
                              "the compile-time form of this)", (unsigned)chain_id);
        infy_panic(R_GOTO_TARGET, b, at);
        return 0;
    }
    if (start_node >= d->n_nodes) {
        char b[128];
        snprintf(b, sizeof b, "start(): node %u is outside chain %u's node table [0,%u) (§8.2)",
                 (unsigned)start_node, (unsigned)chain_id, (unsigned)d->n_nodes);
        infy_panic(R_GOTO_TARGET, b, at);
        return 0;
    }
    sched_lock();
    /* §8.5: focus ids are monotonic and never reused — at the u32 wraparound point the runtime
     * refuses new focuses with R-0802 ("宁停不混"). */
    if (infy_sched.next_focus_id >= 0xFFFFFFFFu) {
        sched_unlock();
        infy_panic(R_FOCUS_WRAP,
                   "focus id space exhausted; new focuses are refused rather than reusing an id "
                   "(§8.5, R-0802)", at);
        return 0;
    }
    InfyFocus* f = (InfyFocus*)calloc(1, sizeof(InfyFocus));
    if (!f) { sched_unlock(); infy_panic(R_IFBC_MEMBUDGET, "focus allocation failed", at); return 0; }
    f->id = infy_sched.next_focus_id++;
    f->seq = f->id;
    f->chain_id = chain_id;
    f->node_idx = start_node;
    f->pending_node = -1;
    f->desc = d;
    f->quota_left = infy_profile.focus_quota ? infy_profile.focus_quota : 1;
    f->started_ns = infy_monotonic_ns();
    f->state = (infy_sched.pressure >= 0.5f) ? INFY_F_THROTTLED : INFY_F_READY;
    if (d->frame_size) {
        f->frame = infy_alloc(d->frame_size, d->frame_align ? d->frame_align : 8);
        if (f->frame) memset(f->frame, 0, (size_t)d->frame_size);
    }
    focus_table_push(f);
    infy_sched.n_alive++;
    if (f->state == INFY_F_READY) ready_enqueue_locked(f);
    else infy_sched.n_throttled++;
    infy_cv_signal(&infy_sched.cv);
    uint32_t id = f->id;
    g_chains_started = 1;        /* §11.1: after this the HostApi is immutable */
    sched_unlock();
    return id;
}

uint32_t infy_focus_count(uint32_t chain_id) {
    if (!infy_sched.inited) return 0;
    sched_lock();
    uint32_t n = 0;
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
        InfyFocus* f = infy_sched.focuses[i];
        if (f && f->chain_id == chain_id && f->state != INFY_F_DONE && f->state != INFY_F_FAILED) n++;
    }
    sched_unlock();
    return n;
}

uint32_t infy_current_focus(void) { return tls_has_focus ? tls_focus_id : 0; }
uint32_t infy_current_chain(void)  { return tls_has_focus ? tls_chain_id : 0; }
uint32_t infy_current_node(void)   { return tls_has_focus ? tls_node_idx : 0; }
const InfyNodeLoc* infy_current_node_loc(void) { return tls_node_loc; }

void infy_yield_now(void) {
    /* preemption point ③: the flag is consumed at the next node boundary (§8.5) */
    if (tls_has_focus) {
        sched_lock();
        for (uint32_t i = 0; i < infy_sched.n_focuses; i++)
            if (infy_sched.focuses[i] && infy_sched.focuses[i]->id == tls_focus_id)
                infy_sched.focuses[i]->yield_req = 1;
        sched_unlock();
    }
}

void infy_request_stop(void) {
    /* §8.2: graceful — every focus finishes the current node's plain *and* linkage actions. */
    if (!infy_sched.inited) return;
    sched_lock();
    infy_sched.stop_requested = 1;
    infy_cv_broadcast(&infy_sched.cv);
    sched_unlock();
}

void infy_focus_pause(uint32_t focus_id) {
    if (!infy_sched.inited) return;
    sched_lock();
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
        InfyFocus* f = infy_sched.focuses[i];
        if (!f || f->id != focus_id) continue;
        if (f->state == INFY_F_RUNNING) f->pause_req = 1;      /* suspends after the current node */
        else if (f->state == INFY_F_READY || f->state == INFY_F_THROTTLED) {
            /* unlink from the ready FIFO */
            InfyFocus** pp = &infy_sched.ready_head;
            while (*pp) {
                if (*pp == f) {
                    *pp = f->qnext;
                    if (infy_sched.ready_tail == f) infy_sched.ready_tail = 0;
                    f->qnext = 0;
                    infy_sched.n_runnable--;
                    break;
                }
                pp = &(*pp)->qnext;
            }
            if (f->state == INFY_F_THROTTLED) infy_sched.n_throttled--;
            f->state = INFY_F_PAUSED;
        }
        break;
    }
    sched_unlock();
}

void infy_focus_resume(uint32_t focus_id) {
    if (!infy_sched.inited) return;
    sched_lock();
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
        InfyFocus* f = infy_sched.focuses[i];
        if (!f || f->id != focus_id || f->state != INFY_F_PAUSED) continue;
        f->pause_req = 0;
        ready_enqueue_locked(f);                                /* re-enqueued at the FIFO tail */
        infy_cv_signal(&infy_sched.cv);
        break;
    }
    sched_unlock();
}

void infy_set_pressure(float p) {
    if (!infy_sched.inited) infy_sched_init(1, 0, 0);
    sched_lock();
    infy_sched.pressure = p;
    if (p < 0.5f) {
        /* backpressure only throttles the *rate of new focuses*; existing order is untouched
         * (§11.4, and §8.5 forbids reordering the ready queue) */
        for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
            InfyFocus* f = infy_sched.focuses[i];
            if (f && f->state == INFY_F_THROTTLED) {
                f->state = INFY_F_READY;
                if (infy_sched.n_throttled) infy_sched.n_throttled--;
                ready_enqueue_locked(f);
            }
        }
        infy_cv_broadcast(&infy_sched.cv);
    }
    sched_unlock();
}

void infy_goto_checked(uint32_t chain_id, uint32_t target, const InfyNodeLoc* at) {
    /* §4.9 UB-013: goto is resolved at compile time (E-0301); this is the runtime fallback for
     * node sets that depend on build configuration. */
    if (!tls_has_focus || chain_id != tls_chain_id) {
        char b[128];
        snprintf(b, sizeof b, "goto: target chain %u is not the current chain %u (§8.2)",
                 (unsigned)chain_id, (unsigned)(tls_has_focus ? tls_chain_id : 0));
        infy_panic(R_GOTO_TARGET, b, at);
        return;
    }
    const InfyChainDesc* d = infy_chain_desc(chain_id);
    if (!d || target >= d->n_nodes) {
        char b[128];
        snprintf(b, sizeof b, "goto: runtime target node %u does not exist (§4.9 UB-013)",
                 (unsigned)target);
        infy_panic(R_GOTO_TARGET, b, at);
        return;
    }
    sched_lock();
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++)
        if (infy_sched.focuses[i] && infy_sched.focuses[i]->id == tls_focus_id)
            infy_sched.focuses[i]->pending_node = (int64_t)target;
    sched_unlock();
}

static uint8_t g_host_stop = 0;
void infy_host_mark_stop(void) { g_host_stop = 1; }
uint8_t infy_host_stop_requested(void) { return g_host_stop; }

void infy_sched_mark_failed(uint32_t focus_id, uint32_t code, uint8_t whole_chain) {
    if (!infy_sched.inited) return;
    sched_lock();
    InfyFocus* fp = 0;
    uint32_t cid = 0xFFFFFFFFu;
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
        InfyFocus* f = infy_sched.focuses[i];
        if (f && f->id == focus_id) { fp = f; cid = f->chain_id; break; }
    }
    if (fp) {
        fp->fail_code = code;
        if (whole_chain) {
            /* §4.16 `chain`: every focus of that chain is marked Failed; the chain's on_fail
             * linkage (when declared) is codegen's job and runs inside the failing node. */
            for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
                InfyFocus* f = infy_sched.focuses[i];
                if (!f || f->chain_id != cid) continue;
                if (f->state == INFY_F_READY || f->state == INFY_F_THROTTLED ||
                    f->state == INFY_F_PAUSED || f->state == INFY_F_RUNNING) {
                    if (f->state == INFY_F_READY) infy_sched.n_runnable--;
                    if (f->state == INFY_F_THROTTLED) infy_sched.n_throttled--;
                    f->state = INFY_F_FAILED;
                    f->fail_code = code;
                    infy_sched.n_failed++;
                    infy_sched.n_alive--;
                }
            }
        } else {
            if (fp->state == INFY_F_READY) infy_sched.n_runnable--;
            if (fp->state == INFY_F_THROTTLED) infy_sched.n_throttled--;
            fp->state = INFY_F_FAILED;
            infy_sched.n_failed++;
            infy_sched.n_alive--;
            infy_cv_broadcast(&infy_sched.cv);
        }
    }
    sched_unlock();
    INFY_UNUSED(cid);
}

/* ---- one node dispatch: preemption point ① lives here (§8.5) ---- */
static void dispatch_node(InfyFocus* f) {
    const InfyChainDesc* d = f->desc;
    f->state = INFY_F_RUNNING;
    if (!d->nodes || f->node_idx >= d->n_nodes) {
        /* nothing left to run: finish the focus instead of re-queueing it (§8.2 chain end) */
        infy_rt_cleanup_drain_scope(&f->cleanup, 1);
        infy_free(f->frame); f->frame = 0;
        f->state = INFY_F_DONE;
        sched_lock();
        infy_sched.n_alive--;
        sched_unlock();
        return;
    }

    uint8_t  had_focus = tls_has_focus;
    uint32_t sf = tls_focus_id, sc = tls_chain_id, sn = tls_node_idx;
    const InfyNodeLoc* sl = tls_node_loc;
    InfyCleanup* saved_cleanup = infy_tls_cleanup;

    InfyTry t;
    memset(&t, 0, sizeof t);
    t.prev = infy_tls_try;
    t.focus_id = f->id; t.chain_id = f->chain_id; t.node_idx = f->node_idx;
    infy_tls_try = &t;

    tls_has_focus = 1;
    tls_focus_id = f->id; tls_chain_id = f->chain_id; tls_node_idx = f->node_idx;
    tls_node_loc = d->locs ? &d->locs[f->node_idx] : 0;
    infy_tls_cleanup = f->cleanup;
    /* §4.6: the host refreshes the snapshot between node dispatches. */
    infy_rt_host_env_refresh();
    env_refresh_tls();

    if (setjmp(t.jb) == 0) {
        if (d->nodes && d->nodes[f->node_idx]) d->nodes[f->node_idx](f->frame);
        infy_tls_try = t.prev;
    } else {
        /* a panic already drained the cleanup deque and marked the focus Failed (§4.16) */
        infy_tls_try = t.prev;
    }
    f->cleanup = infy_tls_cleanup;
    infy_tls_cleanup = saved_cleanup;
    tls_has_focus = had_focus; tls_focus_id = sf; tls_chain_id = sc; tls_node_idx = sn;
    tls_node_loc = sl;

    if (t.caught) {
        /* infy_sched_mark_failed already accounted for it */
        f->qnext = 0;
        return;
    }
    /* §8.2 auto-advance: the node's plain and linkage statements have all run. */
    if (f->pending_node >= 0) { f->node_idx = (uint32_t)f->pending_node; f->pending_node = -1; }
    else f->node_idx++;

    if (infy_sched.stop_requested || infy_host_stop_requested()) {
        /* `stop` is graceful: the current node finished, now drop per §4.7.4 and release. */
        infy_rt_cleanup_drain_scope(&f->cleanup, 1);
        infy_free(f->frame); f->frame = 0;
        f->state = INFY_F_DONE;
        sched_lock();
        infy_sched.n_alive--;
        sched_unlock();
        return;
    }
    if (f->pause_req) {
        f->pause_req = 0;
        f->state = INFY_F_PAUSED;      /* frame and resources retained (§8.2) */
        return;
    }
    if (f->node_idx >= d->n_nodes) {
        infy_rt_cleanup_drain_scope(&f->cleanup, 1);
        infy_free(f->frame); f->frame = 0;
        f->state = INFY_F_DONE;
        sched_lock();
        infy_sched.n_alive--;
        sched_unlock();
        return;
    }
    f->state = INFY_F_READY;
}

/* Bounded round-robin with the per-focus quota (§8.5). A focus that still has quota is offered
 * again first; otherwise it goes to the tail of the ready FIFO, so every ready focus is served
 * within O(number of focuses) work items and nothing is ever "newest first". */
static void focus_requeue(InfyFocus* f) {
    sched_lock();
    if (f->state != INFY_F_READY) { sched_unlock(); return; }
    if (f->quota_left > 0) {
        f->quota_left--;
        f->qnext = infy_sched.ready_head;
        infy_sched.ready_head = f;
        if (!infy_sched.ready_tail) infy_sched.ready_tail = f;
    } else {
        f->quota_left = infy_profile.focus_quota ? infy_profile.focus_quota : 1;
        f->qnext = 0;
        if (infy_sched.ready_tail) infy_sched.ready_tail->qnext = f;
        else infy_sched.ready_head = f;
        infy_sched.ready_tail = f;
    }
    infy_sched.n_runnable++;
    infy_cv_signal(&infy_sched.cv);
    sched_unlock();
}

static InfyFocus* sched_pick(void) {
    sched_lock();
    for (;;) {
        /* admit throttled focuses as soon as backpressure allows (§11.4) */
        if (infy_sched.n_throttled && infy_sched.pressure < 0.5f) {
            for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
                InfyFocus* f = infy_sched.focuses[i];
                if (f && f->state == INFY_F_THROTTLED) {
                    f->state = INFY_F_READY;
                    infy_sched.n_throttled--;
                    ready_enqueue_locked(f);
                    break;
                }
            }
        }
        if (infy_sched.ready_head) {
            InfyFocus* f = infy_sched.ready_head;
            infy_sched.ready_head = f->qnext;
            if (!infy_sched.ready_head) infy_sched.ready_tail = 0;
            f->qnext = 0;
            infy_sched.n_runnable--;
            infy_sched.dispatch_count++;
            sched_unlock();
            return f;
        }
        if (infy_sched.n_runnable == 0 && infy_sched.n_alive == infy_sched.n_throttled &&
            infy_sched.n_throttled > 0) {
            /* only throttled work is left: run it rather than livelock the scheduler */
            for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
                InfyFocus* f = infy_sched.focuses[i];
                if (f && f->state == INFY_F_THROTTLED) {
                    f->state = INFY_F_READY;
                    infy_sched.n_throttled--;
                    ready_enqueue_locked(f);
                    break;
                }
            }
            continue;
        }
        if (infy_sched.n_runnable == 0 || infy_sched.shutdown || infy_sched.stop_requested) {
            /* idle: no runnable focus (everything else is paused/finished and resumable later) */
            sched_unlock();
            return 0;
        }
        infy_cv_wait(&infy_sched.cv, &infy_sched.m);       /* preemption point ⑤ territory */
    }
}

static void* worker_main(void* arg) {
    uint32_t idx = (uint32_t)(uintptr_t)arg;
    init_mono_base_once();
    sched_lock();
    infy_sched.worker_idx = idx;
    sched_unlock();
    for (;;) {
        InfyFocus* f = sched_pick();
        if (!f) break;
        dispatch_node(f);
        focus_requeue(f);
    }
    return 0;
}

#ifdef _WIN32
static unsigned __stdcall worker_stub(void* arg) { worker_main(arg); return 0; }
#endif

int infy_sched_run_until_idle(void) {
    if (!infy_sched.inited) infy_sched_init(1, 0, 0);
    init_mono_base_once();
    uint32_t want = infy_sched.n_workers ? infy_sched.n_workers : 1;
    if (want <= 1) {
        InfyFocus* f;
        while ((f = sched_pick()) != 0) {
            dispatch_node(f);
            focus_requeue(f);
        }
    } else {
#ifdef _WIN32
        uintptr_t handles[64];
        uint32_t made = 0;
        for (uint32_t i = 1; i < want && made < 63; i++) {
            uintptr_t h = _beginthreadex(0, 0, worker_stub, (void*)(uintptr_t)i, 0, 0);
            if (!h) break;
            handles[made++] = h;
        }
        infy_sched.n_workers_live = made + 1;
        worker_main(0);
        for (uint32_t i = 0; i < made; i++) {
            WaitForSingleObject((HANDLE)handles[i], INFINITE);
            CloseHandle((HANDLE)handles[i]);
        }
#else
        pthread_t th[64];
        uint32_t made = 0;
        for (uint32_t i = 1; i < want && made < 64; i++)
            if (pthread_create(&th[made], 0, worker_main, (void*)(uintptr_t)i) == 0) made++;
        infy_sched.n_workers_live = made + 1;
        worker_main(0);
        for (uint32_t i = 0; i < made; i++) pthread_join(th[i], 0);
#endif
    }
    /* wake anyone still waiting so the pool drains */
    sched_lock();
    infy_sched.shutdown = 1;
    infy_cv_broadcast(&infy_sched.cv);
    sched_unlock();
    return infy_sched.n_failed ? 3 : 0;    /* §13.4: exit code 3 on a runtime panic */
}

/* ======================= ResourceLock FIFO (§8.3) =======================
 * Process-global, per resource name: { lock mutex, condvar, FIFO of guard nodes }. Submission order
 * is the queue order; a waiter sleeps until it is at the head, so no waiter can be skipped (no
 * starvation). `Guard` drop releases. Deadlock detection is explicitly *not* promised (§8.3). */

#define INFY_MAX_LOCKS 256
static InfyLockEntry g_locks[INFY_MAX_LOCKS];
static uint32_t g_n_locks = 0;
static InfySrw g_locks_mu;
static int g_locks_ready = 0;
static _Atomic uint64_t g_guard_seq = 0;

typedef struct { InfyLockEntry* lock; InfyLockNode* node; } GuardRef;
#define INFY_MAX_GUARDS 4096
static GuardRef g_guards[INFY_MAX_GUARDS];

static void locks_init_once(void) {
    if (g_locks_ready) return;
    infy_mtx_init(&g_locks_mu);
    g_locks_ready = 1;
}

static InfyLockEntry* lock_find_locked(const char* name, int create) {
    for (uint32_t i = 0; i < g_n_locks; i++)
        if (strncmp(g_locks[i].name, name, sizeof g_locks[i].name) == 0) return &g_locks[i];
    if (!create || g_n_locks >= INFY_MAX_LOCKS) return 0;
    InfyLockEntry* e = &g_locks[g_n_locks++];
    memset(e, 0, sizeof *e);
    snprintf(e->name, sizeof e->name, "%s", name);
    infy_mtx_init(&e->m);
    infy_cv_init(&e->cv);
    return e;
}

uint64_t infy_lock_acquire(const char* resource, const InfyNodeLoc* at) {
    if (!resource) { infy_panic(R_INDEX_OOB, "lock(): NULL resource name", at); return 0; }
    locks_init_once();
    infy_mtx_lock(&g_locks_mu);
    InfyLockEntry* e = lock_find_locked(resource, 1);
    infy_mtx_unlock(&g_locks_mu);
    if (!e) {
        infy_panic(R_IFBC_MEMBUDGET, "ResourceLock table full (§8.3)", at);
        return 0;
    }
    InfyLockNode* nd = (InfyLockNode*)calloc(1, sizeof(InfyLockNode));
    if (!nd) { infy_panic(R_IFBC_MEMBUDGET, "guard allocation failed", at); return 0; }
    nd->guard = atomic_fetch_add(&g_guard_seq, 1ull) + 1ull;
    if (nd->guard == 0) nd->guard = atomic_fetch_add(&g_guard_seq, 1ull) + 1ull;

    infy_mtx_lock(&e->m);
    /* append in submission order: this is the FIFO the spec makes observable (§8.3) */
    if (e->tail) e->tail->next = nd; else e->head = nd;
    e->tail = nd;
    e->waiting++;
    /* sleep until this node is at the head *and* the head is unclaimed: a waiter can never be
     * skipped, which is the no-starvation requirement of §8.3 */
    while (e->head != nd || nd->held) infy_cv_wait(&e->cv, &e->m);
    e->waiting--;
    nd->held = 1;
    infy_mtx_unlock(&e->m);

    uint32_t slot = (uint32_t)(nd->guard % INFY_MAX_GUARDS);
    infy_mtx_lock(&g_locks_mu);
    g_guards[slot].lock = e;
    g_guards[slot].node = nd;
    infy_mtx_unlock(&g_locks_mu);
    return nd->guard;
}

void infy_lock_release(uint64_t guard) {
    if (guard == 0) return;
    locks_init_once();
    uint32_t slot = (uint32_t)(guard % INFY_MAX_GUARDS);
    infy_mtx_lock(&g_locks_mu);
    GuardRef ref = g_guards[slot];
    infy_mtx_unlock(&g_locks_mu);
    if (!ref.lock || !ref.node || ref.node->guard != guard) return;   /* stale/foreign guard */
    infy_mtx_lock(&ref.lock->m);
    ref.node->held = 0;
    ref.node->released = 1;
    if (ref.lock->head == ref.node) {
        InfyLockNode* done = ref.lock->head;
        ref.lock->head = done->next;
        if (!ref.lock->head) ref.lock->tail = 0;
        done->next = 0;
        free(done);
    }
    infy_cv_broadcast(&ref.lock->cv);      /* the new head proceeds; order is the queue order */
    infy_mtx_unlock(&ref.lock->m);
}

uint32_t infy_lock_waiting(const char* resource) {
    locks_init_once();
    infy_mtx_lock(&g_locks_mu);
    InfyLockEntry* e = lock_find_locked(resource, 0);
    uint32_t n = e ? e->waiting : 0;
    infy_mtx_unlock(&g_locks_mu);
    return n;
}

void infy_unlock_all_locks_for_reset(void) {
    for (uint32_t i = 0; i < g_n_locks; i++) {
        InfyLockEntry* e = &g_locks[i];
        infy_mtx_lock(&e->m);
        InfyLockNode* nd = e->head;
        while (nd) { InfyLockNode* nx = nd->next; free(nd); nd = nx; }
        e->head = e->tail = 0; e->waiting = 0;
        infy_mtx_unlock(&e->m);
    }
    memset(g_guards, 0, sizeof g_guards);
}
/* ======================= A-track boundary thunks (§7.6.4, §7.6.5) ======================= */

void* infy_atrack_ptr_in(const void* p, uint64_t len, const InfyNodeLoc* l) {
    /* UB-009: an A-track raw pointer that comes back 0 without an `Option` in its signature is
     * R-0720 — never a silent null that later explodes in C code. */
    if (!p) { infy_panic(R_ATRACK_POINTER, "A-track pointer is null (§4.9 UB-009)", l); return 0; }
    if (len > (uint64_t)(SIZE_MAX / 2u)) {
        infy_panic(R_ATRACK_POINTER, "A-track slice length is not representable (§7.6.4)", l);
        return 0;
    }
    return (void*)p;
}

void infy_atrack_err_out(int32_t err_written, const InfyNodeLoc* l) {
    /* §7.6.4: `Result<T, i32>` compiles to `T ret, int32_t* err`; an external callee that never
     * wrote the out-parameter is R-0722. */
    if (err_written == 0) {
        infy_panic(R_ATRACK_ERR_OUT, "external callee did not write the err out-parameter (§7.6.4)",
                   l);
    }
}

typedef struct InfyArcCbBox {
    void*          obj;
    uint64_t*      count;
    struct InfyArcCbBox* next;
} InfyArcCbBox;

static InfyArcCbBox* g_cb_list = 0;
static uint32_t      g_cb_live = 0;
static InfySrw       g_cb_mu;
static int           g_cb_mu_ready = 0;

static void cb_mu_init(void) { if (!g_cb_mu_ready) { infy_mtx_init(&g_cb_mu); g_cb_mu_ready = 1; } }

/* A callback closure handed to external code is boxed in an Arc (§7.6.5); the external side must
 * release it through the generated `#[export]` free entry, otherwise the leak check at `stop`
 * reports R-0723. */
void* infy_arc_cb_box(void* obj, uint64_t* count) {
    cb_mu_init();
    InfyArcCbBox* b = (InfyArcCbBox*)calloc(1, sizeof(InfyArcCbBox));
    if (!b) return 0;
    b->obj = obj; b->count = count;
    infy_mtx_lock(&g_cb_mu);
    b->next = g_cb_list;
    g_cb_list = b;
    g_cb_live++;
    infy_mtx_unlock(&g_cb_mu);
    return b;
}

void infy_arc_cb_free(void* boxed, const InfyNodeLoc* l) {
    if (!boxed) return;
    cb_mu_init();
    InfyArcCbBox* b = (InfyArcCbBox*)boxed;
    if (b->count && *b->count != 0) {
        char buf[96];
        snprintf(buf, sizeof buf, "callback Arc still has %llu strong counts (§7.6.5)",
                 (unsigned long long)*b->count);
        infy_panic(R_CB_LEAK, buf, l);
        return;
    }
    infy_mtx_lock(&g_cb_mu);
    InfyArcCbBox** pp = &g_cb_list;
    while (*pp) {
        if (*pp == b) { *pp = b->next; if (g_cb_live) g_cb_live--; break; }
        pp = &(*pp)->next;
    }
    infy_mtx_unlock(&g_cb_mu);
    free(b);
}

void infy_leak_check(void) {
    cb_mu_init();
    infy_mtx_lock(&g_cb_mu);
    uint32_t live = g_cb_live;
    infy_mtx_unlock(&g_cb_mu);
    if (live != 0) {
        char buf[96];
        snprintf(buf, sizeof buf, "%u callback Arc(s) still live at stop/unload (§7.6.5, R-0723)",
                 (unsigned)live);
        infy_panic(R_CB_LEAK, buf, infy_current_node_loc());
    }
}

/* ======================= host protocol (§11) ======================= */

static InfyHostApi    g_host;
static uint8_t        g_host_registered = 0;
static uint8_t        g_handshake_done = 0;
static InfySrw        g_host_mu;          /* serialises call_native_event (§11.1) */
static int32_t        g_host_nesting = 0;
static uint8_t        g_chains_started = 0;
static InfySrw        g_query_mu;
static uint8_t        g_query_ready = 0;
static InfyVecMap     g_query_table;      /* key -> {kind, i64 or String} */

typedef struct { InfyHostQueryKind kind; int64_t i; InfyString s; } HostQueryVal;

/* The product-side registry the host claims to implement. `InfyHostApi` has no entry point for
 * "which event ids do you have / what are their signature digests", so §11.3 step 2 cannot be
 * answered by the host through the frozen header; the host installs it through this internal call
 * and the runtime verifies against it (see README, "header gaps"). */
static const uint32_t* g_host_event_ids = 0;
static uint32_t        g_host_event_n = 0;
static const uint64_t* g_host_event_sigs = 0;

void infy_host_register_events(const uint32_t* ids, uint32_t n, const uint64_t* sigs) {
    g_host_event_ids = ids; g_host_event_n = n; g_host_event_sigs = sigs;
}

static int host_has_event(uint32_t id) {
    if (!g_host_event_ids) return -1;                       /* not verifiable at runtime */
    for (uint32_t i = 0; i < g_host_event_n; i++) if (g_host_event_ids[i] == id) return 1;
    return 0;
}

static void host_sync(void) {
    if (!g_query_ready) {
        infy_mtx_init(&g_host_mu);
        infy_mtx_init(&g_query_mu);
        infy_vecmap_init(&g_query_table, 64, sizeof(HostQueryVal));
        g_query_ready = 1;
    }
}

void infy_host_register(const InfyHostApi* host) {
    if (!host) return;
    host_sync();
    if (g_host_registered) {
        if (g_chains_started && host->self != g_host.self) {
            /* §11.1: the trait object is immutable once chains run; swapping it would make
             * E-0900/R-0904 semantics unreproducible. Closest normative code: R-0903. */
            infy_panic(R_IFABI_MISMATCH,
                       "HostApi replaced after chains started (§11.1)", infy_current_node_loc());
            return;
        }
        if (host->self == g_host.self) return;
    }
    g_host = *host;
    g_host_registered = 1;
}

void infy_rt_host_env_refresh(void) {
    if (!g_host_registered || !g_host.env_snapshot) return;
    const InfyEnvSnapshot* s = g_host.env_snapshot(g_host.self);
    if (s) {
        /* The host owns iter/runtime_s/realtime/ram_mb/cpu_use/gpu_use; chain/node/focus stay with
         * the scheduler (§4.6). */
        uint32_t c = g_env_host.chain_id, n = g_env_host.node_idx, f = g_env_host.focus_id;
        g_env_host = *s;
        g_env_host.chain_id = c; g_env_host.node_idx = n; g_env_host.focus_id = f;
    }
}

int infy_host_handshake(uint32_t ifabi, const uint32_t* event_ids, uint32_t n_events,
                        uint64_t env_layout_hash) {
    host_sync();
    /* step 1 — IFABI (§11.3) */
    if (ifabi != INFY_IFABI) {
        char b[128];
        snprintf(b, sizeof b, "artifact IFABI %u != runtime IFABI %u (§11.3 step 1)",
                 (unsigned)ifabi, (unsigned)INFY_IFABI);
        infy_panic(R_IFABI_MISMATCH, b, infy_current_node_loc());
        return -1;
    }
    /* step 2 — every host-implemented event exists in the host registry with a matching digest */
    if (n_events > 0 && !event_ids) {
        infy_panic(R_EVENT_MISSING,
                   "no EventTable handed to the handshake (§11.3 step 2)",
                   infy_current_node_loc());
        return -1;
    }
    for (uint32_t i = 0; i < n_events; i++) {
        int has = host_has_event(event_ids[i]);
        if (has == 0) {
            char b[128];
            snprintf(b, sizeof b, "host does not implement event id %u (§11.3 step 2)",
                     (unsigned)event_ids[i]);
            infy_panic(R_EVENT_MISSING, b, infy_current_node_loc());
            return -1;
        }
        if (has < 0) {
            /* No host registry: §11.3 makes this check a *runtime fallback* for dynamically loaded
             * .infib only — with a statically linked host the guarantee is the linker's. Accepted,
             * and recorded here so `infy conformance --category` can tell the two apart. */
            break;
        }
    }
    /* step 3 — EnvSnapshot layout (field count + offset table) */
    uint64_t want = infy_env_layout_hash();
    if (g_host_registered && g_host.env_snapshot) {
        if (env_layout_hash != want) {
            char b[160];
            snprintf(b, sizeof b, "EnvSnapshot layout 0x%016llx != runtime 0x%016llx (§11.3 step 3)",
                     (unsigned long long)env_layout_hash, (unsigned long long)want);
            infy_panic(R_ENV_LAYOUT, b, infy_current_node_loc());
            return -1;
        }
    } else if (env_layout_hash != want) {
        char b[160];
        snprintf(b, sizeof b, "EnvSnapshot layout 0x%016llx != runtime 0x%016llx (§11.3 step 3)",
                 (unsigned long long)env_layout_hash, (unsigned long long)want);
        infy_panic(R_ENV_LAYOUT, b, infy_current_node_loc());
        return -1;
    }
    g_handshake_done = 1;
    return 0;
}

InfyOutVal infy_host_call_event(uint32_t id, const InfyArgFrame* args, const InfyNodeLoc* at) {
    InfyOutVal out;
    memset(&out, 0, sizeof out);
    host_sync();
    if (g_host_nesting >= 8) {
        /* §11.1: host↔INFY mutual recursion deeper than 8 levels is R-0902 */
        infy_panic(R_HOST_NESTING, "host callback nesting exceeds 8 levels (§11.1)", at);
        return out;
    }
    if (!g_host_registered || !g_host.call_native_event) {
        infy_panic(R_HOST_UNMAPPED, "no HostApi registered for this event call (§11.1)", at);
        return out;
    }
    if (host_has_event(id) == 0) {
        /* §11.2: R-0900 is what a host that does not know the id produces (the typical symptom of
         * a version mismatch). */
        char b[128];
        snprintf(b, sizeof b, "host does not know event id %u — unmapped host error (§11.2)",
                 (unsigned)id);
        infy_panic(R_HOST_UNMAPPED, b, at);
        return out;
    }
    InfyArgFrame empty;
    memset(&empty, 0, sizeof empty);
    const InfyArgFrame* a = args ? args : &empty;
    int8_t rc;
    infy_mtx_lock(&g_host_mu);          /* §11.1: hosts need not lock themselves */
    g_host_nesting++;
    rc = g_host.call_native_event(g_host.self, id, a, &out);
    g_host_nesting--;
    infy_mtx_unlock(&g_host_mu);
    if (rc != 0) {
        char b[128];
        snprintf(b, sizeof b, "host event %u returned %d — unmapped host error (§11: E-0900 family)",
                 (unsigned)id, (int)rc);
        infy_panic(R_HOST_UNMAPPED, b, at);
    }
    return out;
}

uint64_t infy_lock_acquire_host_resource(uint32_t res, uint64_t ts) {
    /* §11.1: acquire/release run *outside* the serialising host lock, or FIFO and the host lock
     * deadlock against each other. */
    if (g_host_registered && g_host.acquire_resource) return g_host.acquire_resource(g_host.self, res, ts);
    return 0;
}
void infy_release_host_resource(uint64_t guard) {
    if (g_host_registered && g_host.release_resource) g_host.release_resource(g_host.self, guard);
}

static uint8_t query_lookup(InfyStr key, HostQueryVal* v) {
    char buf[64];
    if (key.len >= sizeof buf) return 0;
    memset(buf, 0, sizeof buf);
    if (key.len) memcpy(buf, key.ptr, (size_t)key.len);
    host_sync();
    infy_mtx_lock(&g_query_mu);
    void* p = infy_vecmap_get(&g_query_table, buf);
    if (p) { if (v) *v = *(HostQueryVal*)p; infy_mtx_unlock(&g_query_mu); return 1; }
    infy_mtx_unlock(&g_query_mu);
    return 0;
}

uint8_t infy_host_query(InfyStr key, int64_t* out) {
    HostQueryVal v; memset(&v, 0, sizeof v);
    if (!query_lookup(key, &v)) return 0;                 /* §10.8: None means "host has no such key" */
    if (v.kind != INFY_HQ_INT) {
        infy_panic(R_HOST_QUERY_TYP,
                   "std.host.query() on a key whose host-defined type is not i64 (§10.8)",
                   infy_current_node_loc());
        return 0;
    }
    if (out) *out = v.i;
    return 1;
}

uint8_t infy_host_query_str(InfyStr key, InfyString* out) {
    HostQueryVal v; memset(&v, 0, sizeof v);
    if (!query_lookup(key, &v)) return 0;
    if (v.kind != INFY_HQ_STR) {
        infy_panic(R_HOST_QUERY_TYP,
                   "std.host.query_str() on a key whose host-defined type is not str (§10.8)",
                   infy_current_node_loc());
        return 0;
    }
    if (out) *out = infy_string_from(infy_string_as_str(&v.s));
    return 1;
}

int8_t infy_host_set(InfyStr key, InfyStr val) {
    host_sync();
    if (key.len >= 64) return -1;                          /* HostErr: key too long for the table */
    char buf[64]; memset(buf, 0, sizeof buf);
    memcpy(buf, key.ptr, (size_t)key.len);
    HostQueryVal v; memset(&v, 0, sizeof v);
    v.kind = INFY_HQ_STR;
    v.s = infy_string_from(val);
    infy_mtx_lock(&g_query_mu);
    infy_vecmap_put(&g_query_table, buf, &v);
    infy_mtx_unlock(&g_query_mu);
    return 0;
}

void infy_host_set_query(const char* key, InfyHostQueryKind kind, int64_t i, InfyStr s) {
    host_sync();
    if (!key) return;
    char buf[64]; memset(buf, 0, sizeof buf);
    snprintf(buf, sizeof buf, "%s", key);
    HostQueryVal v; memset(&v, 0, sizeof v);
    v.kind = kind; v.i = i;
    if (kind == INFY_HQ_STR) v.s = infy_string_from(s);
    infy_mtx_lock(&g_query_mu);
    infy_vecmap_put(&g_query_table, buf, &v);
    infy_mtx_unlock(&g_query_mu);
}

void infy_host_clear_query(void) {
    host_sync();
    infy_mtx_lock(&g_query_mu);
    for (uint64_t i = 0; i < g_query_table.n; i++) {
        uint8_t* rec = g_query_table.data + i * (g_query_table.key_size + g_query_table.val_size);
        HostQueryVal* v = (HostQueryVal*)(void*)(rec + g_query_table.key_size);
        infy_string_drop(&v->s);
        memset(v, 0, sizeof *v);
    }
    g_query_table.n = 0;
    infy_mtx_unlock(&g_query_mu);
}

uint8_t infy_host_is_registered(void) { return g_host_registered; }
uint8_t infy_host_handshake_done(void) { return g_handshake_done; }
void infy_host_mark_chains_started(void) { g_chains_started = 1; }
/* ======================= artifact container (§7.4) ======================= */

int infy_art_parse_trailer(const uint8_t* file, uint64_t len, InfyArtTrailer* t, const char** why) {
    static const char *e_short = "file is shorter than the 64-byte INFY artifact trailer",
                    *e_magic = "no INFYART2 trailer at end of file (not an INFY artifact)",
                    *e_dup = "duplicate INFYART2 trailer: the artifact was appended twice",
                    *e_crc = "trailer_crc does not match the trailer bytes (§7.4 self-validation)",
                    *e_ifabi = "artifact IFABI differs from the runtime's IFABI (§11.3 step 1)",
                    *e_ver = "unsupported artifact header version",
                    *e_range = "trailer region offsets lie outside the file (§7.4)";
    uint64_t magic_count = 0;
    if (t) memset(t, 0, sizeof *t);
    if (len < INFY_ART_TRAILER_SIZE) { *why = e_short; return -1; }
    const uint8_t* tr = file + (len - INFY_ART_TRAILER_SIZE);
    if (memcmp(tr, INFY_ART_MAGIC, 8) != 0) { *why = e_magic; return -1; }
    /* a second copy of the magic anywhere in the file means the blob was appended twice */
    for (uint64_t i = 0; i + 8 <= len; i++) if (memcmp(file + i, INFY_ART_MAGIC, 8) == 0) magic_count++;
    if (magic_count > 1) { *why = e_dup; return -1; }
    uint32_t crc = infy_art_rd_u32(tr + 56);
    if (infy_art_crc32(tr, 56) != crc) { *why = e_crc; return -1; }
    if (t) {
        t->hdr_ver = (uint16_t)infy_art_rd_u32(tr + 8);
        t->ifabi = (uint16_t)((infy_art_rd_u32(tr + 8) >> 16) & 0xFFFFu);
        t->seg_count = (uint16_t)(infy_art_rd_u32(tr + 12) & 0xFFFFu);
        t->flags = (uint16_t)((infy_art_rd_u32(tr + 12) >> 16) & 0xFFFFu);
        t->blob_off = infy_art_rd_u32(tr + 16);  t->blob_len = infy_art_rd_u32(tr + 20);
        t->meta_off = infy_art_rd_u32(tr + 24);  t->meta_len = infy_art_rd_u32(tr + 28);
        t->ifbc_off = infy_art_rd_u32(tr + 32);  t->ifbc_len = infy_art_rd_u32(tr + 36);
        t->sum_off = infy_art_rd_u32(tr + 40);   t->sum_len = infy_art_rd_u32(tr + 44);
        t->rodata_off = infy_art_rd_u32(tr + 48); t->rodata_len = infy_art_rd_u32(tr + 52);
        t->trailer_crc = crc;
        t->build_id_low = infy_art_rd_u32(tr + 60);
    }
    if (t->hdr_ver != INFY_ART_HDR_VER) { *why = e_ver; return -1; }
    if (t->ifabi != INFY_IFABI) { *why = e_ifabi; return -1; }
    uint64_t blob_end = (uint64_t)t->blob_off + t->blob_len;
    if (blob_end > len - INFY_ART_TRAILER_SIZE) { *why = e_range; return -1; }
    struct { uint32_t off, ln; } regions[4] = {
        { t->meta_off, t->meta_len }, { t->ifbc_off, t->ifbc_len },
        { t->sum_off, t->sum_len }, { t->rodata_off, t->rodata_len }
    };
    for (int i = 0; i < 4; i++)
        if ((uint64_t)regions[i].off + regions[i].ln > t->blob_len) { *why = e_range; return -1; }
    return 0;
}

static int parse_meta_record(const uint8_t* m, uint64_t n, InfyMetaFixed* out, const char** why) {
    if (n < INFY_META_FIXED_BYTES) { *why = ".inf meta fixed record is truncated"; return -1; }
    if (memcmp(m, INFY_META_MAGIC, 8) != 0) { *why = ".inf meta magic mismatch"; return -1; }
    memset(out, 0, sizeof *out);
    out->ifabi = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_IFABI);
    out->opt_level = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_OPT);
    out->overflow_policy = m[INFY_META_OFF_OVERFLOW];
    out->panic_radius = m[INFY_META_OFF_RADIUS];
    out->usize_bits = m[INFY_META_OFF_USIZE];
    out->fast_math = m[INFY_META_OFF_FASTMATH];
    out->lto = m[INFY_META_OFF_LTO];
    out->stack_bytes = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_STACK);
    out->focus_stack_bytes = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_FOCUS_STACK);
    out->focus_quota = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_QUOTA);
    out->iter_guard_steps = infy_art_rd_u64(m + INFY_META_OFF_ITER_GUARD);
    out->n_chains = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_N_CHAINS);
    out->n_events = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_N_EVENTS);
    out->n_nodes = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_N_NODES);
    out->n_ifbc_modules = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_N_IFBC);
    out->n_resource_keys = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_N_RESKEYS);
    out->off_chains = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_CHAINS);
    out->len_chains = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_CHAINS + 4);
    out->off_events = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_EVENTS);
    out->len_events = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_EVENTS + 4);
    out->off_nodes = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_NODES);
    out->len_nodes = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_NODES + 4);
    out->off_strings = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_STRINGS);
    out->len_strings = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_STRINGS + 4);
    out->off_ifbc = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_IFBC);
    out->len_ifbc = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_IFBC + 4);
    out->off_sum = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_SUM);
    out->len_sum = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_SUM + 4);
    out->off_export = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_EXPORT);
    out->len_export = (uint32_t)infy_art_rd_u32(m + INFY_META_OFF_EXPORT + 4);
    out->target_triple.ptr = (const uint8_t*)(uintptr_t)infy_art_rd_u32(m + INFY_META_OFF_S_TARGET);
    out->target_triple.len = infy_art_rd_u32(m + INFY_META_OFF_S_TARGET + 4);
    out->build_id.ptr = (const uint8_t*)(uintptr_t)infy_art_rd_u32(m + INFY_META_OFF_S_BUILDID);
    out->build_id.len = infy_art_rd_u32(m + INFY_META_OFF_S_BUILDID + 4);
    out->platform.ptr = (const uint8_t*)(uintptr_t)infy_art_rd_u32(m + INFY_META_OFF_S_PLATFORM);
    out->platform.len = infy_art_rd_u32(m + INFY_META_OFF_S_PLATFORM + 4);
    out->lang_version.ptr = (const uint8_t*)(uintptr_t)infy_art_rd_u32(m + INFY_META_OFF_S_LANGVER);
    out->lang_version.len = infy_art_rd_u32(m + INFY_META_OFF_S_LANGVER + 4);
    out->source_toolchain.ptr = (const uint8_t*)(uintptr_t)infy_art_rd_u32(m + INFY_META_OFF_S_TOOLCHAIN);
    out->source_toolchain.len = infy_art_rd_u32(m + INFY_META_OFF_S_TOOLCHAIN + 4);
    return 0;
}

static void meta_resolve_strings(InfyMetaFixed* m, const uint8_t* blob, uint64_t blob_len) {
    /* The string pool is a table of (u32 off, u32 len) pairs against the blob; the five string
     * fields of the fixed record carry *pool indices* in `ptr` until resolved here. */
    InfyStr* fields[5] = { &m->target_triple, &m->build_id, &m->platform, &m->lang_version,
                           &m->source_toolchain };
    uint64_t npool = m->len_strings / 8u;
    for (int i = 0; i < 5; i++) {
        uint64_t idx = (uint64_t)(uintptr_t)fields[i]->ptr;
        if (idx >= npool) { fields[i]->ptr = (const uint8_t*)""; fields[i]->len = 0; continue; }
        const uint8_t* e = blob + m->off_strings + idx * 8u;
        uint32_t off = (uint32_t)infy_art_rd_u32(e), ln = (uint32_t)infy_art_rd_u32(e + 4);
        if ((uint64_t)off + ln > blob_len) { fields[i]->ptr = (const uint8_t*)""; fields[i]->len = 0; continue; }
        fields[i]->ptr = blob + off;
        fields[i]->len = ln;
    }
}

int infy_art_read_file(const char* path, InfyMetaFixed* out_meta, uint8_t** blob, uint64_t* blob_len) {
    if (out_meta) memset(out_meta, 0, sizeof *out_meta);
    if (blob) *blob = 0;
    if (blob_len) *blob_len = 0;
    uint8_t* file = 0;
    uint64_t flen = 0;
    InfyIoErr err;
    memset(&err, 0, sizeof err);
    if (infy_fs_read_all(path, &file, &flen, &err) != 0) {
        fprintf(stderr, "artifact rejected: cannot read '%s': %s\n", path ? path : "",
                err.message.ptr ? (const char*)err.message.ptr : "io error");
        return -1;
    }
    InfyArtTrailer t;
    const char* why = 0;
    if (infy_art_parse_trailer(file, flen, &t, &why) != 0) {
        fprintf(stderr, "artifact rejected: %s\n", why);
        infy_free(file);
        return -1;
    }
    const uint8_t* bl = file + t.blob_off;
    InfyMetaFixed m;
    memset(&m, 0, sizeof m);
    if (parse_meta_record(bl + t.meta_off, t.meta_len, &m, &why) != 0) {
        fprintf(stderr, "artifact rejected: %s\n", why);
        infy_free(file);
        return -1;
    }
    if (m.ifabi != INFY_IFABI) {
        fprintf(stderr, "artifact rejected: .inf meta IFABI %u != runtime IFABI %u\n",
                (unsigned)m.ifabi, (unsigned)INFY_IFABI);
        infy_free(file);
        return -1;
    }
    if ((uint64_t)m.off_strings + m.len_strings > t.blob_len ||
        (uint64_t)m.off_chains + m.len_chains > t.blob_len ||
        (uint64_t)m.off_nodes + m.len_nodes > t.blob_len ||
        (uint64_t)m.off_ifbc + m.len_ifbc > t.blob_len ||
        (uint64_t)m.off_sum + m.len_sum > t.blob_len ||
        (uint64_t)m.off_export + m.len_export > t.blob_len) {
        fprintf(stderr, "artifact rejected: .inf meta segment range escapes the blob\n");
        infy_free(file);
        return -1;
    }
    /* The blob is handed out as an owned copy; the file buffer (which also holds the host image)
     * is released. malloc, not infy_alloc: this is tool-side data and must not show up in
     * std.rt.mem_stats(). */
    uint8_t* copy = (uint8_t*)malloc((size_t)(t.blob_len ? t.blob_len : 1));
    if (!copy) { infy_free(file); return -1; }
    memcpy(copy, bl, (size_t)t.blob_len);
    meta_resolve_strings(&m, copy, t.blob_len);
    infy_free(file);
    if (out_meta) *out_meta = m;
    if (blob) *blob = copy; else free(copy);
    if (blob_len) *blob_len = t.blob_len;
    return 0;
}

InfyStr infy_art_string(const InfyMetaFixed* m, const uint8_t* blob, uint32_t idx) {
    InfyStr s = { (const uint8_t*)"", 0 };
    if (!m || !blob) return s;
    uint64_t n = m->len_strings / 8u;
    if (idx >= n) return s;
    const uint8_t* e = blob + m->off_strings + (uint64_t)idx * 8u;
    uint32_t off = (uint32_t)infy_art_rd_u32(e), ln = (uint32_t)infy_art_rd_u32(e + 4);
    s.ptr = blob + off;
    s.len = ln;
    return s;
}

static void dump_str_field(FILE* out, const char* key, InfyStr s) {
    fprintf(out, "%-18s %.*s\n", key, (int)(s.len > 120 ? 120 : s.len),
            s.ptr ? (const char*)s.ptr : "");
}

void infy_art_dump_meta(const InfyMetaFixed* m, const uint8_t* blob, FILE* out) {
    if (!m || !out) return;
    fprintf(out, "INFY artifact (.inf meta)\n");
    fprintf(out, "%-18s %u\n", "ifabi", (unsigned)m->ifabi);
    fprintf(out, "%-18s %u\n", "opt_level", (unsigned)m->opt_level);
    fprintf(out, "%-18s %s\n", "overflow",
            m->overflow_policy == INFY_OV_SATURATE ? "saturate" : "panic");
    fprintf(out, "%-18s %s\n", "panic_radius",
            m->panic_radius == INFY_RADIUS_PROCESS ? "process"
            : m->panic_radius == INFY_RADIUS_CHAIN ? "chain" : "focus");
    fprintf(out, "%-18s %u\n", "usize_bits", (unsigned)m->usize_bits);
    fprintf(out, "%-18s %s\n", "fast_math", m->fast_math ? "true" : "false");
    fprintf(out, "%-18s %u\n", "lto", (unsigned)m->lto);
    fprintf(out, "%-18s %u\n", "stack_bytes", (unsigned)m->stack_bytes);
    fprintf(out, "%-18s %u\n", "focus_stack_bytes", (unsigned)m->focus_stack_bytes);
    fprintf(out, "%-18s %u\n", "focus_quota", (unsigned)m->focus_quota);
    fprintf(out, "%-18s %llu\n", "iter_guard_steps", (unsigned long long)m->iter_guard_steps);
    dump_str_field(out, "target", m->target_triple);
    dump_str_field(out, "platform", m->platform);
    dump_str_field(out, "build_id", m->build_id);
    dump_str_field(out, "lang_version", m->lang_version);
    dump_str_field(out, "source_toolchain", m->source_toolchain);
    fprintf(out, "%-18s %u\n", "n_chains", (unsigned)m->n_chains);
    fprintf(out, "%-18s %u\n", "n_events", (unsigned)m->n_events);
    fprintf(out, "%-18s %u\n", "n_nodes", (unsigned)m->n_nodes);
    fprintf(out, "%-18s %u\n", "n_ifbc_modules", (unsigned)m->n_ifbc_modules);
    fprintf(out, "%-18s %u\n", "n_resource_keys", (unsigned)m->n_resource_keys);
    fprintf(out, "segments\n");
    fprintf(out, "%-18s off=%-10u len=%u\n", "meta", 0u, 0u);
    fprintf(out, "%-18s off=%-10u len=%u\n", "chains", (unsigned)m->off_chains, (unsigned)m->len_chains);
    fprintf(out, "%-18s off=%-10u len=%u\n", "events", (unsigned)m->off_events, (unsigned)m->len_events);
    fprintf(out, "%-18s off=%-10u len=%u\n", "nodes", (unsigned)m->off_nodes, (unsigned)m->len_nodes);
    fprintf(out, "%-18s off=%-10u len=%u\n", "strings", (unsigned)m->off_strings, (unsigned)m->len_strings);
    fprintf(out, "%-18s off=%-10u len=%u\n", "ifbc", (unsigned)m->off_ifbc, (unsigned)m->len_ifbc);
    fprintf(out, "%-18s off=%-10u len=%u\n", "sum", (unsigned)m->off_sum, (unsigned)m->len_sum);
    fprintf(out, "%-18s off=%-10u len=%u\n", "export", (unsigned)m->off_export, (unsigned)m->len_export);
    if (blob && m->len_strings >= 8) {
        uint64_t n = m->len_strings / 8u;
        if (n > 64) n = 64;
        fprintf(out, "string pool (%llu shown)\n", (unsigned long long)n);
        for (uint64_t i = 0; i < n; i++) {
            InfyStr s = infy_art_string(m, blob, (uint32_t)i);
            fprintf(out, "  [%llu] %.*s\n", (unsigned long long)i,
                    (int)(s.len > 100 ? 100 : s.len), (const char*)s.ptr);
        }
    }
}

/* ---- the running executable's own container ---- */
static int self_path(char* out, size_t cap) {
#if defined(_WIN32)
    wchar_t wbuf[4096];
    DWORD n = GetModuleFileNameW(0, wbuf, 4096);
    if (n == 0 || n >= 4096) return -1;
    int w = WideCharToMultiByte(CP_UTF8, 0, wbuf, (int)n, out, (int)cap, 0, 0);
    return (w > 0 && (size_t)w < cap) ? 0 : -1;
#elif defined(__APPLE__)
    uint32_t n = (uint32_t)cap;
    if (_NSGetExecutablePath(out, &n) != 0) return -1;
    return 0;
#else
    ssize_t n = readlink("/proc/self/exe", out, cap - 1);
    if (n <= 0) return -1;
    out[n] = 0;
    return 0;
#endif
}

int infy_art_self(InfyMetaFixed* out_meta, uint8_t** blob, uint64_t* blob_len) {
    static int tried = 0;
    static int result = -1;
    static InfyMetaFixed meta;
    static uint8_t* cached_blob;
    static uint64_t cached_len;
    if (!tried) {
        char path[4096];
        tried = 1;
        if (self_path(path, sizeof path) == 0) {
            uint8_t* b = 0; uint64_t bl = 0;
            if (infy_art_read_file(path, &meta, &b, &bl) == 0) {
                result = 0; cached_blob = b; cached_len = bl;
            } else {
                free(b);
            }
        }
    }
    if (result != 0) return -1;
    if (out_meta) *out_meta = meta;
    if (blob) *blob = cached_blob;
    if (blob_len) *blob_len = cached_len;
    return 0;
}

/* ======================= std.fs / std.process (§10.3, §10.5) ======================= */

static void io_err(InfyIoErr* err, InfyIoErrKind kind, const char* msg, uint32_t raw) {
    if (!err) return;
    err->kind = kind;
    err->raw_errno = raw;
    err->message = infy_string_new();
    if (msg) infy_string_push_str(&err->message, infy_str_of(msg));
}

static InfyIoErrKind kind_from_errno(int e) {
#ifdef _WIN32
    switch (e) {
    case ERROR_FILE_NOT_FOUND: case ERROR_PATH_NOT_FOUND: return IE_NOTFOUND;
    case ERROR_ACCESS_DENIED: case ERROR_PRIVILEGE_NOT_HELD: return IE_DENIED;
    default: return IE_OTHER;
    }
#else
    switch (e) {
    case ENOENT: return IE_NOTFOUND;
    case EACCES: case EPERM: return IE_DENIED;
    case EISDIR: case ENOTDIR: case EXDEV: case EOPNOTSUPP: return IE_UNSUPPORTED;
    default: return IE_OTHER;
    }
#endif
}

#ifdef _WIN32
static int wpath(const char* utf8, wchar_t* wbuf, size_t wn) {
    int n = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, wbuf, (int)wn);
    return n > 0 ? 0 : -1;
}
#endif

int infy_fs_read_all(const char* path, uint8_t** out, uint64_t* len, InfyIoErr* err) {
    if (out) *out = 0;
    if (len) *len = 0;
    if (!path) { io_err(err, IE_UNSUPPORTED, "null path", 0); return -1; }
#ifdef _WIN32
    wchar_t w[4096];
    if (wpath(path, w, 4096) != 0) { io_err(err, IE_UNSUPPORTED, "path is not valid UTF-8", 0); return -1; }
    HANDLE h = CreateFileW(w, GENERIC_READ, FILE_SHARE_READ, 0, OPEN_EXISTING,
                           FILE_ATTRIBUTE_NORMAL, 0);
    if (h == INVALID_HANDLE_VALUE) {
        DWORD e = GetLastError();
        io_err(err, kind_from_errno((int)e), "open for read failed", (uint32_t)e);
        return -1;
    }
    LARGE_INTEGER sz;
    if (!GetFileSizeEx(h, &sz) || sz.QuadPart < 0) {
        DWORD e = GetLastError();
        CloseHandle(h);
        io_err(err, IE_OTHER, "GetFileSizeEx failed", (uint32_t)e);
        return -1;
    }
    uint64_t n = (uint64_t)sz.QuadPart;
    uint8_t* buf = (uint8_t*)infy_alloc(n ? n : 1, 1);
    uint64_t got = 0;
    while (got < n) {
        DWORD r = 0;
        uint32_t chunk = (uint32_t)((n - got) > 0x40000000ull ? 0x40000000ull : (n - got));
        if (!ReadFile(h, buf + got, chunk, &r, 0) || r == 0) break;
        got += r;
    }
    CloseHandle(h);
    if (got != n) {
        DWORD e = GetLastError();
        infy_free(buf);
        io_err(err, IE_PARTIAL, "short read", (uint32_t)e);
        return -1;
    }
    *out = buf; *len = n;
    return 0;
#else
    int fd = open(path, O_RDONLY);
    if (fd < 0) { io_err(err, kind_from_errno(errno), "open for read failed", (uint32_t)errno); return -1; }
    uint64_t cap = 4096, n = 0;
    uint8_t* buf = (uint8_t*)infy_alloc(cap, 1);
    for (;;) {
        if (n == cap) { cap *= 2; buf = (uint8_t*)infy_realloc(buf, cap / 2, cap, 1); }
        ssize_t r = read(fd, buf + n, (size_t)(cap - n));
        if (r < 0) { int e = errno; infy_free(buf); close(fd); io_err(err, IE_OTHER, "read failed", (uint32_t)e); return -1; }
        if (r == 0) break;
        n += (uint64_t)r;
    }
    close(fd);
    *out = buf; *len = n;
    return 0;
#endif
}

int infy_fs_write_all(const char* path, const uint8_t* data, uint64_t len, InfyIoErr* err) {
    if (!path) { io_err(err, IE_UNSUPPORTED, "null path", 0); return -1; }
#ifdef _WIN32
    wchar_t w[4096];
    if (wpath(path, w, 4096) != 0) { io_err(err, IE_UNSUPPORTED, "path is not valid UTF-8", 0); return -1; }
    HANDLE h = CreateFileW(w, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0);
    if (h == INVALID_HANDLE_VALUE) {
        DWORD e = GetLastError();
        io_err(err, kind_from_errno((int)e), "open for write failed", (uint32_t)e);
        return -1;
    }
    uint64_t off = 0;
    while (off < len) {
        DWORD w_done = 0;
        uint32_t chunk = (uint32_t)((len - off) > 0x40000000ull ? 0x40000000ull : (len - off));
        if (!WriteFile(h, data + off, chunk, &w_done, 0) || w_done == 0) {
            DWORD e = GetLastError();
            CloseHandle(h);
            io_err(err, IE_PARTIAL, "short write", (uint32_t)e);
            return -1;
        }
        off += w_done;
    }
    CloseHandle(h);
    return 0;
#else
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) { io_err(err, kind_from_errno(errno), "open for write failed", (uint32_t)errno); return -1; }
    uint64_t off = 0;
    while (off < len) {
        ssize_t w = write(fd, data + off, (size_t)(len - off));
        if (w <= 0) { int e = errno; close(fd); io_err(err, IE_PARTIAL, "short write", (uint32_t)e); return -1; }
        off += (uint64_t)w;
    }
    close(fd);
    return 0;
#endif
}

int infy_fs_exists(const char* path) {
    if (!path) return 0;
#ifdef _WIN32
#ifdef INFY_HAVE_GETFULLPATHATTRIB
    /* Legacy MSVCRT probe. The MSYS2 UCRT headers do not declare _getfullpathattrib, so this path
     * is opt-in at compile time; GetFileAttributesW below is the default and is equivalent for the
     * existence question §10.3 asks. */
    unsigned int attrib = 0;
    char full[4096];
    if (_getfullpathattrib(path, full, sizeof full, &attrib) == 0) return 1;
#endif
    wchar_t w[4096];
    if (wpath(path, w, 4096) != 0) return 0;
    DWORD a = GetFileAttributesW(w);
    return (a != INVALID_FILE_ATTRIBUTES) ? 1 : 0;
#else
    struct stat st;
    return stat(path, &st) == 0 ? 1 : 0;
#endif
}

int infy_fs_mkdir_p(const char* path, InfyIoErr* err) {
    if (!path) { io_err(err, IE_UNSUPPORTED, "null path", 0); return -1; }
    char buf[4096];
    size_t n = strlen(path);
    if (n == 0 || n >= sizeof buf) { io_err(err, IE_UNSUPPORTED, "path length not supported", 0); return -1; }
    memcpy(buf, path, n + 1);
#ifdef _WIN32
    for (size_t i = 0; i <= n; i++) if (buf[i] == '/') buf[i] = '\\';
#endif
    for (size_t i = 1; i <= n; i++) {
        char c = buf[i];
        int is_sep = (c == '/' || c == '\\' || c == 0);
        if (!is_sep) continue;
        char save = buf[i];
        buf[i] = 0;
        if (i > 2 || buf[1] == 0) {                     /* skip drive letters like "C:" */
#ifdef _WIN32
            wchar_t w[4096];
            if (wpath(buf, w, 4096) == 0) {
                DWORD a = GetFileAttributesW(w);
                if (a == INVALID_FILE_ATTRIBUTES) {
                    if (_wmkdir(w) != 0 && GetLastError() != ERROR_ALREADY_EXISTS) {
                        DWORD e = GetLastError();
                        buf[i] = save;
                        io_err(err, kind_from_errno((int)e), "mkdir failed", (uint32_t)e);
                        return -1;
                    }
                }
            }
#else
            struct stat st;
            if (stat(buf, &st) != 0) {
                if (mkdir(buf, 0755) != 0 && errno != EEXIST) {
                    int e = errno;
                    buf[i] = save;
                    io_err(err, kind_from_errno(e), "mkdir failed", (uint32_t)e);
                    return -1;
                }
            }
#endif
        }
        buf[i] = save;
    }
    return 0;
}

/* `InfyFileHandle` carries only a buffer (no fd, no path — see README "header gaps"), so File is a
 * whole-file buffer: open() loads, close() flushes for write modes and releases the buffer. Drop
 * closing it is what §10.3 requires. */
typedef struct { char path[4096]; uint8_t writable; } FileSide;
#define INFY_MAX_FILES 128
static struct { uint8_t* buf; FileSide side; } g_files[INFY_MAX_FILES];

static int file_slot_bind(uint8_t* buf, const char* path, uint8_t writable) {
    for (int i = 0; i < INFY_MAX_FILES; i++) {
        if (g_files[i].buf == 0) {
            g_files[i].buf = buf;
            snprintf(g_files[i].side.path, sizeof g_files[i].side.path, "%s", path);
            g_files[i].side.writable = writable;
            return i;
        }
    }
    return -1;
}
static FileSide* file_slot_find(uint8_t* buf) {
    for (int i = 0; i < INFY_MAX_FILES; i++) if (g_files[i].buf == buf) return &g_files[i].side;
    return 0;
}
static void file_slot_release(uint8_t* buf) {
    for (int i = 0; i < INFY_MAX_FILES; i++) if (g_files[i].buf == buf) { g_files[i].buf = 0; g_files[i].side.path[0] = 0; }
}

int infy_fs_open(const char* path, const char* mode, InfyFileHandle* out, InfyIoErr* err) {
    if (!path || !out) { io_err(err, IE_UNSUPPORTED, "null path or handle", 0); return -1; }
    const char* m = mode ? mode : "r";
    uint8_t writable = (strchr(m, 'w') || strchr(m, 'a') || strchr(m, '+')) ? 1 : 0;
    uint8_t truncate = strchr(m, 'w') ? 1 : 0;
    memset(out, 0, sizeof *out);
    out->readonly = writable ? 0 : 1;
    if (!truncate) {
        uint64_t n = 0; uint8_t* buf = 0;
        if (infy_fs_read_all(path, &buf, &n, err) != 0) {
            if (err && err->kind == IE_NOTFOUND && strchr(m, 'a')) {
                err->kind = IE_NONE;          /* append creates the file (§10.3) */
            } else {
                return -1;
            }
        } else {
            out->buf = buf; out->len = n; out->cap = n;
        }
    }
    if (!out->buf) { out->buf = (uint8_t*)infy_alloc(1, 1); out->cap = 1; out->len = 0; }
    file_slot_bind(out->buf, path, writable);
    return 0;
}

void infy_fs_close(InfyFileHandle* f) {
    if (!f || !f->buf) return;
    FileSide* side = file_slot_find(f->buf);
    if (side && side->writable) {
        InfyIoErr err; memset(&err, 0, sizeof err);
        infy_fs_write_all(side->path, f->buf, f->len, &err);
        if (err.message.ptr) infy_string_drop(&err.message);
    }
    file_slot_release(f->buf);
    infy_free(f->buf);
    f->buf = 0; f->len = 0; f->cap = 0;
}

#ifdef _WIN32
static void quote_win_arg(const char* utf8, wchar_t* out, size_t cap) {
    /* MSVC argv quoting: wrap in quotes, double embedded ones, backslashes before a quote double */
    wchar_t w[4096];
    if (MultiByteToWideChar(CP_UTF8, 0, utf8, -1, w, 4096) <= 0) { out[0] = 0; return; }
    size_t k = 0;
    if (k + 1 < cap) out[k++] = L'"';
    for (size_t i = 0; w[i]; i++) {
        if (w[i] == L'"') { if (k + 2 < cap) { out[k++] = L'\\'; out[k++] = L'"'; } }
        else if (k + 1 < cap) out[k++] = w[i];
    }
    if (k + 1 < cap) out[k++] = L'"';
    out[k] = 0;
}
#endif

int infy_process_exec(const char* cmd, const char* const* argv, uint32_t n_argv, uint32_t timeout_ms,
                      InfyExecResult* out, InfyIoErr* err) {
    if (out) { memset(out, 0, sizeof *out); out->stdout_bytes = infy_string_new();
               out->stderr_bytes = infy_string_new(); out->code = -1; }
    if (!cmd) { io_err(err, IE_UNSUPPORTED, "null command", 0); return -1; }
#ifdef _WIN32
    /* stdout/stderr go to temp files rather than pipes: a pipe that fills while the parent waits
     * deadlocks, and this runtime promises no threading inside std.process. */
    wchar_t exe[4096], line[16384];
    if (MultiByteToWideChar(CP_UTF8, 0, cmd, -1, exe, 4096) <= 0) {
        io_err(err, IE_UNSUPPORTED, "command is not valid UTF-8", 0); return -1;
    }
    quote_win_arg(cmd, line, 16384);
    size_t l = wcslen(line);
    for (uint32_t i = 0; i < n_argv; i++) {
        if (!argv[i]) continue;
        if (l + 2 >= 16384) break;
        line[l++] = L' '; line[l] = 0;
        wchar_t q[4096];
        quote_win_arg(argv[i], q, 4096);
        size_t ql = wcslen(q);
        if (l + ql >= 16384) break;
        wcscpy(line + l, q);
        l += ql;
        line[l] = 0;
    }
    static uint32_t tmp_seq = 0;
    char outp[512], errp[512];
    uint32_t seq = ++tmp_seq;
    snprintf(outp, sizeof outp, "infy-exec-out-%lu.tmp", (unsigned long)seq);
    snprintf(errp, sizeof errp, "infy-exec-err-%lu.tmp", (unsigned long)seq);
    wchar_t wo[512], we[512];
    wpath(outp, wo, 512); wpath(errp, we, 512);
    HANDLE ho = CreateFileW(wo, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0);
    HANDLE he = CreateFileW(we, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0);
    SECURITY_ATTRIBUTES sa; memset(&sa, 0, sizeof sa);
    sa.nLength = sizeof sa; sa.bInheritHandle = 1;
    STARTUPINFOW si; memset(&si, 0, sizeof si);
    si.cb = sizeof si;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdOutput = ho; si.hStdError = he; si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    PROCESS_INFORMATION pi; memset(&pi, 0, sizeof pi);
    BOOL ok = CreateProcessW(exe, line, 0, 0, TRUE, 0, 0, 0, &si, &pi);
    if (ho) CloseHandle(ho);
    if (he) CloseHandle(he);
    if (!ok) {
        DWORD e = GetLastError();
        DeleteFileW(wo); DeleteFileW(we);
        io_err(err, kind_from_errno((int)e), "CreateProcessW failed", (uint32_t)e);
        return -1;
    }
    DWORD wait = timeout_ms ? WaitForSingleObject(pi.hProcess, timeout_ms) : WaitForSingleObject(pi.hProcess, INFINITE);
    if (wait == WAIT_TIMEOUT) {
        TerminateProcess(pi.hProcess, 1);
        out->code = -1;
    }
    DWORD code = 0;
    GetExitCodeProcess(pi.hProcess, &code);
    if (wait != WAIT_TIMEOUT) out->code = (int32_t)code;
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    {
        uint8_t* b = 0; uint64_t n = 0; InfyIoErr e2; memset(&e2, 0, sizeof e2);
        if (infy_fs_read_all(outp, &b, &n, &e2) == 0) {
            infy_string_push_str(&out->stdout_bytes, (InfyStr){ b, n });
            infy_free(b);
        }
        if (e2.message.ptr) infy_string_drop(&e2.message);
        b = 0; n = 0; memset(&e2, 0, sizeof e2);
        if (infy_fs_read_all(errp, &b, &n, &e2) == 0) {
            infy_string_push_str(&out->stderr_bytes, (InfyStr){ b, n });
            infy_free(b);
        }
        if (e2.message.ptr) infy_string_drop(&e2.message);
        _unlink(outp); _unlink(errp);
    }
    return 0;
#else
    int op[2], ep[2];
    if (pipe(op) != 0 || pipe(ep) != 0) {
        io_err(err, IE_OTHER, "pipe() failed", (uint32_t)errno);
        return -1;
    }
    /* argv assembly: argv[0..n_argv) are the arguments, cmd is the program (§10.5) */
    char** av = (char**)calloc((size_t)n_argv + 2, sizeof(char*));
    av[0] = (char*)cmd;
    for (uint32_t i = 0; i < n_argv; i++) av[i + 1] = (char*)argv[i];
    av[n_argv + 1] = 0;
    pid_t pid = fork();
    if (pid < 0) { free(av); io_err(err, IE_OTHER, "fork() failed", (uint32_t)errno); return -1; }
    if (pid == 0) {
        dup2(op[1], 1); dup2(ep[1], 2);
        close(op[0]); close(op[1]); close(ep[0]); close(ep[1]);
        execvp(cmd, av);
        _exit(127);
    }
    free(av);
    close(op[1]); close(ep[1]);
    uint64_t deadline = timeout_ms ? (infy_monotonic_ms() + timeout_ms) : 0;
    for (;;) {
        fd_set rs;
        FD_ZERO(&rs);
        FD_SET(op[0], &rs);
        FD_SET(ep[0], &rs);
        struct timeval tv, *ptv = 0;
        if (deadline) {
            uint64_t now = infy_monotonic_ms();
            uint64_t left = now < deadline ? deadline - now : 0;
            tv.tv_sec = (long)(left / 1000u); tv.tv_usec = (long)((left % 1000u) * 1000u);
            ptv = &tv;
        }
        int r = select(op[0] > ep[0] ? op[0] + 1 : ep[0] + 1, &rs, 0, 0, ptv);
        if (r <= 0) break;                     /* timeout or error: stop reading, reap below */
        if (FD_ISSET(op[0], &rs)) {
            uint8_t b[4096]; ssize_t k = read(op[0], b, sizeof b);
            if (k > 0) infy_string_push_str(&out->stdout_bytes, (InfyStr){ b, (uint64_t)k });
        }
        if (FD_ISSET(ep[0], &rs)) {
            uint8_t b[4096]; ssize_t k = read(ep[0], b, sizeof b);
            if (k > 0) infy_string_push_str(&out->stderr_bytes, (InfyStr){ b, (uint64_t)k });
        }
    }
    int st = 0;
    if (waitpid(pid, &st, 0) < 0) st = -1;
    out->code = WIFEXITED(st) ? (int32_t)WEXITSTATUS(st) : -1;
    close(op[0]); close(ep[0]);
    if (!WIFEXITED(st) && !WIFSIGNALED(st)) { kill(pid, SIGKILL); }
    return 0;
#endif
}

/* ======================= module entry / startup order (§5.4, §7.4) ======================= */

void infy_run_package_inits(const InfyInitFn* fns, uint32_t n) {
    /* The compiler emits the callee list in the normative order: packages in reverse-topological
     * order, files by bytewise name order, within a package in bytewise file order (§5.4). Running
     * them here keeps that order in one place and never reorders it. */
    if (!fns) return;
    init_mono_base_once();
    for (uint32_t i = 0; i < n; i++) {
        if (fns[i]) fns[i]();
    }
}

static const char* default_platform_name(void) {
#if defined(_WIN32)
    return "win10";
#elif defined(__APPLE__)
    return "macos";
#else
    return "linux";
#endif
}

int infy_runtime_start(uint32_t entry_chain, uint32_t entry_node) {
    init_mono_base_once();
    if (!g_env_platform.ptr) g_env_platform = infy_str_of(default_platform_name());
    InfyMetaFixed m;
    uint8_t* blob = 0;
    uint64_t blen = 0;
    if (infy_art_self(&m, &blob, &blen) == 0) {
        infy_set_profile_from_meta(&m);       /* overflow / radius / quota / stacks all come from meta */
    } else {
        /* No readable container: keep the strict defaults (§7.2 — checks are never off) but say so
         * once, because §7.4 requires a runtime to be self-describing. */
        fprintf(stderr, "runtime: no INFY artifact container in this image; using strict defaults "
                        "(overflow=panic, panic_radius=focus)\n");
    }
    if (!infy_sched.inited) {
        infy_sched_init(worker_threads_to_use(), infy_profile.focus_stack_bytes,
                        infy_profile.stack_bytes);
    }
    g_chains_started = 1;
    bump_entry_iterations();               /* env.iter without a host = how often the entry chain ran */
    infy_focus_spawn(entry_chain, entry_node, infy_current_node_loc());
    int code = infy_sched_run_until_idle();
    infy_leak_check();                        /* R-0723 at the stop point (§8.2) */
    return code;
}

/* the log sink B-track print() maps to (§7.7.1 item 4) */
void infy_log(uint8_t level, InfyStr msg, const char* origin_lang, uint32_t origin_line) {
    static const char* names[5] = { "trace", "debug", "info", "warn", "error" };
    const char* lvl = level < 5 ? names[level] : "info";
    fprintf(stderr, "[%s] %s:%u: %.*s\n", lvl, origin_lang ? origin_lang : "b-track",
            (unsigned)origin_line, (int)msg.len, msg.ptr ? (const char*)msg.ptr : "");
    fflush(stderr);
}
/* ======================= coverage notes =======================
 * Not implemented in this translation unit, by design:
 *  - std.net (§10.4): no sockets, no TLS, no network code anywhere in this file.
 *  - the IFBC VM and the `dyn` helpers (infy_rt.h "IFBC virtual machine" block): owned by
 *    runtime/infy_ifbc.c and runtime/infy_dyn.c; nothing here calls them.
 * Everything else declared in infy_rt.h has a definition in this file.
 */
