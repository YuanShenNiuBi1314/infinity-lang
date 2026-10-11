/* test_rt.c — standalone conformance harness for the INFY v2.0 embedded runtime.
 *
 * It links against runtime/infy_rt.o only (no compiler front end needed) and asserts the
 * normative behaviour of §4.4/§4.9/§4.12 (checked arithmetic), §4.7.4 + §3.13 (drop/defer order),
 * §4.16 (panic radius), §8.2–§8.5 (focuses, FIFO ResourceLock, R-0802), §9.2 (byte-exact
 * diagnostics), §10.11 (fmt), §10.12 (time/rand), §10.13 (coll), §6.7 (xxHash64, Debug),
 * §7.4 (artifact container) and §11.3 (host handshake).
 *
 * Diagnostics go to rt_diag.log (stderr is redirected at startup) so the console shows only the
 * assertion results. §9.2 rendering is asserted byte-for-byte by capturing stderr into
 * rt_render.log for one case.
 *
 * Run with `--dump-seed <hex>` to print the rng stream / shuffle permutation for the
 * cross-process reproducibility check (see build_tests.ps1).
 */
#include "infy_rt.h"
#include "infy_rt_internal.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <pthread.h>
#endif

static int g_checks = 0;
static int g_failures = 0;

#define CK(cond) do {                                                              \
        g_checks++;                                                                \
        if (!(cond)) {                                                             \
            g_failures++;                                                          \
            printf("  FAIL %s:%d  %s\n", __FILE__, __LINE__, #cond);               \
        }                                                                          \
    } while (0)

#define CK_EQ(a, b) do {                                                           \
        long long _x = (long long)(a), _y = (long long)(b);                        \
        g_checks++;                                                                \
        if (_x != _y) {                                                            \
            g_failures++;                                                          \
            printf("  FAIL %s:%d  %s == %lld, want %lld\n", __FILE__, __LINE__, #a, _x, _y); \
        }                                                                          \
    } while (0)

#define CK_STR(actual, want) do {                                                  \
        const char* _a = (actual); const char* _w = (want);                        \
        g_checks++;                                                                \
        if (!_a || strcmp(_a, _w) != 0) {                                          \
            g_failures++;                                                          \
            printf("  FAIL %s:%d\n    got : [%s]\n    want: [%s]\n",               \
                   __FILE__, __LINE__, _a ? _a : "(null)", _w);                     \
        }                                                                          \
    } while (0)

static void section(const char* name) { printf("-- %s\n", name); }

/* ============================ panic plumbing ============================ */

static InfyNodeLoc LOC(uint32_t line, uint32_t col, uint32_t node) {
    InfyNodeLoc l;
    l.file = "test/harness.infy";
    l.line = line; l.col = col; l.node_idx = node; l.linkage_count = 0;
    return l;
}

static void (*g_fn0)(void);
static void shim(void* ignored) { (void)ignored; if (g_fn0) g_fn0(); }

/* Asserts that `f` panics with exactly `want` (and, for the negative cases, that it does not
 * return normally). */
static void expect_panic(const char* name, InfyRCode want, void (*f)(void)) {
    g_fn0 = f;
    InfyRCode got = (InfyRCode)0;
    int caught = infy_rt_catch(shim, 0, &got);
    g_checks++;
    if (!caught || got != want) {
        g_failures++;
        printf("  FAIL %s: wanted R-%04X, %s R-%04X\n", name, (unsigned)want,
               caught ? "got" : "no panic (returned normally)", (unsigned)got);
    }
}

/* ============================ observed reports ============================ */

static int g_obs_count = 0;
static uint32_t g_obs_focus[16], g_obs_chain[16], g_obs_node[16], g_obs_code[16];

static void observer(InfyRCode code, const char* detail, uint32_t focus, uint32_t chain,
                     const InfyNodeLoc* loc) {
    (void)detail;
    if (g_obs_count < 16) {
        g_obs_code[g_obs_count] = (uint32_t)code;
        g_obs_focus[g_obs_count] = focus;
        g_obs_chain[g_obs_count] = chain;
        g_obs_node[g_obs_count] = loc ? loc->node_idx : 0;
    }
    g_obs_count++;
}

/* ============================ process-radius capture ============================
 * The `process` panic radius terminates with exit(3) (§13.4). The runtime offers an exit hook so a
 * harness can observe that without dying; it must not return, so it longjumps back to the test. */
static jmp_buf g_proc_exit_jb;
static int g_proc_exit_code = 0;
static int g_proc_exit_armed = 0;
static void proc_exit_hook(int code) {
    g_proc_exit_code = code;
    if (g_proc_exit_armed) {
        g_proc_exit_armed = 0;
        longjmp(g_proc_exit_jb, 1);
    }
    exit((unsigned)code);
}

/* ============================ 1. checked arithmetic ============================ */

static volatile uint64_t g_sink;   /* keep the "ok" results observable for the optimiser */

#define MIN64_TEST (-(int64_t)9223372036854775807LL - 1)
#define MAX64_TEST ((int64_t)9223372036854775807LL)

static void e_add_u8_ov(void)   { g_sink = infy_add_u8(255, 1, &LOC(1, 1, 0)); }
static void e_sub_u16_ov(void)  { g_sink = infy_sub_u16(0, 1, &LOC(2, 1, 0)); }
static void e_mul_u32_ov(void)  { g_sink = infy_mul_u32(0x80000u, 0x10000u, &LOC(3, 1, 0)); }
static void e_add_i8_ov(void)   { g_sink = (uint64_t)infy_add_i8(127, 1, &LOC(4, 1, 0)); }
static void e_sub_i8_ov(void)   { g_sink = (uint64_t)infy_sub_i8(-128, 1, &LOC(5, 1, 0)); }
static void e_mul_i64_ov(void)  { g_sink = (uint64_t)infy_mul_i64(0x4000000000000000LL, 4, &LOC(6, 1, 0)); }
static void e_div_min(void)     { g_sink = (uint64_t)infy_div_i64(MIN64_TEST, -1, &LOC(7, 1, 0)); }
static void e_rem_min(void)     { g_sink = (uint64_t)infy_rem_i64(MIN64_TEST, -1, &LOC(8, 1, 0)); }
static void e_div_u32_zero(void){ g_sink = infy_div_u32(7, 0, &LOC(9, 1, 0)); }
static void e_div_i64_zero(void){ g_sink = (uint64_t)infy_div_i64(7, 0, &LOC(10, 1, 0)); }
static void e_rem_u8_zero(void) { g_sink = infy_rem_u8(7, 0, &LOC(11, 1, 0)); }
static void e_shl_64(void)      { g_sink = infy_shl_u64(1, 64, &LOC(12, 1, 0)); }
static void e_shr_neg(void)     { g_sink = (uint64_t)infy_shr_i64(-8, 64, &LOC(13, 1, 0)); }
static void e_remf_zero(void)   { (void)infy_remf64(1.0, 0.0, &LOC(14, 1, 0)); }
static void e_remf_inf(void)    { g_sink = (uint64_t)infy_remf64(INFINITY, 2.0, &LOC(15, 1, 0)); }
static void e_index_oob(void)   { InfyVec v = { (void*)"ab", 2, 2 }; (void)infy_vec_index(&v, 2, 1, &LOC(16, 1, 0)); }
static void e_slice_oob(void)   { InfySlice s = { (void*)"abcd", 4 }; (void)infy_slice_index(&s, 4, 1, &LOC(17, 1, 0)); }
static void e_str_nul(void)     { static const uint8_t b[3] = {'a', 0, 'b'}; InfyStr s = { b, 3 };
                                  infy_str_check_nul(s, &LOC(18, 1, 0)); }
static void e_cstr_nul(void)    { static const uint8_t b[2] = {'a', 0}; InfyStr s = { b, 2 };
                                  char* c = infy_to_cstr(s, &LOC(19, 1, 0)); free(c); }
static void e_unwrap_none(void) { infy_panic(R_UNWRAP_NONE, "unwrap of None (§4.9 UB-009)", &LOC(20, 1, 0)); }
static void e_bool_bits(void)   { infy_panic(R_BAD_BOOL, "byte 0x02 is not a bool (§4.1)", &LOC(21, 1, 0)); }
static void e_shift_u8(void)    { infy_panic(R_SHIFT_WIDTH, "shift 8 on u8 (§4.12)", &LOC(22, 1, 0)); }
static void e_rng_empty(void)   { InfyRng r = infy_rng_seeded(1); (void)infy_rng_choice_index(&r, 0); }
static void e_rng_range_empty(void) { InfyRng r = infy_rng_seeded(1); (void)infy_rng_range(&r, 5, 5); }
static void e_atrack_null(void) { (void)infy_atrack_ptr_in(0, 16, &LOC(23, 1, 0)); }
static void e_atrack_err(void)  { infy_atrack_err_out(0, &LOC(24, 1, 0)); }
static void e_focus_wrap(void)  { (void)infy_focus_spawn(0, 0, &LOC(25, 1, 0)); }
static void e_goto_missing(void){ infy_goto_checked(0, 999, &LOC(26, 1, 0)); }
static void e_radix(void)       { infy_panic(R_RADIX, "from_str_radix radix 37 (§10.9)", &LOC(27, 1, 0)); }
static void e_iter_guard(void)  { infy_panic(R_ITER_GUARD, "iterator exceeded iter_guard_steps (§10.10)", &LOC(28, 1, 0)); }
static void e_push_nul(void)    { InfyString s = infy_string_new(); infy_string_push(&s, 0); infy_string_drop(&s); }
static void e_push_surr(void)   { InfyString s = infy_string_new(); infy_string_push(&s, 0xD800); infy_string_drop(&s); }

