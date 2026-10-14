/* infy_ifbc.c — INFY v2.0 IFBC loader, verifier, interpreter and dumper (IFABI-2).
 *
 * Implements the "IFBC virtual machine" block of runtime/infy_rt.h:
 *   infy_ifbc_load  — parse the module image and run §7.8.3's six load-time validations.
 *   infy_ifbc_invoke — execute a validated function (operand stack + local window, D.1).
 *   infy_ifbc_dump   — byte-stable text disassembly (`infy ifbc dump`).
 * The dynamic value box operations live in infy_dyn.c.
 *
 * HARD RULES honoured here:
 *   UB-019: no instruction may execute before load passes every validation; a failed
 *           validation rejects the WHOLE module and frees everything allocated.
 *   UB-020: every dyn operation needs a load-time tag checkpoint (adjacency / precedence).
 *   Big-endian everywhere via explicit shifts — the host byte order is never used.
 *   A malformed image is HOSTILE INPUT: every table index and every read is bounds-checked;
 *   the parser can be fed arbitrary bytes and must only return a code, never corrupt memory.
 *
 * See runtime/IFBC.md for the byte-level encoding this file implements (the section,
 * header, line_map and exception_table layouts and the full opcode table are fixed here).
 *
 * CAPABILITY LIMITS (honest, also in IFBC.md):
 *   - ints are fixed 64-bit (no bignum; overflow -> R-0001). See infy_dyn.c.
 *   - D.3's family bytes carry their width in operand 0 (`types[ty].kind`). The merged kind space
 *     makes every integer kind *loadable* (that is what src/ifbc.rs accepts: any numeric kind, plus
 *     `bool` for `not`), but this VM has exactly one int path (i64) and one float path (f64), so a
 *     family instruction whose row kind is neither is refused at execution with R-0706 — the same
 *     limit the old per-(op,type) byte table expressed by simply not assigning those bytes
 *     (deviation D-38 (b): "能力限制原样迁移").
 *   - the object model has no attribute map, no iterator and no set carrier, so `import.attr`,
 *     `inst.get`/`inst.set`, `seq.iter`/`seq.next` and `new.set` decode, load-verify and
 *     disassemble but are refused at execution with R-0710 (deviation D-41, BACKLOG F27).
 *   - coroutines execute single-shot: `coro.resume` runs a generator body to its first
 *     `yield` (or to `frame.exit`) and returns that value; suspend/resume state is NOT
 *     persisted across separate resume calls (no heap-wound continuations). frame.enter /
 *     frame.exit are recognised markers so the R-0707 load check is exact.
 *   - `tailcall.stat` is validated for frame-size legality (R-0709) but executed as an
 *     ordinary call (no native tail-frame reuse), so deep tail-recursive programs are bounded
 *     by the interpreter's call-depth cap (R-1040) rather than running in constant stack.
 *   - exception dispatch uses the module exception_table (region start<=pc<end -> handler);
 *     `finally` depth bookkeeping is limited to the u16 `depth` field; uncaught throw.inf
 *     unwinds all interpreter frames and returns its kind code (no process-visible panic).
 *   - `call.virt` cannot be stack-height analyzed (target unknown statically); the verifier
 *     treats its operand-stack delta as 0 and does not prove a height at a `call.virt` site.
 */
#include "infy_rt.h"

#include <stdlib.h>
#include <string.h>

/* --------------------------------------------------------------------------- code notes
 * R-0701 and R-0713 live in the frozen InfyRCode enum (they were file-local raw integers until
 * the B3 adjudication of deviation D-22). The only code carried locally here is the W-band value
 * below: findings store W and R in one numeric field and disambiguate with the `warn` flag. */
#define IFBC_W_LINEMAP     0x0705  /* W-0705 line-map gap (non-blocking notice)  */

/* =========================================================================== byte order */
static uint16_t be16(const uint8_t* p) { return (uint16_t)((p[0] << 8) | p[1]); }
static uint32_t be32(const uint8_t* p) { return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | (uint32_t)p[3]; }
static uint64_t be64(const uint8_t* p) { return ((uint64_t)be32(p) << 32) | (uint64_t)be32(p + 4); }

/* =========================================================================== sig codes */
/* Closed IFABI scalar-signature domain for import signatures (validation 5). Any value
 * outside [0, SIG_MAX] means the declared host signature is not expressible in IFABI -> R-0708.
 * Since D-38 this enum is ALSO the type_table `kind` space for rows 0..17: the builder seeds the
 * first 18 rows with kind == index, so a `u8` signature byte and a `u32` type index name one
 * space (docs/internals/ifbc-encoding-rust.md 1.3). */
enum { SIG_VOID = 0, SIG_UNIT, SIG_BOOL, SIG_CHAR, SIG_I8, SIG_I16, SIG_I32, SIG_I64,
       SIG_U8, SIG_U16, SIG_U32, SIG_U64, SIG_F32, SIG_F64, SIG_STR, SIG_BYTES, SIG_PTR,
       SIG_DYN, SIG_MAX = SIG_DYN };

/* kind values outside the closed signature domain — legal in `type_table`, never in a signature
 * byte (R-0708 there). D-38's extended static classes. */
enum { K_LIST = 18, K_TUPLE, K_DICT, K_FUNC, K_CORO, K_INSTANCE, K_REF, K_BOX, K_RC, K_ARC,
       K_OPTION, K_RESULT, K_NEVER, K_USIZE = 31, K_ISIZE = 32, K_MAX = K_ISIZE };

/* import kinds */
enum { IMP_HOST = 0, IMP_EMBED = 1, IMP_EVENT = 2 };

/* The `InfyDynTag` a kind is visible as in the dyn model (§7.7.7) — the mirror of
 * `ifbc::TyKind::dyn_tag`, which stopped being a numeric identity when D-38 renumbered the kinds.
 * Returns 0 for a kind the dyn model cannot show (then `is.dyn`/`cast.dyn` are R-0710). */
static int kind_to_dyn_tag(uint32_t kind, InfyDynTag* out) {
    switch (kind) {
        case SIG_UNIT:  *out = DY_NONE;     return 1;
        case SIG_I64:   *out = DY_INT;      return 1;
        case SIG_F64:   *out = DY_FLOAT;    return 1;
        case SIG_STR:   *out = DY_STR;      return 1;
        case SIG_BYTES: *out = DY_BYTES;    return 1;
        case K_LIST:    *out = DY_LIST;     return 1;
        case K_TUPLE:   *out = DY_TUPLE;    return 1;
        case K_DICT:    *out = DY_DICT;     return 1;
        case SIG_BOOL:  *out = DY_BOOL;     return 1;
        case K_FUNC:    *out = DY_FUNC;     return 1;
        case K_CORO:    *out = DY_CORO;     return 1;
        case SIG_DYN:   *out = DY_INSTANCE; return 1;
        default: return 0;
    }
}

/* D.3's numeric kinds — the set `src/ifbc.rs::TyKind::is_numeric` allows as a family suffix
 * (`bool` additionally for `not`). Load-time only: see fam_path() for what executes. */
static int kind_is_numeric(uint32_t kind) {
    switch (kind) {
        case SIG_I8: case SIG_I16: case SIG_I32: case SIG_I64:
        case SIG_U8: case SIG_U16: case SIG_U32: case SIG_U64:
        case SIG_F32: case SIG_F64: case K_USIZE: case K_ISIZE: return 1;
        default: return 0;
    }
}

/* =========================================================================== opcodes */
/* D.2 constants/locals 0x00-0x1F. dup/dup2/swap share one spec row; we fix
 * dup=0x06, dup2=0x0A, swap=0x0B (0x07 pop, 0x08 ldnull, 0x09 ld.frame are pinned).
 * D-38 (c): guard.* live in D.3's own block at 0x27/0x2F with NO operand; the 0x0C/0x0D bytes this
 * loader used for them are free again, and the global-window pair takes 0x0E/0x0F (D-38 (i)). */
enum {
    OP_NOP = 0x00, OP_LDL = 0x01, OP_STL = 0x02, OP_LDW = 0x03, OP_PUSH = 0x04,
    OP_PUSH_B = 0x05, OP_DUP = 0x06, OP_POP = 0x07, OP_LDNULL = 0x08, OP_LDFRAME = 0x09,
    OP_DUP2 = 0x0A, OP_SWAP = 0x0B, OP_LDG = 0x0E, OP_STG = 0x0F,
    /* D.3 arithmetic/comparison 0x20-0x36: one byte per FAMILY, operand 0 = type_table index
     * (D-38 (b) / DEV-IFBC-1). The exec path is chosen from `types[operand0].kind`. */
    OP_ADD = 0x20, OP_SUB = 0x21, OP_MUL = 0x22, OP_DIV = 0x23, OP_REM = 0x24,
    OP_NEG = 0x25, OP_ABS = 0x26, OP_GUARD_NZ = 0x27, OP_AND = 0x28, OP_OR = 0x29,
    OP_XOR = 0x2A, OP_NOT = 0x2B, OP_SHL = 0x2C, OP_SHR = 0x2D, OP_SHRA = 0x2E,
    OP_GUARD_SH = 0x2F, OP_CMPLT = 0x30, OP_CMPLE = 0x31, OP_CMPGT = 0x32, OP_CMPGE = 0x33,
    OP_CMPEQ = 0x34, OP_CMPNE = 0x35, OP_CMP = 0x36,
    /* D.4 control 0x40-0x5F */
    OP_JMP = 0x40, OP_BR = 0x41, OP_BRF = 0x42, OP_SWITCH = 0x43, OP_TICKLOOP = 0x44,
    OP_LEAVE = 0x45, OP_THROW_INF = 0x46, OP_ENTER_TRY = 0x47,
    /* D.5 calls 0x60-0x7F (0x68/0x69 = the import pair, D-38 (i) / DEV-IFBC-6) */
    OP_CALL_STAT = 0x60, OP_CALL_VIRT = 0x61, OP_CALL_HOST = 0x62, OP_CALL_EMBED = 0x63,
    OP_RET = 0x64, OP_RET_MULTI = 0x65, OP_CLOSURE_NEW = 0x66, OP_TAILCALL_STAT = 0x67,
    OP_IMPORT = 0x68, OP_IMPORT_ATTR = 0x69,
    /* D.6 objects/memory 0x80-0x9F (0x84..0x87, 0x8B..0x8F, 0x92/0x93 = the gap ops) */
    OP_NEW_STR = 0x80, OP_NEW_LIST = 0x81, OP_NEW_DICT = 0x82, OP_NEW_FRAME = 0x83,
    OP_SEQ_PUSH = 0x84, OP_SEQ_LEN = 0x85, OP_SEQ_NEXT = 0x86, OP_SEQ_ITER = 0x87,
    OP_IDX_GET = 0x88, OP_IDX_SET = 0x89, OP_SLICE = 0x8A,
    OP_INST_GET = 0x8B, OP_INST_SET = 0x8C, OP_MAP_INSERT = 0x8D, OP_STR_CONCAT = 0x8E,
    OP_SEQ_CONCAT = 0x8F,
    OP_REF_INC = 0x90, OP_REF_DEC = 0x91, OP_NEW_TUPLE = 0x92, OP_NEW_SET = 0x93,
    OP_MOVE_OUT = 0x94,
    /* D.7 dynamic values 0xA0-0xBF (0xA5/0xA6 = truthiness and identity) */
    OP_TAG_GET = 0xA0, OP_CAST_DYN = 0xA1, OP_IS_DYN = 0xA2, OP_BOX_DYN = 0xA3,
    OP_UNBOX_DYN = 0xA4, OP_DYN_TRUTHY = 0xA5, OP_DYN_IDENTITY = 0xA6,
    OP_INVOKE_DYNAMIC = 0xA8,
    /* D.8 coroutines 0xC0-0xDF */
    OP_FRAME_ENTER = 0xC0, OP_YIELD = 0xC1, OP_YIELD_FROM = 0xC2, OP_FRAME_EXIT = 0xC3,
    OP_CORO_RESUME = 0xC4, OP_CORO_THROW = 0xC5,
    /* D.9 type mirror 0xE0-0xEF */
    OP_TYPEOF = 0xE0, OP_SIZE_OF = 0xE1, OP_ALIGN_OF = 0xE2, OP_FIELD_OFF = 0xE3
};

/* operand count (fixed, u32) — the one variable-length mnemonic is switch (trailing table). */
#define OPS_MAX 4
/* D-40 choice 5: the loader's hostile cap on a switch table, and src/ifbc.rs::MAX_SWITCH_CASES is
 * the same number. It was OPS_MAX*16 = 64 here while the Rust side emitted at most 48, so the two
 * sides could disagree about whether a stream is linearly scannable at all. */
#define MAX_SWITCH_CASES 48u
/* 0xFFFFFFFF is `ifbc::NONE32`: an operand that names no table row at all. The Rust verifier's
 * range checks skip it (`decode`'s `in_bounds`), so the loader must skip it too — everywhere the
 * instruction still needs a row to run, the per-op check below rejects it with its own code. */
#define NONE32 0xFFFFFFFFu
#define OUT_OF_RANGE(v, len) ((v) != NONE32 && (v) >= (len))   /* true == index not a row */
typedef struct { const char* name; uint8_t nops; int8_t delta; uint8_t special; } OpInfo;
enum { SP_NONE = 0, SP_CALL_STAT, SP_CALL_HOST, SP_RET_MULTI, SP_CLOSURE, SP_TAILCALL,
       SP_INVOKE_DYN, SP_SWITCH, SP_TERMINATOR,
       /* D-38 addendum / D-40: the container constructors pop an operand-named count. */
       SP_NEW_LIST, SP_NEW_DICT, SP_NEW_COUNT };

