/* test_ifbc.c — standalone harness for the IFBC loader/verifier/interpreter/object model.
 *
 * Builds IFBC module images BYTE BY BYTE in memory (see build_img/emit_img) and asserts the
 * behaviour mandated by §7.8.3 / appendix D / UB-019 / UB-020. It is compiled against
 * test_ifbc_stubs.c (a harness-only stand-in for infy_rt.c), NOT the production runtime.
 *
 * Build+run:
 *   gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c \
 *       runtime/test_ifbc.c runtime/test_ifbc_stubs.c -o ifbc_tests
 *   ./ifbc_tests
 */
#include "infy_rt.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ===== symbols implemented in infy_ifbc.c / infy_dyn.c but NOT declared in infy_rt.h =====
 * (header gaps reported to the front end: there is no module-unload or host-bind entry point
 *  in the frozen contract, and the extra dyn constructors are private.) */
extern void infy_ifbc_module_free(InfyIfbcModule* m);
typedef int (*InfyHostThunk)(InfyDyn** args, uint32_t nargs, InfyDyn** ret, const InfyNodeLoc* at);
void infy_ifbc_bind_host(InfyIfbcModule* m, uint32_t import_idx, InfyHostThunk fn);
uint64_t infy_stub_live_objs(void);
uint64_t infy_stub_live_bytes(void);

/* ===== opcode byte values — MUST match infy_ifbc.c's OPS table ===== */
enum {
    OP_NOP = 0x00, OP_LDL = 0x01, OP_STL = 0x02, OP_LDW = 0x03, OP_PUSH = 0x04, OP_PUSHB = 0x05,
    OP_DUP = 0x06, OP_POP = 0x07, OP_LDNULL = 0x08, OP_LDFRAME = 0x09, OP_DUP2 = 0x0A,
    OP_SWAP = 0x0B, OP_GUARD_NZ = 0x0C, OP_GUARD_SH = 0x0D,
    OP_ADD_I = 0x20, OP_SUB_I = 0x21, OP_MUL_I = 0x22, OP_DIV_I = 0x23, OP_REM_I = 0x24,
    OP_NEG_I = 0x25, OP_ABS_I = 0x26, OP_AND_I = 0x27, OP_OR_I = 0x28, OP_XOR_I = 0x29,
    OP_NOT_I = 0x2A, OP_SHL_I = 0x2B, OP_SHR_I = 0x2C, OP_SHRA_I = 0x2D, OP_GUARD_NZ2 = 0x2F,
    OP_ADD_F = 0x35,
    OP_JMP = 0x40, OP_BR = 0x41, OP_BRF = 0x42, OP_SWITCH = 0x43, OP_TICKLOOP = 0x44,
    OP_LEAVE = 0x45, OP_THROW = 0x46, OP_ENTER_TRY = 0x47,
    OP_CALL_STAT = 0x60, OP_CALL_VIRT = 0x61, OP_CALL_HOST = 0x62, OP_CALL_EMBED = 0x63,
    OP_RET = 0x64, OP_RET_MULTI = 0x65, OP_CLOSURE = 0x66, OP_TAILCALL = 0x67,
    OP_NEW_STR = 0x80, OP_NEW_LIST = 0x81, OP_NEW_DICT = 0x82, OP_NEW_FRAME = 0x83,
    OP_IDX_GET = 0x88, OP_IDX_SET = 0x89, OP_SLICE = 0x8A, OP_REF_INC = 0x90, OP_REF_DEC = 0x91,
    OP_MOVE_OUT = 0x94,
    OP_TAG_GET = 0xA0, OP_CAST_DYN = 0xA1, OP_IS_DYN = 0xA2, OP_BOX_DYN = 0xA3, OP_UNBOX = 0xA4,
    OP_INVOKE_DYN = 0xA8,
    OP_FRAME_ENTER = 0xC0, OP_YIELD = 0xC1, OP_YIELD_FROM = 0xC2, OP_FRAME_EXIT = 0xC3,
    OP_CORO_RESUME = 0xC4, OP_CORO_THROW = 0xC5,
    OP_TYPEOF = 0xE0, OP_SIZE_OF = 0xE1, OP_ALIGN_OF = 0xE2, OP_FIELD_OFF = 0xE3
};

/* ===== growable byte buffer ===== */
typedef struct { uint8_t* b; size_t n, cap; } Buf;
static void bu_init(Buf* x) { x->b = 0; x->n = x->cap = 0; }
static void bu_need(Buf* x, size_t k) {
    if (x->n + k <= x->cap) return;
    size_t nc = x->cap ? x->cap : 64;
    while (nc < x->n + k) nc *= 2;
    uint8_t* nb = (uint8_t*)realloc(x->b, nc);
    if (!nb) { fprintf(stderr, "OOM\n"); exit(2); }
    x->b = nb; x->cap = nc;
}
static void bu_u8(Buf* x, uint8_t v) { bu_need(x, 1); x->b[x->n++] = v; }
static void bu_u16(Buf* x, uint16_t v) { bu_need(x, 2); x->b[x->n++] = (uint8_t)(v >> 8); x->b[x->n++] = (uint8_t)v; }
static void bu_u32(Buf* x, uint32_t v) { bu_need(x, 4); x->b[x->n++] = (uint8_t)(v >> 24); x->b[x->n++] = (uint8_t)(v >> 16); x->b[x->n++] = (uint8_t)(v >> 8); x->b[x->n++] = (uint8_t)v; }
static void bu_u64(Buf* x, uint64_t v) { bu_u32(x, (uint32_t)(v >> 32)); bu_u32(x, (uint32_t)v); }
static void bu_raw(Buf* x, const void* p, size_t k) { bu_need(x, k); memcpy(x->b + x->n, p, k); x->n += k; }
static void bu_free(Buf* x) { free(x->b); bu_init(x); }