static void test_arithmetic(void) {
    section("checked arithmetic (§4.4, §4.9, §4.12)");

    /* overflow -> panic under the default profile */
    expect_panic("add_u8 255+1", R_INT_OVERFLOW, e_add_u8_ov);
    expect_panic("sub_u16 0-1", R_INT_OVERFLOW, e_sub_u16_ov);
    expect_panic("mul_u32", R_INT_OVERFLOW, e_mul_u32_ov);
    expect_panic("add_i8 127+1", R_INT_OVERFLOW, e_add_i8_ov);
    expect_panic("sub_i8 -128-1", R_INT_OVERFLOW, e_sub_i8_ov);
    expect_panic("mul_i64", R_INT_OVERFLOW, e_mul_i64_ov);
    expect_panic("i64::MIN / -1", R_INT_OVERFLOW, e_div_min);
    expect_panic("i64::MIN % -1", R_INT_OVERFLOW, e_rem_min);

    /* ... and saturate under overflow = "saturate" (§4.4), with no panic at all */
    infy_profile.overflow = INFY_OV_SATURATE;
    CK_EQ(infy_add_u8(255, 1, &LOC(1, 1, 0)), 255);
    CK_EQ(infy_sub_u16(0, 1, &LOC(1, 1, 0)), 0);
    CK_EQ(infy_mul_u32(0x80000u, 0x10000u, &LOC(1, 1, 0)), 0xFFFFFFFFu);
    CK_EQ(infy_add_i8(127, 1, &LOC(1, 1, 0)), 127);
    CK_EQ(infy_sub_i8(-128, 1, &LOC(1, 1, 0)), -128);
    CK_EQ(infy_div_i64(MIN64_TEST, -1, &LOC(1, 1, 0)), MAX64_TEST);
    CK_EQ(infy_rem_i64(MIN64_TEST, -1, &LOC(1, 1, 0)), 0);
    infy_profile.overflow = INFY_OV_PANIC;

    /* div/rem by zero is R-0002, never silent (§4.9 UB-004) */
    expect_panic("div_u32 by 0", R_DIV_ZERO, e_div_u32_zero);
    expect_panic("div_i64 by 0", R_DIV_ZERO, e_div_i64_zero);
    expect_panic("rem_u8 by 0", R_DIV_ZERO, e_rem_u8_zero);
    expect_panic("f64 % 0.0", R_DIV_ZERO, e_remf_zero);
    expect_panic("inf % x", R_INF_REM, e_remf_inf);

    /* shift width >= type width is R-0007, not a no-op (§4.12) */
    expect_panic("shl by 64", R_SHIFT_WIDTH, e_shl_64);
    expect_panic("shr by 64", R_SHIFT_WIDTH, e_shr_neg);
    expect_panic("shift u8 width", R_SHIFT_WIDTH, e_shift_u8);

    /* index / slice bounds are R-0004 (§4.9 UB-008) */
    expect_panic("vec index oob", R_INDEX_OOB, e_index_oob);
    expect_panic("slice index oob", R_INDEX_OOB, e_slice_oob);

    /* NUL in a str crossing an A-track boundary is R-0003 (§4.9 UB-007) */
    expect_panic("str_check_nul", R_NUL_IN_STR, e_str_nul);
    expect_panic("to_cstr with NUL", R_NUL_IN_STR, e_cstr_nul);

    /* other normative R- codes the runtime itself raises */
    expect_panic("unwrap None", R_UNWRAP_NONE, e_unwrap_none);
    expect_panic("bool bit pattern", R_BAD_BOOL, e_bool_bits);
    expect_panic("rng choice(0)", R_INDEX_OOB, e_rng_empty);
    expect_panic("rng empty range", R_INDEX_OOB, e_rng_range_empty);
    expect_panic("A-track null in", R_ATRACK_POINTER, e_atrack_null);
    expect_panic("A-track err not written", R_ATRACK_ERR_OUT, e_atrack_err);
    expect_panic("radix out of range", R_RADIX, e_radix);
    expect_panic("iter guard", R_ITER_GUARD, e_iter_guard);

    /* §4.12: division truncates toward zero, % takes the sign of the dividend */
    CK_EQ(infy_div_i32(-7, 2, &LOC(1, 1, 0)), -3);
    CK_EQ(infy_div_i32(7, -2, &LOC(1, 1, 0)), -3);
    CK_EQ(infy_div_i32(-7, -2, &LOC(1, 1, 0)), 3);
    CK_EQ(infy_rem_i32(-7, 2, &LOC(1, 1, 0)), -1);
    CK_EQ(infy_rem_i32(7, -2, &LOC(1, 1, 0)), 1);
    CK_EQ(infy_rem_i64(-7, -2, &LOC(1, 1, 0)), -1);
    /* the §4.12 identity: (a/b)*b + a%b == a */
    for (int64_t a = -20; a <= 20; a++) {
        for (int64_t b = -5; b <= 5; b++) {
            if (b == 0) continue;
            int64_t q = infy_div_i64(a, b, &LOC(1, 1, 0));
            int64_t r = infy_rem_i64(a, b, &LOC(1, 1, 0));
            CK_EQ(q * b + r, a);
            CK((b > 0) == 1 || r <= 0 || r == 0);           /* sign of r follows a, not b */
            if (a > 0) CK(r >= 0 ? 1 : 0);
            if (a < 0) CK(r <= 0 ? 1 : 0);
        }
    }
    /* no false positives on in-range arithmetic */
    CK_EQ(infy_add_u64(0xFFFFFFFFFFFFFFFFull, 0, &LOC(1, 1, 0)), 0xFFFFFFFFFFFFFFFFull);
    CK_EQ(infy_add_i64(MAX64_TEST - 1, 1, &LOC(1, 1, 0)), MAX64_TEST);
    CK_EQ(infy_mul_i64(MIN64_TEST, 1, &LOC(1, 1, 0)), MIN64_TEST);
    CK_EQ(infy_shl_u64(1, 63, &LOC(1, 1, 0)), 0x8000000000000000ull);
    CK_EQ(infy_shr_i64(-8, 1, &LOC(1, 1, 0)), -4);
    CK_EQ(infy_shr_i64(-1, 63, &LOC(1, 1, 0)), -1);
    CK_EQ(infy_shr_u64(0xFFFFFFFFFFFFFFFFull, 63, &LOC(1, 1, 0)), 1);
    CK_EQ(infy_wrapping_add_u64(0xFFFFFFFFFFFFFFFFull, 1), 0);
    CK_EQ(infy_wrapping_add_i64(MAX64_TEST, 1), MIN64_TEST);
    CK_EQ(infy_saturating_add_u64(0xFFFFFFFFFFFFFFFFull, 1), 0xFFFFFFFFFFFFFFFFull);
    CK_EQ(infy_saturating_add_i64(MAX64_TEST, 1), MAX64_TEST);
    CK_EQ(infy_saturating_add_i64(MIN64_TEST, -1), MIN64_TEST);
    CK_EQ(infy_wrapping_shl_u64(1, 64), 1);
    CK(fabs(infy_remf64(7.5, 2.0, &LOC(1, 1, 0)) - 1.5) < 1e-12);
    CK(fabs(infy_remf64(-7.5, 2.0, &LOC(1, 1, 0)) + 1.5) < 1e-12);
    CK(fabs(infy_remf32(5.5f, 2.0f, &LOC(1, 1, 0)) - 1.5f) < 1e-6f);
}

/* ============================ 2. drop / defer order (§4.7.4, §3.13) ============================ */

static char g_order[512];
static void rec(const char* tag) {
    size_t n = strlen(g_order);
    snprintf(g_order + n, sizeof g_order - n, "%s", tag);
}
struct Box { int id; };
static void drop_a(void* p) { (void)p; rec("(a)"); }
static void drop_b(void* p) { (void)p; rec("(b)"); }
static void drop_c(void* p) { (void)p; rec("(c)"); }
static void defer_x(void* p) { (void)p; rec("[x]"); }
static void defer_y(void* p) { (void)p; rec("[y]"); }
static void drop_panic(void* p) { (void)p; infy_panic(R_UNWRAP_NONE, "panic inside Drop", &LOC(50, 1, 0)); }

static void e_double_panic(void) {
    infy_scope_mark();
    infy_cleanup_push(drop_panic, 0, 0);
    infy_scope_end();          /* runs a Drop which panics: §4.16 -> process radius, R-0011 */
}

static void test_drop_order(void) {
    section("drop / defer order (§4.7.4, §3.13)");

    /* codegen emits: scope mark, push per binding in declaration order, scope end.
     * §4.7.4: locals drop in reverse declaration order; §3.13: defers run after every variable
     * drop of the scope, also in reverse declaration order. */
    g_order[0] = 0;
    infy_scope_mark();
    infy_cleanup_push(drop_a, 0, 0);
    infy_cleanup_push(defer_x, 0, 1);
    infy_cleanup_push(drop_b, 0, 0);
    infy_cleanup_push(defer_y, 0, 1);
    infy_cleanup_push(drop_c, 0, 0);
    infy_scope_end();
    CK_STR(g_order, "(c)(b)(a)[y][x]");

    /* nested scopes: the inner scope must not touch the outer scope's bindings */
    g_order[0] = 0;
    infy_scope_mark();
    infy_cleanup_push(drop_a, 0, 0);
    infy_scope_mark();
    infy_cleanup_push(drop_b, 0, 0);
    infy_scope_end();
    infy_scope_end();
    CK_STR(g_order, "(b)(a)");

    /* pop_run: the single-entry primitive declared in infy_rt.h runs the innermost entry */
    g_order[0] = 0;
    infy_cleanup_push(drop_a, 0, 0);
    infy_cleanup_push(drop_b, 0, 0);
    infy_cleanup_pop_run();
    infy_cleanup_pop_run();
    CK_STR(g_order, "(b)(a)");
    CK(g_order[0] != 0);

    /* #[no_drop]: pop without running (§4.7.4) */
    g_order[0] = 0;
    infy_cleanup_push(drop_a, 0, 0);
    infy_cleanup_push(drop_b, 0, 0);
    infy_cleanup_pop_noop();
    infy_cleanup_pop_noop();
    CK_STR(g_order, "");

    /* unwind drain is LIFO, drops then defers */
    g_order[0] = 0;
    infy_cleanup_push(defer_x, 0, 1);
    infy_cleanup_push(drop_a, 0, 0);
    infy_cleanup_push(drop_b, 0, 0);
    infy_cleanup_drain();
    CK_STR(g_order, "(b)(a)[x]");

    /* §4.16: a panic raised while a Drop runs is a double panic -> R-0011 + process radius.
     * The exit hook captures exit(3) so the harness survives it. */
    g_proc_exit_armed = 1;
    g_proc_exit_code = 0;
    if (setjmp(g_proc_exit_jb) == 0) {
        infy_test_set_exit_hook(proc_exit_hook);
        e_double_panic();          /* never returns normally */
        g_proc_exit_armed = 0;
        g_failures++;
        printf("  FAIL double panic returned normally\n");
    } else {
        g_proc_exit_armed = 0;
    }
    infy_test_set_exit_hook(0);
    CK_EQ(g_proc_exit_code, 3);    /* §13.4 exit code 3 for a runtime panic */
    /* longjmp skipped the unwinding flag reset — the runtime contract is that the process is
     * terminating at that point, so the harness restores it explicitly. */
    infy_tls_unwinding = 0;
    infy_tls_cleanup = 0;
    /* the panicking Drop's own entry was consumed; the ones before it stay on the deque */
    infy_cleanup_drain();
    g_order[0] = 0;
}

/* ============================ 3. Vec capacity growth (§10.13 ABI) ============================ */

static void test_vec_cap(void) {
    section("Vec growth factors (§10.13, runtime-abi §1)");

    CK_EQ(infy_vec_new_cap(0, 1), 4);            /* first allocation starts at 4 */
    CK_EQ(infy_vec_new_cap(0, 0), 0);            /* never grows below need */
    CK_EQ(infy_vec_new_cap(4, 4), 4);            /* already big enough */
    CK_EQ(infy_vec_new_cap(4, 5), 8);            /* x2 */
    CK_EQ(infy_vec_new_cap(8, 9), 16);
    CK_EQ(infy_vec_new_cap(512, 513), 1024);     /* top of the x2 band */
    CK_EQ(infy_vec_new_cap(1024, 1025), 1536);   /* x3/2 starts above 1024 */
    CK_EQ(infy_vec_new_cap(1000, 1001), 1500);   /* 1000*2 = 2000 > 1024 -> x3/2 */
    CK_EQ(infy_vec_new_cap(600, 601), 900);      /* inside the x3/2 band, monotone */
    CK_EQ(infy_vec_new_cap(1048576, 1048577), 1310720);   /* x5/2? no: x5/4 above 1 MiB */
    CK_EQ(infy_vec_new_cap(700000, 700001), 875000);      /* 1.5x would pass 1 MiB -> x5/4 */
    CK_EQ(infy_vec_new_cap(2, 100), 128);                 /* reserve applies the sequence until >= n */
    CK(infy_vec_new_cap(16, 1000000) >= 1000000);

    /* the growth is observable through capacity(), i.e. it *is* the ABI: pushing must land exactly
     * on these numbers, and growth never shrinks */
    InfyVec v = { 0, 0, 0 };
    for (int i = 0; i < 4; i++) infy_vec_push(&v, &i, sizeof(int), alignof(int));
    CK_EQ(v.len, 4);
    CK_EQ(v.cap, 4);
    int x = 4;
    infy_vec_push(&v, &x, sizeof(int), alignof(int));
    CK_EQ(v.cap, 8);
    CK_EQ(v.len, 5);
    for (int i = 5; i < 9; i++) infy_vec_push(&v, &i, sizeof(int), alignof(int));
    CK_EQ(v.cap, 16);
    /* elements are address-stable modulo reallocation and readable (§4.8) */
    for (int i = 0; i < 9; i++) CK_EQ(*(int*)infy_vec_index(&v, (uint64_t)i, sizeof(int), &LOC(1,1,0)), i);
    infy_vec_drop(&v, sizeof(int), 0);
    CK_EQ(v.cap, 0);

    /* String uses the same sequence (§10.13) */
    InfyString s = infy_string_new();
    CK_EQ(s.cap, 0);
    infy_string_push(&s, 'h');
    CK_EQ(s.cap, 4);
    for (int i = 0; i < 4; i++) infy_string_push(&s, 'i');
    CK_EQ(s.cap, 8);
    infy_string_drop(&s);
}

/* ============================ 4. str / NFC / Ord (§4.4, §2.10, §10.13) ============================ */