static const OpInfo OPS[256] = {
    [OP_NOP]        = { "nop",         0,  0, SP_NONE },
    [OP_LDL]        = { "ldl",         1, +1, SP_NONE },
    [OP_STL]        = { "stl",         1, -1, SP_NONE },
    [OP_LDW]        = { "ldw",         2, +1, SP_NONE },   /* ldw i,ty — operand 1 is an annotation */
    [OP_PUSH]       = { "push",        2, +1, SP_NONE },   /* push k,ty (D-38 (d)) */
    [OP_PUSH_B]     = { "push.b",      1, +1, SP_NONE },
    [OP_DUP]        = { "dup",         0, +1, SP_NONE },
    [OP_POP]        = { "pop",         0, -1, SP_NONE },
    [OP_LDNULL]     = { "ldnull",      0, +1, SP_NONE },   /* rejected: R-0706 */
    [OP_LDFRAME]    = { "ld.frame",    0, +1, SP_NONE },
    [OP_DUP2]       = { "dup2",        0, +2, SP_NONE },
    [OP_SWAP]       = { "swap",        0,  0, SP_NONE },
    [OP_LDG]        = { "ldg",         1, +1, SP_NONE },
    [OP_STG]        = { "stg",         1, -1, SP_NONE },

    /* D.3 families: the name is the family; the dumper appends ".<kind suffix>" from operand 0,
     * exactly as src/ifbc.rs::Instr::render does (add.i64). */
    [OP_ADD]        = { "add",         1, -1, SP_NONE },
    [OP_SUB]        = { "sub",         1, -1, SP_NONE },
    [OP_MUL]        = { "mul",         1, -1, SP_NONE },
    [OP_DIV]        = { "div",         1, -1, SP_NONE },
    [OP_REM]        = { "rem",         1, -1, SP_NONE },
    [OP_NEG]        = { "neg",         1,  0, SP_NONE },
    [OP_ABS]        = { "abs",         1,  0, SP_NONE },
    [OP_GUARD_NZ]   = { "guard.nonzero",0, 0, SP_NONE },   /* D-38 (c): no operand */
    [OP_AND]        = { "and",         1, -1, SP_NONE },
    [OP_OR]         = { "or",          1, -1, SP_NONE },
    [OP_XOR]        = { "xor",         1, -1, SP_NONE },
    [OP_NOT]        = { "not",         1,  0, SP_NONE },
    [OP_SHL]        = { "shl",         1, -1, SP_NONE },
    [OP_SHR]        = { "shr",         1, -1, SP_NONE },
    [OP_SHRA]       = { "shr.a",       1, -1, SP_NONE },
    [OP_GUARD_SH]   = { "guard.shift", 0,  0, SP_NONE },   /* D-38 (c): no operand */
    [OP_CMPLT]      = { "cmplt",       1, -1, SP_NONE },
    [OP_CMPLE]      = { "cmple",       1, -1, SP_NONE },
    [OP_CMPGT]      = { "cmpgt",       1, -1, SP_NONE },
    [OP_CMPGE]      = { "cmpge",       1, -1, SP_NONE },
    [OP_CMPEQ]      = { "cmpeq",       1, -1, SP_NONE },
    [OP_CMPNE]      = { "cmpne",       1, -1, SP_NONE },
    [OP_CMP]        = { "cmp",         1, -1, SP_NONE },

    [OP_JMP]        = { "jmp",         1,  0, SP_TERMINATOR },
    [OP_BR]         = { "br",          1, -1, SP_NONE },
    [OP_BRF]        = { "brf",         1, -1, SP_NONE },
    [OP_SWITCH]     = { "switch",      1, -1, SP_SWITCH },   /* + trailing n u32 */
    [OP_TICKLOOP]   = { "tickloop",    1,  0, SP_NONE },
    [OP_LEAVE]      = { "leave",       0,  0, SP_NONE },
    [OP_THROW_INF]  = { "throw.inf",   1, -1, SP_TERMINATOR },
    [OP_ENTER_TRY]  = { "enter.try",   2,  0, SP_NONE },

    [OP_CALL_STAT]  = { "call.stat",   1,  0, SP_CALL_STAT },
    [OP_CALL_VIRT]  = { "call.virt",   1,  0, SP_NONE },       /* height not provable */
    [OP_CALL_HOST]  = { "call.host",   1,  0, SP_CALL_HOST },
    [OP_CALL_EMBED] = { "call.embed",  1,  0, SP_CALL_HOST },
    [OP_RET]        = { "ret",         0, -1, SP_TERMINATOR },
    [OP_RET_MULTI]  = { "ret.multi",   1,  0, SP_RET_MULTI },  /* SP_TERMINATOR-ish */
    [OP_CLOSURE_NEW]= { "closure.new", 2,  0, SP_CLOSURE },
    [OP_TAILCALL_STAT]={ "tailcall.stat",1, 0, SP_TAILCALL },
    [OP_IMPORT]     = { "import",      1,  0, SP_NONE },       /* load-time capability marker */
    [OP_IMPORT_ATTR]= { "import.attr", 1,  0, SP_NONE },

    [OP_NEW_STR]    = { "new.str",     1, +1, SP_NONE },
    [OP_NEW_LIST]   = { "new.list",    2,  0, SP_NEW_LIST },   /* ty,count */
    [OP_NEW_DICT]   = { "new.dict",    3,  0, SP_NEW_DICT },   /* kt,vt,count */
    [OP_NEW_FRAME]  = { "new.frame",   1, +1, SP_NONE },
    [OP_SEQ_PUSH]   = { "seq.push",    1, -1, SP_NONE },
    [OP_SEQ_LEN]    = { "seq.len",     1, +1, SP_NONE },
    [OP_SEQ_NEXT]   = { "seq.next",    1, +1, SP_NONE },
    [OP_SEQ_ITER]   = { "seq.iter",    1,  0, SP_NONE },
    [OP_IDX_GET]    = { "idx.get",     1, -1, SP_NONE },
    [OP_IDX_SET]    = { "idx.set",     1, -3, SP_NONE },
    [OP_SLICE]      = { "slice",       1, -2, SP_NONE },
    [OP_INST_GET]   = { "inst.get",    1,  0, SP_NONE },
    [OP_INST_SET]   = { "inst.set",    1, -2, SP_NONE },
    [OP_MAP_INSERT] = { "map.insert",  1, -2, SP_NONE },
    [OP_STR_CONCAT] = { "str.concat",  1, -1, SP_NONE },
    [OP_SEQ_CONCAT] = { "seq.concat",  1, -1, SP_NONE },
    [OP_REF_INC]    = { "ref.inc",     0,  0, SP_NONE },
    [OP_REF_DEC]    = { "ref.dec",     0,  0, SP_NONE },
    [OP_NEW_TUPLE]  = { "new.tuple",   1,  0, SP_NEW_COUNT },
    [OP_NEW_SET]    = { "new.set",     1,  0, SP_NEW_COUNT },  /* F25: the count slot IS the type */
    [OP_MOVE_OUT]   = { "move.out",    1, +1, SP_NONE },

    [OP_TAG_GET]    = { "tag.get",     0,  0, SP_NONE },
    [OP_CAST_DYN]   = { "cast.dyn",    1,  0, SP_NONE },
    [OP_IS_DYN]     = { "is.dyn",      1,  0, SP_NONE },
    [OP_BOX_DYN]    = { "box.dyn",     0,  0, SP_NONE },
    [OP_UNBOX_DYN]  = { "unbox.dyn",   0,  0, SP_NONE },
    [OP_DYN_TRUTHY] = { "dyn.truthy",  0, +1, SP_NONE },
    [OP_DYN_IDENTITY]={ "dyn.identity",0, -1, SP_NONE },
    [OP_INVOKE_DYNAMIC]={ "invoke.dynamic",1,0, SP_INVOKE_DYN },

    [OP_FRAME_ENTER]={ "frame.enter", 0,  0, SP_NONE },
    [OP_YIELD]      = { "yield",       0,  0, SP_NONE },
    [OP_YIELD_FROM] = { "yield.from",  0,  0, SP_NONE },
    [OP_FRAME_EXIT] = { "frame.exit",  0,  0, SP_TERMINATOR },
    [OP_CORO_RESUME]={ "coro.resume", 0, -1, SP_NONE },
    [OP_CORO_THROW] = { "coro.throw",  0, -1, SP_NONE },

    [OP_TYPEOF]     = { "typeof.idx",  1, +1, SP_NONE },
    [OP_SIZE_OF]    = { "size_of.idx", 1, +1, SP_NONE },
    [OP_ALIGN_OF]   = { "align_of.idx",1, +1, SP_NONE },
    [OP_FIELD_OFF]  = { "field_off.idx",2,+1, SP_NONE }        /* ty, field */
};

/* instruction length in bytes (code-relative). Returns 0 for an unknown opcode so the linear
 * scanner can keep looking for more errors in --all-errors mode. */
static uint32_t insn_len(const uint8_t* code, uint32_t code_len, uint32_t pc) {
    if (pc >= code_len) return 0;
    uint8_t op = code[pc];
    const OpInfo* inf = &OPS[op];
    if (!inf->name) return 0;
    if (inf->special == SP_SWITCH) {
        if (pc + 1 + 4 > code_len) return 0;
        uint32_t n = be32(code + pc + 1);
        if (n > MAX_SWITCH_CASES) return 0; /* hostile: absurd count */
        uint64_t sz = 1u + 4u + 4u * n;
        if (pc + sz > code_len) return 0;
        return (uint32_t)sz;
    }
    uint64_t sz = 1u + 4u * inf->nops;
    if (pc + sz > code_len) return 0;
    return (uint32_t)sz;
}

/* =========================================================================== parsed model */
/* D-38 (a): the merged 48-byte record. `sig_str`/`sig_hash` are the signature text's pool index and
 * its digest — `infy doc` and diagnostics read them, the VM's checks here still use the fields it
 * always used. */
struct InfyIfbcFunc {
    uint32_t code_off, code_len, locals, max_stack, nargs, ret_arity;
    uint32_t ret_sig, name_str, sig_str, sig_hash, flags, frame_kind;
};
/* `kind` is the D-38 unified kind space (SIG_* for rows 0..17, K_* beyond); `size`/`align` are the
 * producer's, and the seeded scalar rows carry 0 there by design (D-40 choice 1). */
typedef struct { uint32_t name_str, kind, flags, size, align, nfields; const uint8_t* fields; } PType;
typedef struct { uint32_t name_str, sig_hash; uint8_t kind, nparams, ret_sig, flags;
                 const uint8_t* psigs; } PImport;
typedef struct { uint32_t off, line; } PLine;
typedef struct { uint32_t start, end, handler; uint32_t depth, kind; } PExc;
typedef struct { const uint8_t* p; uint32_t n; } PStr;

/* Host thunk: the runtime's dispatch target for call.host. Signatures are compared at load;
 * at execution the interpreter calls through this table and NEVER a raw address. */
typedef int (*InfyHostThunk)(InfyDyn** args, uint32_t nargs, InfyDyn** ret, const InfyNodeLoc* at);

struct InfyIfbcModule {
    uint8_t*  img;         /* owned copy of the module image */
    uint64_t  img_len;
    uint16_t  ifbc_version, ifabi, lang_tag;
    uint8_t   frame_kind;
    uint32_t  func_count;
    uint64_t  max_mem_bytes;   /* §7.9 per-unit budget -> R-0709 at run time */
    /* code is a view into img */
    const uint8_t* code; uint32_t code_len;
    /* tables */
    struct InfyIfbcFunc* funcs;
    PType*   types;   uint32_t n_types;
    PImport* imports; uint32_t n_imports;
    PLine*   lines;   uint32_t n_lines;
    PExc*    excs;    uint32_t n_excs;
    PStr*    strings; uint32_t n_strings;
    InfyHostThunk* thunks;   /* parallel to imports; NULL entry -> unmapped */
};

/* private dyn-model API (defined in infy_dyn.c) */
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
extern InfyDyn* infy_dyn_func(const struct InfyIfbcFunc*, InfyDyn**, uint32_t);
extern InfyDyn* infy_dyn_coro(uint32_t);
extern uint32_t infy_dyn_coro_func(const InfyDyn*);
extern void*    infy_dyn_coro_ctl(const InfyDyn*);
extern void     infy_dyn_coro_set_ctl(InfyDyn*, void*);
extern void (*infy_dyn_coro_release)(void*);

/* =========================================================================== parsing */
typedef struct { const uint8_t* p; uint64_t n; uint64_t c; int bad; } Rd;
static void rd_init(Rd* r, const uint8_t* p, uint64_t n) { r->p = p; r->n = n; r->c = 0; r->bad = 0; }
static int rd_u8(Rd* r, uint8_t* o) { if (r->bad || r->c + 1 > r->n) { r->bad = 1; return 0; } *o = r->p[r->c]; r->c++; return 1; }
static int rd_u16(Rd* r, uint16_t* o) { if (r->bad || r->c + 2 > r->n) { r->bad = 1; return 0; } *o = be16(r->p + r->c); r->c += 2; return 1; }
static int rd_u32(Rd* r, uint32_t* o) { if (r->bad || r->c + 4 > r->n) { r->bad = 1; return 0; } *o = be32(r->p + r->c); r->c += 4; return 1; }
static int rd_u64(Rd* r, uint64_t* o) { if (r->bad || r->c + 8 > r->n) { r->bad = 1; return 0; } *o = be64(r->p + r->c); r->c += 8; return 1; }
static int rd_bytes(Rd* r, uint64_t len, const uint8_t** o) {
    if (r->bad || r->c + len > r->n) { r->bad = 1; return 0; } *o = r->p + r->c; r->c += len; return 1;
}
static int rd_skip(Rd* r, uint64_t len) { if (r->bad || r->c + len > r->n) { r->bad = 1; return 0; } r->c += len; return 1; }

#define HDR_BYTES 47u /* 4+2+2+2+32+4+1 */

/* Fill a parsed view of the image (no copy; all pointers reference `img`). Returns 0 ok,
 * else an R-code (structural failures are the umbrella R-0703). `m` may be a full module or a
 * stack scratch used by the dumper; only the fields set here are touched. */