/* instruction emitters (opcode + fixed u32 operands). Return the offset where emitted. */
static size_t c_off(Buf* cb) { return cb->n; }
static void c0(Buf* cb, uint8_t op) { bu_u8(cb, op); }
static void c1(Buf* cb, uint8_t op, uint32_t a) { bu_u8(cb, op); bu_u32(cb, a); }
static void c2(Buf* cb, uint8_t op, uint32_t a, uint32_t b) { bu_u8(cb, op); bu_u32(cb, a); bu_u32(cb, b); }
static void cswitch(Buf* cb, uint32_t n, const uint32_t* tgts) { bu_u8(cb, OP_SWITCH); bu_u32(cb, n); for (uint32_t i = 0; i < n; i++) bu_u32(cb, tgts[i]); }

/* ===== image assembly ===== */
typedef struct { uint32_t code_off, code_len, locals, max_stack, nargs, ret_arity, ret_sig, name, flags, frame_kind; } FDesc;
typedef struct { uint32_t off, line; } LDesc;
typedef struct { uint32_t start, end, handler; uint16_t depth, kind; } XDesc;

static void emit_img(Buf* out, const FDesc* funcs, uint32_t nf,
                     const uint8_t* code, uint32_t clen,
                     uint32_t ntypes, int type_reprc,
                     uint32_t nimports, int import_signed,
                     const LDesc* lines, uint32_t nl,
                     const XDesc* excs, uint32_t nx) {
    /* header */
    bu_raw(out, INFY_IFBC_MAGIC, 4);
    bu_u16(out, 1);           /* ifbc_version */
    bu_u16(out, INFY_IFABI);  /* ifabi */
    bu_u16(out, 0x0100);      /* lang_tag (CPY-313-ish) */
    for (int i = 0; i < 32; i++) bu_u8(out, (uint8_t)i); /* module_hash */
    bu_u32(out, nf);          /* func_count */
    bu_u8(out, 0);            /* frame_kind: single-address stack + local window */

    /* type_table */
    bu_u32(out, ntypes);
    for (uint32_t i = 0; i < ntypes; i++) {
        bu_u32(out, 0);                 /* name_str */
        bu_u32(out, 1);                 /* kind: struct */
        bu_u32(out, type_reprc ? 1u : 0u); /* flags bit0 = repr(C) */
        bu_u32(out, 16);                /* size */
        bu_u32(out, 8);                 /* align */
        bu_u32(out, 2);                 /* nfields */
        bu_u32(out, 0); bu_u32(out, 0);  bu_u32(out, 0); /* field0: name,off,ty */
        bu_u32(out, 0); bu_u32(out, 8);  bu_u32(out, 0); /* field1: name,off,ty */
    }

    /* import_table */
    bu_u32(out, nimports);
    for (uint32_t i = 0; i < nimports; i++) {
        bu_u32(out, 0);                 /* name_str */
        bu_u8(out, 0);                  /* kind: host */
        bu_u8(out, 2);                  /* nparams */
        bu_u8(out, 7);                  /* ret_sig = I64 */
        bu_u8(out, import_signed ? 1u : 0u); /* flags bit0 = IFABI-signed */
        bu_u8(out, 7);                  /* param0 = I64 */
        bu_u8(out, 7);                  /* param1 = I64 */
    }

    /* func_table */
    bu_u32(out, nf);
    bu_u64(out, 0);            /* max_mem_bytes (no runtime budget in tests) */
    for (uint32_t i = 0; i < nf; i++) {
        const FDesc* f = &funcs[i];
        bu_u32(out, f->code_off); bu_u32(out, f->code_len); bu_u32(out, f->locals);
        bu_u32(out, f->max_stack); bu_u32(out, f->nargs); bu_u32(out, f->ret_arity);
        bu_u32(out, f->ret_sig); bu_u32(out, f->name); bu_u32(out, f->flags); bu_u32(out, f->frame_kind);
    }

    /* code */
    bu_u32(out, clen);
    bu_raw(out, code, clen);

    /* line_map */
    bu_u32(out, nl);
    for (uint32_t i = 0; i < nl; i++) { bu_u32(out, lines[i].off); bu_u32(out, lines[i].line); }

    /* exception_table */
    bu_u32(out, nx);
    for (uint32_t i = 0; i < nx; i++) { bu_u32(out, excs[i].start); bu_u32(out, excs[i].end); bu_u32(out, excs[i].handler); bu_u16(out, excs[i].depth); bu_u16(out, excs[i].kind); }

    /* string_pool: exactly one 1-byte string "s" (index 0). */
    bu_u32(out, 1);
    bu_u32(out, 1);
    bu_u8(out, 's');
}

/* ===== scenario builder ===== */
typedef enum {
    SC_VALID = 0, SC_NO_MAP, SC_LDNULL, SC_LDNULL_MID, SC_DIV_NOGUARD, SC_SHL_NOGUARD,
    SC_YIELD_NORMAL, SC_UNBOX_NOIS, SC_INVOKE_BAD, SC_FIELDOFF_BAD, SC_JUMP_MID,
    SC_STACK_DIVERGE, SC_HOST_BAD, SC_TAILCALL_BAD, SC_TICKET, SC_ALL_ERRORS,
    SC_CLOSURE_OK, SC_THROW_OK, SC_OBJ_OK, SC_SWITCH_OK
} Scenario;