static void test_strings(void) {
    section("str equality/NFC + Ord (§4.4, §2.10)");

    /* "e" U+0301  ==  "é" U+00E9 : §4.4 says `==` on str compares NFC content */
    static const uint8_t decomposed[] = { 0x65, 0xCC, 0x81 };                /* e + COMBINING ACUTE */
    static const uint8_t composed[]   = { 0xC3, 0xA9 };                      /* U+00E9 */
    InfyStr a = { decomposed, sizeof decomposed };
    InfyStr b = { composed, sizeof composed };
    CK_EQ(infy_str_eq_nfc(a, b), 1);
    CK_EQ(infy_str_eq_nfc(b, a), 1);
    CK_EQ(infy_str_eq_nfc(a, a), 1);
    CK(infy_str_eq_nfc(a, infy_str_of("e")) == 0);

    /* Hangul is algorithmic: 한 = U+AC00 = L(1112) V(1161) T(11A8) */
    static const uint8_t hangul_s[]  = { 0xED, 0x95, 0x9C };                 /* U+D55C 한 */
    static const uint8_t hangul_j[]  = { 0xE1, 0x84, 0x92,                   /* U+1112 */
                                         0xEA, 0xB5, 0xAD,                   /* U+1161 */
                                         0xE0, 0xB2, 0xA8 };                 /* U+11A8 */
    CK_EQ(infy_str_eq_nfc((InfyStr){ hangul_s, sizeof hangul_s },
                          (InfyStr){ hangul_j, sizeof hangul_j }), 1);

    /* canonical *ordering* before composition: cedilla (ccc 202) must sort below an acute
     * (ccc 230), so the two byte-different inputs normalise to the same string */
    static const uint8_t seq1[] = { 'C', 0xCC, 0xA7, 0xCC, 0x81 };   /* C + cedilla + acute */
    static const uint8_t seq2[] = { 'C', 0xCC, 0x81, 0xCC, 0xA7 };   /* C + acute + cedilla */
    CK_EQ(infy_str_eq_nfc((InfyStr){ seq1, sizeof seq1 },
                          (InfyStr){ seq2, sizeof seq2 }), 1);
    /* two marks of the same class are not reordered, so these stay different (§2.10) */
    static const uint8_t ring_acute[] = { 'A', 0xCC, 0x8A, 0xCC, 0x81 };
    static const uint8_t acute_ring[] = { 'A', 0xCC, 0x81, 0xCC, 0x8A };
    CK_EQ(infy_str_eq_nfc((InfyStr){ ring_acute, sizeof ring_acute },
                          (InfyStr){ acute_ring, sizeof acute_ring }), 0);

    /* byte-identical strings are always equal (fast path, §4.4) */
    CK_EQ(infy_str_eq_nfc(infy_str_of("hello"), infy_str_of("hello")), 1);
    CK_EQ(infy_str_eq_nfc(infy_str_of(""), infy_str_of("")), 1);

    /* Ord: code-point order == UTF-8 byte order (§10.13 iteration must be stable) */
    CK_EQ(infy_str_cmp(infy_str_of("abc"), infy_str_of("abd")), -1);
    CK_EQ(infy_str_cmp(infy_str_of("abd"), infy_str_of("abc")), 1);
    CK_EQ(infy_str_cmp(infy_str_of("abc"), infy_str_of("abc")), 0);
    CK_EQ(infy_str_cmp(infy_str_of("ab"), infy_str_of("abc")), -1);
    CK_EQ(infy_str_cmp(infy_str_of(""), infy_str_of("a")), -1);
    CK_EQ(infy_str_cmp(infy_str_of("é"), infy_str_of("z")), -1);   /* U+00E9 < U+007A? no: 0xC3 > 0x7A */
    CK_EQ(infy_str_cmp(infy_str_of("a"), infy_str_of("A")), 1);

    /* str view of a literal does not copy (§4.1) */
    const char* lit = "rodata-backed";
    InfyStr sv = infy_str_of(lit);
    CK(sv.ptr == (const uint8_t*)lit);
    CK_EQ(sv.len, strlen(lit));

    /* String ops */
    InfyString s = infy_string_from(infy_str_of("ab"));
    infy_string_push_str(&s, infy_str_of("cd"));
    s = infy_string_concat(s, infy_str_of("!"));
    CK_STR((const char*)s.ptr, "abcd!");
    CK_EQ(s.len, 5);
    InfyStr view = infy_string_as_str(&s);
    CK_EQ(view.len, 5);
    infy_string_push(&s, 0x1F600);                          /* emoji: 4-byte UTF-8 */
    CK_EQ(s.len, 9);
    infy_string_drop(&s);
    CK_EQ(s.cap, 0);

    /* to_cstr is explicit (§7.6.4) and NUL-free */
    char* c = infy_to_cstr(infy_str_of("plain"), &LOC(1, 1, 0));
    CK_STR(c, "plain");
    infy_free(c);

    /* push of U+0000 is R-0003 (§4.9 UB-007); push of a surrogate is an illegal char (§4.1) */
    expect_panic("String::push(0)", R_NUL_IN_STR, e_push_nul);
    expect_panic("String::push(surrogate)", R_BAD_BOOL, e_push_surr);
}

/* ============================ 5. Map iterates by Ord (§10.13) ============================ */

static int cmp_u64(const void* x, const void* y) {
    uint64_t a = *(const uint64_t*)x, b = *(const uint64_t*)y;
    return a < b ? -1 : (a > b ? 1 : 0);
}
static uint64_t g_iter[64];
static int g_iter_n;
static uint64_t g_iter_val[64];
static void collect(const void* k, const void* v, void* ctx) {
    (void)ctx;
    if (g_iter_n < 64) { g_iter[g_iter_n] = *(const uint64_t*)k; g_iter_val[g_iter_n] = *(const uint64_t*)v; }
    g_iter_n++;
}

static void test_map(void) {
    section("Map ordered iteration (§10.13, §10.13 ord never hash)");

    InfyMap* m = infy_map_new(sizeof(uint64_t), sizeof(uint64_t), cmp_u64);
    /* insertion order deliberately unsorted, several replacements */
    static const uint64_t order[] = { 50, 3, 99, 7, 42, 1, 8, 8, 50, 23 };
    for (size_t i = 0; i < sizeof order / sizeof order[0]; i++) {
        uint64_t val = order[i] * 10 + 1;
        infy_map_put(m, &order[i], &val);
    }
    CK_EQ(infy_map_len(m), 8);
    g_iter_n = 0;
    infy_map_iter(m, collect, 0);
    CK_EQ(g_iter_n, 8);
    int sorted = 1;
    for (int i = 1; i < g_iter_n; i++) if (g_iter[i - 1] >= g_iter[i]) sorted = 0;
    CK(sorted);
    CK_EQ(g_iter[0], 1); CK_EQ(g_iter[1], 3); CK_EQ(g_iter[2], 7); CK_EQ(g_iter[3], 8);
    CK_EQ(g_iter[4], 23); CK_EQ(g_iter[5], 42); CK_EQ(g_iter[6], 50); CK_EQ(g_iter[7], 99);
    /* replacement kept the *latest* value and the original slot */
    uint64_t* p = (uint64_t*)infy_map_get(m, &(uint64_t){ 50 });
    CK(p != 0);
    if (p) CK_EQ(*p, 501);
    CK(infy_map_get(m, &(uint64_t){ 1000 }) == 0);
    infy_map_drop(m);

    /* a second Map built in a different insertion order iterates identically: the order is the
     * key Ord, never the hash/insertion order (§10.13, §12.4 reproducibility) */
    InfyMap* m2 = infy_map_new(sizeof(uint64_t), sizeof(uint64_t), cmp_u64);
    static const uint64_t order2[] = { 99, 42, 8, 50, 1, 7, 23, 3 };
    for (size_t i = 0; i < 8; i++) {
        uint64_t val = order2[i] * 10 + 1;
        infy_map_put(m2, &order2[i], &val);
    }
    uint64_t first1 = 0, first2 = 0;
    {
        g_iter_n = 0; infy_map_iter(m, collect, 0); first1 = g_iter[0];
        g_iter_n = 0; infy_map_iter(m2, collect, 0); first2 = g_iter[0];
    }
    CK_EQ(first1, first2);
    CK_EQ(infy_map_len(m2), 8);
    infy_map_drop(m2);
    infy_map_drop(m);

    /* VecMap keeps insertion order (§10.13) and the Deque is a FIFO ring (used by the scheduler) */
    {
        InfyVecMap vm;
        infy_vecmap_init(&vm, sizeof(uint64_t), sizeof(uint64_t));
        for (uint64_t i = 0; i < 5; i++) {
            uint64_t v = i * 100;
            infy_vecmap_put(&vm, &i, &v);
        }
        uint64_t key = 2, val = 777;
        infy_vecmap_put(&vm, &key, &val);           /* replacement keeps the slot */
        CK_EQ(vm.n, 5);
        CK_EQ(*(uint64_t*)infy_vecmap_get(&vm, &key), 777);
        key = 9; val = 900;
        infy_vecmap_put(&vm, &key, &val);
        CK_EQ(vm.n, 6);
        CK_EQ(vm.data[5 * (vm.key_size + vm.val_size) + vm.key_size], 0);  /* last inserted */
        CK_EQ(*(uint64_t*)(vm.data + 5 * (vm.key_size + vm.val_size) + vm.key_size), 900);
        infy_vecmap_free(&vm);

        InfyDeque d;
        infy_deque_init(&d, sizeof(uint64_t));
        for (uint64_t i = 0; i < 10; i++) infy_deque_push_back(&d, &i);
        CK_EQ(infy_deque_len(&d), 10);
        uint64_t out = 0;
        CK_EQ(infy_deque_pop_front(&d, &out), 1);
        CK_EQ(out, 0);
        for (int i = 0; i < 8; i++) CK_EQ(infy_deque_pop_front(&d, &out), 1);
        CK_EQ(out, 8);
        CK_EQ(infy_deque_pop_front(&d, &out), 1);
        CK_EQ(out, 9);
        CK_EQ(infy_deque_pop_front(&d, &out), 0);   /* empty, not UB */
        infy_deque_free(&d);
    }
}

/* ============================ 6. xxHash64 (§6.7) ============================ */

static void test_hash(void) {
    section("xxHash64 (§6.7 Hash derive algorithm)");

    /* externally published reference vectors (xxhash python module docs, seed 0) */
    CK_EQ(infy_xxhash64("", 0, 0), 0xEF46DB3751D8F993ull);
    CK_EQ(infy_xxhash64("xxhash", 6, 0), 0x32DD38952C4BC720ull);
    CK_EQ(infy_xxhash64("xxhash", 6, 20141025), 0xB559B98D844E0635ull);

    /* determinism + avalanche, self-checked (no external source needed) */
    uint64_t h1 = infy_xxhash64("the same bytes", 14, 7);
    uint64_t h2 = infy_xxhash64("the same bytes", 14, 7);
    CK_EQ(h1, h2);
    CK(infy_xxhash64("the same bytes", 14, 8) != h1);
    CK(infy_xxhash64("the samebytEs", 13, 7) != h1);

    /* every long-input path (>= 32 bytes, the four-accumulator loop) is exercised */
    static uint8_t big[1000];
    for (int i = 0; i < 1000; i++) big[i] = (uint8_t)(i * 7 + 3);
    for (int len = 0; len <= 64; len++) {
        uint64_t a = infy_xxhash64(big, (uint64_t)len, 0);
        uint64_t b = infy_xxhash64(big, (uint64_t)len, 0);
        CK_EQ(a, b);
    }
    uint64_t base = infy_xxhash64(big, 1000, 0);
    uint64_t oneoff = infy_xxhash64(big, 999, 0);
    CK(base != oneoff);
    /* avalanche: flipping one input bit changes about half the output bits */
    uint8_t copy[1000];
    memcpy(copy, big, 1000);
    copy[700] ^= 0x01;
    uint64_t h3 = infy_xxhash64(copy, 1000, 0);
    int diff = 0;
    for (uint64_t d = base ^ h3; d; d >>= 1) diff += (int)(d & 1u);
    CK(diff >= 16 && diff <= 48);

    /* the Hash derive feed: order-sensitive, deterministic, xxHash64-based */
    uint64_t s1 = 0, s2 = 0;
    infy_hash_feed_u64(&s1, 1); infy_hash_feed_u64(&s1, 2);
    infy_hash_feed_u64(&s2, 2); infy_hash_feed_u64(&s2, 1);
    CK(s1 != s2);
    uint64_t s3 = 0;
    infy_hash_feed_u64(&s3, 1); infy_hash_feed_u64(&s3, 2);
    CK_EQ(s1, s3);
    /* the documented fold: feed(v) == xxhash64(le64(v), 8, state) */
    uint8_t one[8] = { 1, 0, 0, 0, 0, 0, 0, 0 };
    uint8_t two[8] = { 2, 0, 0, 0, 0, 0, 0, 0 };
    uint64_t expect = infy_xxhash64(two, 8, infy_xxhash64(one, 8, 0));
    CK_EQ(s1, expect);
}

/* ============================ 7. std.fmt (§10.11) ============================ */

static InfyFmtArg A_INT(int64_t v)  { InfyFmtArg a; memset(&a, 0, sizeof a); a.kind = FA_I64; a.v.i = v; return a; }
static InfyFmtArg A_UINT(uint64_t v){ InfyFmtArg a; memset(&a, 0, sizeof a); a.kind = FA_U64; a.v.u = v; return a; }
static InfyFmtArg A_F64(double v)   { InfyFmtArg a; memset(&a, 0, sizeof a); a.kind = FA_F64; a.v.f = v; return a; }
static InfyFmtArg A_STR(const char* s) {
    InfyFmtArg a; memset(&a, 0, sizeof a); a.kind = FA_STR; a.v.s = infy_str_of(s); return a;
}
static InfyFmtArg A_BOOL(uint8_t b) { InfyFmtArg a; memset(&a, 0, sizeof a); a.kind = FA_BOOL; a.v.b = b; return a; }
static InfyFmtArg A_CHAR(uint32_t c){ InfyFmtArg a; memset(&a, 0, sizeof a); a.kind = FA_CHAR; a.v.c = c; return a; }