static int parse_sections(const uint8_t* img, uint64_t len, struct InfyIfbcModule* m) {
    Rd r; rd_init(&r, img, len);
    if (len < HDR_BYTES || memcmp(img, INFY_IFBC_MAGIC, 4) != 0) return R_IFBC_INVALID;
    r.c = 4;
    if (!rd_u16(&r, &m->ifbc_version)) return R_IFBC_INVALID;
    if (!rd_u16(&r, &m->ifabi))         return R_IFBC_INVALID;
    if (!rd_u16(&r, &m->lang_tag))      return R_IFBC_INVALID;
    if (!rd_skip(&r, 32))               return R_IFBC_INVALID;   /* module_hash */
    if (!rd_u32(&r, &m->func_count))    return R_IFBC_INVALID;
    if (!rd_u8(&r, &m->frame_kind))     return R_IFBC_INVALID;
    /* §7.8.1: the module-wide frame_kind is the value model; 0 is the only defined value. */
    if (m->frame_kind != INFY_FRAME_STACK) return R_IFBC_INVALID;
    if (m->ifabi != INFY_IFABI) return R_IFABI_MISMATCH;

    /* ---- type_table (D-38 (a): eight u32 per row, then the field triples) ---- */
    uint32_t n_types;
    if (!rd_u32(&r, &n_types) || n_types > (1u << 20)) return R_IFBC_INVALID;
    m->n_types = n_types;
    m->types = n_types ? (PType*)calloc(n_types, sizeof(PType)) : 0;
    if (n_types && !m->types) return R_IFBC_INVALID;
    for (uint32_t i = 0; i < n_types; i++) {
        PType* t = &m->types[i];
        uint32_t payload_a, payload_b;
        if (!rd_u32(&r, &t->name_str) || !rd_u32(&r, &t->kind) || !rd_u32(&r, &t->flags) ||
            !rd_u32(&r, &t->size) || !rd_u32(&r, &t->align) || !rd_u32(&r, &t->nfields) ||
            !rd_u32(&r, &payload_a) || !rd_u32(&r, &payload_b)) { return R_IFBC_INVALID; }
        /* `payload_a`/`payload_b` are the producing side's list/dict element and generic-argument
         * rows; the VM has no use for them (D-38: "C 侧忽略，非新语义"), but the kind must still be
         * one of the 33 names both sides share — src/ifbc.rs::decode_types rejects it otherwise. */
        (void)payload_a; (void)payload_b;
        if (t->kind > K_MAX) return R_IFBC_INVALID;
        if (t->nfields > (1u << 20)) return R_IFBC_INVALID;
        const uint8_t* f = 0;
        if (!rd_bytes(&r, (uint64_t)t->nfields * 12u, &f)) return R_IFBC_INVALID;
        t->fields = f;
    }

    /* ---- import_table (the C row plus D-38's trailing signature digest) ---- */
    uint32_t n_imports;
    if (!rd_u32(&r, &n_imports) || n_imports > (1u << 20)) return R_IFBC_INVALID;
    m->n_imports = n_imports;
    m->imports = n_imports ? (PImport*)calloc(n_imports, sizeof(PImport)) : 0;
    if (n_imports && !m->imports) return R_IFBC_INVALID;
    for (uint32_t i = 0; i < n_imports; i++) {
        PImport* im = &m->imports[i];
        uint8_t kind, npar, ret, flags;
        if (!rd_u32(&r, &im->name_str) || !rd_u8(&r, &kind) || !rd_u8(&r, &npar) ||
            !rd_u8(&r, &ret) || !rd_u8(&r, &flags) || !rd_u32(&r, &im->sig_hash)) return R_IFBC_INVALID;
        /* three kinds exist (host/embed/event); src/ifbc.rs::decode_imports fails on anything else. */
        if (kind > IMP_EVENT) return R_IFBC_INVALID;
        im->kind = kind; im->nparams = npar; im->ret_sig = ret; im->flags = flags;
        if (npar) { const uint8_t* ps = 0; if (!rd_bytes(&r, npar, &ps)) return R_IFBC_INVALID; im->psigs = ps; }
        else im->psigs = 0;
    }

    /* ---- func_table (48-byte records) ---- */
    uint32_t n_funcs;
    if (!rd_u32(&r, &n_funcs) || n_funcs > (1u << 20)) return R_IFBC_INVALID;
    if (n_funcs != m->func_count) return R_IFBC_INVALID;   /* header.func_count must agree */
    if (!rd_u64(&r, &m->max_mem_bytes)) return R_IFBC_INVALID;
    m->funcs = n_funcs ? (struct InfyIfbcFunc*)calloc(n_funcs, sizeof(struct InfyIfbcFunc)) : 0;
    if (n_funcs && !m->funcs) return R_IFBC_INVALID;
    for (uint32_t i = 0; i < n_funcs; i++) {
        struct InfyIfbcFunc* f = &m->funcs[i];
        if (!rd_u32(&r, &f->code_off) || !rd_u32(&r, &f->code_len) || !rd_u32(&r, &f->locals) ||
            !rd_u32(&r, &f->max_stack) || !rd_u32(&r, &f->nargs) || !rd_u32(&r, &f->ret_arity) ||
            !rd_u32(&r, &f->ret_sig) || !rd_u32(&r, &f->name_str) || !rd_u32(&r, &f->sig_str) ||
            !rd_u32(&r, &f->sig_hash) || !rd_u32(&r, &f->flags) || !rd_u32(&r, &f->frame_kind))
            return R_IFBC_INVALID;
        if (f->locals > (1u << 24) || f->max_stack > (1u << 24) || f->nargs > (1u << 20) ||
            f->ret_arity > (1u << 20)) return R_IFBC_INVALID;
        /* Appendix D.8 needs a per-function frame kind; only two values are defined (D-23). */
        if (f->frame_kind > INFY_FRAME_COROUTINE) return R_IFBC_INVALID;
    }

    /* ---- code ---- */
    if (!rd_u32(&r, &m->code_len)) return R_IFBC_INVALID;
    const uint8_t* code = 0;
    if (!rd_bytes(&r, m->code_len, &code)) return R_IFBC_INVALID;
    m->code = code;
    /* every function's code window must sit inside the code section */
    for (uint32_t i = 0; i < n_funcs; i++) {
        struct InfyIfbcFunc* f = &m->funcs[i];
        if ((uint64_t)f->code_off + f->code_len > m->code_len) return R_IFBC_INVALID;
    }

    /* ---- line_map ---- */
    uint32_t n_lines;
    if (!rd_u32(&r, &n_lines) || n_lines > (1u << 24)) return R_IFBC_INVALID;
    m->n_lines = n_lines;
    m->lines = n_lines ? (PLine*)calloc(n_lines, sizeof(PLine)) : 0;
    if (n_lines && !m->lines) return R_IFBC_INVALID;
    for (uint32_t i = 0; i < n_lines; i++) {
        if (!rd_u32(&r, &m->lines[i].off) || !rd_u32(&r, &m->lines[i].line)) return R_IFBC_INVALID;
    }

    /* ---- exception_table ---- */
    uint32_t n_excs;
    if (!rd_u32(&r, &n_excs) || n_excs > (1u << 24)) return R_IFBC_INVALID;
    m->n_excs = n_excs;
    m->excs = n_excs ? (PExc*)calloc(n_excs, sizeof(PExc)) : 0;
    if (n_excs && !m->excs) return R_IFBC_INVALID;
    for (uint32_t i = 0; i < n_excs; i++) {
        PExc* e = &m->excs[i];
        uint16_t depth, kind;
        if (!rd_u32(&r, &e->start) || !rd_u32(&r, &e->end) || !rd_u32(&r, &e->handler) ||
            !rd_u16(&r, &depth) || !rd_u16(&r, &kind)) return R_IFBC_INVALID;
        e->depth = depth; e->kind = kind;
    }

    /* ---- string_pool (last, bounded by remaining bytes) ---- */
    uint32_t n_strings;
    if (!rd_u32(&r, &n_strings) || n_strings > (1u << 24)) return R_IFBC_INVALID;
    m->n_strings = n_strings;
    m->strings = n_strings ? (PStr*)calloc(n_strings, sizeof(PStr)) : 0;
    if (n_strings && !m->strings) return R_IFBC_INVALID;
    for (uint32_t i = 0; i < n_strings; i++) {
        uint32_t sl;
        if (!rd_u32(&r, &sl) || sl > (1u << 26)) return R_IFBC_INVALID;
        const uint8_t* sp = 0;
        if (!rd_bytes(&r, sl, &sp)) return R_IFBC_INVALID;
        m->strings[i].p = sp; m->strings[i].n = sl;
    }
    if (r.bad) return R_IFBC_INVALID;
    return 0;
}

static void module_free_internals(struct InfyIfbcModule* m) {
    if (!m) return;
    free(m->funcs); free(m->types); free(m->imports);
    free(m->lines); free(m->excs); free(m->strings); free(m->thunks);
    if (m->img) infy_free(m->img);
}

/* extra exported teardown (not in the frozen header — reported as a header gap) */
void infy_ifbc_module_free(InfyIfbcModule* m) {
    if (!m) return;
    module_free_internals(m);
    free(m);
}

void infy_ifbc_bind_host(InfyIfbcModule* m, uint32_t import_idx, InfyHostThunk fn) {
    if (!m || !m->thunks || import_idx >= m->n_imports) return;
    m->thunks[import_idx] = fn;
}

/* =========================================================================== diagnostics */
/* A Finding carries a `warn` level so the line-map notice (displayed as W-0705) can share the
 * numeric code 0x0705 with the blocking error R_IFBC_STACK without first_error_code confusing
 * them: R- and W- diagnostics reuse the same 4-digit number, only the letter differs. */
typedef struct { uint16_t code; uint32_t func_idx; uint32_t off; uint8_t warn; char msg[160]; } Finding;
typedef struct { Finding* v; uint32_t n, cap; } Findings;
static void findings_push(Findings* f, uint16_t code, uint32_t fi, uint32_t off, uint8_t warn, const char* msg) {
    if (f->n == f->cap) {
        uint32_t nc = f->cap ? f->cap * 2 : 16;
        Finding* nv = (Finding*)realloc(f->v, nc * sizeof(Finding));
        if (!nv) return;
        f->v = nv; f->cap = nc;
    }
    Finding* e = &f->v[f->n++];
    e->code = code; e->func_idx = fi; e->off = off; e->warn = warn;
    strncpy(e->msg, msg, sizeof e->msg - 1);
    e->msg[sizeof e->msg - 1] = 0;
}
static void findings_add(Findings* f, uint16_t code, uint32_t fi, uint32_t off, const char* fmt, ...) {
    char tmp[160]; va_list ap; va_start(ap, fmt); vsnprintf(tmp, sizeof tmp, fmt, ap); va_end(ap);
    findings_push(f, code, fi, off, 0, tmp);
}
static void findings_warn(Findings* f, uint16_t code, uint32_t fi, uint32_t off, const char* fmt, ...) {
    char tmp[160]; va_list ap; va_start(ap, fmt); vsnprintf(tmp, sizeof tmp, fmt, ap); va_end(ap);
    findings_push(f, code, fi, off, 1, tmp);
}

/* =========================================================================== validation */

/* The set of instruction boundary offsets of one function, in decode order.
 * Returns count (0 = empty), or (uint32_t)-1 on a nonlinearity (caller maps to R-0704). */
static uint32_t* func_boundaries(const struct InfyIfbcModule* m, const struct InfyIfbcFunc* f,
                                 uint32_t* out_n, int* bad) {
    *bad = 0; *out_n = 0;
    uint32_t cap = 16, n = 0;
    uint32_t* b = (uint32_t*)malloc(cap * sizeof(uint32_t));
    if (!b) { *bad = 1; return 0; }
    uint32_t pc = f->code_off, end = f->code_off + f->code_len;
    while (pc < end) {
        if (pc >= m->code_len) { free(b); *bad = 1; return 0; }
        uint8_t op = m->code[pc];
        const OpInfo* inf = &OPS[op];
        if (!inf->name) {              /* unknown opcode: cannot size the instruction */
            free(b); *bad = 1; return 0;
        }
        uint32_t sz = insn_len(m->code, m->code_len, pc);
        if (sz == 0 || pc + sz > end) { free(b); *bad = 1; return 0; }  /* overrun the region */
        if (n == cap) { uint32_t nc = cap * 2; uint32_t* nb = realloc(b, nc * sizeof(uint32_t)); if (!nb) { free(b); *bad = 1; return 0; } b = nb; cap = nc; }
        b[n++] = pc;
        pc += sz;
    }
    *out_n = n;
    return b;
}

static int is_boundary(const uint32_t* b, uint32_t n, uint32_t off) {
    for (uint32_t i = 0; i < n; i++) if (b[i] == off) return 1;
    return 0;
}

/* read up to OPS_MAX operands of the instruction at pc (already bounds-checked by insn_len) */
static void insn_operands(const struct InfyIfbcModule* m, uint32_t pc, uint8_t op,
                          uint32_t* o /* [OPS_MAX] */) {
    const OpInfo* inf = &OPS[op];
    for (int i = 0; i < OPS_MAX; i++) o[i] = 0;
    for (uint8_t i = 0; i < inf->nops && i < OPS_MAX; i++) o[i] = be32(m->code + pc + 1 + 4 * i);
}

/* stack delta / successors for the height pass. Returns base delta for simple ops; call-* use
 * their special code and the callee arity. */
static int sim_delta(const struct InfyIfbcModule* m, uint32_t fi, uint8_t op, const uint32_t* o, int* term) {
    (void)fi;
    *term = 0;
    const OpInfo* inf = &OPS[op];
    switch (inf->special) {
        case SP_TERMINATOR: *term = 1; return inf->delta;
        case SP_CALL_STAT: {
            if (o[0] < m->func_count) { *term = 0; return (int)(1 - m->funcs[o[0]].nargs); }
            *term = 0; return 0;
        }
        case SP_CALL_HOST: {
            if (o[0] < m->n_imports) { *term = 0; return (int)(1 - m->imports[o[0]].nparams); }
            *term = 0; return 0;
        }
        case SP_INVOKE_DYN: {
            if (o[0] < m->func_count) { *term = 0; return (int)(1 - m->funcs[o[0]].nargs); }
            *term = 0; return 0;
        }
        case SP_RET_MULTI: *term = 1; return (int)(1 - (o[0] > 1024u ? 1024u : o[0]));
        case SP_CLOSURE: { uint32_t cap = o[1] > 1024u ? 1024u : o[1]; return (int)(1 - cap); }
        /* D-38 addendum / D-40: the constructors take their element count from an operand, so the
         * height pass must read the same slot the executor will (clamped: a hostile count is an
         * unreachable stack depth, which the range check below then rejects as R-0705). */
        case SP_NEW_LIST: { uint32_t c = o[1] > 1024u ? 1024u : o[1]; return (int)(1 - c); }
        case SP_NEW_DICT: { uint32_t c = o[2] > 1024u ? 1024u : o[2]; return (int)(1 - 2 * (int)c); }
        case SP_NEW_COUNT: { uint32_t c = o[0] > 1024u ? 1024u : o[0]; return (int)(1 - c); }
        case SP_TAILCALL: *term = 1; if (o[0] < m->func_count) return -(int)m->funcs[o[0]].nargs; return 0;
        case SP_SWITCH: *term = 0; return inf->delta;
        default: return inf->delta;
    }
}

/* validation 1 — linearity. Builds boundaries, then checks every control-flow target lands on
 * a boundary inside the function region. Any failure -> R-0704 (unknown opcode handled here too). */
static int check_linearity(const struct InfyIfbcModule* m, uint32_t fi, uint32_t** bounds_out,
                           uint32_t* nbounds_out, Findings* rep) {
    const struct InfyIfbcFunc* f = &m->funcs[fi];
    uint32_t nb = 0; int bad = 0;
    uint32_t* b = func_boundaries(m, f, &nb, &bad);
    uint32_t lo = f->code_off, hi = f->code_off + f->code_len;
    if (bad) {
        /* distinguish unknown opcode vs nonlinearity: unknown -> R-0706, else R-0704 */
        const uint8_t* code = m->code;
        uint32_t pc = lo;
        int found_unknown = 0;
        while (pc < hi) {
            if (pc >= m->code_len) break;
            uint8_t op = code[pc];
            if (!OPS[op].name) { found_unknown = 1; findings_add(rep, R_IFBC_TYPE, fi, pc, "unknown opcode 0x%02X", op); break; }
            uint32_t sz = insn_len(code, m->code_len, pc);
            if (sz == 0) { break; }
            pc += sz;
        }
        if (b) free(b);
        if (found_unknown) return 1;   /* type violation (R-0706) already recorded */
        findings_add(rep, R_IFBC_NONLINEAR, fi, lo, "instruction stream not linearly decodable");
        return 1;
    }
    *bounds_out = b; *nbounds_out = nb;

    /* jump targets must be boundaries within [lo,hi) */
    for (uint32_t i = 0; i < nb; i++) {
        uint32_t pc = b[i];
        uint8_t op = m->code[pc];
        uint32_t o[OPS_MAX]; insn_operands(m, pc, op, o);
        switch (op) {
            case OP_JMP: case OP_BR: case OP_BRF:
                if (!(o[0] >= lo && o[0] < hi && is_boundary(b, nb, o[0])))
                    findings_add(rep, R_IFBC_NONLINEAR, fi, pc, "jump target 0x%X is not an instruction boundary", o[0]);
                break;
            case OP_ENTER_TRY:
                /* handler_off, region_end both boundaries */
                if (!(o[0] >= lo && o[0] < hi && is_boundary(b, nb, o[0]))) findings_add(rep, R_IFBC_NONLINEAR, fi, pc, "enter.try handler 0x%X not a boundary", o[0]);
                if (!(o[1] >= lo && o[1] <= hi)) findings_add(rep, R_IFBC_NONLINEAR, fi, pc, "enter.try region_end 0x%X out of region", o[1]);
                break;
            case OP_SWITCH: {
                uint32_t n = o[0]; if (n > MAX_SWITCH_CASES) n = 0;
                uint32_t tb = pc + 5; /* after opcode + n operand */
                for (uint32_t k = 0; k < n; k++) {
                    uint32_t tgt = be32(m->code + tb + 4 * k);
                    if (!(tgt >= lo && tgt < hi && is_boundary(b, nb, tgt))) {
                        findings_add(rep, R_IFBC_NONLINEAR, fi, pc, "switch case %u target 0x%X not a boundary", k, tgt);
                        break;
                    }
                }
                break;
            }
            default: break;
        }
    }
    /* any recorded nonlinearity finding for this function is an error */
    for (uint32_t i = 0; i < rep->n; i++)
        if (rep->v[i].code == R_IFBC_NONLINEAR && rep->v[i].func_idx == fi) return 1;
    return 0;
}

static uint32_t bound_index(const uint32_t* b, uint32_t nb, uint32_t off) {
    for (uint32_t i = 0; i < nb; i++) if (b[i] == off) return i;
    return nb; /* not found */
}