/* assemble one scenario's image into `out` (caller frees). */
static void build_img(Scenario s, Buf* out) {
    Buf cb; bu_init(&cb);
    FDesc funcs[4]; uint32_t nf = 1;
    LDesc lines[32]; uint32_t nl = 0;
    XDesc excs[4]; uint32_t nx = 0;
    uint32_t ntypes = 1, nimports = 1;
    int type_reprc = 1, import_signed = 1;
    memset(funcs, 0, sizeof funcs);

    switch (s) {
        case SC_VALID: {
            size_t o0 = c_off(&cb); (void)o0;
            c1(&cb, OP_LDL, 0);
            c1(&cb, OP_LDL, 1);
            c0(&cb, OP_ADD_I);
            c0(&cb, OP_RET);
            size_t cl = c_off(&cb);
            funcs[0] = (FDesc){0, (uint32_t)cl, 2, 2, 2, 1, 7, 0, 1, 0};
            lines[0] = (LDesc){0, 10}; lines[1] = (LDesc){5, 10}; lines[2] = (LDesc){10, 11}; lines[3] = (LDesc){11, 12}; nl = 4;
            break;
        }
        case SC_NO_MAP: {
            c1(&cb, OP_LDL, 0); c1(&cb, OP_LDL, 1); c0(&cb, OP_ADD_I); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 2, 2, 2, 1, 7, 0, 1, 0};
            nl = 0; /* intentionally empty -> W-0705 (non-blocking) */
            break;
        }
        case SC_LDNULL: { /* ldnull first */
            c0(&cb, OP_LDNULL); c0(&cb, OP_POP);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 2, 0, 0, 0, 0, 0, 0};
            break;
        }
        case SC_LDNULL_MID: { /* ldnull after real instructions */
            c1(&cb, OP_PUSHB, 1); c0(&cb, OP_LDNULL); c0(&cb, OP_POP); c0(&cb, OP_POP);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 0, 0, 0, 0, 0};
            break;
        }
        case SC_DIV_NOGUARD: {
            c1(&cb, OP_PUSHB, 3); c1(&cb, OP_PUSHB, 4); c0(&cb, OP_DIV_I); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_SHL_NOGUARD: {
            c1(&cb, OP_PUSHB, 3); c1(&cb, OP_PUSHB, 2); c0(&cb, OP_SHL_I); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_YIELD_NORMAL: {
            c1(&cb, OP_PUSHB, 7); c0(&cb, OP_YIELD); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_UNBOX_NOIS: {
            c1(&cb, OP_PUSHB, 1); c0(&cb, OP_UNBOX); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_INVOKE_BAD: { /* invoke.dynamic to func index 999 (>= func_count) */
            c1(&cb, OP_PUSHB, 1); c1(&cb, OP_INVOKE_DYN, 999); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_FIELDOFF_BAD: { /* field_off on a non-repr(C) type */
            type_reprc = 0;
            c2(&cb, OP_FIELD_OFF, 0, 0); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 2, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_JUMP_MID: { /* jmp to offset 1 (mid push.b) */
            c1(&cb, OP_PUSHB, 5); /* [0,5) */
            c1(&cb, OP_JMP, 1);   /* target 1 -> not a boundary */
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 0, 0, 0, 0, 0};
            break;
        }
        case SC_STACK_DIVERGE: {
            /* 0 push.b ; 5 brf->20 ; 10 push.b ; 15 jmp->30 ; 20 push.b ; 25 push.b ; 30 ret */
            c1(&cb, OP_PUSHB, 9);
            c1(&cb, OP_BRF, 20);
            c1(&cb, OP_PUSHB, 1);
            c1(&cb, OP_JMP, 30);
            c1(&cb, OP_PUSHB, 2);
            c1(&cb, OP_PUSHB, 3);
            c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 2, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_HOST_BAD: { /* call.host to an import that is not IFABI-signed */
            import_signed = 0;
            c1(&cb, OP_PUSHB, 1); c1(&cb, OP_PUSHB, 2); c1(&cb, OP_CALL_HOST, 0); c0(&cb, OP_RET);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_TAILCALL_BAD: { /* tailcall to a frame-size-incompatible function */
            nf = 2;
            Buf cb0; bu_init(&cb0); Buf cb1; bu_init(&cb1);
            c1(&cb0, OP_TAILCALL, 1);
            c1(&cb1, OP_PUSHB, 1); c0(&cb1, OP_RET);
            uint32_t l0 = (uint32_t)cb0.n, l1 = (uint32_t)cb1.n;
            bu_raw(&cb, cb0.b, cb0.n);
            uint32_t off1 = (uint32_t)cb.n;
            bu_raw(&cb, cb1.b, cb1.n);
            funcs[0] = (FDesc){0, l0, 2, 1, 0, 1, 7, 0, 1, 0};   /* locals 2 */
            funcs[1] = (FDesc){off1, l1, 1, 2, 0, 1, 7, 0, 1, 0}; /* locals 1 -> mismatch */
            bu_free(&cb0); bu_free(&cb1);
            break;
        }
        case SC_TICKET: { /* tickloop 3 then jmp back to itself: infinite without budget */
            c1(&cb, OP_TICKLOOP, 3);
            c1(&cb, OP_JMP, 0);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 2, 0, 0, 0, 0, 0, 0};
            break;
        }
        case SC_ALL_ERRORS: { /* func0 ldnull (R-0706), func1 yield-normal (R-0707) */
            nf = 2;
            Buf cba; bu_init(&cba); Buf cbb; bu_init(&cbb);
            c1(&cba, OP_PUSHB, 1); c0(&cba, OP_LDNULL); c0(&cba, OP_POP); c0(&cba, OP_POP);
            c1(&cbb, OP_PUSHB, 1); c0(&cbb, OP_YIELD); c0(&cbb, OP_POP);
            uint32_t la = (uint32_t)cba.n, lb = (uint32_t)cbb.n;
            bu_raw(&cb, cba.b, cba.n); uint32_t offb = (uint32_t)cb.n; bu_raw(&cb, cbb.b, cbb.n);
            funcs[0] = (FDesc){0, la, 0, 4, 0, 0, 0, 0, 0, 0};
            funcs[1] = (FDesc){offb, lb, 0, 4, 0, 0, 0, 0, 0, 0};
            bu_free(&cba); bu_free(&cbb);
            break;
        }
        case SC_CLOSURE_OK: { /* closure.new -> call.virt -> ret (executed) */
            nf = 2;
            Buf cbA; bu_init(&cbA); Buf cbB; bu_init(&cbB);
            /* funcA: closure.new funcB(0 caps); then a call via invoke.dynamic 1? use closure + call.virt needs receiver; use call.stat */
            c1(&cbA, OP_PUSHB, 21);            /* h->1 */
            c2(&cbA, OP_CLOSURE, 1, 0);         /* closure to funcB, 0 caps: h->1 */
            c1(&cbA, OP_CALL_STAT, 1);          /* call funcB (nargs0): h unchanged */
            c0(&cbA, OP_RET);
            c1(&cbB, OP_PUSHB, 21);
            c0(&cbB, OP_RET);
            uint32_t lA = (uint32_t)cbA.n, lB = (uint32_t)cbB.n;
            bu_raw(&cb, cbA.b, cbA.n); uint32_t offB = (uint32_t)cb.n; bu_raw(&cb, cbB.b, cbB.n);
            funcs[0] = (FDesc){0, lA, 0, 8, 0, 1, 7, 0, 1, 0};
            funcs[1] = (FDesc){offB, lB, 0, 8, 0, 1, 7, 0, 1, 0};
            bu_free(&cbA); bu_free(&cbB);
            break;
        }
        case SC_THROW_OK: { /* throw.inf with a matching exception region -> jumps to handler */
            c1(&cb, OP_PUSHB, 1);   /* 0..4  error payload */
            c1(&cb, OP_THROW, 0);   /* 5..9  throw.inf k=0 */
            c1(&cb, OP_PUSHB, 42);  /* 10..14 handler pushes 42 */
            c0(&cb, OP_RET);        /* 15     return 42 */
            excs[0] = (XDesc){5, 10, 10, 0, 0}; nx = 1;
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_OBJ_OK: { /* new.list; push two ints; idx.get; add; ret */
            c0(&cb, OP_NEW_LIST);      /* h->1 */
            c1(&cb, OP_PUSHB, 7);      /* h->2 */
            /* push into list: reuse idx.set? our list build needs set at index; use seq via idx.set on grown list.
               Simpler: new.list then use host? Just verify load+exec of new.list + ret the list. */
            c0(&cb, OP_RET);           /* returns the list (h->0) */
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        case SC_SWITCH_OK: { /* switch whose case table is an inline data island */
            c1(&cb, OP_PUSHB, 0);            /* selector 0 at offset 0 (5 bytes) */
            uint32_t tgts[1];
            tgts[0] = 14;                    /* case 0 -> offset 14 (the nop landing pad) */
            cswitch(&cb, 1, tgts);           /* switch n=1: opcode+count+1 target = 9 bytes (5..13) */
            c0(&cb, OP_NOP);                 /* 14: landing pad */
            c1(&cb, OP_PUSHB, 42);           /* 15 */
            c0(&cb, OP_RET);                 /* 20 */
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 1, 7, 0, 1, 0};
            break;
        }
        default: {
            c0(&cb, OP_NOP);
            funcs[0] = (FDesc){0, (uint32_t)c_off(&cb), 0, 4, 0, 0, 0, 0, 0, 0};
            break;
        }
    }

    uint32_t nf_final = nf;
    uint32_t ntypes_final = (s == SC_JUMP_MID || s == SC_STACK_DIVERGE || s == SC_TICKET || s == SC_ALL_ERRORS) ? 0 : ntypes;
    uint32_t nimports_final = (s == SC_JUMP_MID || s == SC_STACK_DIVERGE || s == SC_TICKET) ? 0 : nimports;
    if (s == SC_ALL_ERRORS) ntypes_final = 0;

    emit_img(out, funcs, nf_final, cb.b, (uint32_t)cb.n,
             ntypes_final, type_reprc,
             nimports_final, import_signed,
             lines, nl, excs, nx);
    bu_free(&cb);
}

/* ===== test scaffolding ===== */
static int g_pass = 0, g_fail = 0, g_asserts = 0;
#define CHECK(cond, name) do { g_asserts++; if (cond) { g_pass++; } \
    else { g_fail++; fprintf(stderr, "FAIL: %s (line %d)\n", (name), __LINE__); } } while (0)

static uint8_t* last_img; static size_t last_len;
static void remember(Buf* b) { free(last_img); last_img = (uint8_t*)malloc(b->n); memcpy(last_img, b->b, b->n); last_len = b->n; }

static int load_scenario(Scenario s, InfyIfbcModule** mout, FILE* rep, uint8_t all) {
    Buf img; bu_init(&img); build_img(s, &img);
    if (mout) *mout = 0;
    int rc = infy_ifbc_load(img.b, img.n, mout, all, rep);
    if (s == SC_VALID || s == SC_NO_MAP || s == SC_CLOSURE_OK || s == SC_THROW_OK || s == SC_OBJ_OK || s == SC_TICKET || s == SC_SWITCH_OK) remember(&img);
    bu_free(&img);
    return rc;
}

static FILE* mem_stream(void) { return tmpfile(); }
/* read a FILE* fully into a malloc buffer, set *n */
static char* slurp(FILE* f, long* n) {
    fflush(f);
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    char* b = (char*)malloc((size_t)sz + 1);
    size_t rd = fread(b, 1, (size_t)sz, f);
    b[rd] = 0; if (n) *n = (long)rd;
    return b;
}
static int count_prefix_lines(const char* text, const char* prefix) {
    int c = 0; const char* p = text; int plen = (int)strlen(prefix);
    while ((p = strstr(p, prefix)) != 0) { if (p == text || p[-1] == '\n') c++; p += plen; }
    return c;
}
static int strstr_prefix(const char* hay, size_t n, const char* pre);

/* ===== (a) valid module loads and executes ===== */
static void test_valid_add(void) {
    InfyIfbcModule* m = 0;
    uint64_t base = infy_stub_live_objs();
    int rc = load_scenario(SC_VALID, &m, 0, 0);
    CHECK(rc == 0 && m != 0, "(a) valid module loads");
    if (m) {
        InfyDyn* a = infy_dyn_int(2); InfyDyn* b = infy_dyn_int(3);
        InfyDyn* args[2] = { a, b };
        InfyDyn* ret = 0;
        int irc = infy_ifbc_invoke(m, 0, args, 2, &ret, 0, infy_current_node_loc());
        CHECK(irc == 0, "(a) invoke add returns 0");
        CHECK(ret && ret->tag == DY_INT && ret->v.i == 5, "(a) 2 + 3 == 5");
        if (ret) infy_dyn_decr(ret);
        infy_dyn_decr(a); infy_dyn_decr(b);
        infy_ifbc_module_free(m);
    }
    CHECK(infy_stub_live_objs() == base, "(a) no leak after load+invoke+free");
}

/* ===== (b) each of the six validations rejects with the exact code ===== */
static void test_six_validations(void) {
    struct { Scenario s; int code; const char* nm; } t[] = {
        { SC_JUMP_MID,     R_IFBC_NONLINEAR, "(b) R-0704 linearity" },
        { SC_STACK_DIVERGE,R_IFBC_STACK,     "(b) R-0705 stack height" },
        { SC_LDNULL,       R_IFBC_TYPE,      "(b) R-0706 operand type (ldnull)" },
        { SC_DIV_NOGUARD,  R_IFBC_TYPE,      "(b) R-0706 div w/o guard" },
        { SC_SHL_NOGUARD,  R_IFBC_TYPE,      "(b) R-0706 shl w/o guard" },
        { SC_YIELD_NORMAL, R_IFBC_FRAME,     "(b) R-0707 frame legality" },
        { SC_HOST_BAD,     R_IFBC_HOSTSIG,   "(b) R-0708 host signature" },
        { SC_TAILCALL_BAD, R_IFBC_MEMBUDGET, "(b) R-0709 tail-call frame size" },
    };
    for (size_t i = 0; i < sizeof t / sizeof t[0]; i++) {
        uint64_t base = infy_stub_live_objs();
        InfyIfbcModule* m = (InfyIfbcModule*)0x1;
        int rc = load_scenario(t[i].s, &m, 0, 0);
        CHECK(rc == t[i].code, t[i].nm);
        CHECK(m == 0, "  rejected module left no handle");
        CHECK(infy_stub_live_objs() == base, "  failed load frees the image copy (UB-019 no partial state)");
    }
    /* --all-errors reports more than one violation */
    FILE* r = mem_stream();
    int rc = load_scenario(SC_ALL_ERRORS, 0, r, 1);
    CHECK(rc == R_IFBC_TYPE, "(b) all-errors returns the first code");
    long n = 0; char* txt = slurp(r, &n);
    int rlines = count_prefix_lines(txt, "R-");
    CHECK(rlines >= 2, "(b) --all-errors lists >1 violation");
    free(txt); fclose(r);
}

/* ===== (c) ldnull rejected wherever it appears ===== */
static void test_ldnull_anywhere(void) {
    CHECK(load_scenario(SC_LDNULL, 0, 0, 0) == R_IFBC_TYPE, "(c) ldnull at first position rejected");
    CHECK(load_scenario(SC_LDNULL_MID, 0, 0, 0) == R_IFBC_TYPE, "(c) ldnull mid-stream rejected");
}

/* ===== (e) yield in a non-coroutine frame -> R-0707 ===== */
static void test_yield_frame(void) {
    CHECK(load_scenario(SC_YIELD_NORMAL, 0, 0, 0) == R_IFBC_FRAME, "(e) yield in non-coroutine frame -> R-0707");
}

/* ===== (f) tickloop tiny budget -> R-0701 without touching the process ===== */
static void test_tickloop_timeout(void) {
    InfyIfbcModule* m = 0;
    int rc = load_scenario(SC_TICKET, &m, 0, 0);
    CHECK(rc == 0 && m != 0, "(f) tickloop module loads (loop is valid bytecode)");
    if (m) {
        uint64_t base = infy_stub_live_objs();
        InfyDyn* ret = 0;
        int irc = infy_ifbc_invoke(m, 0, 0, 0, &ret, 60000, infy_current_node_loc());
        CHECK(irc == 0x0701, "(f) tickloop budget trips R-0701");
        CHECK(ret == 0, "(f) no value delivered on timeout");
        CHECK(infy_stub_live_objs() == base, "(f) timeout unwinds frames without touching process/leaking");
        infy_ifbc_module_free(m);
    }
}

/* ===== (g) unbox.dyn with no preceding is.dyn -> R-0710 ===== */
static void test_unbox_checkpoint(void) {
    CHECK(load_scenario(SC_UNBOX_NOIS, 0, 0, 0) == R_DYN_TAG, "(g) unbox.dyn without is.dyn -> R-0710");
}

/* ===== (h) invoke.dynamic outside func_table -> R-0712 ===== */
static void test_invoke_target(void) {
    CHECK(load_scenario(SC_INVOKE_BAD, 0, 0, 0) == R_DYN_TARGET, "(h) invoke.dynamic idx outside func_table -> R-0712");
}

/* ===== (i) field_off on a non-repr(C) type -> R-0713 ===== */
static void test_field_off(void) {
    CHECK(load_scenario(SC_FIELDOFF_BAD, 0, 0, 0) == 0x0713, "(i) field_off on non-repr(C) -> R-0713");
}

/* ===== (j)/(k) already covered; assert they are distinct from linearity codes ===== */
static void test_jump_and_height(void) {
    CHECK(load_scenario(SC_JUMP_MID, 0, 0, 0) == R_IFBC_NONLINEAR, "(j) jump into mid-instruction -> R-0704");
    CHECK(load_scenario(SC_STACK_DIVERGE, 0, 0, 0) == R_IFBC_STACK, "(k) stack height divergence at merge -> R-0705");
}

/* ===== (l) infy_ifbc_dump twice on the same image is byte-identical ===== */
static void test_dump_determinism(void) {
    /* build a valid image once, keep the bytes */
    Buf img; bu_init(&img); build_img(SC_VALID, &img);
    FILE* f1 = mem_stream();
    FILE* f2 = mem_stream();
    infy_ifbc_dump(img.b, img.n, f1);
    infy_ifbc_dump(img.b, img.n, f2);
    long n1 = 0, n2 = 0;
    char* a = slurp(f1, &n1); char* b = slurp(f2, &n2);
    CHECK(n1 > 40, "(l) dump produced output");
    CHECK(n1 == n2 && memcmp(a, b, (size_t)n1) == 0, "(l) dump is byte-stable");
    CHECK(strstr(a, "add.i64") != 0, "(l) dump disassembles add.i64");
    CHECK(strstr(a, "func_count") != 0, "(l) dump prints header");
    free(a); free(b); fclose(f1); fclose(f2); bu_free(&img);
}

/* ===== W-0705 line-map gap is non-blocking ===== */
static void test_linemap_warning(void) {
    FILE* r = mem_stream();
    int rc = load_scenario(SC_NO_MAP, 0, r, 1);
    CHECK(rc == 0, "(W) missing line map still loads (non-blocking)");
    long n = 0; char* txt = slurp(r, &n);
    CHECK(strstr(txt, "W-0705") != 0, "(W) report carries W-0705 notice");
    free(txt); fclose(r);
}

/* ===== executed paths: closure + call.stat, exception dispatch ===== */
static void test_exec_paths(void) {
    InfyIfbcModule* m = 0;
    uint64_t base = infy_stub_live_objs();
    int rc = load_scenario(SC_CLOSURE_OK, &m, 0, 0);
    CHECK(rc == 0 && m != 0, "(x) closure module loads");
    if (m) {
        InfyDyn* ret = 0;
        int irc = infy_ifbc_invoke(m, 0, 0, 0, &ret, 0, infy_current_node_loc());
        CHECK(irc == 0 && ret && ret->tag == DY_INT && ret->v.i == 21, "(x) call.stat executes, returns 21");
        if (ret) infy_dyn_decr(ret);
        infy_ifbc_module_free(m);
    }
    m = 0;
    rc = load_scenario(SC_THROW_OK, &m, 0, 0);
    CHECK(rc == 0 && m != 0, "(x) throw/handler module loads");
    if (m) {
        InfyDyn* ret = 0;
        int irc = infy_ifbc_invoke(m, 0, 0, 0, &ret, 0, infy_current_node_loc());
        CHECK(irc == 0 && ret && ret->tag == DY_INT && ret->v.i == 42, "(x) throw.inf dispatches to handler -> 42");
        if (ret) infy_dyn_decr(ret);
        infy_ifbc_module_free(m);
    }
    m = 0;
    rc = load_scenario(SC_SWITCH_OK, &m, 0, 0);
    CHECK(rc == 0 && m != 0, "(x) switch (inline case-table island) loads");
    if (m) {
        InfyDyn* ret = 0;
        int irc = infy_ifbc_invoke(m, 0, 0, 0, &ret, 0, infy_current_node_loc());
        CHECK(irc == 0 && ret && ret->tag == DY_INT && ret->v.i == 42, "(x) switch executes case 0 -> 42");
        if (ret) infy_dyn_decr(ret);
        infy_ifbc_module_free(m);
    }
    CHECK(infy_stub_live_objs() == base, "(x) exec paths leak nothing");
}

/* ===== (m) object model: list/dict/tuple/str, refcounts reach zero, no leaks ===== */
extern InfyDyn* infy_dyn_list_new(void);
extern InfyDyn* infy_dyn_tuple_new(void);
extern int      infy_dyn_seq_push(InfyDyn*, InfyDyn*);
extern InfyDyn* infy_dyn_seq_get(const InfyDyn*, uint64_t);
extern int      infy_dyn_seq_set(InfyDyn*, uint64_t, InfyDyn*);
extern uint64_t infy_dyn_len(const InfyDyn*);
extern InfyDyn* infy_dyn_dict_new(void);
extern int      infy_dyn_dict_set(InfyDyn*, InfyDyn*, InfyDyn*);
extern InfyDyn* infy_dyn_dict_get(InfyDyn*, InfyDyn*);
extern int      infy_dyn_eq(const InfyDyn*, const InfyDyn*);
extern InfyDyn* infy_dyn_bytes_new(const uint8_t*, uint64_t);

static void test_object_model(void) {
    uint64_t base = infy_stub_live_objs();

    /* list */
    InfyDyn* l = infy_dyn_list_new();
    infy_dyn_seq_push(l, infy_dyn_int(10));
    infy_dyn_seq_push(l, infy_dyn_int(20));
    infy_dyn_seq_push(l, infy_dyn_int(30));
    CHECK(infy_dyn_len(l) == 3, "(m) list length 3");
    InfyDyn* e1 = infy_dyn_seq_get(l, 1);
    CHECK(e1 && e1->tag == DY_INT && e1->v.i == 20, "(m) list get index1 == 20");
    infy_dyn_seq_set(l, 0, infy_dyn_int(99));
    CHECK(infy_dyn_seq_get(l, 0)->v.i == 99, "(m) list set index0 == 99");
    CHECK(infy_dyn_seq_get(l, 99) == 0, "(m) list get out-of-range == NULL");

    /* tuple */
    InfyDyn* t = infy_dyn_tuple_new();
    infy_dyn_seq_push(t, infy_dyn_int(1));
    infy_dyn_seq_push(t, infy_dyn_bool(1));
    CHECK(infy_dyn_len(t) == 2 && t->tag == DY_TUPLE, "(m) tuple length 2");

    /* dict insertion order */
    InfyDyn* d = infy_dyn_dict_new();
    const char* kbs = "b"; const char* kas = "a"; const char* kcs = "c";
    infy_dyn_dict_set(d, infy_dyn_str((InfyStr){(const uint8_t*)kbs,1}), infy_dyn_int(2));
    infy_dyn_dict_set(d, infy_dyn_str((InfyStr){(const uint8_t*)kas,1}), infy_dyn_int(1));
    infy_dyn_dict_set(d, infy_dyn_str((InfyStr){(const uint8_t*)kcs,1}), infy_dyn_int(3));
    infy_dyn_dict_set(d, infy_dyn_str((InfyStr){(const uint8_t*)kas,1}), infy_dyn_int(9)); /* replace a */
    CHECK(infy_dyn_len(d) == 3, "(m) dict keeps 3 keys after replace");
    InfyDyn* ka = infy_dyn_str((InfyStr){(const uint8_t*)kas,1});
    InfyDyn* va = infy_dyn_dict_get(d, ka);
    CHECK(va && va->v.i == 9, "(m) dict get a == 9 (replaced)");
    infy_dyn_decr(ka);
    /* insertion order: first key is 'b' */
    CHECK(d->v.dict.keys[0]->tag == DY_STR && d->v.dict.keys[0]->v.s.p[0] == 'b', "(m) dict insertion order: 'b' first");

    /* str equality */
    InfyDyn* s1 = infy_dyn_str((InfyStr){(const uint8_t*)"hi",2});
    InfyDyn* s2 = infy_dyn_str((InfyStr){(const uint8_t*)"hi",2});
    CHECK(infy_dyn_eq(s1, s2), "(m) str equality");

    /* predicates */
    int ok = 0;
    InfyDyn* probe = infy_dyn_int(5);
    CHECK(infy_dyn_is(probe, DY_INT) == 1, "(m) infy_dyn_is true for matching tag");
    CHECK(infy_dyn_is(probe, DY_STR) == 0, "(m) infy_dyn_is false for mismatched tag");
    infy_dyn_decr(probe);

    int64_t v = infy_dyn_as_i64(s1, &ok);
    CHECK(ok == 0 && v == 0, "(m) as_i64 on str -> not ok");
    int ok2 = 0;
    int64_t v2 = infy_dyn_as_i64(infy_dyn_seq_get(l, 0), &ok2);
    CHECK(ok2 == 1 && v2 == 99, "(m) as_i64 on int -> ok, value 99");

    /* to_json */
    InfyString js = infy_string_new();
    infy_dyn_to_json(infy_dyn_seq_get(l, 0), &js); /* "99" */
    InfyStr jr = infy_string_as_str(&js);
    CHECK(jr.len == 2 && jr.ptr[0] == '9' && jr.ptr[1] == '9', "(m) to_json int 99");
    infy_string_drop(&js);
    js = infy_string_new();
    infy_dyn_to_json(l, &js);
    jr = infy_string_as_str(&js);
    CHECK(jr.len > 3 && jr.ptr[0] == '[' && strstr_prefix((const char*)jr.ptr, jr.len, "[99,20,30]"), "(m) to_json list");
    infy_string_drop(&js);
    js = infy_string_new();
    infy_dyn_to_json(d, &js);
    jr = infy_string_as_str(&js);
    CHECK(strstr_prefix((const char*)jr.ptr, jr.len, "{\"b\":2,\"a\":9,\"c\":3}"), "(m) to_json dict insertion order");
    infy_string_drop(&js);

    /* release everything; refcounts must reach zero with no leak */
    infy_dyn_decr(l); infy_dyn_decr(t); infy_dyn_decr(d); infy_dyn_decr(s1); infy_dyn_decr(s2);
    CHECK(infy_stub_live_objs() == base, "(m) list/dict/tuple/str all released to zero");
    /* we did not decr the box created inside the is() CHECK; not tracked, so just assert no growth trend */
    /* Now a clean refcount-to-zero check: */
    uint64_t b2 = infy_stub_live_objs();
    InfyDyn* nest = infy_dyn_list_new();
    InfyDyn* sub = infy_dyn_dict_new();
    infy_dyn_dict_set(sub, infy_dyn_str((InfyStr){(const uint8_t*)"x",1}), infy_dyn_int(7));
    infy_dyn_seq_push(nest, sub);          /* nest owns sub */
    infy_dyn_seq_push(nest, infy_dyn_bytes_new((const uint8_t*)"abc", 3));
    infy_dyn_incr(nest);                    /* rc 2 */
    infy_dyn_decr(nest);                    /* rc 1, nothing freed */
    infy_dyn_decr(nest);                    /* rc 0, frees nest->sub->(str,val), and bytes */
    CHECK(infy_stub_live_objs() == b2, "(m) nested list->dict refcount reaches 0, no leak");
}

/* helper: prefix match (defined before use in file order) */
static int strstr_prefix(const char* hay, size_t n, const char* pre) {
    size_t pl = strlen(pre);
    if (pl > n) return 0;
    return memcmp(hay, pre, pl) == 0;
}

/* ===== hostile-input fuzz: the loader must survive arbitrary bytes without OOB ===== */
static uint64_t lcg(uint64_t* st) { *st = (*st) * 6364136223846793005ull + 1442695040888963407ull; return *st >> 33; }

/* deterministic truncation sweep: every prefix of a valid image must return a code, never crash,
 * and never leave a partially-initialised module (the boundary every bounds-check protects). */
static void test_truncation_sweep(void) {
    Buf img; bu_init(&img); build_img(SC_VALID, &img);
    uint64_t base = infy_stub_live_objs();
    int clean = 1;
    for (size_t k = 0; k <= img.n; k++) {
        InfyIfbcModule* mod = (InfyIfbcModule*)0xdead;
        int rc = infy_ifbc_load(img.b, k, &mod, 0, 0);
        if (k == img.n) { if (!(rc == 0 && mod)) clean = 0; }        /* full image must load */
        else { if (rc == 0 || mod != 0) clean = 0; }                 /* truncated image must reject + NULL handle */
        if (mod) { infy_ifbc_module_free(mod); mod = 0; }            /* free exactly once */
        FILE* dev = mem_stream();
        infy_ifbc_dump(img.b, k, dev);                               /* dump must not crash on any prefix */
        fclose(dev);
    }
    bu_free(&img);
    CHECK(clean, "(trunc) every prefix of a valid image is safely rejected");
    CHECK(infy_stub_live_objs() == base, "(trunc) truncation sweep leaks nothing");
}

/* deterministic single-byte mutation sweep: flip one byte at a time in a valid image and ensure
 * load either succeeds or returns a specific validation code, never a crash, never a partial. */
static void test_byte_mutation_sweep(void) {
    Buf img; bu_init(&img); build_img(SC_VALID, &img);
    uint64_t base = infy_stub_live_objs();
    int clean = 1;
    for (size_t i = 0; i < img.n; i++) {
        for (int b = 0; b < 8; b++) {
            Buf c = img;                                /* shallow copy of ptr/len, we mutate a byte */
            c.b = (uint8_t*)malloc(img.n);
            memcpy(c.b, img.b, img.n);
            c.b[i] ^= (uint8_t)(1u << b);
            InfyIfbcModule* mod = (InfyIfbcModule*)0xdead;
            int rc = infy_ifbc_load(c.b, c.n, &mod, 1, 0); /* all-errors too */
            if (rc && mod) clean = 0;                    /* rejected but returned a handle = partial */
            if (mod) infy_ifbc_module_free(mod);
            free(c.b);
        }
    }
    bu_free(&img);
    CHECK(clean, "(mut) single-byte mutations never crash or leave a partial module");
    CHECK(infy_stub_live_objs() == base, "(mut) mutation sweep leaks nothing");
}

static void test_hostile(void) {
    uint64_t base = infy_stub_live_objs();
    uint64_t st = 0x12345678;
    int survived = 1;
    for (int it = 0; it < 800; it++) {
        size_t n = (size_t)(lcg(&st) % 200);
        uint8_t* buf = (uint8_t*)malloc(n ? n : 1);
        /* seed with valid magic sometimes, then corrupt */
        Buf img; bu_init(&img); build_img(SC_VALID, &img);
        size_t m = img.n < n ? img.n : n;
        memcpy(buf, img.b, m);
        for (size_t k = m; k < n; k++) buf[k] = (uint8_t)(lcg(&st) & 0xFF);
        for (size_t k = 0; k < n; k++) if ((lcg(&st) & 7) == 0) buf[k] = (uint8_t)(lcg(&st) & 0xFF);
        bu_free(&img);
        InfyIfbcModule* mod = (InfyIfbcModule*)0xdead;
        int rc = infy_ifbc_load(buf, n, &mod, 0, 0);
        survived = survived && (rc == 0 ? mod != 0 : mod == 0); /* success->handle, failure->NULL */
        if (mod) infy_ifbc_module_free(mod);
        free(buf);
        /* dump should also survive */
        size_t nn = (size_t)(lcg(&st) % 200); uint8_t* b2 = (uint8_t*)malloc(nn ? nn : 1);
        for (size_t k = 0; k < nn; k++) b2[k] = (uint8_t)(lcg(&st) & 0xFF);
        FILE* dev = mem_stream();
        infy_ifbc_dump(b2, nn, dev);
        fclose(dev); free(b2);
    }
    CHECK(survived, "(fuzz) arbitrary bytes never corrupt memory / always return a code or handle");
    CHECK(infy_stub_live_objs() == base, "(fuzz) no leak across hostile loads");
}

int main(void) {
    test_valid_add();
    test_six_validations();
    test_ldnull_anywhere();
    test_yield_frame();
    test_tickloop_timeout();
    test_unbox_checkpoint();
    test_invoke_target();
    test_field_off();
    test_jump_and_height();
    test_dump_determinism();
    test_linemap_warning();
    test_exec_paths();
    test_object_model();
    test_truncation_sweep();
    test_byte_mutation_sweep();
    test_hostile();

    printf("IFBC harness: %d asserts, %d pass, %d FAIL\n", g_asserts, g_pass, g_fail);
    return g_fail ? 1 : 0;
}