static InfyFmtSpec S0(uint32_t arg) {
    InfyFmtSpec s; memset(&s, 0, sizeof s);
    s.arg_index = arg; s.width = -1; s.precision = -1; s.which = FY_AUTO;
    return s;
}
static char g_fmtbuf[1024];
static const char* fmt_run_str(const InfyFmtSpec* specs, uint32_t n, const InfyFmtArg* args, uint32_t na) {
    InfyString r = infy_fmt_run(specs, n, args, na);
    size_t n2 = r.len < sizeof g_fmtbuf - 1 ? (size_t)r.len : sizeof g_fmtbuf - 1;
    memcpy(g_fmtbuf, r.ptr ? r.ptr : (const uint8_t*)"", n2);
    g_fmtbuf[n2] = 0;
    infy_string_drop(&r);
    return g_fmtbuf;
}
static const char* one(InfyFmtSpec sp, InfyFmtArg arg) {
    InfyFmtSpec specs[1];
    specs[0] = sp;
    InfyFmtArg args[1] = { arg };
    return fmt_run_str(specs, 1, args, 1);
}

/* a generated Debug impl consults the pretty hint because InfyDebugVt cannot carry a spec */
static void point_write(InfyString* out, const void* obj);
static const InfyDebugVt point_vt = { (void (*)(struct InfyString*, const void*))point_write };
struct Point { int64_t x; int64_t y; };
static void point_write(InfyString* out, const void* obj) {
    const struct Point* p = (const struct Point*)obj;
    const char* fields[2] = { "x", "y" };
    InfyFmtArg vals[2];
    memset(vals, 0, sizeof vals);
    vals[0].kind = FA_I64; vals[0].v.i = p->x;
    vals[1].kind = FA_I64; vals[1].v.i = p->y;
    infy_debug_write_struct(out, "Point", fields, vals, 2, 0, infy_debug_pretty_hint());
}

static void fmt_bad_index_thunk(void);

static void test_fmt(void) {
    section("std.fmt byte-exact output (§10.11) + Debug (§6.7)");

    /* "{} {}" with an i64 and a str */
    {
        InfyFmtSpec sp[2];
        sp[0] = S0(0); sp[0].lit = 0; sp[0].lit_len = 0;
        sp[1] = S0(INFY_FMT_TAIL_ONLY); sp[1].lit = " items"; sp[1].lit_len = 6;
        InfyFmtArg args[2] = { A_INT(3), A_STR("unused") };
        CK_STR(fmt_run_str(sp, 2, args, 2), "3 items");
    }
    /* "{0} {1} {0}" positional */
    {
        InfyFmtSpec sp[3];
        sp[0] = S0(0);
        sp[1] = S0(1); sp[1].lit = " "; sp[1].lit_len = 1;
        sp[2] = S0(0); sp[2].lit = " "; sp[2].lit_len = 1;
        InfyFmtArg args[2] = { A_STR("a"), A_STR("b") };
        CK_STR(fmt_run_str(sp, 3, args, 2), "a b a");
        /* a literal-only tail spec carries the text after the last placeholder */
        InfyFmtSpec tail[1];
        tail[0] = S0(INFY_FMT_TAIL_ONLY); tail[0].lit = "just text"; tail[0].lit_len = 9;
        CK_STR(fmt_run_str(tail, 1, args, 2), "just text");
    }
    /* "{w}x{h}" — {name} is resolved to an index by codegen (§10.11) */
    {
        InfyFmtSpec sp[2];
        sp[0] = S0(0);                                    /* w */
        sp[1] = S0(1); sp[1].lit = "x"; sp[1].lit_len = 1; /* h */
        InfyFmtArg args[2] = { A_UINT(8), A_UINT(2) };
        CK_STR(fmt_run_str(sp, 2, args, 2), "8x");
    }
    /* "{:>10}" right align, space fill */
    {
        InfyFmtSpec s = S0(0); s.width = 10; s.align = '>';
        CK_STR(one(s, A_INT(42)), "        42");
        CK_EQ(strlen(one(s, A_INT(42))), 10);
    }
    /* "{:0>10}" zero fill, sign stays in front */
    {
        InfyFmtSpec s = S0(0); s.width = 10; s.align = '>'; s.fill = '0'; s.zero_pad = 1;
        CK_STR(one(s, A_UINT(42)), "0000000042");
        CK_STR(one(s, A_INT(-42)), "-000000042");
    }
    /* "{:<10}" left align and "{:^10}" centred (extra pad on the right) */
    {
        InfyFmtSpec s = S0(0); s.width = 10; s.align = '<';
        CK_STR(one(s, A_STR("ab")), "ab        ");
        s.align = '^';
        CK_STR(one(s, A_STR("ab")), "    ab    ");
        s.fill = '-';
        CK_STR(one(s, A_STR("ab")), "----ab----");
    }
    /* "{:.3}" precision: float rounds, str truncates on a code-point boundary */
    {
        InfyFmtSpec s = S0(0); s.precision = 3;
        CK_STR(one(s, A_F64(3.14159)), "3.142");
        CK_STR(one(s, A_STR("hello world")), "hel");
        s.precision = 4;
        CK_STR(one(s, A_STR("日本語x")), "日本");
    }
    /* "{:e}" exponent form: shortest round-trip, no '+' and no leading zeros */
    {
        InfyFmtSpec s = S0(0); s.which = FY_EXP;
        CK_STR(one(s, A_F64(1.5)), "1.5e0");
        CK_STR(one(s, A_F64(100000.0)), "1e5");
        CK_STR(one(s, A_F64(0.001)), "1e-3");
        CK_STR(one(s, A_F64(-1.5)), "-1.5e0");
        CK_STR(one(s, A_F64(0.0)), "0e0");
        s.precision = 2;
        CK_STR(one(s, A_F64(1.5)), "1.50e0");
    }
    /* default float Display: shortest that round-trips, no ".0" tail */
    {
        InfyFmtSpec s = S0(0);
        CK_STR(one(s, A_F64(1.0)), "1");
        CK_STR(one(s, A_F64(1.5)), "1.5");
        CK_STR(one(s, A_F64(1.0 / 3.0)), "0.3333333333333333");
        CK_STR(one(s, A_F64(-0.0)), "-0");
    }
    /* "{:?}" Debug: strings quoted and escaped, chars in single quotes, ints plain */
    {
        InfyFmtSpec s = S0(0); s.which = FY_DEBUG;
        CK_STR(one(s, A_STR("a\nb\"c")), "\"a\\nb\\\"c\"");
        CK_STR(one(s, A_INT(42)), "42");
        CK_STR(one(s, A_BOOL(1)), "true");
        CK_STR(one(s, A_CHAR('x')), "'x'");
        CK_STR(one(s, A_CHAR('\n')), "'\\n'");
    }
    /* "{:?}" / "{:#?}" on a struct through its Debug vtable */
    {
        struct Point p = { 1, 2 };
        InfyFmtArg a; memset(&a, 0, sizeof a);
        a.kind = FA_DEBUG_PTR; a.v.d.obj = &p; a.v.d.vt = &point_vt;
        InfyFmtSpec s = S0(0); s.which = FY_DEBUG;
        CK_STR(one(s, a), "Point { x: 1, y: 2 }");
        InfyFmtSpec sp = S0(0); sp.which = FY_DEBUG_PRETTY; sp.alt = 1;
        CK_STR(one(sp, a), "Point {\n    x: 1,\n    y: 2,\n}");
        /* width applies to the rendered debug text too */
        InfyFmtSpec sw = S0(0); sw.which = FY_DEBUG; sw.width = 20; sw.align = '>';
        CK_STR(one(sw, a), "   Point { x: 1, y: 2 }");
    }
    /* radix conversions with the alt flag: {:#x} {:#X} {:b} {:#o} */
    {
        InfyFmtSpec s = S0(0); s.which = FY_LOWER_HEX; s.alt = 1;
        CK_STR(one(s, A_UINT(255)), "0xff");
        s.alt = 0;
        CK_STR(one(s, A_UINT(255)), "ff");
        s.which = FY_UPPER_HEX; s.alt = 1;
        CK_STR(one(s, A_UINT(255)), "0xFF");
        s.which = FY_BINARY; s.alt = 1;
        CK_STR(one(s, A_UINT(5)), "0b101");
        s.which = FY_OCTAL; s.alt = 1;
        CK_STR(one(s, A_UINT(8)), "0o10");
        s.which = FY_LOWER_HEX; s.alt = 0; s.width = 6; s.fill = '0'; s.zero_pad = 1;
        CK_STR(one(s, A_UINT(255)), "0000ff");
        CK_STR(one(S0(0), A_INT(-255)), "-255");
        /* FY_POINTER prints the boxed address, not the value */
        struct Point pp = { 3, 4 };
        InfyFmtArg pa; memset(&pa, 0, sizeof pa);
        pa.kind = FA_DEBUG_PTR; pa.v.d.obj = &pp; pa.v.d.vt = &point_vt;
        InfyFmtSpec sp = S0(0); sp.which = FY_POINTER;
        CK(strncmp(one(sp, pa), "0x", 2) == 0);
    }
    /* §6.7 repr(C) tuple Debug form */
    {
        const char* fields[2] = { "0", "1" };
        InfyFmtArg vals[2] = { A_INT(7), A_STR("t") };
        InfyString out = infy_string_new();
        infy_debug_write_struct(&out, "Pair", fields, vals, 2, 1, 0);
        CK_STR((const char*)out.ptr, "Pair(7, \"t\")");
        infy_string_drop(&out);
        out = infy_string_new();
        infy_debug_write_struct(&out, "Pair", fields, vals, 2, 1, 1);
        CK_STR((const char*)out.ptr, "Pair(\n    7,\n    \"t\",\n)");
        infy_string_drop(&out);
        out = infy_string_new();
        infy_debug_write_struct(&out, "Empty", 0, 0, 0, 0, 1);
        CK_STR((const char*)out.ptr, "Empty {}");
        infy_string_drop(&out);
    }
    /* the direct value writers used by generated impls */
    {
        InfyFmtSpec s = S0(0); s.width = 4; s.align = '>';
        InfyString o = infy_string_new();
        infy_fmt_write_int(&o, -7, &s);
        infy_fmt_write_uint(&o, 9, &s);
        infy_fmt_write_bool(&o, 0);
        infy_fmt_write_char(&o, '!');
        infy_fmt_write_float(&o, 2.5, &s);
        infy_fmt_write_str(&o, infy_str_of("z"), &s);
        CK_STR((const char*)o.ptr, "  -7   9false! 2.5   z");
        infy_string_drop(&o);
    }
    /* a runtime out-of-range argument index is a normative panic, never an empty hole */
    expect_panic("fmt arg index out of range", R_INDEX_OOB, fmt_bad_index_thunk);
}

static InfyFmtSpec g_badspec[1];
static InfyFmtArg g_badargs[1];
static void fmt_bad_index_thunk(void) {
    g_badspec[0] = S0(3);
    g_badargs[0] = A_INT(1);
    infy_string_drop(infy_fmt_run(g_badspec, 1, g_badargs, 1));
}

/* ============================ 8. time + rand (§10.12) ============================ */

static uint64_t g_stream[8];
static uint64_t g_range_vals[16];
static int g_perm[12];

static void dump_rng(uint64_t seed) {
    InfyRng r = infy_rng_seeded(seed);
    for (int i = 0; i < 8; i++) g_stream[i] = infy_rng_next_u64(&r);
    InfyRng r2 = infy_rng_seeded(seed);
    for (int i = 0; i < 16; i++) g_range_vals[i] = infy_rng_range(&r2, 10, 20);
    InfyRng r3 = infy_rng_seeded(seed);
    for (int i = 0; i < 12; i++) g_perm[i] = i;
    infy_rng_shuffle(&r3, g_perm, 12, sizeof(int));
}