/* validation 2 — stack height per basic block (fixpoint over boundaries). R-0705. */
static int check_height(const struct InfyIfbcModule* m, uint32_t fi, const uint32_t* b, uint32_t nb, Findings* rep) {
    const struct InfyIfbcFunc* f = &m->funcs[fi];
    int32_t* h = (int32_t*)malloc(nb * sizeof(int32_t));
    if (!h) return 0;
    for (uint32_t i = 0; i < nb; i++) h[i] = -1;   /* -1 == not yet reached */
    h[0] = 0;                                       /* entry: operand stack empty (args -> locals) */
    int conflict = 0;
    uint32_t pass_guard = 0;
    int changed = 1;
    while (changed && pass_guard++ < nb + 2) {
        changed = 0;
        for (uint32_t i = 0; i < nb; i++) {
            if (h[i] < 0) continue;
            uint32_t pc = b[i];
            uint8_t op = m->code[pc];
            uint32_t o[OPS_MAX]; insn_operands(m, pc, op, o);
            int term = 0;
            int delta = sim_delta(m, fi, op, o, &term);
            int nh = h[i] + delta;
            if (nh < 0 || (uint32_t)nh > f->max_stack) {
                findings_add(rep, R_IFBC_STACK, fi, pc, "operand stack height %d out of [0,%u] at %s", nh, f->max_stack, OPS[op].name);
                conflict = 1; continue;
            }
            /* fallthrough successor */
            uint32_t flen = insn_len(m->code, m->code_len, pc);
            if (!term) {
                uint32_t fi_idx = bound_index(b, nb, pc + flen);
                if (fi_idx < nb) {
                    if (h[fi_idx] < 0) { h[fi_idx] = nh; changed = 1; }
                    else if (h[fi_idx] != nh) { findings_add(rep, R_IFBC_STACK, fi, pc, "stack height %d vs %d at merge 0x%X", nh, h[fi_idx], pc + flen); conflict = 1; }
                }
            }
            /* branch successors */
            if (op == OP_JMP) {
                uint32_t t = bound_index(b, nb, o[0]);
                if (t < nb) { if (h[t] < 0) { h[t] = nh; changed = 1; } else if (h[t] != nh) { findings_add(rep, R_IFBC_STACK, fi, pc, "stack height %d vs %d at jump 0x%X", nh, h[t], o[0]); conflict = 1; } }
            } else if (op == OP_BR || op == OP_BRF) {
                uint32_t t = bound_index(b, nb, o[0]);
                if (t < nb) { if (h[t] < 0) { h[t] = nh; changed = 1; } else if (h[t] != nh) { findings_add(rep, R_IFBC_STACK, fi, pc, "stack height %d vs %d at branch 0x%X", nh, h[t], o[0]); conflict = 1; } }
                uint32_t fl = bound_index(b, nb, pc + flen);
                if (fl < nb) { if (h[fl] < 0) { h[fl] = nh; changed = 1; } else if (h[fl] != nh) { findings_add(rep, R_IFBC_STACK, fi, pc, "stack height divergence at branch fallthrough 0x%X", pc + flen); conflict = 1; } }
            } else if (op == OP_SWITCH) {
                uint32_t n = o[0]; if (n > MAX_SWITCH_CASES) n = 0;
                uint32_t tb = pc + 5;
                for (uint32_t k = 0; k < n; k++) {
                    uint32_t tgt = be32(m->code + tb + 4 * k);
                    uint32_t t = bound_index(b, nb, tgt);
                    if (t < nb) { if (h[t] < 0) { h[t] = nh; changed = 1; } else if (h[t] != nh) { findings_add(rep, R_IFBC_STACK, fi, pc, "stack height divergence at switch target 0x%X", tgt); conflict = 1; } }
                }
                uint32_t fl = bound_index(b, nb, pc + flen);
                if (fl < nb) { if (h[fl] < 0) { h[fl] = nh; changed = 1; } else if (h[fl] != nh) { findings_add(rep, R_IFBC_STACK, fi, pc, "stack height divergence at switch fallthrough"); conflict = 1; } }
            }
        }
    }
    free(h);
    /* a reported conflict for this function is an error */
    for (uint32_t i = 0; i < rep->n; i++)
        if (rep->v[i].code == R_IFBC_STACK && rep->v[i].func_idx == fi) return 1;
    return conflict;
}

/* validation 3 — operand type vs suffix (R-0706) plus the dyn / invoke / field_off checks
 * (R-0710 / R-0712 / R-0713) that appendix D folds into the type-consistency family.
 * The operand *forms* are the ones src/ifbc.rs::Op::operand_kinds declares, so the two sides reach
 * the same conclusion about the same image (D-38 (a)/(b)); NONE32 is skipped as "names no row"
 * exactly where the Rust range check skips it, and the ops that genuinely need a row to run
 * (ldw's annotation, the dyn tag, field_off) reject it with their own code. */
static int check_type(const struct InfyIfbcModule* m, uint32_t fi, const uint32_t* b, uint32_t nb, Findings* rep) {
    const struct InfyIfbcFunc* f = &m->funcs[fi];
    int saw_isdyn = 0, saw_tagget = 0, err = 0;
    /* one type_table operand, range-checked */
#define TY_IN_RANGE(slot) do {                                                              \
        if (OUT_OF_RANGE(o[slot], m->n_types)) {                                            \
            findings_add(rep, R_IFBC_TYPE, fi, pc, "%s: type index %u out of range (%u)",   \
                         OPS[op].name, o[slot], m->n_types); bad_here = 1;                  \
        }                                                                                   \
    } while (0)
    for (uint32_t i = 0; i < nb; i++) {
        uint32_t pc = b[i];
        uint8_t op = m->code[pc];
        uint32_t o[OPS_MAX]; insn_operands(m, pc, op, o);
        int bad_here = 0;
        switch (op) {
            case OP_LDNULL: findings_add(rep, R_IFBC_TYPE, fi, pc, "ldnull is forbidden (no null; use Option)"); bad_here = 1; break;
            case OP_LDL: case OP_STL: case OP_MOVE_OUT: case OP_LDG: case OP_STG:
                if (o[0] >= f->locals) { findings_add(rep, R_IFBC_TYPE, fi, pc, "local index %u >= frame.locals %u", o[0], f->locals); bad_here = 1; } break;
            case OP_LDW:
                /* D-38 (e): operand 1 is the wide-type annotation, read as a type_table row. */
                if (o[1] >= m->n_types) { findings_add(rep, R_IFBC_TYPE, fi, pc, "ldw annotation index %u out of range", o[1]); bad_here = 1; }
                else if (!(m->types[o[1]].kind == SIG_I64 || m->types[o[1]].kind == SIG_U64 ||
                           m->types[o[1]].kind == SIG_F64)) {
                    findings_add(rep, R_IFBC_TYPE, fi, pc, "ldw annotation kind %u is not i64/u64/f64", m->types[o[1]].kind); bad_here = 1;
                }
                if (o[0] >= f->locals) { findings_add(rep, R_IFBC_TYPE, fi, pc, "ldw local index out of range"); bad_here = 1; } break;
            case OP_PUSH:
                if (o[0] >= m->n_strings) { findings_add(rep, R_IFBC_TYPE, fi, pc, "string pool index %u out of range (%u)", o[0], m->n_strings); bad_here = 1; }
                TY_IN_RANGE(1);
                break;
            case OP_NEW_STR: case OP_THROW_INF: case OP_IMPORT_ATTR: case OP_INST_GET: case OP_INST_SET:
                if (o[0] >= m->n_strings) { findings_add(rep, R_IFBC_TYPE, fi, pc, "%s: string pool index %u out of range (%u)", OPS[op].name, o[0], m->n_strings); bad_here = 1; } break;
            case OP_IMPORT:
                if (OUT_OF_RANGE(o[0], m->n_imports)) { findings_add(rep, R_IFBC_TYPE, fi, pc, "import index %u out of range (%u)", o[0], m->n_imports); bad_here = 1; } break;
            case OP_NEW_FRAME:
                if (o[0] >= m->func_count) { findings_add(rep, R_IFBC_TYPE, fi, pc, "new.frame target %u not in func_table", o[0]); bad_here = 1; } break;
            case OP_ADD: case OP_SUB: case OP_MUL: case OP_DIV: case OP_REM: case OP_NEG: case OP_ABS:
            case OP_AND: case OP_OR: case OP_XOR: case OP_NOT: case OP_SHL: case OP_SHR: case OP_SHRA:
            case OP_CMPLT: case OP_CMPLE: case OP_CMPGT: case OP_CMPGE: case OP_CMPEQ: case OP_CMPNE:
            case OP_CMP:
                /* D-38 (b): operand 0 IS the suffix. Load-time accepts every kind the Rust verifier
                 * accepts (any numeric kind, plus bool for `not`); which of them this VM can execute
                 * is fam_path()'s answer at run time. */
                if (o[0] >= m->n_types) { findings_add(rep, R_IFBC_TYPE, fi, pc, "%s: type index %u out of range (%u)", OPS[op].name, o[0], m->n_types); bad_here = 1; }
                else {
                    uint32_t k = m->types[o[0]].kind;
                    if (!(kind_is_numeric(k) || (k == SIG_BOOL && op == OP_NOT)))
                        { findings_add(rep, R_IFBC_TYPE, fi, pc, "%s: kind %u is not a numeric suffix (D.3)", OPS[op].name, k); bad_here = 1; }
                }
                break;
            case OP_NEW_LIST: TY_IN_RANGE(0); break;
            case OP_NEW_DICT: TY_IN_RANGE(0); TY_IN_RANGE(1); break;
            case OP_SEQ_PUSH: case OP_SEQ_LEN: case OP_SEQ_NEXT: case OP_SEQ_ITER:
            case OP_IDX_GET: case OP_IDX_SET: case OP_SLICE:
            case OP_MAP_INSERT: case OP_STR_CONCAT: case OP_SEQ_CONCAT:
            case OP_TYPEOF: case OP_SIZE_OF: case OP_ALIGN_OF:
                TY_IN_RANGE(0); break;
            case OP_CAST_DYN: case OP_IS_DYN:
                if (o[0] >= m->n_types) { findings_add(rep, R_IFBC_TYPE, fi, pc, "type index %u out of range (%u)", o[0], m->n_types); bad_here = 1; }
                else {
                    /* D-38 renumbered the kinds, so the dyn tag is a table lookup, not an equality. */
                    InfyDynTag want;
                    if (!kind_to_dyn_tag(m->types[o[0]].kind, &want))
                        { findings_add(rep, R_DYN_TAG, fi, pc, "%s: kind %u has no dyn-visible tag (UB-020)", OPS[op].name, m->types[o[0]].kind); bad_here = 1; }
                }
                break;
            case OP_FIELD_OFF:
                if (o[0] >= m->n_types) { findings_add(rep, R_IFBC_TYPE, fi, pc, "field_off type index out of range"); bad_here = 1; }
                else if (!(m->types[o[0]].flags & 1u)) { findings_add(rep, R_FIELD_OFF_REPR, fi, pc, "field_off on a non-repr(C) type"); bad_here = 1; }
                else if (o[1] >= m->types[o[0]].nfields) { findings_add(rep, R_IFBC_TYPE, fi, pc, "field_off field index out of range"); bad_here = 1; }
                break;
            case OP_CALL_STAT: case OP_TAILCALL_STAT:
                if (o[0] >= m->func_count) { findings_add(rep, R_IFBC_FRAME, fi, pc, "call/tailcall target %u not in func_table", o[0]); bad_here = 1; } break;
            case OP_CALL_VIRT:
                if (o[0] >= m->n_types) { findings_add(rep, R_IFBC_TYPE, fi, pc, "call.virt slot type index out of range"); bad_here = 1; } break;
            default: break;
        }
        /* D.3's mandatory guards, checked in the instruction stream (UB-003/UB-004 on the bytecode
         * side). Outside the switch because these bytes are the family cases above. */
        if (op == OP_DIV || op == OP_REM) {
            if (i == 0 || m->code[b[i - 1]] != OP_GUARD_NZ) { findings_add(rep, R_IFBC_TYPE, fi, pc, "div/rem not immediately preceded by guard.nonzero"); bad_here = 1; }
        } else if (op == OP_SHL || op == OP_SHR || op == OP_SHRA) {
            if (i == 0 || m->code[b[i - 1]] != OP_GUARD_SH) { findings_add(rep, R_IFBC_TYPE, fi, pc, "shl/shr not immediately preceded by guard.shift"); bad_here = 1; }
        }
        /* dyn tag checkpoints (UB-020) */
        if (op == OP_IS_DYN) saw_isdyn = 1;
        if (op == OP_TAG_GET) saw_tagget = 1;
        if (op == OP_UNBOX_DYN || op == OP_CAST_DYN) {
            if (!(saw_isdyn || saw_tagget)) { findings_add(rep, R_DYN_TAG, fi, pc, "dyn operation (%s) has no preceding tag checkpoint", OPS[op].name); bad_here = 1; }
        }
        if (op == OP_INVOKE_DYNAMIC) {
            if (o[0] >= m->func_count) { findings_add(rep, R_DYN_TARGET, fi, pc, "invoke.dynamic target %u outside func_table", o[0]); bad_here = 1; }
        }
        if (bad_here) err = 1;
    }
#undef TY_IN_RANGE
    return err;
}

/* validation 4 — frame legality (R-0707): yield/coroutine ops only in coroutine frames;
 * return arity consistent with func_table.ret_arity. */
static int check_frame(const struct InfyIfbcModule* m, uint32_t fi, const uint32_t* b, uint32_t nb, Findings* rep) {
    const struct InfyIfbcFunc* f = &m->funcs[fi];
    int err = 0;
    int ret_arity_seen = -1;
    for (uint32_t i = 0; i < nb; i++) {
        uint32_t pc = b[i];
        uint8_t op = m->code[pc];
        uint32_t o[OPS_MAX]; insn_operands(m, pc, op, o);
        int is_coro_op = (op == OP_YIELD || op == OP_YIELD_FROM || op == OP_FRAME_ENTER || op == OP_FRAME_EXIT);
        if (is_coro_op && f->frame_kind != INFY_FRAME_COROUTINE) {
            findings_add(rep, R_IFBC_FRAME, fi, pc, "%s in a non-coroutine frame", OPS[op].name); err = 1;
        }
        if (op == OP_RET) {
            int ar = 1;
            if (f->flags & 1u) { /* returns a value */ }
            if (ret_arity_seen < 0) ret_arity_seen = ar; else if (ret_arity_seen != ar) { findings_add(rep, R_IFBC_FRAME, fi, pc, "inconsistent return arity across paths"); err = 1; }
        } else if (op == OP_RET_MULTI) {
            if (ret_arity_seen < 0) ret_arity_seen = (int)o[0]; else if (ret_arity_seen != (int)o[0]) { findings_add(rep, R_IFBC_FRAME, fi, pc, "ret.multi arity %u inconsistent with prior return", o[0]); err = 1; }
            if (o[0] != f->ret_arity) { findings_add(rep, R_IFBC_FRAME, fi, pc, "ret.multi arity %u != declared %u", o[0], f->ret_arity); err = 1; }
        }
    }
    /* declared ret_arity >1 must be satisfied by ret.multi, single by ret */
    if (f->ret_arity > 1 && ret_arity_seen == 1) { findings_add(rep, R_IFBC_FRAME, fi, f->code_off, "function declares tuple return %u but returns a single value", f->ret_arity); err = 1; }
    if (f->ret_arity == 1 && ret_arity_seen > 1) { findings_add(rep, R_IFBC_FRAME, fi, f->code_off, "function declares scalar return but returns a tuple"); err = 1; }
    return err;
}

/* validation 5 — call.host / call.embed signature vs IFABI (R-0708). */
static int check_host(const struct InfyIfbcModule* m, uint32_t fi, const uint32_t* b, uint32_t nb, Findings* rep) {
    int err = 0;
    for (uint32_t i = 0; i < nb; i++) {
        uint32_t pc = b[i];
        uint8_t op = m->code[pc];
        if (op != OP_CALL_HOST && op != OP_CALL_EMBED) continue;
        uint32_t idx = be32(m->code + pc + 1);
        if (idx >= m->n_imports) { findings_add(rep, R_IFBC_HOSTSIG, fi, pc, "call.%s index %u not in import_table", op == OP_CALL_HOST ? "host" : "embed", idx); err = 1; continue; }
        const PImport* im = &m->imports[idx];
        uint8_t want = (op == OP_CALL_HOST) ? IMP_HOST : IMP_EMBED;
        if (im->kind != want) { findings_add(rep, R_IFBC_HOSTSIG, fi, pc, "import %u kind %u != required %u", idx, im->kind, want); err = 1; }
        if (!(im->flags & 1u)) { findings_add(rep, R_IFBC_HOSTSIG, fi, pc, "import %u is not IFABI-signed", idx); err = 1; }
        if (im->ret_sig > SIG_MAX) { findings_add(rep, R_IFBC_HOSTSIG, fi, pc, "import %u return sig code %u out of IFABI domain", idx, im->ret_sig); err = 1; }
        for (uint32_t k = 0; k < im->nparams; k++) {
            if (im->psigs[k] > SIG_MAX) { findings_add(rep, R_IFBC_HOSTSIG, fi, pc, "import %u param %u sig code out of IFABI domain", idx, k); err = 1; break; }
        }
    }
    return err;
}

/* D.10's "边界" row — tail-call / closure.new frame legality. D.10 maps it to R-0709, but §9.7
 * reserves R-0709 for "embedded unit exceeded max_mem_bytes" (§7.9 says the same), so this
 * implementation reports these findings as R-0707, which §7.8.3 item 4 defines as frame
 * legality and which already covers "call/tailcall target not in func_table" above.
 * Deviation D-22 records the clash and this choice. */
static int check_boundary(const struct InfyIfbcModule* m, uint32_t fi, const uint32_t* b, uint32_t nb, Findings* rep) {
    const struct InfyIfbcFunc* f = &m->funcs[fi];
    int err = 0;
    for (uint32_t i = 0; i < nb; i++) {
        uint32_t pc = b[i];
        uint8_t op = m->code[pc];
        if (op != OP_TAILCALL_STAT) continue;
        uint32_t t = be32(m->code + pc + 1);
        if (t >= m->func_count) continue; /* already flagged by type/frame */
        const struct InfyIfbcFunc* c = &m->funcs[t];
        /* tail call can only reuse the frame when the shapes match exactly */
        if (c->locals != f->locals || c->max_stack != f->max_stack ||
            c->nargs != f->nargs || c->ret_arity != f->ret_arity || c->frame_kind != f->frame_kind) {
            findings_add(rep, R_IFBC_FRAME, fi, pc, "tailcall to f%u not frame-size compatible (locals %u/%u stack %u/%u)", t, c->locals, f->locals, c->max_stack, f->max_stack);
            err = 1;
        }
    }
    /* closure.new target must exist (boundary legality of the boxed frame) */
    for (uint32_t i = 0; i < nb; i++) {
        uint32_t pc = b[i];
        if (m->code[pc] != OP_CLOSURE_NEW) continue;
        uint32_t code_idx = be32(m->code + pc + 1);
        if (code_idx >= m->func_count) { findings_add(rep, R_IFBC_FRAME, fi, pc, "closure.new code_idx %u outside func_table", code_idx); err = 1; }
    }
    return err;
}

/* validation 6 (line-map completeness) -> W-0705, non-blocking. Every executable instruction
 * boundary must be covered by a line_map entry. */
static int check_linemap(const struct InfyIfbcModule* m, uint32_t fi, const uint32_t* b, uint32_t nb, Findings* rep) {
    if (m->n_lines == 0) { findings_warn(rep, IFBC_W_LINEMAP, fi, m->funcs[fi].code_off, "no line_map: %u instructions uncovered", nb); return 0; }
    for (uint32_t i = 0; i < nb; i++) {
        int covered = 0;
        for (uint32_t k = 0; k < m->n_lines; k++) if (m->lines[k].off == b[i]) { covered = 1; break; }
        if (!covered) { findings_warn(rep, IFBC_W_LINEMAP, fi, b[i], "instruction at 0x%X has no line_map entry", b[i]); break; /* report once per function */ }
    }
    return 0;
}

/* first code (by validation order) among findings; warnings (W-) never block. */
static uint16_t first_error_code(const Findings* rep) {
    for (uint32_t i = 0; i < rep->n; i++) {
        if (rep->v[i].warn) continue;
        return rep->v[i].code;
    }
    return 0;
}

static void write_findings(FILE* report, const Findings* rep) {
    if (!report) return;
    for (uint32_t i = 0; i < rep->n; i++) {
        const Finding* e = &rep->v[i];
        const char* lvl = e->warn ? "W" : "R";
        fprintf(report, "%s-%04X func[%u] @0x%X: %s\n", lvl, e->code, e->func_idx, e->off, e->msg);
    }
    fflush(report);
}

/* =========================================================================== load */
int infy_ifbc_load(const uint8_t* image, uint64_t len, InfyIfbcModule** out, uint8_t all_errors, FILE* report) {
    if (out) *out = 0;
    if (!image || len < HDR_BYTES) return R_IFBC_INVALID;

    struct InfyIfbcModule* m = (struct InfyIfbcModule*)calloc(1, sizeof(*m));
    if (!m) return R_IFBC_INVALID;

    /* copy the image so the module owns a stable buffer (the caller may release the source) */
    m->img = (uint8_t*)infy_alloc(len, 8);
    if (!m->img) { free(m); return R_IFBC_INVALID; }
    memcpy(m->img, image, (size_t)len);
    m->img_len = len;

    int rc = parse_sections(m->img, m->img_len, m);
    if (rc) {
        if (report) fprintf(report, "R-%04X parse: malformed IFBC container\n", rc);
        module_free_internals(m); free(m);
        return rc;
    }

    /* §7.8.3 / D.10: per function in func_table order, wholly before any execution. */
    Findings rep = {0};
    uint16_t reject = 0;
    for (uint32_t fi = 0; fi < m->func_count && !reject; fi++) {
        uint32_t* b = 0; uint32_t nb = 0; Findings local = {0};
        if (check_linearity(m, fi, &b, &nb, &local)) {
            goto collect; /* boundaries absent when nonlinearity fails; still validate the rest */
        }
        check_type(m, fi, b, nb, &local);
        check_height(m, fi, b, nb, &local);
        check_frame(m, fi, b, nb, &local);
        check_host(m, fi, b, nb, &local);
        check_boundary(m, fi, b, nb, &local);
        check_linemap(m, fi, b, nb, &local);
    collect:
        free(b);
        /* merge: append preserving warn level, and possibly decide reject */
        for (uint32_t i = 0; i < local.n; i++) findings_push(&rep, local.v[i].code, local.v[i].func_idx, local.v[i].off, local.v[i].warn, local.v[i].msg);
        free(local.v);
        uint16_t e = first_error_code(&rep);
        if (e && !all_errors) reject = e;    /* first-failure rejects the whole module */
    }

    if (all_errors && report) write_findings(report, &rep);
    else if (!all_errors && report) { /* default mode: emit only the first blocking finding */
        for (uint32_t i = 0; i < rep.n; i++) {
            if (rep.v[i].warn) continue;
            write_findings(report, &(Findings){ &rep.v[i], 1, 1 });
            break;
        }
    }

    reject = all_errors ? first_error_code(&rep) : reject;
    free(rep.v);

    if (reject) { module_free_internals(m); free(m); return (int)reject; }

    m->thunks = m->n_imports ? (InfyHostThunk*)calloc(m->n_imports, sizeof(InfyHostThunk)) : 0;
    if (m->n_imports && !m->thunks) { module_free_internals(m); free(m); return R_IFBC_INVALID; }
    if (out) *out = m; else { module_free_internals(m); free(m); }
    return 0;
}

/* =========================================================================== interpreter */
#define MAX_FRAMES 4096u
#define EXEC_STEP_CAP 100000000ull

typedef struct {
    struct InfyIfbcFunc* f;
    InfyDyn** locals; uint32_t locals_n; uint8_t* moved;   /* moved[i]: local was moved out */
    InfyDyn** stack; uint32_t sp, stack_cap;
    uint32_t pc;              /* function-relative? no: global code offset */
    uint64_t ticks;           /* tickloop budget counter */
    uint8_t in_coro;          /* running as a coroutine body */
} VmFrame;

static void frame_release(VmFrame* fr) {
    if (!fr) return;
    if (fr->locals) { for (uint32_t i = 0; i < fr->locals_n; i++) infy_dyn_decr(fr->locals[i]); free(fr->locals); }
    if (fr->moved) free(fr->moved);
    if (fr->stack) { for (uint32_t i = 0; i < fr->sp; i++) infy_dyn_decr(fr->stack[i]); free(fr->stack); }
}

static int vpush(VmFrame* fr, InfyDyn* v) {
    if (fr->sp + 1 > fr->stack_cap) {
        uint32_t nc = fr->stack_cap ? fr->stack_cap * 2 : 8;
        if (nc > fr->f->max_stack + 8) nc = fr->f->max_stack + 8;
        if (fr->sp + 1 > nc) return 0; /* overflow beyond declared frame */
        InfyDyn** ns = (InfyDyn**)realloc(fr->stack, nc * sizeof(InfyDyn*));
        if (!ns) return 0;
        fr->stack = ns; fr->stack_cap = nc;
    }
    fr->stack[fr->sp++] = v;
    return 1;
}
static InfyDyn* vpop(VmFrame* fr) {
    if (fr->sp == 0) return 0;
    return fr->stack[--fr->sp]; /* ownership transfers to the caller */
}

/* helper: read the two top int operands or fail; keeps semantics for i64 arith */
static int as_int(InfyDyn* d, int64_t* out) { if (d && (d->tag == DY_INT || d->tag == DY_BOOL)) { *out = d->v.i; return 1; } return 0; }
static int as_flt(InfyDyn* d, double* out) { if (d && d->tag == DY_FLOAT) { *out = d->v.f; return 1; } return 0; }

static int checked_add_i64(int64_t a, int64_t b, int64_t* r) {
    if (b > 0 && a > INT64_MAX - b) return 0;
    if (b < 0 && a < INT64_MIN - b) return 0;
    *r = a + b; return 1;
}
static int checked_sub_i64(int64_t a, int64_t b, int64_t* r) {
    if (b < 0 && a > INT64_MAX + b) return 0;
    if (b > 0 && a < INT64_MIN + b) return 0;
    *r = a - b; return 1;
}
static int checked_mul_i64(int64_t a, int64_t b, int64_t* r) {
    if (a == 0 || b == 0) { *r = 0; return 1; }
    if (a == -1) { if (b == INT64_MIN) return 0; *r = -b; return 1; }
    if (b == -1) { if (a == INT64_MIN) return 0; *r = -a; return 1; }
    if (a > 0) { if (b > 0 && a > INT64_MAX / b) return 0; if (b < 0 && b < INT64_MIN / a) return 0; }
    else { if (b > 0 && a < INT64_MIN / b) return 0; if (b < 0 && a != 0 && b < INT64_MIN / a) return 0; }
    *r = a * b; return 1;
}

/* D-38 (b): the family byte plus `types[operand0].kind` replace the old per-(op,type) byte table.
 * Load-time accepts every kind src/ifbc.rs accepts (any numeric kind, plus bool for `not`); this VM
 * has exactly one int path (i64) and one float path (f64), so any other kind is refused here with
 * R-0706 instead of being computed at the wrong width — that is the old table's "unsupported
 * suffix" limit, migrated, not widened (§3 of IFBC.md). The bitwise/shift families have no float
 * path at all. */
static int fam_path(const struct InfyIfbcModule* m, uint8_t op, uint32_t ty, int* is_flt) {
    *is_flt = 0;
    if (ty >= m->n_types) return R_IFBC_TYPE;
    uint32_t k = m->types[ty].kind;
    if (k == SIG_F64) {
        switch (op) {
            case OP_AND: case OP_OR: case OP_XOR: case OP_SHL: case OP_SHR: case OP_SHRA:
            case OP_NOT: return R_IFBC_TYPE;
            default: *is_flt = 1; return 0;
        }
    }
    if (k == SIG_I64) return 0;
    if (k == SIG_BOOL && op == OP_NOT) return 0;
    return R_IFBC_TYPE;
}

/* The dyn tag an `is.dyn T` / `cast.dyn T` operand names: since D-38 the operand is a type_table
 * index, not an InfyDynTag, so it goes through the kind table. Returns 0 when the row does not
 * exist or has no dyn-visible tag (already rejected at load as R-0706 / R-0710). */
static int dyn_want_tag(const struct InfyIfbcModule* m, uint32_t ty, InfyDynTag* out) {
    if (ty >= m->n_types) return 0;
    return kind_to_dyn_tag(m->types[ty].kind, out);
}

/* forward for nested execution (call.stat / coro.resume) */
static int exec(InfyIfbcModule* m, uint32_t fi, InfyDyn** args, uint32_t nargs,
                uint64_t deadline_ms, int has_deadline, uint64_t* steps,
                InfyDyn** out_ret, int* out_yielded, uint32_t depth);