static void test_rand(void) {
    section("rand: explicit seed, PCG32, unbiased Fisher-Yates (§10.12)");

    dump_rng(0x123456789ABCDEF0ull);
    uint64_t first[8];
    memcpy(first, g_stream, sizeof first);
    dump_rng(0x123456789ABCDEF0ull);
    for (int i = 0; i < 8; i++) CK_EQ(g_stream[i], first[i]);        /* same seed => same stream */
    dump_rng(0x123456789ABCDEF1ull);
    int all_same = 1;
    for (int i = 0; i < 8; i++) if (g_stream[i] != first[i]) all_same = 0;
    CK(!all_same);                                                    /* different seed differs */

    /* the stream is not the trivial identity of the seed and shows no short period */
    dump_rng(42);
    CK(g_stream[0] != 42 && g_stream[1] != g_stream[0]);
    int distinct = 1;
    for (int i = 1; i < 8; i++) for (int j = 0; j < i; j++) if (g_stream[i] == g_stream[j]) distinct = 0;
    CK(distinct);

    /* range() is half-open, deterministic, and covers the whole interval without bias */
    dump_rng(0x123456789ABCDEF0ull);
    for (int i = 0; i < 16; i++) { CK(g_range_vals[i] >= 10); CK(g_range_vals[i] < 20); }
    /* distribution smoke test over [0,3): every bucket hit, none wildly over-represented */
    InfyRng r = infy_rng_seeded(7);
    int buckets[3] = { 0, 0, 0 };
    for (int i = 0; i < 30000; i++) {
        uint64_t v = infy_rng_range(&r, 0, 3);
        CK(v < 3);
        buckets[v]++;
    }
    for (int b = 0; b < 3; b++) CK(buckets[b] > 9000 && buckets[b] < 11000);

    /* choice_index over n and empty n -> R-0004 (tested in the arithmetic section) */
    r = infy_rng_seeded(9);
    for (int i = 0; i < 500; i++) CK(infy_rng_choice_index(&r, 5) < 5);

    /* shuffle: tail-to-head Fisher-Yates with range(0..=i) is normative, so the permutation for a
     * fixed seed is a golden value (§12.4 equivalence matrix, §10.12). */
    dump_rng(0xDEADBEEFCAFEF00Dull);
    static const int golden_perm[12] = { 5, 10, 7, 2, 0, 3, 1, 9, 11, 4, 8, 6 };
    for (int i = 0; i < 12; i++) CK_EQ(g_perm[i], golden_perm[i]);
    /* it is a permutation: every index appears exactly once (no bias-introducing duplicates) */
    int seen[12] = { 0 };
    for (int i = 0; i < 12; i++) { CK(g_perm[i] >= 0 && g_perm[i] < 12); seen[g_perm[i]]++; }
    for (int i = 0; i < 12; i++) CK_EQ(seen[i], 1);
    /* shuffle of 0 and 1 elements is a no-op, never UB */
    int one[1] = { 7 };
    infy_rng_shuffle(&r, one, 0, sizeof(int));
    CK_EQ(one[0], 7);
    infy_rng_shuffle(&r, one, 1, sizeof(int));
    CK_EQ(one[0], 7);
    /* and the whole thing is reproducible from the seed alone — no time, no libc rand() */
    int a[12], b[12];
    for (int i = 0; i < 12; i++) { a[i] = i; b[i] = i; }
    InfyRng ra = infy_rng_seeded(5), rb = infy_rng_seeded(5);
    infy_rng_shuffle(&ra, a, 12, sizeof(int));
    infy_rng_shuffle(&rb, b, 12, sizeof(int));
    for (int i = 0; i < 12; i++) CK_EQ(a[i], b[i]);

    /* time: monotonic never goes backwards, tick() is ms since start (§4.5) */
    uint64_t t0 = infy_monotonic_ns();
    uint64_t m0 = infy_monotonic_ms();
    infy_sleep_ms(5);
    uint64_t t1 = infy_monotonic_ns();
    CK(t1 >= t0);
    CK(infy_monotonic_ms() >= m0);
    CK(t1 - t0 >= 1000000ull);              /* at least ~1 ms of real progress after sleep(5) */
    uint64_t unix1 = infy_unix_seconds();
    CK(unix1 > 1700000000ull);              /* sane wall clock, only read for env.realtime */
}

/* ============================ 9. ResourceLock FIFO (§8.3) ============================ */

#ifdef _WIN32
typedef HANDLE infy_test_thread;
#define THREAD_FN unsigned __stdcall
#define THREAD_RET 0
static void thread_start(infy_test_thread* t, unsigned (__stdcall* fn)(void*), void* arg) {
    *t = (HANDLE)_beginthreadex(0, 0, fn, arg, 0, 0);
}
static void thread_join(infy_test_thread t) { WaitForSingleObject(t, INFINITE); CloseHandle(t); }
#else
typedef pthread_t infy_test_thread;
#define THREAD_FN void*
#define THREAD_RET 0
static void thread_start(infy_test_thread* t, void* (*fn)(void*), void* arg) { pthread_create(t, 0, fn, arg); }
static void thread_join(infy_test_thread t) { pthread_join(t, 0); }
#endif

static InfySrw g_fifo_mu;
static char g_fifo_log[256];
static volatile int g_release_token;
static volatile int inside = 0, max_inside = 0, counter = 0;
static infy_test_thread t1a, t2a;

struct WaiterArg { const char* tag; int idx; };

static THREAD_FN waiter_queue_b(void* p);
static THREAD_FN waiter_queue_c(void* p);
static THREAD_FN waiter_spin(void* p);

static void fifo_note(const char* tag) {
    infy_mtx_lock(&g_fifo_mu);
    size_t n = strlen(g_fifo_log);
    snprintf(g_fifo_log + n, sizeof g_fifo_log - n, "%s", tag);
    infy_mtx_unlock(&g_fifo_mu);
}

/* each waiter: acquire (blocks until it is the FIFO head), note its tag, then wait until the
 * previous holder signalled that it is done, then release */
struct WaiterArg { const char* tag; int idx; };

static THREAD_FN waiter_queue_b(void* p) {
    struct WaiterArg* a = (struct WaiterArg*)p;
    fifo_note("(B-queued)");
    uint64_t g = infy_lock_acquire("db:orders:writer", &LOC(100, 1, 0));
    fifo_note("[B]");
    infy_lock_release(g);
    fifo_note("(B-done)");
    a->idx = 1;
    return THREAD_RET;
}
static THREAD_FN waiter_queue_c(void* p) {
    struct WaiterArg* a = (struct WaiterArg*)p;
    fifo_note("(C-queued)");
    uint64_t g = infy_lock_acquire("db:orders:writer", &LOC(101, 1, 0));
    fifo_note("[C]");
    infy_lock_release(g);
    fifo_note("(C-done)");
    a->idx = 1;
    return THREAD_RET;
}

static void test_lock_fifo(void) {
    section("ResourceLock FIFO + no starvation (§8.3, §8.5)");

    infy_mtx_init(&g_fifo_mu);
    const char* res = "db:orders:writer";

    /* 1) the plain mutex property: nested acquisitions from different threads serialise */
    uint64_t g1 = infy_lock_acquire(res, &LOC(90, 1, 0));
    CK(g1 != 0);

    /* 2) two waiters queue while the main thread holds the resource; the service order must be
     *    exactly the submission order (B submitted before C), which is what §8.3 makes observable */
    static struct WaiterArg ab = { "B", 0 }, ac = { "C", 0 };
    g_fifo_log[0] = 0;
    infy_test_thread tb, tc;
    thread_start(&tb, (THREAD_FN(*)(void*))waiter_queue_b, &ab);
    while (infy_lock_waiting(res) < 1) infy_sleep_ms(1);      /* B is in the queue */
    thread_start(&tc, (THREAD_FN(*)(void*))waiter_queue_c, &ac);
    while (infy_lock_waiting(res) < 2) infy_sleep_ms(1);      /* C queued behind B */
    CK_EQ(infy_lock_waiting(res), 2);
    infy_lock_release(g1);                                    /* main releases; B must go first */
    thread_join(tb);
    thread_join(tc);
    CK_EQ(ab.idx, 1);
    CK_EQ(ac.idx, 1);
    /* the recorded trace: B queued, C queued, B served, B done, C served, C done — i.e. C could
     * not overtake B even though it was contending for the same resource */
    CK_STR(g_fifo_log, "(B-queued)(C-queued)[B](B-done)[C](C-done)");

    /* 3) two threads hammering the same resource both finish: no starvation, mutual exclusion */
    g_fifo_log[0] = 0;
    inside = max_inside = counter = 0;
    static struct WaiterArg t1 = { "X", 0 }, t2 = { "Y", 0 };
    thread_start(&t1a, (THREAD_FN(*)(void*))waiter_spin, &t1);
    thread_start(&t2a, (THREAD_FN(*)(void*))waiter_spin, &t2);
    thread_join(t1a);
    thread_join(t2a);
    CK_EQ(counter, 200);            /* 2 threads x 100 increments, all visible */
    CK_EQ(max_inside, 1);           /* never two holders at once */
    CK(infy_lock_waiting(res) == 0);
}

static THREAD_FN waiter_spin(void* p) {
    struct WaiterArg* a = (struct WaiterArg*)p;
    (void)a;
    for (int i = 0; i < 100; i++) {
        uint64_t g = infy_lock_acquire("db:orders:writer", &LOC(102, 1, 0));
        int was = inside;
        inside = was + 1;
        if (inside > max_inside) max_inside = inside;
        counter = counter + 1;
        inside = inside - 1;
        infy_lock_release(g);
    }
    return THREAD_RET;
}

/* ============================ 10. panic radius + focuses (§4.16, §8.5) ============================ */

typedef struct { uint32_t magic; } TestFrame;

static char g_run_log[1024];
static void run_note(const char* tag) {
    size_t n = strlen(g_run_log);
    snprintf(g_run_log + n, sizeof g_run_log - n, "%s", tag);
}
static void f_drop_a(void* p) { (void)p; run_note("<dropA"); }
static void f_drop_b(void* p) { (void)p; run_note("<dropB"); }
static void f_defer_x(void* p) { (void)p; run_note("<deferX"); }
static void f_defer_y(void* p) { (void)p; run_note("<deferY"); }

static void node_enter(void* frame) {
    TestFrame* f = (TestFrame*)frame;
    f->magic = 0xFEEDFACEu;
    run_note("(n0)");
    infy_cleanup_push(f_drop_a, 0, 0);
    infy_cleanup_push(f_defer_x, 0, 1);
    infy_cleanup_push(f_drop_b, 0, 0);
    infy_cleanup_push(f_defer_y, 0, 1);
}
static int g_fail_focus_id = -1;
static void node_body(void* frame) {
    TestFrame* f = (TestFrame*)frame;
    (void)f;
    uint32_t id = infy_current_focus();
    if ((int)id == g_fail_focus_id) {
        char b[64];
        snprintf(b, sizeof b, "(n1-panic F%u)", (unsigned)id);
        run_note(b);
        infy_panic(R_INDEX_OOB, "index 9 out of bounds for length 4 (§4.9 UB-008)", &LOC(200, 5, 1));
    }
    run_note("(n1)");
}
static void node_tail(void* frame) {
    TestFrame* f = (TestFrame*)frame;
    (void)f;
    run_note("(n2)");
}
static void node_stop(void* frame) {
    TestFrame* f = (TestFrame*)frame;
    (void)f;
    run_note("(stop)");
    infy_request_stop();
}

static const InfyNodeFn nodes_work[] = { node_enter, node_body, node_tail };
static const InfyNodeLoc locs_work[] = {
    { "test/harness.infy", 10, 1, 0, 0 },
    { "test/harness.infy", 20, 5, 1, 1 },
    { "test/harness.infy", 30, 5, 2, 0 }
};
static const uint32_t links_work[] = { 0, 1, 0 };
static const InfyChainDesc chain_work = { "WorkChain", 3, 0, sizeof(TestFrame), 8,
                                          nodes_work, locs_work, links_work };

static const InfyNodeFn nodes_stop[] = { node_enter, node_stop, node_tail };
static const InfyChainDesc chain_stop = { "StopChain", 3, 0, sizeof(TestFrame), 8,
                                          nodes_stop, locs_work, links_work };