/* run the whole program; on any runtime error all frames unwind (no process-side effect). */
static int exec(InfyIfbcModule* m, uint32_t fi, InfyDyn** args, uint32_t nargs,
                uint64_t deadline_ms, int has_deadline, uint64_t* steps,
                InfyDyn** out_ret, int* out_yielded, uint32_t depth) {
    if (depth > MAX_FRAMES) return R_STACK_OVERFLOW;
    struct InfyIfbcFunc* f = &m->funcs[fi];
    VmFrame fr = {0};
    fr.f = f;
    fr.pc = f->code_off;
    fr.in_coro = (f->frame_kind == INFY_FRAME_COROUTINE);
    fr.locals_n = f->locals;
    fr.locals = f->locals ? (InfyDyn**)calloc(f->locals, sizeof(InfyDyn*)) : 0;
    fr.moved = f->locals ? (uint8_t*)calloc(f->locals, 1) : 0;
    fr.stack_cap = f->max_stack ? f->max_stack : 8;
    fr.stack = (InfyDyn**)calloc(fr.stack_cap, sizeof(InfyDyn*));
    if ((f->locals && !fr.locals) || (f->locals && !fr.moved) || !fr.stack) { frame_release(&fr); return R_IFBC_INVALID; }
    for (uint32_t i = 0; i < nargs && i < f->locals; i++) fr.locals[i] = infy_dyn_incr(args[i]);

    uint32_t lo = f->code_off, hi = f->code_off + f->code_len;
    int code = 0;

    while (fr.pc >= lo && fr.pc < hi) {
        if (++(*steps) > EXEC_STEP_CAP) { code = R_EMBED_TIMEOUT; break; }
        uint8_t op = m->code[fr.pc];
        if (!OPS[op].name) { code = R_IFBC_TYPE; break; }
        uint32_t o[OPS_MAX]; insn_operands(m, fr.pc, op, o);
        uint32_t isz = insn_len(m->code, m->code_len, fr.pc);
        if (isz == 0) { code = R_IFBC_NONLINEAR; break; }
        uint32_t next = fr.pc + isz;

        switch (op) {
            case OP_NOP: break;
            case OP_LDL: case OP_LDG: {
                /* D-38 (i) puts the global-window pair at 0x0E/0x0F. Both sides validate its operand
                 * against the frame's local window (src/ifbc.rs::check_operands treats Ldg/Stg as
                 * OperandKind::Slot), and this VM has no module-level store, so the window the
                 * operand addresses *is* the frame's — deviation D-41. */
                if (o[0] >= fr.locals_n) { code = R_IFBC_TYPE; break; }
                if (fr.moved[o[0]]) { code = R_MOVED_LOCAL; break; }
                InfyDyn* v = fr.locals[o[0]];
                if (!v) v = infy_dyn_new(DY_NONE);
                else infy_dyn_incr(v);
                if (!vpush(&fr, v)) { infy_dyn_decr(v); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_STL: case OP_STG: {
                if (o[0] >= fr.locals_n) { code = R_IFBC_TYPE; break; }
                InfyDyn* v = vpop(&fr);
                if (fr.locals[o[0]]) infy_dyn_decr(fr.locals[o[0]]);
                fr.locals[o[0]] = v; fr.moved[o[0]] = 0;
                break;
            }
            case OP_LDW: {
                if (o[0] >= fr.locals_n) { code = R_IFBC_TYPE; break; }
                InfyDyn* v = fr.locals[o[0]];
                if (!v) v = infy_dyn_new(DY_NONE); else infy_dyn_incr(v);
                if (!vpush(&fr, v)) { infy_dyn_decr(v); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_PUSH: {
                /* push k,ty (D-38 (d)): the pool entry is the constant, operand 1 is the type
                 * annotation validation item 3 reads — the execution still only reads the pool. */
                if (o[0] >= m->n_strings) { code = R_IFBC_TYPE; break; }
                InfyDyn* s = infy_dyn_str((InfyStr){ m->strings[o[0]].p, m->strings[o[0]].n });
                if (!s || !vpush(&fr, s)) { if (s) infy_dyn_decr(s); code = s ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            case OP_PUSH_B: {
                InfyDyn* s = infy_dyn_int((int64_t)(int8_t)(o[0] & 0xFF));
                if (!s || !vpush(&fr, s)) { if (s) infy_dyn_decr(s); code = s ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            case OP_DUP: {
                if (fr.sp == 0) { code = R_IFBC_STACK; break; }
                InfyDyn* v = infy_dyn_incr(fr.stack[fr.sp - 1]);
                if (!vpush(&fr, v)) { infy_dyn_decr(v); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_DUP2: {
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* a = infy_dyn_incr(fr.stack[fr.sp - 2]);
                InfyDyn* b = infy_dyn_incr(fr.stack[fr.sp - 1]);
                if (!vpush(&fr, a) || !vpush(&fr, b)) { infy_dyn_decr(a); infy_dyn_decr(b); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_SWAP: {
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* t = fr.stack[fr.sp - 1]; fr.stack[fr.sp - 1] = fr.stack[fr.sp - 2]; fr.stack[fr.sp - 2] = t;
                break;
            }
            case OP_POP: { InfyDyn* v = vpop(&fr); if (v) infy_dyn_decr(v); break; }
            case OP_LDNULL: code = R_IFBC_TYPE; break;
            case OP_LDFRAME: { InfyDyn* v = infy_dyn_int((int64_t)fi); if (!v || !vpush(&fr, v)) { if (v) infy_dyn_decr(v); code = v ? R_IFBC_STACK : R_IFBC_INVALID; } break; }
            case OP_GUARD_NZ: {
                if (fr.sp == 0) { code = R_IFBC_STACK; break; }
                int64_t d; if (as_int(fr.stack[fr.sp - 1], &d) && d == 0) { code = R_DIV_ZERO; break; }
                break;
            }
            case OP_GUARD_SH: {
                if (fr.sp == 0) { code = R_IFBC_STACK; break; }
                int64_t n; if (as_int(fr.stack[fr.sp - 1], &n) && (n < 0 || n >= 64)) { code = R_SHIFT_WIDTH; break; }
                break;
            }
            /* ---- D.3 families, binary (pop2): byte = family, operand 0 = type_table row ---- */
            case OP_ADD: case OP_SUB: case OP_MUL: case OP_DIV: case OP_REM:
            case OP_AND: case OP_OR: case OP_XOR: case OP_SHL: case OP_SHR: case OP_SHRA:
            case OP_CMPLT: case OP_CMPLE: case OP_CMPGT: case OP_CMPGE: case OP_CMPEQ:
            case OP_CMPNE: case OP_CMP: {
                int flt = 0; int frc = fam_path(m, op, o[0], &flt);
                if (frc) { code = frc; break; }
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* b = vpop(&fr); InfyDyn* a = vpop(&fr);
                int64_t ri = 0; double rf = 0; int is_flt_res = 0;
                if (flt) {
                    double x, y;
                    if (!as_flt(a, &x) || !as_flt(b, &y)) { code = R_DYN_TAG; }
                    else {
                        double ar = 0; int64_t cr = 0;
                        switch (op) {
                            case OP_ADD: ar = x + y; is_flt_res = 1; break;
                            case OP_SUB: ar = x - y; is_flt_res = 1; break;
                            case OP_MUL: ar = x * y; is_flt_res = 1; break;
                            case OP_DIV: if (y == 0.0) { code = R_DIV_ZERO; break; } ar = x / y; is_flt_res = 1; break;
                            case OP_REM:
                                if (y == 0.0) { code = R_DIV_ZERO; break; }
                                if (x != x || x == 1.0/0.0 || x == -1.0/0.0) { code = R_INF_REM; break; }
                                ar = x - y * (double)(long long)(x / y); is_flt_res = 1; break;
                            case OP_CMPLT: cr = x <  y; break;
                            case OP_CMPLE: cr = x <= y; break;
                            case OP_CMPGT: cr = x >  y; break;
                            case OP_CMPGE: cr = x >= y; break;
                            case OP_CMPEQ: cr = x == y; break;   /* nan == nan -> false per §4.4 */
                            case OP_CMPNE: cr = x != y; break;
                            default:       cr = (x < y) ? -1 : (x > y ? 1 : 0); break;   /* cmp */
                        }
                        rf = ar; ri = cr;
                    }
                } else {
                    int64_t x, y, r = 0;
                    if (!as_int(a, &x) || !as_int(b, &y)) { code = R_DYN_TAG; }
                    else switch (op) {
                        case OP_ADD: if (!checked_add_i64(x, y, &r)) { code = R_INT_OVERFLOW; break; } break;
                        case OP_SUB: if (!checked_sub_i64(x, y, &r)) { code = R_INT_OVERFLOW; break; } break;
                        case OP_MUL: if (!checked_mul_i64(x, y, &r)) { code = R_INT_OVERFLOW; break; } break;
                        case OP_DIV: if (y == 0) { code = R_DIV_ZERO; break; }
                                     if (x == INT64_MIN && y == -1) r = INT64_MIN; /* saturate; avoids C signed overflow UB */
                                     else r = x / y;
                                     break;
                        case OP_REM: if (y == 0) { code = R_DIV_ZERO; break; }
                                     r = (x == INT64_MIN && y == -1) ? 0 : x % y; break;
                        case OP_AND: r = x & y; break;
                        case OP_OR:  r = x | y; break;
                        case OP_XOR: r = x ^ y; break;
                        case OP_SHL: if (y < 0 || y >= 64) { code = R_SHIFT_WIDTH; break; } r = (int64_t)((uint64_t)x << y); break;
                        case OP_SHR: if (y < 0 || y >= 64) { code = R_SHIFT_WIDTH; break; } r = (int64_t)((uint64_t)x >> y); break;
                        case OP_SHRA: if (y < 0 || y >= 64) { code = R_SHIFT_WIDTH; break; } r = x >> y; break;
                        case OP_CMPLT: r = x <  y; break;
                        case OP_CMPLE: r = x <= y; break;
                        case OP_CMPGT: r = x >  y; break;
                        case OP_CMPGE: r = x >= y; break;
                        case OP_CMPEQ: r = x == y; break;
                        case OP_CMPNE: r = x != y; break;
                        default: r = (x < y) ? -1 : (x > y ? 1 : 0); break;   /* cmp: three-state */
                    }
                    ri = r;
                }
                infy_dyn_decr(a); infy_dyn_decr(b);
                if (code) break;
                InfyDyn* res = is_flt_res ? infy_dyn_float(rf) : infy_dyn_int(ri);
                if (!res || !vpush(&fr, res)) { if (res) infy_dyn_decr(res); code = res ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            /* ---- D.3 families, unary (pop1 push1) ---- */
            case OP_NEG: case OP_ABS: case OP_NOT: {
                int flt = 0; int frc = fam_path(m, op, o[0], &flt);
                if (frc) { code = frc; break; }
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* a = vpop(&fr);
                InfyDyn* res;
                if (flt) {
                    double x; if (!as_flt(a, &x)) { infy_dyn_decr(a); code = R_DYN_TAG; break; }
                    double r = (op == OP_NEG) ? -x : (x < 0 ? -x : x);   /* NEG / ABS (NOT is refused) */
                    infy_dyn_decr(a);
                    res = infy_dyn_float(r);
                } else {
                    int64_t x; if (!as_int(a, &x)) { infy_dyn_decr(a); code = R_DYN_TAG; break; }
                    int64_t r;
                    if (op == OP_NEG) { if (x == INT64_MIN) { infy_dyn_decr(a); code = R_INT_OVERFLOW; break; } r = -x; }
                    else if (op == OP_ABS) { if (x == INT64_MIN) { infy_dyn_decr(a); code = R_INT_OVERFLOW; break; } r = x < 0 ? -x : x; }
                    else r = ~x;                                  /* `not bool` is bitwise, like every other int kind */
                    infy_dyn_decr(a);
                    res = infy_dyn_int(r);
                }
                if (!res || !vpush(&fr, res)) { if (res) infy_dyn_decr(res); code = res ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            /* ---- control ---- */
            case OP_JMP: {
                if (o[0] < lo || o[0] >= hi) { code = R_IFBC_NONLINEAR; break; }
                fr.pc = o[0]; continue;
            }
            case OP_BR: case OP_BRF: {
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* v = vpop(&fr); int64_t t = 0; int truthy = as_int(v, &t) ? (t != 0) : (v && v->tag != DY_NONE);
                if (v) infy_dyn_decr(v);
                if ((op == OP_BR && truthy) || (op == OP_BRF && !truthy)) { if (o[0] < lo || o[0] >= hi) { code = R_IFBC_NONLINEAR; break; } fr.pc = o[0]; continue; }
                break;
            }
            case OP_SWITCH: {
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* v = vpop(&fr); int64_t idx = -1; if (!as_int(v, &idx)) { infy_dyn_decr(v); code = R_DYN_TAG; break; } infy_dyn_decr(v);
                uint32_t n = o[0]; uint32_t tb = fr.pc + 5;
                int64_t tgt = (int64_t)next; /* default = fallthrough after table */
                if (idx >= 0 && (uint32_t)idx < n && tb + 4u * (uint32_t)idx + 4u <= m->code_len) tgt = (int64_t)be32(m->code + tb + 4u * (uint32_t)idx);
                if (tgt < (int64_t)lo || tgt >= (int64_t)hi) { code = R_IFBC_NONLINEAR; break; }
                fr.pc = (uint32_t)tgt; continue;
            }
            case OP_TICKLOOP: {
                fr.ticks++;
                if (o[0] && fr.ticks > o[0]) { code = R_EMBED_TIMEOUT; break; }
                if (has_deadline && infy_monotonic_ms() >= deadline_ms) { code = R_EMBED_TIMEOUT; break; }
                break;
            }
            case OP_LEAVE: break;
            case OP_ENTER_TRY: break; /* region markers; dispatch happens on throw.inf */
            case OP_THROW_INF: {
                /* exception-table dispatch: innermost region with start<=pc<end */
                int found = -1;
                for (uint32_t k = 0; k < m->n_excs; k++) {
                    if (m->excs[k].start <= fr.pc && fr.pc < m->excs[k].end) {
                        if (found < 0 || m->excs[k].depth >= m->excs[found].depth) found = (int)k;
                    }
                }
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* payload = vpop(&fr);
                if (payload) infy_dyn_decr(payload);
                if (found >= 0 && m->excs[found].handler >= lo && m->excs[found].handler < hi) {
                    fr.pc = m->excs[found].handler; continue;
                }
                /* uncaught: propagate kind as the return code (contained, no panic) */
                code = found >= 0 ? (int)m->excs[found].kind : R_IFBC_INVALID;
                break;
            }
            /* ---- calls ---- */
            case OP_CALL_STAT: {
                if (o[0] >= m->func_count) { code = R_IFBC_FRAME; break; }
                uint32_t cnargs = m->funcs[o[0]].nargs;
                if (fr.sp < cnargs) { code = R_IFBC_STACK; break; }
                InfyDyn** cargs = (InfyDyn**)calloc(cnargs ? cnargs : 1, sizeof(InfyDyn*));
                if (!cargs) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = cnargs; k-- > 0;) cargs[k] = vpop(&fr); /* move ownership */
                InfyDyn* cv = 0; int cy = 0;
                int rc2 = exec(m, o[0], cargs, cnargs, deadline_ms, has_deadline, steps, &cv, &cy, depth + 1);
                for (uint32_t k = 0; k < cnargs; k++) if (cargs[k]) infy_dyn_decr(cargs[k]);
                free(cargs);
                if (rc2) { if (cv) infy_dyn_decr(cv); code = rc2; break; }
                if (!cv) cv = infy_dyn_new(DY_NONE);
                if (!vpush(&fr, cv)) { infy_dyn_decr(cv); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_TAILCALL_STAT: {
                if (o[0] >= m->func_count) { code = R_IFBC_FRAME; break; }
                uint32_t cnargs = m->funcs[o[0]].nargs;
                if (fr.sp < cnargs) { code = R_IFBC_STACK; break; }
                InfyDyn** cargs = (InfyDyn**)calloc(cnargs ? cnargs : 1, sizeof(InfyDyn*));
                if (!cargs) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = cnargs; k-- > 0;) cargs[k] = vpop(&fr);
                InfyDyn* cv = 0; int cy = 0;
                int rc2 = exec(m, o[0], cargs, cnargs, deadline_ms, has_deadline, steps, &cv, &cy, depth + 1);
                for (uint32_t k = 0; k < cnargs; k++) if (cargs[k]) infy_dyn_decr(cargs[k]);
                free(cargs);
                if (rc2) { if (cv) infy_dyn_decr(cv); code = rc2; break; }
                if (!cv) cv = infy_dyn_new(DY_NONE);
                *out_ret = cv; *out_yielded = 0; fr.pc = hi; goto done; /* tail-call result is our result */
            }
            case OP_CALL_HOST: case OP_CALL_EMBED: {
                if (o[0] >= m->n_imports) { code = R_IFBC_HOSTSIG; break; }
                const PImport* im = &m->imports[o[0]];
                uint32_t np = im->nparams;
                if (fr.sp < np) { code = R_IFBC_STACK; break; }
                InfyDyn** hargs = (InfyDyn**)calloc(np ? np : 1, sizeof(InfyDyn*));
                if (!hargs) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = np; k-- > 0;) hargs[k] = vpop(&fr);
                InfyDyn* hv = 0;
                int rc2;
                if (m->thunks && o[0] < m->n_imports && m->thunks[o[0]]) rc2 = m->thunks[o[0]](hargs, np, &hv, 0);
                else rc2 = R_HOST_UNMAPPED;
                for (uint32_t k = 0; k < np; k++) if (hargs[k]) infy_dyn_decr(hargs[k]);
                free(hargs);
                if (rc2) { if (hv) infy_dyn_decr(hv); code = rc2; break; }
                if (!hv) hv = infy_dyn_new(DY_NONE);
                if (!vpush(&fr, hv)) { infy_dyn_decr(hv); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_IMPORT: {
                /* 0x68 is a *load-time* instruction: §7.7.2's capability scan counts which imports a
                 * unit actually reaches, and the operand must be a row (checked in validation 3).
                 * δ = (0,0), so executing it is exactly the no-op the shared table declares. */
                if (o[0] >= m->n_imports) { code = R_IFBC_TYPE; break; }
                break;
            }
            case OP_CALL_VIRT: {
                /* dynamic dispatch on a DY_FUNC at top of stack, with its own captured arity */
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* callee = fr.stack[fr.sp - 1];
                if (!callee || callee->tag != DY_FUNC) { code = R_DYN_TAG; break; }
                const struct InfyIfbcFunc* tf = callee->v.clo.fn;
                if (!tf) { code = R_DYN_TARGET; break; }
                uint32_t tidx = (uint32_t)(tf - m->funcs);
                uint32_t cnargs = tf->nargs + 1; /* receiver + args: conservative */
                if (cnargs > fr.sp) { code = R_IFBC_STACK; break; }
                InfyDyn** cargs = (InfyDyn**)calloc(cnargs, sizeof(InfyDyn*));
                if (!cargs) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = cnargs; k-- > 0;) cargs[k] = vpop(&fr);
                InfyDyn* cv = 0; int cy = 0;
                int rc2 = exec(m, tidx, cargs, cnargs, deadline_ms, has_deadline, steps, &cv, &cy, depth + 1);
                for (uint32_t k = 0; k < cnargs; k++) if (cargs[k]) infy_dyn_decr(cargs[k]);
                free(cargs);
                if (rc2) { if (cv) infy_dyn_decr(cv); code = rc2; break; }
                if (!cv) cv = infy_dyn_new(DY_NONE);
                if (!vpush(&fr, cv)) { infy_dyn_decr(cv); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_INVOKE_DYNAMIC: {
                if (o[0] >= m->func_count) { code = R_DYN_TARGET; break; }
                uint32_t cnargs = m->funcs[o[0]].nargs;
                if (fr.sp < cnargs) { code = R_IFBC_STACK; break; }
                InfyDyn** cargs = (InfyDyn**)calloc(cnargs ? cnargs : 1, sizeof(InfyDyn*));
                if (!cargs) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = cnargs; k-- > 0;) cargs[k] = vpop(&fr);
                InfyDyn* cv = 0; int cy = 0;
                int rc2 = exec(m, o[0], cargs, cnargs, deadline_ms, has_deadline, steps, &cv, &cy, depth + 1);
                for (uint32_t k = 0; k < cnargs; k++) if (cargs[k]) infy_dyn_decr(cargs[k]);
                free(cargs);
                if (rc2) { if (cv) infy_dyn_decr(cv); code = rc2; break; }
                if (!cv) cv = infy_dyn_new(DY_NONE);
                if (!vpush(&fr, cv)) { infy_dyn_decr(cv); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_RET_MULTI: {
                uint32_t n = o[0] > 1024u ? 1024u : o[0];
                if (fr.sp < n) { code = R_IFBC_STACK; break; }
                InfyDyn* tup = infy_dyn_tuple_new();
                if (!tup) { code = R_IFBC_INVALID; break; }
                /* preserve left-to-right order */
                for (uint32_t k = 0; k < n; k++) { InfyDyn* m2 = vpop(&fr); if (!m2 || !infy_dyn_seq_push(tup, m2)) { if (m2) infy_dyn_decr(m2); } }
                /* reverse the tuple to restore order (popped reverse) */
                InfyDyn** items = (InfyDyn**)tup->v.list.items;
                for (uint32_t l = 0, r2 = tup->v.list.n; l < r2 / 2; l++) { uint32_t j = r2 - 1 - l; InfyDyn* t2 = items[l]; items[l] = items[j]; items[j] = t2; }
                *out_ret = tup; *out_yielded = 0; fr.pc = hi; goto done;
            }
            case OP_RET: case OP_FRAME_EXIT: {
                InfyDyn* v = vpop(&fr);
                *out_ret = v ? v : infy_dyn_new(DY_NONE);
                *out_yielded = (op == OP_FRAME_EXIT) ? 0 : 0;
                fr.pc = hi; goto done;
            }
            case OP_CLOSURE_NEW: {
                if (o[0] >= m->func_count) { code = R_IFBC_FRAME; break; }
                uint32_t capn = o[1] > 1024u ? 1024u : o[1];
                if (fr.sp < capn) { code = R_IFBC_STACK; break; }
                InfyDyn** cap = capn ? (InfyDyn**)calloc(capn, sizeof(InfyDyn*)) : 0;
                if (capn && !cap) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = capn; k-- > 0;) cap[k] = vpop(&fr);
                InfyDyn* cl = infy_dyn_func(&m->funcs[o[0]], cap, capn);
                if (!cl || !vpush(&fr, cl)) { if (cl) infy_dyn_decr(cl); code = cl ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            /* ---- objects ---- */
            case OP_NEW_STR: {
                if (o[0] >= m->n_strings) { code = R_IFBC_TYPE; break; }
                InfyDyn* s = infy_dyn_str((InfyStr){ m->strings[o[0]].p, m->strings[o[0]].n });
                if (!s || !vpush(&fr, s)) { if (s) infy_dyn_decr(s); code = s ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            case OP_NEW_LIST: case OP_NEW_TUPLE: {
                /* new.list elem_ty,count / new.tuple n: the count is what makes the stack form
                 * decidable (D-40), so the elements are already sitting on the stack. */
                uint32_t n = (op == OP_NEW_LIST) ? o[1] : o[0];
                if (n > 1024u || fr.sp < n) { code = R_IFBC_STACK; break; }
                InfyDyn* cont = (op == OP_NEW_LIST) ? infy_dyn_list_new() : infy_dyn_tuple_new();
                if (!cont) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = 0; k < n; k++) {                 /* left to right, ownership moves */
                    InfyDyn* v = fr.stack[fr.sp - n + k];
                    if (!infy_dyn_seq_push(cont, v)) { infy_dyn_decr(v); infy_dyn_decr(cont); code = R_IFBC_INVALID; break; }
                }
                if (code) break;
                fr.sp -= n;
                if (!vpush(&fr, cont)) { infy_dyn_decr(cont); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_NEW_DICT: {
                /* new.dict kt,vt,count: count key/value pairs, pairs in push order. */
                uint32_t n = o[2];
                if (n > 512u || fr.sp < 2u * n) { code = R_IFBC_STACK; break; }
                InfyDyn* d = infy_dyn_dict_new();
                if (!d) { code = R_IFBC_INVALID; break; }
                for (uint32_t k = 0; k < n; k++) {
                    InfyDyn* key = fr.stack[fr.sp - 2u * n + 2u * k];
                    InfyDyn* val = fr.stack[fr.sp - 2u * n + 2u * k + 1u];
                    if (!infy_dyn_dict_set(d, key, val)) { infy_dyn_decr(key); infy_dyn_decr(val); infy_dyn_decr(d); code = R_IFBC_INVALID; break; }
                }
                if (code) break;
                fr.sp -= 2u * n;                                    /* dict_set took both references */
                if (!vpush(&fr, d)) { infy_dyn_decr(d); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_NEW_FRAME: {
                if (o[0] >= m->func_count) { code = R_IFBC_FRAME; break; }
                InfyDyn* c = infy_dyn_coro(o[0]);
                if (!c || !vpush(&fr, c)) { if (c) infy_dyn_decr(c); code = c ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            /* ---- D.6 gap ops (D-38 (i)) ---- */
            case OP_SEQ_PUSH: {
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* val = vpop(&fr); InfyDyn* cont = vpop(&fr);
                if (!cont || (cont->tag != DY_LIST && cont->tag != DY_TUPLE)) {
                    if (val) infy_dyn_decr(val);
                    if (cont) infy_dyn_decr(cont);
                    code = R_DYN_TAG; break;
                }
                if (!infy_dyn_seq_push(cont, val)) {            /* takes val on success only */
                    infy_dyn_decr(val); infy_dyn_decr(cont); code = R_IFBC_INVALID; break;
                }
                if (!vpush(&fr, cont)) { infy_dyn_decr(cont); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_SEQ_LEN: {
                /* δ = (0,1) per the shared table: the container stays on the stack and the length is
                 * pushed above it. */
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* cont = fr.stack[fr.sp - 1];
                if (!cont || (cont->tag != DY_LIST && cont->tag != DY_TUPLE && cont->tag != DY_DICT &&
                              cont->tag != DY_STR && cont->tag != DY_BYTES)) { code = R_DYN_TAG; break; }
                uint64_t n = cont->tag == DY_STR || cont->tag == DY_BYTES ? cont->v.s.n : infy_dyn_len(cont);
                InfyDyn* l = infy_dyn_int((int64_t)n);
                if (!l || !vpush(&fr, l)) { if (l) infy_dyn_decr(l); code = l ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            case OP_MAP_INSERT: {
                if (fr.sp < 3) { code = R_IFBC_STACK; break; }
                InfyDyn* val = vpop(&fr); InfyDyn* key = vpop(&fr); InfyDyn* cont = vpop(&fr);
                if (!cont || cont->tag != DY_DICT) {
                    if (val) infy_dyn_decr(val);
                    if (key) infy_dyn_decr(key);
                    if (cont) infy_dyn_decr(cont);
                    code = R_DYN_TAG; break;
                }
                if (!infy_dyn_dict_set(cont, key, val)) {       /* takes both on success only */
                    infy_dyn_decr(val); infy_dyn_decr(key); infy_dyn_decr(cont); code = R_IFBC_INVALID; break;
                }
                if (!vpush(&fr, cont)) { infy_dyn_decr(cont); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_STR_CONCAT: {
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* b = vpop(&fr); InfyDyn* a = vpop(&fr);
                if (!a || !b || (a->tag != DY_STR && a->tag != DY_BYTES) || (b->tag != DY_STR && b->tag != DY_BYTES)) {
                    if (a) infy_dyn_decr(a);
                    if (b) infy_dyn_decr(b);
                    code = R_DYN_TAG; break;
                }
                uint64_t n = a->v.s.n + b->v.s.n;
                uint8_t* buf = (uint8_t*)infy_alloc(n ? n : 1, 1);
                if (!buf) { infy_dyn_decr(a); infy_dyn_decr(b); code = R_IFBC_INVALID; break; }
                memcpy(buf, a->v.s.p, (size_t)a->v.s.n);
                memcpy(buf + a->v.s.n, b->v.s.p, (size_t)b->v.s.n);
                InfyDyn* out = infy_dyn_bytes_new(buf, n);   /* copies */
                infy_free(buf);
                infy_dyn_decr(a); infy_dyn_decr(b);
                if (!out) { code = R_IFBC_INVALID; break; }
                out->tag = DY_STR;                            /* the operands were strings */
                if (!vpush(&fr, out)) { infy_dyn_decr(out); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_SEQ_CONCAT: {
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* b = vpop(&fr); InfyDyn* a = vpop(&fr);
                if (!a || !b || (a->tag != DY_LIST && a->tag != DY_TUPLE) || (b->tag != DY_LIST && b->tag != DY_TUPLE)) {
                    if (a) infy_dyn_decr(a);
                    if (b) infy_dyn_decr(b);
                    code = R_DYN_TAG; break;
                }
                InfyDyn* out = infy_dyn_list_new();
                if (!out) { infy_dyn_decr(a); infy_dyn_decr(b); code = R_IFBC_INVALID; break; }
                for (uint32_t k = 0; k < 2; k++) {
                    InfyDyn* src = k ? b : a;
                    for (uint64_t i = 0; i < infy_dyn_len(src); i++) {
                        InfyDyn* e = infy_dyn_seq_get(src, i);
                        if (e && !infy_dyn_seq_push(out, infy_dyn_incr(e))) { infy_dyn_decr(e); infy_dyn_decr(out); out = 0; break; }
                    }
                }
                infy_dyn_decr(a); infy_dyn_decr(b);
                if (!out) { code = R_IFBC_INVALID; break; }
                if (!vpush(&fr, out)) { infy_dyn_decr(out); code = R_IFBC_STACK; break; }
                break;
            }
            /* ---- ops the object model cannot carry (deviation D-41, BACKLOG F27) ----
             * `seq.iter`/`seq.next` need an iterator value, `import.attr`/`inst.get`/`inst.set` need
             * an attribute map, `new.set` needs a set — none of which `InfyDyn` (infy_rt.h) has a
             * slot for. They decode, load-verify and disassemble like any other instruction; the
             * interpreter refuses them explicitly instead of guessing a stand-in. */
            case OP_SEQ_ITER: case OP_SEQ_NEXT: case OP_INST_GET: case OP_INST_SET:
            case OP_IMPORT_ATTR: case OP_NEW_SET: {
                code = R_DYN_TAG; break;
            }
            case OP_IDX_GET: {
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* idx = vpop(&fr); InfyDyn* cont = vpop(&fr);
                InfyDyn* got = 0;
                if (cont && (cont->tag == DY_LIST || cont->tag == DY_TUPLE)) {
                    int64_t i;
                    if (!as_int(idx, &i)) code = R_DYN_TAG;
                    else if (i >= 0 && (uint64_t)i < infy_dyn_len(cont)) {
                        InfyDyn* e = infy_dyn_seq_get(cont, (uint64_t)i);
                        got = e ? infy_dyn_incr(e) : 0;
                    } else code = R_INDEX_OOB;
                } else if (cont && cont->tag == DY_DICT) {
                    InfyDyn* e = infy_dyn_dict_get(cont, idx); if (e) got = infy_dyn_incr(e); else { code = R_UNWRAP_NONE; }
                } else if (cont && (cont->tag == DY_STR || cont->tag == DY_BYTES)) {
                    int64_t i; if (as_int(idx, &i) && i >= 0 && (uint64_t)i < cont->v.s.n) {
                        if (cont->tag == DY_BYTES) { got = infy_dyn_int((int64_t)cont->v.s.p[i]); }
                        else { uint8_t c2[1]; c2[0] = cont->v.s.p[i]; got = infy_dyn_bytes_new(c2, 1); }
                    } else code = R_INDEX_OOB;
                } else code = R_DYN_TAG;
                infy_dyn_decr(idx); infy_dyn_decr(cont);
                if (code) break;
                if (!got) { got = infy_dyn_new(DY_NONE); }
                if (!vpush(&fr, got)) { infy_dyn_decr(got); code = R_IFBC_STACK; break; }
                break;
            }
            case OP_IDX_SET: {
                if (fr.sp < 3) { code = R_IFBC_STACK; break; }
                InfyDyn* val = vpop(&fr); InfyDyn* idx = vpop(&fr); InfyDyn* cont = vpop(&fr);
                int ok = 0;
                if (cont && (cont->tag == DY_LIST)) {
                    int64_t i; if (as_int(idx, &i) && i >= 0 && (uint64_t)i < infy_dyn_len(cont)) ok = infy_dyn_seq_set(cont, (uint64_t)i, val);
                } else if (cont && cont->tag == DY_DICT) {
                    ok = infy_dyn_dict_set(cont, idx, val); idx = 0; val = 0; /* ownership consumed */
                } else code = R_DYN_TAG;
                if (!ok && cont && cont->tag == DY_LIST && val) { infy_dyn_decr(val); val = 0; code = R_INDEX_OOB; }
                if (val) infy_dyn_decr(val);
                if (idx) infy_dyn_decr(idx);
                infy_dyn_decr(cont);
                if (code) break;
                break;
            }
            case OP_SLICE: {
                if (fr.sp < 3) { code = R_IFBC_STACK; break; }
                InfyDyn* hi = vpop(&fr); InfyDyn* lo = vpop(&fr); InfyDyn* cont = vpop(&fr);
                InfyDyn* out = 0;
                int64_t a, b;
                if (cont && (cont->tag == DY_LIST) && as_int(lo, &a) && as_int(hi, &b)) {
                    uint64_t ln = infy_dyn_len(cont);
                    if (a < 0) a = 0;
                    if (b > (int64_t)ln) b = (int64_t)ln;
                    if (a > b) a = b;
                    out = infy_dyn_list_new();
                    for (int64_t k = a; k < b && out; k++) { InfyDyn* e = infy_dyn_seq_get(cont, (uint64_t)k); if (e) infy_dyn_seq_push(out, infy_dyn_incr(e)); }
                } else if (cont && (cont->tag == DY_STR || cont->tag == DY_BYTES) && as_int(lo, &a) && as_int(hi, &b)) {
                    if (a < 0) a = 0;
                    if (b > (int64_t)cont->v.s.n) b = (int64_t)cont->v.s.n;
                    if (a > b) a = b;
                    out = infy_dyn_bytes_new(cont->v.s.p + a, (uint64_t)(b - a));
                    if (cont->tag == DY_STR && out) out->tag = DY_STR;
                } else code = R_DYN_TAG;
                infy_dyn_decr(hi); infy_dyn_decr(lo); infy_dyn_decr(cont);
                if (code) break;
                if (!out || !vpush(&fr, out)) { if (out) infy_dyn_decr(out); code = out ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            case OP_REF_INC: { if (fr.sp) infy_dyn_incr(fr.stack[fr.sp - 1]); break; }
            case OP_REF_DEC: {
                if (fr.sp == 0) break;
                InfyDyn* v = fr.stack[fr.sp - 1];
                if (v && v->rc > 1) v->rc--;
                else if (v && v->rc == 1) { /* last strong ref but the slot keeps the pointer: replace with NONE */
                    InfyDyn* none = infy_dyn_new(DY_NONE); fr.stack[fr.sp - 1] = none; infy_dyn_decr(v);
                }
                break;
            }
            case OP_MOVE_OUT: {
                if (o[0] >= fr.locals_n) { code = R_IFBC_TYPE; break; }
                InfyDyn* v = fr.locals[o[0]];
                fr.moved[o[0]] = 1; fr.locals[o[0]] = 0;   /* slot ownership leaves the local */
                if (!v) v = infy_dyn_new(DY_NONE);
                if (!vpush(&fr, v)) { infy_dyn_decr(v); code = R_IFBC_STACK; break; }
                break;
            }
            /* ---- dyn ---- */
            case OP_DYN_TRUTHY: {
                /* δ = (0,1): the value stays, its truthiness is pushed above it (§7.7.6: every
                 * Python `if` needs this without a bool type). */
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* v = fr.stack[fr.sp - 1];
                int64_t t = 0;
                if (!v) { code = R_DYN_TAG; break; }
                switch (v->tag) {
                    case DY_NONE: t = 0; break;
                    case DY_INT: case DY_BOOL: t = v->v.i != 0; break;
                    case DY_FLOAT: t = v->v.f != 0.0; break;
                    case DY_STR: case DY_BYTES: t = v->v.s.n != 0; break;
                    case DY_LIST: case DY_TUPLE: case DY_DICT: t = infy_dyn_len(v) != 0; break;
                    case DY_FUNC: case DY_CORO: case DY_INSTANCE: t = 1; break;
                    default: code = R_DYN_TAG; break;
                }
                if (code) break;
                InfyDyn* r = infy_dyn_bool((int)t);
                if (!r || !vpush(&fr, r)) { if (r) infy_dyn_decr(r); code = r ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            case OP_DYN_IDENTITY: {
                /* `is` in the literal sense: same box, not equal contents (§4.4 separates the two). */
                if (fr.sp < 2) { code = R_IFBC_STACK; break; }
                InfyDyn* b = vpop(&fr); InfyDyn* a = vpop(&fr);
                int64_t same = (a && b && a == b) ? 1 : 0;
                if (a) infy_dyn_decr(a);
                if (b) infy_dyn_decr(b);
                InfyDyn* r = infy_dyn_bool((int)same);
                if (!r || !vpush(&fr, r)) { if (r) infy_dyn_decr(r); code = r ? R_IFBC_STACK : R_IFBC_INVALID; break; }
                break;
            }
            case OP_TAG_GET: { if (fr.sp < 1) { code = R_IFBC_STACK; break; } InfyDyn* v = vpop(&fr); InfyDyn* t = infy_dyn_int((int64_t)v->tag); infy_dyn_decr(v); if (!t || !vpush(&fr, t)) { if (t) infy_dyn_decr(t); code = t ? R_IFBC_STACK : R_IFBC_INVALID; } break; }
            case OP_IS_DYN: {
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDynTag want; if (!dyn_want_tag(m, o[0], &want)) { code = R_DYN_TAG; break; }
                InfyDyn* v = vpop(&fr); InfyDyn* r = infy_dyn_int((v && v->tag == want) ? 1 : 0); infy_dyn_decr(v); if (!r || !vpush(&fr, r)) { if (r) infy_dyn_decr(r); code = r ? R_IFBC_STACK : R_IFBC_INVALID; } break;
            }
            case OP_CAST_DYN: {
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDynTag want; if (!dyn_want_tag(m, o[0], &want)) { code = R_DYN_TAG; break; }
                InfyDyn* v = fr.stack[fr.sp - 1]; if (v && v->tag == want) { /* keep */ } else { code = R_DYN_TAG; } break;
            }
            case OP_BOX_DYN: { break; /* values are already InfyDyn boxes */ }
            case OP_UNBOX_DYN: { if (fr.sp < 1) { code = R_IFBC_STACK; break; } InfyDyn* v = fr.stack[fr.sp - 1]; if (!v || v->tag == DY_NONE) { code = R_DYN_TAG; } break; }
            /* ---- coroutines (single-shot) ---- */
            case OP_FRAME_ENTER: break;
            case OP_YIELD: case OP_YIELD_FROM: {
                InfyDyn* v = vpop(&fr);
                *out_ret = v ? v : infy_dyn_new(DY_NONE);
                *out_yielded = 1;
                fr.pc = hi; goto done;
            }
            case OP_CORO_RESUME: case OP_CORO_THROW: {
                if (fr.sp < 1) { code = R_IFBC_STACK; break; }
                InfyDyn* arg = (op == OP_CORO_RESUME) ? vpop(&fr) : 0;
                InfyDyn* coro = vpop(&fr);
                if (!coro || coro->tag != DY_CORO) { if (arg) infy_dyn_decr(arg); if (coro) infy_dyn_decr(coro); code = R_DYN_TAG; break; }
                uint32_t cfi = infy_dyn_coro_func(coro);
                if (cfi >= m->func_count) { if (arg) infy_dyn_decr(arg); infy_dyn_decr(coro); code = R_DYN_TARGET; break; }
                InfyDyn* send = arg ? arg : 0;
                InfyDyn* cv = 0; int cy = 0;
                int rc2 = exec(m, cfi, send ? &send : 0, send ? 1 : 0, deadline_ms, has_deadline, steps, &cv, &cy, depth + 1);
                if (send) infy_dyn_decr(send);
                infy_dyn_decr(coro);
                if (rc2) { if (cv) infy_dyn_decr(cv); code = rc2; break; }
                if (!cv) cv = infy_dyn_new(DY_NONE);
                if (!vpush(&fr, cv)) { infy_dyn_decr(cv); code = R_IFBC_STACK; break; }
                break;
            }
            /* ---- type mirror ---- */
            case OP_TYPEOF: { InfyDyn* t = infy_dyn_int((int64_t)o[0]); if (!t || !vpush(&fr, t)) { if (t) infy_dyn_decr(t); code = t ? R_IFBC_STACK : R_IFBC_INVALID; } break; }
            case OP_SIZE_OF: { if (o[0] >= m->n_types) { code = R_IFBC_TYPE; break; } InfyDyn* t = infy_dyn_int((int64_t)m->types[o[0]].size); if (!t || !vpush(&fr, t)) { if (t) infy_dyn_decr(t); code = t ? R_IFBC_STACK : R_IFBC_INVALID; } break; }
            case OP_ALIGN_OF: { if (o[0] >= m->n_types) { code = R_IFBC_TYPE; break; } InfyDyn* t = infy_dyn_int((int64_t)m->types[o[0]].align); if (!t || !vpush(&fr, t)) { if (t) infy_dyn_decr(t); code = t ? R_IFBC_STACK : R_IFBC_INVALID; } break; }
            case OP_FIELD_OFF: {
                if (o[0] >= m->n_types || !(m->types[o[0]].flags & 1u)) { code = R_FIELD_OFF_REPR; break; }
                if (o[1] >= m->types[o[0]].nfields) { code = R_IFBC_TYPE; break; }
                uint32_t off = be32(m->types[o[0]].fields + o[1] * 12u + 4u);
                InfyDyn* t = infy_dyn_int((int64_t)off);
                if (!t || !vpush(&fr, t)) { if (t) infy_dyn_decr(t); code = t ? R_IFBC_STACK : R_IFBC_INVALID; }
                break;
            }
            default: code = R_IFBC_TYPE; break;
        }
        if (code) break;
        fr.pc = next;
    }

done:
    if (!code && !*out_yielded && !*out_ret && fr.pc >= hi) {
        /* fell off the end without a ret: implicit unit return */
        *out_ret = infy_dyn_new(DY_NONE);
    }
    frame_release(&fr);
    return code;
}

/* =========================================================================== invoke */
int infy_ifbc_invoke(InfyIfbcModule* m, uint32_t func_idx, InfyDyn** args, uint32_t nargs,
                     InfyDyn** ret, uint64_t timeout_ms, const InfyNodeLoc* at) {
    (void)at;
    if (ret) *ret = 0;
    if (!m || func_idx >= m->func_count || !m->funcs) return R_IFBC_INVALID;
    /* UB-019 is guaranteed: only a module returned by a successful infy_ifbc_load reaches here. */
    uint64_t deadline = 0; int has_deadline = 0;
    if (timeout_ms > 0) { deadline = infy_monotonic_ms() + timeout_ms; has_deadline = 1; }
    uint64_t steps = 0;
    InfyDyn* out = 0; int yielded = 0;
    int code = exec(m, func_idx, args, nargs, deadline, has_deadline, &steps, &out, &yielded, 0);
    if (m->max_mem_bytes) {
        InfyMemStats st = infy_mem_stats();
        if (st.live_bytes > m->max_mem_bytes) { if (out) infy_dyn_decr(out); return R_IFBC_MEMBUDGET; }
    }
    if (code) { if (out) infy_dyn_decr(out); return code; }
    if (ret) *ret = out; else if (out) infy_dyn_decr(out);
    return 0;
}

/* =========================================================================== dump */
/* The kind names of D-38's unified space — the same strings src/ifbc.rs::TyKind::suffix prints, so
 * a family mnemonic reads identically on both sides (`add.i64`). */
static const char* kind_name(uint32_t kind) {
    static const char* const n[K_MAX + 1] = {
        "void", "unit", "bool", "char", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64",
        "f32", "f64", "str", "bytes", "ptr", "dyn", "list", "tuple", "dict", "func", "coro",
        "instance", "ref", "box", "rc", "arc", "option", "result", "never", "usize", "isize"
    };
    return kind <= K_MAX ? n[kind] : "?";
}

/* The D.3 block 0x20..0x36 minus the two guard slots: those bytes are families, and their operand 0
 * is the type index the mnemonic is printed with. */
static int is_family_byte(uint8_t op) {
    if (op == OP_GUARD_NZ || op == OP_GUARD_SH) return 0;
    return op >= OP_ADD && op <= OP_CMP;
}

static void dump_be(FILE* out, const char* label, uint64_t v, int bytes) {
    fprintf(out, "%s=0x", label);
    for (int i = bytes * 2 - 1; i >= 0; i--) fputc("0123456789abcdef"[(v >> (i * 4)) & 0xF], out);
    fprintf(out, " (%llu)\n", (unsigned long long)v);
}

void infy_ifbc_dump(const uint8_t* image, uint64_t len, FILE* out) {
    if (!out) return;
    if (!image || len < HDR_BYTES) { fprintf(out, "IFBC: image too short (%llu bytes)\n", (unsigned long long)len); return; }
    struct InfyIfbcModule m; memset(&m, 0, sizeof m);
    int rc = parse_sections(image, len, &m);
    if (rc) { fprintf(out, "IFBC: parse failed (R-%04X)\n", rc); goto bail; }

    fprintf(out, "IFBC module\n");
    fprintf(out, "  magic=IFB1\n");
    dump_be(out, "  ifbc_version", m.ifbc_version, 2);
    dump_be(out, "  ifabi", m.ifabi, 2);
    dump_be(out, "  lang_tag", m.lang_tag, 2);
    fprintf(out, "  frame_kind=%u\n", m.frame_kind);
    fprintf(out, "  func_count=%u\n", m.func_count);
    fprintf(out, "  n_types=%u n_imports=%u n_lines=%u n_excs=%u n_strings=%u code_len=%u max_mem_bytes=%llu\n",
            m.n_types, m.n_imports, m.n_lines, m.n_excs, m.n_strings, m.code_len,
            (unsigned long long)m.max_mem_bytes);

    fprintf(out, "type_table:\n");
    for (uint32_t i = 0; i < m.n_types; i++)
        fprintf(out, "  [%u] kind=%u(%s) name=%u repr_c=%u size=%u align=%u nfields=%u\n",
                i, m.types[i].kind, kind_name(m.types[i].kind), m.types[i].name_str,
                (m.types[i].flags & 1u) ? 1u : 0u, m.types[i].size, m.types[i].align, m.types[i].nfields);

    fprintf(out, "import_table:\n");
    for (uint32_t i = 0; i < m.n_imports; i++) {
        fprintf(out, "  [%u] kind=%u signed=%u ret=%u nparams=%u sig_hash=0x%08X (", i, m.imports[i].kind,
                (m.imports[i].flags & 1u) ? 1u : 0u, m.imports[i].ret_sig, m.imports[i].nparams, m.imports[i].sig_hash);
        for (uint32_t k = 0; k < m.imports[i].nparams; k++) fprintf(out, "%u%s", m.imports[i].psigs[k], k + 1 < m.imports[i].nparams ? "," : "");
        fprintf(out, ")\n");
    }

    fprintf(out, "func_table:\n");
    for (uint32_t i = 0; i < m.func_count; i++) {
        const struct InfyIfbcFunc* f = &m.funcs[i];
        fprintf(out, "  [%u] code=[0x%X,0x%X) locals=%u max_stack=%u nargs=%u ret_arity=%u ret_sig=%u frame_kind=%u flags=0x%X name=%u sig_str=%u sig_hash=0x%08X\n",
                i, f->code_off, f->code_off + f->code_len, f->locals, f->max_stack, f->nargs, f->ret_arity, f->ret_sig, f->frame_kind, f->flags,
                f->name_str, f->sig_str, f->sig_hash);
    }

    fprintf(out, "code (disassembly):\n");
    for (uint32_t i = 0; i < m.func_count; i++) {
        const struct InfyIfbcFunc* f = &m.funcs[i];
        uint32_t pc = f->code_off, hi = f->code_off + f->code_len;
        fprintf(out, "  f%u:\n", i);
        while (pc < hi) {
            uint8_t op = m.code[pc];
            const OpInfo* inf = &OPS[op];
            uint32_t sz = insn_len(m.code, m.code_len, pc);
            if (!inf->name || sz == 0) { fprintf(out, "    0x%X: <bad 0x%02X>\n", pc, op); break; }
            /* D.3's families print their suffix inline (add.i64) and operand 0 therefore does not
             * repeat in the operand list — the same rendering rule as src/ifbc.rs::Instr::render. */
            char mn[40];
            int fam = is_family_byte(op);
            if (fam) {
                uint32_t ty = be32(m.code + pc + 1);
                snprintf(mn, sizeof mn, "%s.%s", inf->name,
                         ty < m.n_types ? kind_name(m.types[ty].kind) : "?");
            } else {
                snprintf(mn, sizeof mn, "%s", inf->name);
            }
            fprintf(out, "    0x%X: %-15s", pc, mn);
            if (inf->special == SP_SWITCH) {
                uint32_t n = be32(m.code + pc + 1);
                if (n > MAX_SWITCH_CASES) n = 0;
                fprintf(out, "n=%u", n);
                for (uint32_t k = 0; k < n && pc + 5 + 4 * k + 4 <= m.code_len; k++) fprintf(out, " [0x%X]", be32(m.code + pc + 5 + 4 * k));
                fprintf(out, "\n");
            } else {
                for (uint8_t k = fam ? 1 : 0; k < inf->nops; k++) fprintf(out, " 0x%X", be32(m.code + pc + 1 + 4 * k));
                fprintf(out, "\n");
            }
            pc += sz;
        }
    }

    fprintf(out, "line_map:\n");
    for (uint32_t i = 0; i < m.n_lines; i++) fprintf(out, "  0x%X -> line %u\n", m.lines[i].off, m.lines[i].line);
    fprintf(out, "exception_table:\n");
    for (uint32_t i = 0; i < m.n_excs; i++) fprintf(out, "  [0x%X,0x%X) -> 0x%X depth=%u kind=%u\n", m.excs[i].start, m.excs[i].end, m.excs[i].handler, m.excs[i].depth, m.excs[i].kind);
    fprintf(out, "string_pool:\n");
    for (uint32_t i = 0; i < m.n_strings; i++) {
        fprintf(out, "  [%u] len=%u bytes=", i, m.strings[i].n);
        for (uint32_t k = 0; k < m.strings[i].n && k < 64; k++) fprintf(out, "%02X", m.strings[i].p[k]);
        fprintf(out, "\n");
    }

bail:
    free(m.funcs); free(m.types); free(m.imports); free(m.lines); free(m.excs); free(m.strings);
}