static void test_focus_radius(void) {
    section("panic radius focus + drop order on unwind (§4.16, §4.7.4, §8.5)");

    infy_sched_reset();
    infy_chain_register(&chain_work);      /* chain_id 0 */
    infy_chain_register(&chain_stop);      /* chain_id 1 */
    infy_sched_init(1, 0, 0);              /* one worker: deterministic round-robin */
    infy_profile.panic_radius = INFY_RADIUS_FOCUS;

    g_run_log[0] = 0;
    uint32_t f1 = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    uint32_t f2 = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    uint32_t f3 = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    CK(f1 != f2 && f2 != f3);
    g_fail_focus_id = (int)f2;
    g_obs_count = 0;
    infy_test_set_report_observer(observer);

    int code = infy_sched_run_until_idle();
    infy_test_set_report_observer(0);

    /* §13.4: a runtime panic makes the process exit code 3 even though the others finished */
    CK_EQ(code, 3);
    /* the two healthy focuses ran all three nodes; the failing one stopped after node 1 */
    CK(strstr(g_run_log, "(n0)") != 0);
    CK(strstr(g_run_log, "(n1-panic") != 0);
    CK_EQ(infy_sched.n_failed, 1u);
    CK_EQ(infy_sched.n_alive, 0u);
    uint32_t done = 0, failed = 0;
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
        if (infy_sched.focuses[i]->state == INFY_F_DONE) done++;
        if (infy_sched.focuses[i]->state == INFY_F_FAILED) failed++;
    }
    CK_EQ(done, 2u);
    CK_EQ(failed, 1u);
    /* §12.4: the R- report carried the focus/chain/node triple */
    CK(g_obs_count >= 1);
    CK_EQ(g_obs_code[0], (uint32_t)R_INDEX_OOB);
    CK_EQ(g_obs_focus[0], f2);
    CK_EQ(g_obs_chain[0], 0u);
    CK_EQ(g_obs_node[0], 1u);

    /* the unwinding focus drained its deque LIFO: drops in reverse declaration order, then the
     * defers in reverse declaration order (§4.7.4 + §3.13) */
    CK(strstr(g_run_log, "<dropB<dropA<deferY<deferX") != 0);
    /* the healthy focuses released their ownership scope at chain end, same order */
    {
        int n_drops = 0, n_defers = 0;
        for (const char* p = strstr(g_run_log, "<dropB"); p; p = strstr(p + 1, "<dropB")) n_drops++;
        for (const char* p = strstr(g_run_log, "<deferY"); p; p = strstr(p + 1, "<deferY")) n_defers++;
        CK_EQ(n_drops, 3);
        CK_EQ(n_defers, 3);
    }
    /* env triple is per focus, not global */
    CK_EQ(infy_chain_count(), 2u);
    CK(infy_chain_desc(0) == &chain_work);
    CK(infy_chain_desc(2) == 0);

    /* §8.2 stop: graceful — the current node finishes, then every focus stops and drops */
    infy_sched_reset();
    infy_sched_init(1, 0, 0);
    g_run_log[0] = 0;
    uint32_t s1 = infy_focus_spawn(1, 0, &LOC(1, 1, 0));
    uint32_t s2 = infy_focus_spawn(1, 0, &LOC(1, 1, 0));
    CK(s1 != s2);
    code = infy_sched_run_until_idle();
    /* node 1 of the first focus to run asks for stop; both focuses then finish the node they are
     * in and stop without running node 2 */
    CK(strstr(g_run_log, "(stop)") != 0);
    CK_EQ(infy_sched.stop_requested, 1u);
    CK_EQ(infy_sched.n_failed, 0u);
    CK_EQ(code, 0);
    CK_EQ(infy_focus_count(1), 0u);
}

static void test_radius_chain(void) {
    section("panic radius chain (§4.16)");

    infy_sched_reset();
    infy_sched_init(1, 0, 0);
    infy_profile.panic_radius = INFY_RADIUS_CHAIN;
    g_run_log[0] = 0;
    uint32_t a = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    uint32_t b = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    uint32_t c = infy_focus_spawn(1, 0, &LOC(1, 1, 0));      /* other chain: untouched */
    g_fail_focus_id = (int)b;
    (void)infy_sched_run_until_idle();
    uint32_t failed_chain0 = 0, done_chain1 = 0;
    for (uint32_t i = 0; i < infy_sched.n_focuses; i++) {
        InfyFocus* f = infy_sched.focuses[i];
        if (f->chain_id == 0 && f->state == INFY_F_FAILED) failed_chain0++;
        if (f->id == c && f->state == INFY_F_DONE) done_chain1++;
    }
    CK_EQ(failed_chain0, 2u);         /* the whole chain failed, not just the panicking focus */
    CK_EQ(done_chain1, 1u);           /* the other chain kept running (§4.16 chain radius) */
    CK(a != b);
    /* every failed focus released its own ownership scope */
    CK(strstr(g_run_log, "<dropB<dropA<deferY<deferX") != 0);
    infy_profile.panic_radius = INFY_RADIUS_FOCUS;
    infy_sched_reset();
    infy_sched_init(1, 0, 0);
}

static void test_focus_wraparound(void) {
    section("R-0802 at the focus-id wraparound (§8.5)");

    infy_test_set_focus_seq(0xFFFFFFFFu);
    expect_panic("focus id wraparound", R_FOCUS_WRAP, e_focus_wrap);
    /* ids are never reused: a refusal leaves the counter where it was */
    CK_EQ(infy_sched.next_focus_id, 0xFFFFFFFFu);
    infy_test_set_focus_seq(1000u);
    uint32_t id = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    CK_EQ(id, 1000u);
    id = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    CK_EQ(id, 1001u);
    /* goto to a node that does not exist is R-0010 (§4.9 UB-013) */
    infy_sched_reset();
    infy_sched_init(1, 0, 0);
    infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    g_run_log[0] = 0;
    expect_panic("goto missing target", R_GOTO_TARGET, e_goto_missing);
}

static void test_pause_yield(void) {
    section("pause/resume, yield_now, focus_count, pressure (§8.2, §8.5, §11.4)");

    infy_sched_reset();
    infy_sched_init(1, 0, 0);
    g_run_log[0] = 0;
    uint32_t f1 = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    uint32_t f2 = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    CK_EQ(infy_focus_count(0), 2u);
    infy_focus_pause(f2);                     /* suspends before it ever ran */
    CK_EQ(infy_focus_count(0), 2u);
    uint32_t runnable = infy_sched.n_runnable;
    CK_EQ(runnable, 1u);                      /* only f1 is queued */
    (void)infy_sched_run_until_idle();
    CK_EQ(infy_sched.focuses[1]->state, INFY_F_PAUSED);
    infy_focus_resume(f2);                    /* re-enqueued at the FIFO tail */
    CK_EQ(infy_sched.focuses[1]->state, INFY_F_READY);
    g_fail_focus_id = -1;
    (void)infy_sched_run_until_idle();
    CK_EQ(infy_sched.focuses[1]->state, INFY_F_DONE);
    CK(f1 != f2);

    /* yield_now sets the flag consumed at the next node boundary (preemption point ③) */
    infy_sched_reset();
    infy_sched_init(1, 0, 0);
    infy_yield_now();                          /* outside a focus: must not crash */
    CK(1);

    /* backpressure throttles admission but never reorders or fails a start() (§11.4) */
    infy_set_pressure(0.9f);
    CK_EQ(infy_sched.pressure >= 0.5f ? 1u : 0u, 1u);
    uint32_t tf = infy_focus_spawn(0, 0, &LOC(1, 1, 0));
    CK_EQ(infy_sched.n_throttled, 1u);
    CK(tf != 0u);
    infy_set_pressure(0.1f);
    CK_EQ(infy_sched.n_throttled, 0u);
    CK_EQ(infy_sched.n_runnable, 1u);
    (void)infy_sched_run_until_idle();
    infy_set_pressure(0.0f);
    infy_sched_reset();
    infy_sched_init(1, 0, 0);
}

/* ============================ 11. artifact container (§7.4) ============================ */

static void wr16(uint8_t* p, uint16_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
static void put_str(uint8_t* blob, uint32_t* pool_at, uint32_t* pool_len, uint32_t* npairs,
                    const char* s) {
    uint32_t off = *pool_len;
    uint32_t ln = (uint32_t)strlen(s);
    memcpy(blob + off, s, ln);
    *pool_len += ln;
    infy_art_wr_u32(blob + *pool_at + (*npairs) * 8u + 0u, off);
    infy_art_wr_u32(blob + *pool_at + (*npairs) * 8u + 4u, ln);
    (*npairs)++;
}

/* exe bytes + blob + 64-byte trailer, exactly the §7.4/abi §6 container shape */
static void build_artifact(const char* path, int break_crc) {
    static const char fake_exe[] = "MZxfake-host-image-PE-or-ELF";
    uint32_t exe_len = (uint32_t)strlen(fake_exe);

    /* blob layout: meta(200) | strings pair table | rodata bytes */
    uint32_t meta_len = INFY_META_FIXED_BYTES;
    uint32_t pair_count = 5;
    uint32_t pairs_len = pair_count * 8u;
    uint32_t rodata_off = meta_len + pairs_len;
    uint32_t rodata_cap = 256;
    uint32_t blob_len = rodata_off + rodata_cap;
    uint8_t* blob = (uint8_t*)calloc(blob_len, 1);

    memcpy(blob, INFY_META_MAGIC, 8);
    infy_art_wr_u32(blob + INFY_META_OFF_IFABI, INFY_IFABI);
    infy_art_wr_u32(blob + INFY_META_OFF_OPT, 2);
    blob[INFY_META_OFF_OVERFLOW] = 0;              /* panic */
    blob[INFY_META_OFF_RADIUS] = 0;                /* focus */
    blob[INFY_META_OFF_USIZE] = 64;
    blob[INFY_META_OFF_FASTMATH] = 0;
    blob[INFY_META_OFF_LTO] = 1;
    infy_art_wr_u32(blob + INFY_META_OFF_STACK, 8u * 1024u * 1024u);
    infy_art_wr_u32(blob + INFY_META_OFF_FOCUS_STACK, 256u * 1024u);
    infy_art_wr_u32(blob + INFY_META_OFF_QUOTA, 1);
    infy_art_wr_u64(blob + INFY_META_OFF_ITER_GUARD, 0x100000000ull);
    infy_art_wr_u32(blob + INFY_META_OFF_N_CHAINS, 2);
    infy_art_wr_u32(blob + INFY_META_OFF_N_EVENTS, 7);
    infy_art_wr_u32(blob + INFY_META_OFF_N_NODES, 11);
    infy_art_wr_u32(blob + INFY_META_OFF_N_IFBC, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_N_RESKEYS, 1);
    infy_art_wr_u32(blob + INFY_META_OFF_CHAINS, meta_len);      /* empty-but-declared regions */
    infy_art_wr_u32(blob + INFY_META_OFF_CHAINS + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_EVENTS, meta_len);
    infy_art_wr_u32(blob + INFY_META_OFF_EVENTS + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_NODES, meta_len);
    infy_art_wr_u32(blob + INFY_META_OFF_NODES + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_STRINGS, rodata_off - rodata_cap);   /* = meta+len+pairs? */
    infy_art_wr_u32(blob + INFY_META_OFF_STRINGS, meta_len);
    infy_art_wr_u32(blob + INFY_META_OFF_STRINGS + 4, pairs_len);
    infy_art_wr_u32(blob + INFY_META_OFF_IFBC, meta_len);
    infy_art_wr_u32(blob + INFY_META_OFF_IFBC + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_SUM, meta_len);
    infy_art_wr_u32(blob + INFY_META_OFF_SUM + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_EXPORT, meta_len);
    infy_art_wr_u32(blob + INFY_META_OFF_EXPORT + 4, 0);

    uint32_t pool_at = meta_len, pool_len = rodata_off, npairs = 0;
    uint32_t idx_target = npairs; put_str(blob, &pool_at, &pool_len, &npairs, "x86_64-pc-windows-gnu");
    uint32_t idx_build  = npairs; put_str(blob, &pool_at, &pool_len, &npairs, "deadbeefcafebabe00000000000000ff");
    uint32_t idx_plat   = npairs; put_str(blob, &pool_at, &pool_len, &npairs, "win10");
    uint32_t idx_lang   = npairs; put_str(blob, &pool_at, &pool_len, &npairs, "INFY-v2.0.0");
    uint32_t idx_tool   = npairs; put_str(blob, &pool_at, &pool_len, &npairs, "infyc 0.1.0");
    CK(npairs == pair_count);
    infy_art_wr_u32(blob + INFY_META_OFF_S_TARGET, idx_target);
    infy_art_wr_u32(blob + INFY_META_OFF_S_TARGET + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_S_BUILDID, idx_build);
    infy_art_wr_u32(blob + INFY_META_OFF_S_BUILDID + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_S_PLATFORM, idx_plat);
    infy_art_wr_u32(blob + INFY_META_OFF_S_PLATFORM + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_S_LANGVER, idx_lang);
    infy_art_wr_u32(blob + INFY_META_OFF_S_LANGVER + 4, 0);
    infy_art_wr_u32(blob + INFY_META_OFF_S_TOOLCHAIN, idx_tool);
    infy_art_wr_u32(blob + INFY_META_OFF_S_TOOLCHAIN + 4, 0);
    for (int i = 0; i < 32; i++) blob[INFY_META_OFF_BUILDID32 + i] = (uint8_t)(0x11 * (i % 8) + i);

    uint8_t trailer[64];
    memset(trailer, 0, sizeof trailer);
    memcpy(trailer, INFY_ART_MAGIC, 8);
    wr16(trailer + 8, INFY_ART_HDR_VER);
    wr16(trailer + 10, INFY_IFABI);
    wr16(trailer + 12, 6);                 /* seg_count: the six §7.4 segments */
    wr16(trailer + 14, 0);
    infy_art_wr_u32(trailer + 16, exe_len);
    infy_art_wr_u32(trailer + 20, blob_len);
    infy_art_wr_u32(trailer + 24, 0);            /* meta */
    infy_art_wr_u32(trailer + 28, meta_len);
    infy_art_wr_u32(trailer + 32, meta_len);     /* ifbc (empty) */
    infy_art_wr_u32(trailer + 36, 0);
    infy_art_wr_u32(trailer + 40, meta_len);     /* sum (empty) */
    infy_art_wr_u32(trailer + 44, 0);
    infy_art_wr_u32(trailer + 48, rodata_off);   /* rodata */
    infy_art_wr_u32(trailer + 52, pool_len - rodata_off);
    infy_art_wr_u32(trailer + 60, *(uint32_t*)(blob + INFY_META_OFF_BUILDID32));
    infy_art_wr_u32(trailer + 56, infy_art_crc32(trailer, 56));

    FILE* f = fopen(path, "wb");
    fwrite(fake_exe, 1, exe_len, f);
    fwrite(blob, 1, blob_len, f);
    if (break_crc) trailer[56] ^= 0xFF;          /* a single flipped bit must be rejected */
    fwrite(trailer, 1, 64, f);
    fclose(f);
    free(blob);
}

static void test_artifact(void) {
    section("artifact container (§7.4, §7.3 build-id)");

    build_artifact("rt_artifact.bin", 0);
    InfyMetaFixed m;
    uint8_t* blob = 0;
    uint64_t blen = 0;
    CK_EQ(infy_art_read_file("rt_artifact.bin", &m, &blob, &blen), 0);
    CK(blob != 0);
    CK(m.ifabi == INFY_IFABI);
    CK_EQ(m.opt_level, 2u);
    CK_EQ(m.overflow_policy, (uint8_t)INFY_OV_PANIC);
    CK_EQ(m.panic_radius, (uint8_t)INFY_RADIUS_FOCUS);
    CK_EQ(m.usize_bits, 64u);
    CK_EQ(m.focus_quota, 1u);
    CK_EQ(m.iter_guard_steps, 0x100000000ull);
    CK_EQ(m.n_chains, 2u); CK_EQ(m.n_events, 7u); CK_EQ(m.n_nodes, 11u);
    CK_EQ(m.n_ifbc_modules, 0u); CK_EQ(m.n_resource_keys, 1u);
    /* string pool resolution through infy_art_string */
    InfyStr s0 = infy_art_string(&m, blob, 0);
    CK_EQ(s0.len, strlen("x86_64-pc-windows-gnu"));
    CK(memcmp(s0.ptr, "x86_64-pc-windows-gnu", s0.len) == 0);
    InfyStr s2 = infy_art_string(&m, blob, 2);
    CK_EQ(s2.len, 5u);
    CK(memcmp(s2.ptr, "win10", 5) == 0);
    CK(infy_art_string(&m, blob, 999).len == 0);        /* out of range, never OOB */
    /* the resolved meta strings point into the blob (rodata index, no copy) */
    CK_EQ(m.platform.len, 5u);
    CK(memcmp(m.platform.ptr, "win10", 5) == 0);
    CK(m.build_id.len > 0);
    CK_EQ(m.n_events, 7u);

    FILE* dump = fopen("rt_artifact_dump.txt", "wb");
    infy_art_dump_meta(&m, blob, dump);
    fclose(dump);
    dump = fopen("rt_artifact_dump.txt", "rb");
    char dbuf[4096];
    size_t dn = fread(dbuf, 1, sizeof dbuf - 1, dump);
    fclose(dump);
    dbuf[dn] = 0;
    CK(dn > 100);
    CK(strstr(dbuf, "ifabi") != 0);
    CK(strstr(dbuf, "panic_radius") != 0);
    CK(strstr(dbuf, "x86_64-pc-windows-gnu") != 0);

    free(blob);

    /* a corrupted trailer CRC must reject the file (§7.4 self-validation) */
    build_artifact("rt_artifact_bad.bin", 1);
    uint8_t* b2 = 0; uint64_t l2 = 0;
    CK_EQ(infy_art_read_file("rt_artifact_bad.bin", &m, &b2, &l2), -1);
    CK(b2 == 0);
    /* a file with no trailer at all is not an artifact */
    {
        FILE* f = fopen("rt_artifact_none.bin", "wb");
        fwrite("just some bytes, no INFY here", 1, 29, f);
        fclose(f);
        CK_EQ(infy_art_read_file("rt_artifact_none.bin", &m, &b2, &l2), -1);
    }
    /* a duplicate trailer means the blob was appended twice */
    {
        FILE* src = fopen("rt_artifact.bin", "rb");
        fseek(src, 0, SEEK_END);
        long sz = ftell(src);
        fseek(src, 0, SEEK_SET);
        uint8_t* img = (uint8_t*)malloc((size_t)sz * 2u);
        fread(img, 1, (size_t)sz, src);
        fclose(src);
        FILE* dst = fopen("rt_artifact_dup.bin", "wb");
        fwrite(img, 1, (size_t)sz, dst);
        fwrite(img + sz - 64, 1, 64, dst);
        fclose(dst);
        free(img);
        CK_EQ(infy_art_read_file("rt_artifact_dup.bin", &m, &b2, &l2), -1);
    }
    /* this harness binary is not an INFY artifact, so infy_art_self() fails cleanly */
    CK_EQ(infy_art_self(&m, &b2, &l2), -1);
}

/* ============================ 12. host protocol (§11) ============================ */

static InfyEnvSnapshot g_fake_env;
static const InfyEnvSnapshot* fake_env_static(void) { return &g_fake_env; }

static const InfyEnvSnapshot* fake_env(void* self) { (void)self; return fake_env_static(); }
static int8_t fake_event(void* self, uint32_t id, const InfyArgFrame* args, InfyOutVal* out) {
    (void)self; (void)args;
    if (id == 4040u) return -1;                       /* a host that reports an unmapped error */
    if (out) { out->tag = 1; out->v.i = (int64_t)id * 2; }
    return 0;
}
static uint64_t fake_acquire(void* self, uint32_t res, uint64_t ts) {
    (void)self; (void)res; (void)ts; return 777;
}
static void fake_release(void* self, uint64_t g) { (void)self; (void)g; }
static uint32_t fake_spawn(void* self, uint32_t c, uint32_t n) {
    (void)self; return infy_focus_spawn(c, n, 0);
}
static void fake_pause(void* self, uint32_t f) { (void)self; infy_focus_pause(f); }
static void fake_stop(void* self) { (void)self; infy_request_stop(); }
static void fake_pressure(void* self, float p) { (void)self; infy_set_pressure(p); }

static const InfyHostApi g_fake_host = {
    0, fake_env, fake_event, fake_acquire, fake_release, fake_spawn, fake_pause, fake_stop,
    fake_pressure
};

static void e_hs_ifabi(void)  { (void)infy_host_handshake(1, 0, 0, infy_env_layout_hash()); }
static void e_hs_events(void) {
    static const uint32_t ids[2] = { 5, 7 };
    (void)infy_host_handshake(INFY_IFABI, ids, 2, infy_env_layout_hash());
}
static void e_hs_env(void)    { (void)infy_host_handshake(INFY_IFABI, 0, 0, 0xDEADBEEFDEADBEEFull); }
static void e_hs_no_table(void) { (void)infy_host_handshake(INFY_IFABI, 0, 3, infy_env_layout_hash()); }
static void e_call_unknown(void) { InfyArgFrame fr; memset(&fr, 0, sizeof fr);
                                   (void)infy_host_call_event(31337, &fr, &LOC(300, 1, 0)); }
static void e_call_err(void)  { InfyArgFrame fr; memset(&fr, 0, sizeof fr);
                                 (void)infy_host_call_event(4040, &fr, &LOC(301, 1, 0)); }
static void e_call_deep(void) {
    infy_test_set_host_nesting(8);
    InfyArgFrame fr; memset(&fr, 0, sizeof fr);
    (void)infy_host_call_event(5, &fr, &LOC(302, 1, 0));
    infy_test_set_host_nesting(0);
}
static void e_query_wrong_type(void) {
    int64_t x = 0;
    (void)infy_host_query(infy_str_of("ci/attempt"), &x);
}

static void test_host(void) {
    section("host protocol: three-step handshake, events, query (§11.1–§11.4)");

    infy_host_register(&g_fake_host);
    CK_EQ(infy_host_is_registered(), 1u);

    /* step 1: IFABI must match exactly, else R-0903 (§11.3) */
    expect_panic("handshake IFABI", R_IFABI_MISMATCH, e_hs_ifabi);
    /* step 2: an event the host does not implement is R-0904 */
    static const uint32_t host_ids[1] = { 5 };
    infy_host_register_events(host_ids, 1, 0);
    expect_panic("handshake events", R_EVENT_MISSING, e_hs_events);
    expect_panic("handshake missing table", R_EVENT_MISSING, e_hs_no_table);
    /* step 3: EnvSnapshot layout, else R-0905 */
    expect_panic("handshake env layout", R_ENV_LAYOUT, e_hs_env);

    /* all three steps pass */
    uint64_t hash = infy_env_layout_hash();
    CK_EQ(infy_host_handshake(INFY_IFABI, host_ids, 1, hash), 0);
    CK_EQ(infy_host_handshake_done(), 1u);

    /* unknown id -> R-0900 (the version-mismatch symptom, §11.2); host error -> R-0900 */
    infy_host_register_events(0, 0, 0);                 /* no registry: only the host answers */
    static const uint32_t both[2] = { 5, 4040 };
    infy_host_register_events(both, 2, 0);
    expect_panic("event id unknown to host", R_HOST_UNMAPPED, e_call_unknown);
    expect_panic("host returned an error", R_HOST_UNMAPPED, e_call_err);
    /* nesting deeper than 8 is R-0902 (§11.1) */
    expect_panic("host nesting > 8", R_HOST_NESTING, e_call_deep);

    /* a well-formed call returns the value the host wrote */
    InfyArgFrame fr; memset(&fr, 0, sizeof fr);
    InfyOutVal v = infy_host_call_event(5, &fr, &LOC(303, 1, 0));
    CK_EQ(v.tag, 1u);
    CK_EQ(v.v.i, 10);

    /* std.host.query: keys are host-named, the language layer enumerates nothing (§10.8) */
    infy_host_set_query("queue/depth", INFY_HQ_INT, 42, infy_str_of(""));
    int64_t got = 0;
    CK_EQ(infy_host_query(infy_str_of("queue/depth"), &got), 1u);
    CK_EQ(got, 42);
    CK_EQ(infy_host_query(infy_str_of("no/such/key"), &got), 0u);   /* None, not an error */
    infy_host_set_query("ci/attempt", INFY_HQ_STR, 0, infy_str_of("seven"));
    InfyString qs = infy_string_new();
    CK_EQ(infy_host_query_str(infy_str_of("ci/attempt"), &qs), 1u);
    CK_STR((const char*)qs.ptr, "seven");
    infy_string_drop(&qs);
    /* wrong key type is R-0901, not a silent reinterpretation */
    expect_panic("query type mismatch", R_HOST_QUERY_TYP, e_query_wrong_type);
    /* std.host.set is the writable side (§10.8) */
    CK_EQ(infy_host_set(infy_str_of("manual/key"), infy_str_of("value")), 0);
    CK_EQ(infy_host_query_str(infy_str_of("manual/key"), &qs), 1u);
    CK_STR((const char*)qs.ptr, "value");
    infy_string_drop(&qs);

    /* acquire_resource/release_resource run outside the serialising host lock (§11.1) */
    CK_EQ(infy_lock_acquire_host_resource(3, 1u), 777u);
    infy_release_host_resource(777);

    /* env snapshot: fixed layout, offset loads, per-focus triple (§4.6) */
    g_fake_env.iter = 17; g_fake_env.ram_mb = 4096; g_fake_env.cpu_use = 0.25f;
    g_fake_env.platform = infy_str_of("win10");
    infy_env_update(&g_fake_env);
    const InfyEnvSnapshot* e = infy_env();
    CK_EQ(e->iter, 17u);
    CK_EQ(e->ram_mb, 4096u);
    CK(fabs(e->cpu_use - 0.25f) < 1e-6f);
    CK_EQ(infy_env_layout_hash(), infy_env_layout_hash());
    /* the triple is stable across calls in the same dispatch context */
    CK_EQ(infy_current_focus(), 0u);
    CK_EQ(infy_current_chain(), 0u);
    CK(infy_current_node_loc() == 0);
}

/* ============================ 13. §9.2 rendering, byte for byte ============================ */

static void e_render_panic(void) {
    InfyNodeLoc l = { "rt_sample.infy", 12, 21, 3, 1 };
    infy_panic(R_INDEX_OOB, "index 5 out of bounds for length 4", &l);
}

static char* slurp(const char* path, size_t* n) {
    FILE* f = fopen(path, "rb");
    if (!f) { *n = 0; return 0; }
    fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
    char* b = (char*)malloc((size_t)sz + 1);
    size_t r = fread(b, 1, (size_t)sz, f);
    b[r] = 0;
    fclose(f);
    *n = r;
    return b;
}

static void test_diag_rendering(void) {
    section("§9.2 diagnostic geometry, byte-exact");

    /* the source file the diagnostic points at, so the runtime can print the real line */
    FILE* src = fopen("rt_sample.infy", "wb");
    for (int i = 1; i <= 11; i++) fprintf(src, "// filler line %d\n", i);
    fprintf(src, "    var secs: u32 = ticks * 1.5;\n");
    fclose(src);

    freopen("rt_render.log", "wb", stderr);
    infy_rt_catch(e_render_panic, 0, 0);
    fflush(stderr);
    freopen("rt_diag.log", "ab", stderr);

    size_t n = 0;
    char* got = slurp("rt_render.log", &n);
    CK(got != 0);
    /* expected: title row, header row, blank gutter, source row, caret row, focus/chain/node row,
     * note row. gw = digits(12)+1 = 3, so the gutter is "  │" and the caret sits at row index
     * gw + col_start = 3 + 21 = 24 (§9.2 / src/diag.rs geometry). */
    const char* want =
        "runtime[R-0004]: " "\xe6\x95\xb0\xe7\xbb\x84\xe8\xb6\x8a\xe7\x95\x8c" "\n"
        "  \xe2\x94\x8c\xe2\x94\x80 rt_sample.infy:12:21\n"
        "  \xe2\x94\x82\n"
        "12\xe2\x94\x82     var secs: u32 = ticks * 1.5;\n"
        "  \xe2\x94\x82                     ^  index out of bounds\n"
        "  = focus: 0, chain: 0, node: rt_sample.infy:12:21 (node_idx 3)\n"
        "  = note: index 5 out of bounds for length 4\n";
    CK_STR(got, want);

    /* a location whose file cannot be read prints the documented placeholder */
    static const char* badfile = "rt_does_not_exist.infy";
    freopen("rt_render2.log", "wb", stderr);
    InfyNodeLoc l2 = { badfile, 7, 3, 1, 0 };
    infy_report(R_UNWRAP_NONE, 0, 4, 9, &l2);
    fflush(stderr);
    freopen("rt_diag.log", "ab", stderr);
    char* g2 = slurp("rt_render2.log", &n);
    CK(strstr(g2, "<source unavailable>") != 0);
    CK(strstr(g2, "runtime[R-0005]: ") != 0);
    CK(strstr(g2, "  = focus: 4, chain: 9, node: rt_does_not_exist.infy:7:3 (node_idx 1)") != 0);
    /* gutter geometry for a single-digit line number: gw = 2 */
    CK(strncmp(g2, "runtime[R-0005]", 15) == 0);
    CK(strstr(g2, "\n  \xe2\x94\x94") == 0);
    CK(strstr(g2, "7\xe2\x94\x82 <source unavailable>") == 0);
    free(g2);

    /* every normative R- code renders with its table title (§9.3/§9.7) */
    freopen("rt_render3.log", "wb", stderr);
    static const uint32_t codes[] = { 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007,
                                      0x0008, 0x0009, 0x0010, 0x0011, 0x0012, 0x0703, 0x0710,
                                      0x0720, 0x0722, 0x0723, 0x0730, 0x0802, 0x0803, 0x0900,
                                      0x0901, 0x0902, 0x0903, 0x0904, 0x0905, 0x1010, 0x1040 };
    for (size_t i = 0; i < sizeof codes / sizeof codes[0]; i++)
        infy_report((InfyRCode)codes[i], 0, 1, 1, &LOC(1, 1, 0));
    fflush(stderr);
    freopen("rt_diag.log", "ab", stderr);
    char* g3 = slurp("rt_render3.log", &n);
    for (size_t i = 0; i < sizeof codes / sizeof codes[0]; i++) {
        char needle[32];
        snprintf(needle, sizeof needle, "runtime[R-%04X]: ", (unsigned)codes[i]);
        CK(strstr(g3, needle) != 0);
    }
    /* the mandatory triple (§12.4) appears on every one of them */
    int triples = 0;
    for (const char* p = strstr(g3, "  = focus: "); p; p = strstr(p + 1, "  = focus: ")) triples++;
    CK_EQ(triples, (int)(sizeof codes / sizeof codes[0]));
    free(g3);
}

/* ============================ 14. std.fs (§10.3) ============================ */

static void test_fs(void) {
    section("std.fs (§10.3)");

    InfyIoErr err; memset(&err, 0, sizeof err);
    CK_EQ(infy_fs_write_all("rt_fs_probe.txt", (const uint8_t*)"hello infy", 10, &err), 0);
    CK_EQ(infy_fs_exists("rt_fs_probe.txt"), 1);
    CK_EQ(infy_fs_exists("rt_no_such_file_xyz.txt"), 0);
    uint8_t* data = 0; uint64_t len = 0;
    CK_EQ(infy_fs_read_all("rt_fs_probe.txt", &data, &len, &err), 0);
    CK_EQ(len, 10u);
    CK(memcmp(data, "hello infy", 10) == 0);
    infy_free(data);

    /* a missing file reports IE_NOTFOUND with a message, never a crash (§4.9 determinism) */
    memset(&err, 0, sizeof err);
    CK_EQ(infy_fs_read_all("rt_no_such_file_xyz.txt", &data, &len, &err), -1);
    CK_EQ(err.kind, IE_NOTFOUND);
    CK(err.message.len > 0);
    infy_string_drop(&err.message);

    /* mkdir_p creates the whole chain */
    memset(&err, 0, sizeof err);
    CK_EQ(infy_fs_mkdir_p("rt_fs_dir/a/b/c", &err), 0);
    CK_EQ(infy_fs_exists("rt_fs_dir/a/b/c"), 1);
    CK_EQ(infy_fs_mkdir_p("rt_fs_dir/a/b/c", &err), 0);        /* idempotent */

    /* File is a whole-file buffer; its Drop closes it (§10.3), so close flushes */
    InfyFileHandle fh;
    memset(&err, 0, sizeof err);
    CK_EQ(infy_fs_open("rt_fs_handle.txt", "w", &fh, &err), 0);
    CK_EQ(fh.readonly, 0u);
    memcpy(fh.buf, "written by close", 18);
    fh.len = 18;
    infy_fs_close(&fh);
    memset(&err, 0, sizeof err);
    CK_EQ(infy_fs_read_all("rt_fs_handle.txt", &data, &len, &err), 0);
    CK_EQ(len, 18u);
    CK(memcmp(data, "written by close", 18) == 0);
    infy_free(data);

    /* read-only handle: close must not write anything back */
    memset(&err, 0, sizeof err);
    CK_EQ(infy_fs_open("rt_fs_probe.txt", "r", &fh, &err), 0);
    CK_EQ(fh.readonly, 1u);
    CK_EQ(fh.len, 10u);
    infy_fs_close(&fh);
    CK_EQ(infy_fs_exists("rt_fs_probe.txt"), 1);
    memset(&err, 0, sizeof err);
    CK_EQ(infy_fs_read_all("rt_fs_probe.txt", &data, &len, &err), 0);
    CK_EQ(len, 10u);
    infy_free(data);
}

/* ============================ 15. mem stats + log + inits (§10.14, §7.7.1) ============================ */

static int g_init_order[4];
static int g_init_n;
static void init0(void) { g_init_order[g_init_n++] = 0; }
static void init1(void) { g_init_order[g_init_n++] = 1; }
static void init2(void) { g_init_order[g_init_n++] = 2; }

static void test_misc(void) {
    section("mem stats, leak check, log sink, package inits (§10.14, §7.6.5, §5.4)");

    InfyMemStats before = infy_mem_stats();
    void* p = infy_alloc(1000, 8);
    CK(p != 0);
    /* the 16-byte header keeps the payload aligned and the block trackable (§4.8 address stability) */
    CK((((uintptr_t)p) % 8) == 0);
    InfyMemStats mid = infy_mem_stats();
    CK(mid.live_bytes > before.live_bytes);
    CK(mid.live_objs > before.live_objs);
    CK(mid.total_allocs > before.total_allocs);
    void* q = infy_realloc(p, 1000, 2000, 8);
    CK(q != 0);
    memset(q, 0x5A, 2000);
    CK(((uint8_t*)q)[1999] == 0x5A);
    infy_free(q);
    InfyMemStats after = infy_mem_stats();
    CK_EQ(after.live_bytes, before.live_bytes);
    CK_EQ(after.live_objs, before.live_objs);
    infy_free(0);                       /* not UB, a no-op */

    /* §7.6.5: a callback Arc still held at unload is R-0723 */
    uint64_t cnt = 1;
    int payload = 7;
    void* boxed = infy_arc_cb_box(&payload, &cnt);
    CK(boxed != 0);
    expect_panic("leak check with live callback", R_CB_LEAK, e_leak_check);
    /* releasing it (count dropped to 0 first) makes the check pass */
    cnt = 0;
    expect_panic("arc_cb_free with live count", R_CB_LEAK, e_free_leaky);
    infy_arc_cb_free(boxed, &LOC(1, 1, 0));
    infy_leak_check();                  /* must not panic now */

    /* the log sink B-track print maps to (§7.7.1 item 4) */
    freopen("rt_log.log", "wb", stderr);
    infy_log(2, infy_str_of("normalised 3 rows"), "python", 88);
    infy_log(4, infy_str_of("bad"), "python", 1);
    fflush(stderr);
    freopen("rt_diag.log", "ab", stderr);
    size_t n = 0;
    char* lg = slurp("rt_log.log", &n);
    CK(strstr(lg, "[info] python:88: normalised 3 rows") != 0);
    CK(strstr(lg, "[error] python:1: bad") != 0);
    free(lg);

    /* package inits run in the order the compiler emits (§5.4) */
    static const InfyInitFn fns[3] = { init1, init0, init2 };
    g_init_n = 0;
    infy_run_package_inits(fns, 3);
    CK_EQ(g_init_n, 3);
    CK_EQ(g_init_order[0], 1);
    CK_EQ(g_init_order[1], 0);
    CK_EQ(g_init_order[2], 2);
    infy_run_package_inits(0, 0);
}

void e_leak_check(void) { infy_leak_check(); }
void e_free_leaky(void) {
    static uint64_t c = 2;
    static int obj = 1;
    void* b = infy_arc_cb_box(&obj, &c);
    infy_arc_cb_free(b, &LOC(2, 1, 0));
    c = 0;
    infy_arc_cb_free(b, &LOC(2, 1, 0));   /* already released: no-op, must not double free */
}

/* ============================ main ============================ */

int main(int argc, char** argv) {
    /* diagnostics go to a log so the console shows only assertion results */
    freopen("rt_diag.log", "wb", stderr);

    if (argc > 2 && strcmp(argv[1], "--dump-seed") == 0) {
        /* cross-process reproducibility probe: same seed must print the same bytes twice */
        unsigned long long seed = strtoull(argv[2], 0, 16);
        dump_rng(seed);
        for (int i = 0; i < 8; i++) printf("%016llx\n", (unsigned long long)g_stream[i]);
        for (int i = 0; i < 16; i++) printf("%llu\n", (unsigned long long)g_range_vals[i]);
        for (int i = 0; i < 12; i++) printf("%d ", g_perm[i]);
        printf("\n");
        freopen("rt_diag.log", "ab", stderr);
        return 0;
    }

    init_mono_base_once();

    test_arithmetic();
    test_drop_order();
    test_vec_cap();
    test_strings();
    test_map();
    test_hash();
    test_fmt();
    test_rand();
    test_lock_fifo();
    test_focus_radius();
    test_radius_chain();
    test_focus_wraparound();
    test_pause_yield();
    test_artifact();
    test_host();
    test_diag_rendering();
    test_fs();
    test_misc();

    printf("\n%d assertions, %d failures\n", g_checks, g_failures);
    fflush(stdout);
    return g_failures ? 1 : 0;
}
