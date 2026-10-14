# IFBC — INFY ByteCode, byte-level encoding (this implementation)

Normative for `runtime/infy_ifbc.c` + `runtime/infy_dyn.c` (IFABI‑2, IFBC version 1).
This file pins every byte a producer must emit for a module to load, because the language
spec (appendix D, hole **C‑7 ①**) fixes the *semantics* of `line_map` / `exception_table` but
not their bytes, and leaves the container header / `type_table` / `import_table` / `func_table`
/ `string_pool` entirely open. **What is here is the de‑facto standard for this implementation.**
A third party can produce a loadable `.infib` IFBC section from this document alone.

Everything is **big‑endian**, written by explicit shifts; the host byte order is never used.

## 1. Module container

```
header | type_table | import_table | func_table | code | line_map | exception_table | string_pool
```

The sections appear in exactly this order. Each section is **self‑delimiting by a leading
`u32 count`** (tables), a **leading `u32 byte‑length`** (`code`), or, for the last section
(`string_pool`), a count plus its variable records. The loader walks them sequentially and
bounds‑checks **every** read against the remaining image size; a short/truncated/absurd count is
rejected with **R‑0703** (the umbrella load‑failure code). Arbitrary bytes are hostile input: the
loader returns a code, it does not read or write out of bounds.

> **D4(2) has landed (2026-10-15, commit `e5555dc`).** §1.2/§1.3/§1.5 and §3 below now describe the
> one merged wire format deviation **D-38** adjudicated, which the Rust side (`src/ifbc.rs`, mirrored
> in `docs/internals/ifbc-encoding-rust.md`) already wrote. The two sides have met on bytes since
> then: `runtime/fixtures/ifbc_rust_writer.bin` is the Rust encoder's committed output and
> `runtime/test_ifbc.c::test_rust_writer_fixture` loads **and executes** it with this loader. What is
> still open is D4(3) — comparing the two disassemblers *instruction by instruction* on the same
> image (this file's `infy_ifbc_dump` and `ifbc::disassemble` render the same shapes but not the same
> text), and D1, which is what would fill `.inf ifbc` in a real artifact.

### 1.1 header (47 bytes, fixed by §7.8.1 / runtime‑abi.md §9)

| bytes | field | notes |
|---|---|---|
| 0..3   | `magic` = `"IFB1"` | must match |
| 4..5   | `ifbc_version` u16 | this impl reads 1 |
| 6..7   | `ifabi` u16 | must equal `INFY_IFABI` (=2); else **R‑0903** |
| 8..9   | `lang_tag` u16 | source language + bytecode/capability version (diagnostic) |
| 10..41 | `module_hash[32]` | not checked by the VM |
| 42..45 | `func_count` u32 | must equal the `func_table` count |
| 46     | `frame_kind` u8 | **module‑wide value model**: must be `0` = single‑address operand stack + local window (§7.8.1); any other value → R‑0703. The *per‑function* coroutine flag lives in `func_table` (see 1.5), and its numeric table is deviation D‑23. |

### 1.2 type_table
```
u32 n_types                     # capped at 2^20; else R-0703
n_types × {
   u32 name_str                 # string_pool index (diagnostic only, not validated)
   u32 kind                    # D-38's unified space: SIG_* (0..17, see §2) then K_* (18..32)
   u32 flags                   # bit0 = #[repr(C)] (required by field_off, see 5.3)
   u32 size
   u32 align
   u32 nfields                 # capped at 2^20
   u32 payload_a              # list/tuple element, dict key — read and ignored here
   u32 payload_b              # dict value / generic argument   — read and ignored here
   nfields × { u32 name_str, u32 off, u32 ty }   # ty indexes type_table
}
```
`type_id` = the 0-based index of the record. `size_of.idx` / `align_of.idx` / `typeof.idx` /
`is.dyn` / `cast.dyn` / every D.3 family operand is a `type_id` and is bounds-checked against
`n_types` (R-0706); `is.dyn`/`cast.dyn` additionally need a row whose **kind** has a dyn-visible tag
(R-0710) — the kind is no longer numerically equal to an `InfyDynTag` (D-38 renumbered it), so the
loader maps it through `kind_to_dyn_tag`. A `kind` above 32 is a structural failure (R-0703), which
is what `src/ifbc.rs::decode_types` does too. Producers seed rows `0..17` as the scalar kinds so
**type index == scalar code** there (`seed_scalar_rows` in Rust, `N_SCALAR_ROWS` in the harness); the
seeded rows carry `size = align = 0` by design (D-40 choice 1), so a producer that wants
`size_of.idx` to mean something fills them itself.

### 1.3 import_table
```
u32 n_imports                   # capped at 2^20
n_imports × {
   u32 name_str
   u8  kind                    # 0=host  1=embed  2=event  (anything else = R-0703)
   u8  nparams
   u8  ret_sig                 # a scalar code from §2
   u8  flags                   # bit0 = "IFABI-signed" (see 5.5)
   u32 sig_hash                # D-38's trailing digest: validation 5's fast comparison value
   nparams × u8 param_sig      # scalar codes from §2
}
```

### 1.4 code
```
u32 code_len
code_len × byte                 # the instruction stream (see §3); function windows index here
```
`code` offsets are **global into this section**. `exception_table` and `line_map` also use global
`code` offsets; control‑flow instruction operands (`jmp`/`br`/`brf`/`switch` cases and default,
`enter.try`) are global `code` offsets too.

### 1.5 func_table
```
u32 n_funcs                     # must == header.func_count, else R-0703
u64 max_mem_bytes               # §7.9 per-unit arena budget; 0 = no limit → R-0709 at run time
n_funcs × {                     # 48-byte fixed record (D-38 (a): twelve u32)
   u32 code_off                 # global offset into code
   u32 code_len                 # must satisfy code_off+code_len <= code_len(section)
   u32 locals                  # local-window slot count (ldl/stl/ldw/move.out and ldg/stg bound)
   u32 max_stack               # declared operand-stack ceiling (height check)
   u32 nargs
   u32 ret_arity               # 0=unit, 1=scalar, n=tuple (ret / ret.multi match)
   u32 ret_sig                 # scalar code (§2), diagnostic
   u32 name_str
   u32 sig_str                 # string_pool index of the canonical signature text (`infy doc`)
   u32 sig_hash                # digest of that text: validation 5's fast path
   u32 flags                   # bit0 = has_return / F_ENTRY on the Rust side (same bit set, D-38)
   u32 frame_kind              # 0=stack  1=coroutine  (yield legality, R-0707; >1 rejected at load, deviation D-23)
}
```
The VM reads the ten fields it always used; `sig_str`/`sig_hash` are for the producer-side tools and
the dumper. Because the record is fixed-width, a reader that skipped the two new slots would address
`flags` where `sig_str` is — `test_ifbc.c`'s `FD()` macro therefore stamps `sig_hash` with a
non-zero sentinel, so a field-order drift fails the harness instead of loading a corrupt record.
`func_idx` = 0‑based index. Closures store a pointer to their `InfyIfbcFunc` record; the runtime
recovers `func_idx` as `ptr - funcs`, so the `clo.fn` field in the frozen `InfyDyn` union needs no
extra slot.

### 1.6 line_map (adopted verbatim from runtime‑abi.md §9)
```
u32 count
count × { u32 code_off, u32 line }      # sorted by code_off
```

### 1.7 exception_table (adopted verbatim from runtime‑abi.md §9)
```
u32 count
count × { u32 start, u32 end, u32 handler, u16 depth, u16 kind }
```
`throw.inf` dispatch searches for the innermost (largest `depth`) region with
`start <= pc < end` and resumes at `handler` (same function frame). `kind` is the normalized
error selector; an uncaught `throw.inf` unwinds all interpreter frames and surfaces `kind`.

### 1.8 string_pool
```
u32 n_strings
n_strings × { u32 len; len × byte }     # raw bytes (UTF-8 str or bytes; no NUL required)
```
`push k` / `new.str k` operands are indices here; bounds‑checked against `n_strings` (R‑0706).

## 2. IFABI scalar signature codes (`ret_sig` / `param_sig`)

A closed domain; any value `> 17` is not expressible in IFABI ⇒ **R‑0708**. This table is the
`SIG_*` enum in `infy_ifbc.c` (line 54) — it is the domain **D-38** unifies `type_table.kind` with,
so a signature byte and a `type_table` index in `0..=17` address one space.

| code | type | code | type | code | type |
|---|---|---|---|---|---|
| 0 | void | 6 | i32 | 12 | f32 |
| 1 | unit `()` | 7 | i64 | 13 | f64 |
| 2 | bool | 8 | u8 | 14 | str |
| 3 | char | 9 | u16 | 15 | bytes |
| 4 | i8 | 10 | u32 | 16 | ptr |
| 5 | i16 | 11 | u64 | 17 | dyn |

(Earlier revisions of this file numbered the block `9 = u64, 10 = f32, 11 = f64, 12 = str, 13 =
bytes, 14 = ptr, 15 = dyn` with `16/17` reserved, i.e. it dropped `u16`/`u32` — that table did not
match the enum the loader validates against. Corrected against `infy_ifbc.c`, as **D-38** pinned.)

(For `ldw` the annotation operand is a `type_table` row whose **kind** must be `i64`(7) / `u64`(11) /
`f64`(13) — the three wide scalars the loader accepts at `check_type`; else R‑0706. Because rows
`0..17` are seeded so index == kind, a producer normally writes the code itself.)

## 3. Instruction encoding

Each instruction is `opcode:u8` followed by **0..3 `operand:u32`** (big‑endian). Operand count is
implied by the opcode, so the verifier decodes in a single forward O(n) pass with no backtracking.
Byte length = `1 + 4*nops`. The only longer mnemonic is `switch` (below), which still has a
statically computable length, keeping the pass single‑and‑forward.

**Decision on grouped/unnamed spec rows** (appendix D lists some rows as several mnemonics or
ranges; these byte values are fixed here and shared by verifier / interpreter / dumper):

* `dup / dup2 / swap` (D.2 row `0x06`) → **`dup`=0x06, `dup2`=0x0A, `swap`=0x0B**.
* `pop`=0x07, `ldnull`=0x08, `ld.frame`=0x09 are pinned by D.2 (and `ldnull`@0x08 by UB‑019 tests).
* `guard.nonzero` / `guard.shift` → **0x27 / 0x2F with no operand** (D‑38 (c)). Those are D.3's own
  gaps; the `0x0C`/`0x0D` bytes this loader used for them are unassigned again, and `0x0E`/`0x0F`
  took the global-window pair (`ldg`/`stg`). D.3's adjacency rule only inspects the previous byte,
  and the type travels on the guarded instruction's own operand 0.
* D.3 says every type suffix is a distinct opcode. The full cross-product (~20 ops × ~22 kinds) does
  **not** fit the 32-slot `0x20–0x3F` block — the same spec hole both sides started from. Since
  **D-38 (b)** the answer is the family form: **one byte per family, operand 0 = `type_table` row**,
  and `exec` selects the path from that row's `kind`. The capability limit of the old byte table is
  migrated, not widened: this VM has one int path (`i64`) and one float path (`f64`), so a family
  instruction over any other kind loads (the Rust verifier accepts any numeric kind, plus `bool` for
  `not`) and is refused at execution with **R-0706**. `fam_path()` is that rule in one place.
* The gap ops D-38 (i) handed to this side (`0x0E/0x0F`, `0x68/0x69`, `0x84..0x87`, `0x8B..0x8F`,
  `0x92/0x93`, `0xA5/0xA6`) decode, verify and disassemble like any other instruction. Six of them
  have no carrier in the `InfyDyn` object model (no attribute map, no iterator, no set), so the
  interpreter refuses them explicitly — see §7 and deviation **D-41**.

### 3.1 opcode table

| byte | mnemonic | nops | operands | operand‑stack effect |
|---|---|---|---|---|
| 0x00 | `nop` | 0 | | — |
| 0x01 | `ldl i` | 1 | `i<locals` | push `locals[i]` |
| 0x02 | `stl i` | 1 | `i<locals` | pop → `locals[i]` |
| 0x03 | `ldw i,ty` | 2 | `i<locals`, `ty<n_types` and `types[ty].kind` ∈ {i64,u64,f64} | push wide (merged to one slot) |
| 0x04 | `push k,ty` | 2 | `k<n_strings`, `ty<n_types` *(or* `NONE32`*— annotation only)* | push `string_pool[k]` (DY_STR); the annotation is for validation 3 (D‑38 (d)) |
| 0x05 | `push.b k` | 1 | | push int (k & 0xFF, sign‑extended byte) |
| 0x06 | `dup` | 0 | | push copy of TOS |
| 0x07 | `pop` | 0 | | discard TOS |
| 0x08 | `ldnull` | 0 | | **forbidden → R‑0706 wherever it appears** |
| 0x09 | `ld.frame` | 0 | | push frame id (debug) |
| 0x0A | `dup2` | 0 | | push copies of top two |
| 0x0B | `swap` | 0 | | swap top two |
| 0x0C/0x0D | *(unassigned)* | | D‑38 (c): the `guard.*` bytes that used to sit here moved to D.3's own block | |
| 0x0E | `ldg i` | 1 | `i<locals` | push `locals[i]` — see D‑41: the "global window" is the frame's local window here |
| 0x0F | `stg i` | 1 | `i<locals` | pop → `locals[i]` |
| 0x20 | `add ty` | 1 | `ty` numeric | pop2 push1 (checked, R‑0001) |
| 0x21 | `sub ty` | 1 | | checked |
| 0x22 | `mul ty` | 1 | | checked |
| 0x23 | `div ty` | 1 | | **requires preceding `guard.nonzero`** |
| 0x24 | `rem ty` | 1 | | **requires preceding `guard.nonzero`** |
| 0x25 | `neg ty` | 1 | | pop1 push1 |
| 0x26 | `abs ty` | 1 | | pop1 push1 |
| 0x27 | `guard.nonzero` | **0** | | peek divisor; 0 ⇒ R‑0002 at run |
| 0x28 | `and ty` | 1 | | pop2 push1 (no float path) |
| 0x29 | `or ty` | 1 | | pop2 push1 (no float path) |
| 0x2A | `xor ty` | 1 | | pop2 push1 (no float path) |
| 0x2B | `not ty` | 1 | numeric *or* `bool` | pop1 push1, bitwise (no float path) |
| 0x2C | `shl ty` | 1 | | **requires preceding `guard.shift`** (no float path) |
| 0x2D | `shr ty` | 1 | | **requires preceding `guard.shift`** (logical; no float path) |
| 0x2E | `shr.a ty` | 1 | | **requires preceding `guard.shift`** (arithmetic; no float path) |
| 0x2F | `guard.shift` | **0** | | peek shift count; ∉[0,64) ⇒ R‑0007 |
| 0x30 | `cmplt ty` | 1 | | pop2 push int |
| 0x31 | `cmple ty` | 1 | | |
| 0x32 | `cmpgt ty` | 1 | | |
| 0x33 | `cmpge ty` | 1 | | |
| 0x34 | `cmpeq ty` | 1 | | `nan==nan` ⇒ 0 (false) |
| 0x35 | `cmpne ty` | 1 | | |
| 0x36 | `cmp ty` | 1 | | three‑state (‑1/0/1) |
| 0x37..0x3F | *(unassigned)* | | D‑38 (b): the old `add.f64`…`cmpeq.f64` bytes 0x35..0x3D and 0x3E/0x3F are gone — the width is now operand 0's `kind` | |
| 0x40 | `jmp off` | 1 | off = global boundary | — (terminator) |
| 0x41 | `br off` | 1 | | pop bool; jump if true |
| 0x42 | `brf off` | 1 | | pop bool; jump if false |
| 0x43 | `switch n` | 1 | `n ≤ 48` (`MAX_SWITCH_CASES`, the same cap `src/ifbc.rs` emits at), then `n×u32` case targets inline | pop index; case or fall‑through after table |
| 0x44 | `tickloop n` | 1 | per‑loop iteration budget | back‑edge counter; over budget or clock ⇒ R‑0701 |
| 0x45 | `leave` | 0 | | finally exit marker |
| 0x46 | `throw.inf k` | 1 | k = string_pool selector | pop payload; exception dispatch |
| 0x47 | `enter.try h_end,c_end` | 2 | both global boundaries | region marker (bounds check) |
| 0x60 | `call.stat f` | 1 | `f<func_count` | pop `funcs[f].nargs`, push result |
| 0x61 | `call.virt idx` | 1 | `idx<n_types` | dynamic call on a DY_FUNC; **not height‑provable (δ 0)** |
| 0x62 | `call.host idx` | 1 | `idx<n_imports`, signed | routes through thunk table (§5.5); never a raw address |
| 0x63 | `call.embed idx` | 1 | `idx<n_imports`, kind=embed | via thunk table |
| 0x64 | `ret` | 0 | | pop1, return (ret_arity must be 1) |
| 0x65 | `ret.multi n` | 1 | `n==ret_arity` | pop n → tuple |
| 0x66 | `closure.new code_idx,cap_n` | 2 | `code_idx<func_count` | pop cap_n, push DY_FUNC |
| 0x67 | `tailcall.stat f` | 1 | `f<func_count` | pop callee args; frame‑size legality (§5.6) |
| 0x68 | `import idx` | 1 | `idx<n_imports` | δ (0,0): the row is a **load‑time** capability claim (§7.7.2 scan item 3); executing it is the no-op the shared table declares |
| 0x69 | `import.attr k` | 1 | `k<n_strings` | δ (1,1); **refused at execution** (R‑0710) — no attribute map in `InfyDyn` (D‑41) |
| 0x80 | `new.str k` | 1 | `k<n_strings` | push owned DY_STR |
| 0x81 | `new.list ty,count` | 2 | `ty<n_types` | pop `count` values (left to right) → DY_LIST; `count = 0` is the empty container |
| 0x82 | `new.dict kt,vt,count` | 3 | `kt/vt<n_types` | pop `2*count` as pairs → DY_DICT (insertion‑ordered) |
| 0x83 | `new.frame f` | 1 | `f<func_count` | push DY_CORO |
| 0x84 | `seq.push ty` | 1 | `ty<n_types` | pop val, pop container (list/tuple), push container back |
| 0x85 | `seq.len ty` | 1 | `ty<n_types` | δ (0,1): container stays, its length is pushed above it |
| 0x86 | `seq.next ty` | 1 | `ty<n_types` | δ (1,2); **refused at execution** (R‑0710) — no iterator carrier |
| 0x87 | `seq.iter ty` | 1 | `ty<n_types` | δ (1,1); **refused at execution** (R‑0710) — no iterator carrier |
| 0x88 | `idx.get ty` | 1 | | pop idx,pop cont, push elem (R‑0004/R‑0005) |
| 0x89 | `idx.set ty` | 1 | | pop val,idx,cont |
| 0x8A | `slice ty` | 1 | | pop hi,lo,cont, push slice |
| 0x8B | `inst.get k` | 1 | `k<n_strings` | δ (1,1); **refused at execution** (R‑0710) — no attribute map (D‑41) |
| 0x8C | `inst.set k` | 1 | `k<n_strings` | δ (2,0); **refused at execution** (R‑0710) — no attribute map |
| 0x8D | `map.insert ty` | 1 | `ty<n_types` | pop val, key, DY_DICT → insert, push container |
| 0x8E | `str.concat ty` | 1 | `ty<n_types` | pop two DY_STR/DY_BYTES, push the join as DY_STR |
| 0x8F | `seq.concat ty` | 1 | `ty<n_types` | pop two DY_LIST/DY_TUPLE, push the concatenation as a new DY_LIST |
| 0x90 | `ref.inc` | 0 | | rc++ on TOS (peek) |
| 0x91 | `ref.dec` | 0 | | rc-- on TOS; last strong ref released per §4.7.4 |
| 0x92 | `new.tuple n` | 1 | | pop `n` values → DY_TUPLE |
| 0x93 | `new.set ty` | 1 | `ty<n_types` | δ (1 pop = operand 0, 1 push) per the shared table; **refused at execution** (R‑0710) — no set carrier, and the operand shape is the registered defect **F25** |
| 0x94 | `move.out i` | 1 | `i<locals` | push `locals[i]`, mark moved; later `ldl i` ⇒ R‑0731 |
| 0xA0 | `tag.get` | 0 | | pop dyn, push DY_INT tag |
| 0xA1 | `cast.dyn T` | 1 | `T<n_types`, `types[T].kind` has a dyn tag | tag compare (needs checkpoint §5.3); mismatch R‑0710 |
| 0xA2 | `is.dyn T` | 1 | same | pop dyn, push DY_INT 0/1 |
| 0xA3 | `box.dyn` | 0 | | identity (values are already InfyDyn) |
| 0xA4 | `unbox.dyn` | 0 | | requires a preceding `is.dyn`/`tag.get` (§5.3) |
| 0xA5 | `dyn.truthy` | 0 | | δ (0,1): value stays, its truthiness is pushed above it |
| 0xA6 | `dyn.identity` | 0 | | pop two, push DY_BOOL — pointer identity, not `cmpeq` (§4.4 separates them) |
| 0xA8 | `invoke.dynamic f` | 1 | `f<func_count` else **R‑0712** | pop callee args, push result |
| 0xC0 | `frame.enter` | 0 | | coroutine‑only (§5.4) |
| 0xC1 | `yield` | 0 | | **coroutine‑only, else R‑0707** |
| 0xC2 | `yield.from` | 0 | | coroutine‑only |
| 0xC3 | `frame.exit` | 0 | | coroutine end (terminator) |
| 0xC4 | `coro.resume` | 0 | | pop arg,pop DY_CORO; run to first yield; push value |
| 0xC5 | `coro.throw` | 0 | | pop DY_CORO; push value |
| 0xE0 | `typeof.idx T` | 1 | `T<n_types` | push type id |
| 0xE1 | `size_of.idx T` | 1 | `T<n_types` | push `types[T].size` |
| 0xE2 | `align_of.idx T` | 1 | `T<n_types` | push `types[T].align` |
| 0xE3 | `field_off.idx T,field` | 2 | `T<n_types`, `field<nfields`, **T must be repr(C) else R‑0713** | push field byte offset |

Any byte with no entry above is an unknown opcode ⇒ **R‑0706** (and the module is rejected).
`switch` byte length = `1 + 4 + 4*n`; the `n` case targets sit inside the enclosing function's
`[code_off, code_off+code_len)` and are consumed as part of the switch instruction, so they are
never mistaken for executable instructions by the linear pass (the data‑island rule that keeps
D.1's fixed‑width single‑pass property).

## 4. Execution model (D.1)

`infy_ifbc_invoke(module, func_idx, args, nargs, &ret, timeout_ms, at)`:
* a frame is a **local window** of `func_table.locals` slots plus an **operand stack**; incoming
  args are copied into `locals[0..nargs)` (so `ldl 0/1/...` reads them). `ret` transfers one owned
  `InfyDyn*` to the caller through `&ret`; on any contained failure `&ret` stays NULL.
* the interpreter is a recursive `exec` (one C frame per IFBC frame) with a **call‑depth cap**
  (`R‑1040`).
* every value on the stack/locals/coroutine is a reference‑counted `InfyDyn*`; ownership is tracked
  so `infy_dyn_decr` frees at rc→0 (§4.7.4 order) and `infy_ifbc_invoke` leaves nothing behind.

## 5. Load‑time validation order and failure mapping

`infy_ifbc_load` runs, **per function in `func_table` order, wholly before any instruction
executes** (UB‑019). Any failure rejects the WHOLE module (not just the function), frees every
allocation, and returns the code. With `all_errors!=1` every violation is reported to the
`report` FILE and the return is the first error code; otherwise the first failure short‑circuits.

Within a function the checks run in the D.10 order (1→6), then 6b, then the extras:

1. **Linearity** — forward fixed‑width decode; unknown opcode → R‑0706; a region that cannot be
   decoded to its end → **R‑0704**; every control‑flow target must be an instruction boundary
   inside `[code_off,code_off+code_len)` → **R‑0704**.
2. **Stack height** — fixpoint over basic blocks; entry height 0; a merge reached with two heights,
   or a height outside `[0, max_stack]` → **R‑0705**.
3. **Operand type vs suffix** — `ldnull` wherever it appears, `ldw`'s annotation *kind*, every table
   index range (with `NONE32` skipped as "names no row", the same exception `src/ifbc.rs` applies),
   each D.3 family's operand‑0 kind being numeric (`bool` additionally for `not`), `div`/`rem` not
   immediately preceded by `guard.nonzero`, `shl`/`shr`/`shr.a` not immediately preceded by
   `guard.shift` → **R‑0706**. Which of the accepted kinds this VM can *execute* is `fam_path()`.
4. **Frame legality** — `yield`/`yield.from`/`frame.enter`/`frame.exit` in a frame whose
   `frame_kind != coroutine`; a `ret`/`ret.multi` arity inconsistent with `ret_arity` → **R‑0707**.
5. **Host signature** — a `call.host`/`call.embed` target absent from `import_table`, wrong kind,
   missing the IFABI‑signed flag, or a `sig` code outside §2 → **R‑0708**.
6. **Boundary / tail‑call frame size** — a `tailcall.stat` to a frame whose `locals`/`max_stack`/
   `nargs`/`ret_arity`/`frame_kind` differ from the caller, or a `closure.new code_idx` outside
   `func_table` → **R‑0707** (`R_IFBC_FRAME`, the same code as "call/tailcall target not in
   `func_table`"). D.10 maps this row to R‑0709, but §9.7 and §7.9 both reserve R‑0709 for
   "embedded unit exceeded `max_mem_bytes`" — deviation **D‑22** records the clash and this
   mapping, so R‑0709 is produced only by the run‑time arena check (§7).
   6b. **Line‑map completeness** — an instruction boundary with no covering `line_map` entry is a
   non‑blocking **W‑0705** (the module still loads).

Extra dyn/reference checks folded into pass 3 (their codes are not in the R‑0704..0709 band):

* **UB‑020 tag checkpoints** — every `unbox.dyn`/`cast.dyn` must be preceded (in program order) by
  an `is.dyn` or `tag.get`; otherwise → **R‑0710** (`R_DYN_TAG`). This is the load‑time half of
  §7.7.7; the runtime half is `cast.dyn`/`unbox.dyn` mismatch, also R‑0710.
* **`invoke.dynamic` target** — index outside `func_table` → **R‑0712** (`R_DYN_TARGET`).
* **`field_off` on non‑`repr(C)`** — type whose `flags` bit0 is clear → **R‑0713**
  (`R_FIELD_OFF_REPR`; the header gap is closed — `InfyRCode` carries it, as it now carries
  R‑0701 `R_EMBED_TIMEOUT` too).

### 5.5 host routing and signature comparison
`call.host idx`/`call.embed idx` never branch to an address embedded in the bytecode. The runtime
keeps a **thunk table** parallel to `import_table`; `infy_ifbc_bind_host(module, idx, fn)` installs
a C function pointer at load (an added entry point — see §7). Signatures are compared byte‑wise
against IFABI: the loader requires the import's `flags` bit0 (the "IFABI‑signed" stamp the compiler
sets after checking the thunk), that every `param_sig`/`ret_sig` is inside the §2 closed set, and
that `kind` matches the call site. At run time the args are popped left‑to‑right into `InfyDyn**`,
the thunk is called, and its result is pushed; an unbound thunk yields **R‑0900**.

## 6. `infy_ifbc_dump` text format (byte‑stable)

Same image ⇒ same bytes. No addresses, timestamps or hash‑randomized ordering. Structure:

```
IFBC module
  magic=IFB1
  ifbc_version=0x0001 (1)
  ifabi=0x0002 (2)
  lang_tag=0x<4hex> (<n>)
  frame_kind=<u>
  func_count=<u>
  n_types=… n_imports=… n_lines=… n_excs=… n_strings=… code_len=… max_mem_bytes=…
type_table:
  [i] kind=<n>(<name>) name=… repr_c=… size=… align=… nfields=…
import_table:
  [i] kind=… signed=… ret=… nparams=… sig_hash=0x<8hex> (<param codes, comma separated>)
func_table:
  [i] code=[0xOFF,0xEND) locals=… max_stack=… nargs=… ret_arity=… ret_sig=… frame_kind=… flags=0x… name=… sig_str=… sig_hash=0x<8hex>
code (disassembly):
  f<i>:
    0x<off>: <mnemonic> [ <operand hex> … ]     (a D.3 family prints as `add.i64` and its type
                                                operand is *not* repeated; switch prints n=… and
                                                the [0x…] case list)
line_map:      0x<off> -> line <n>
exception_table:[0xSTART,0xEND) -> 0x<handler> depth=… kind=…
string_pool:   [i] len=… bytes=<hex bytes>
```
The mnemonic+suffix rule is the one `src/ifbc.rs::Instr::render` uses, so the two disassemblers name
an instruction the same way even though the surrounding text differs — D4(3) has to reconcile the
*line* format, not the naming.
If the container fails to parse, the dump prints `IFBC: parse failed (R-XXXX)` and stops (and does
not crash on any prefix of a truncated image).

## 7. Deviations, header gaps, capability limits

* **Header gaps** (`infy_rt.h`): `InfyRCode` now carries `R‑0701` (`R_EMBED_TIMEOUT`) and `R‑0713`
  (`R_FIELD_OFF_REPR`) — closed in the B3 round, which is also when the raw‑integer `#define`s this
  file used before were removed. Still open: the header declares no module‑unload entry point and no
  host‑thunk bind entry point, so this impl keeps `infy_ifbc_module_free` / `infy_ifbc_bind_host` as
  group‑(b) private entry points (declared in `infy_rt_internal.h`).
* **`R‑0709` overload — adjudicated (deviation **D‑22**)**: D.10 maps the load‑time "boundary" row
  (tail‑call frame size, `closure.new` outside `func_table`) to R‑0709 while §9.7 and §7.9 reserve
  that number for "`@embed` exceeds `max_mem_bytes`". §9.7 is the code table, so the boundary
  findings report **R‑0707** (frame legality, §7.8.3 item 4 — the same code "call/tailcall target
  not in `func_table`" already used) and R‑0709 now means only the arena check. The unused
  `R_IFBC_BOUNDARY = 0x070A` was deleted: no spec table defines 0x070A and `InfyRCode` is a closed
  set. `R‑0702`/`E‑0701` were not used as the escape hatch — spec:1508 keeps them permanently dead.
* **`frame_kind`** (numeric table = deviation **D‑23**): the header carries one module‑wide
  `frame_kind` u8, which §7.8.1 defines as the *value model* with a single legal value `0`
  (stack + local window), so the loader rejects any other header value with R‑0703. D.8's
  "coroutine frame" test is per function and lives in `func_table.frame_kind`:
  `INFY_FRAME_STACK = 0`, `INFY_FRAME_COROUTINE = 1` (both in `infy_rt.h`). The spec names the
  values ("0", "coroutine") but never numbers them, so `1` is this implementation's fixed choice,
  and a function record with `frame_kind > 1` is rejected at load.
* **`InfyDyn` union** has no slot for a coroutine continuation or a `repr(C)` instance: DY_CORO
  stashes a heap control‑block pointer in `v.list.items` and its `func_idx` in `v.list.esz`
  (`u32`); a per‑module `coro_release` hook frees it. DY_INSTANCE holds only its tag.
* **D‑41 (D4(2)'s implementation choices on this side).** (a) `ldg`/`stg` address the *frame's local
  window*: both sides validate that operand as a slot (`operand_kinds` → `OperandKind::Slot`, so
  `i < func.locals`), and this VM has no module‑level store, so there is nothing else the number
  could address. (b) Six instructions decode, load‑verify and disassemble but are **refused at
  execution with R‑0710**, because the object model has no carrier for them: `import.attr`,
  `inst.get`, `inst.set` (no attribute map), `seq.iter`, `seq.next` (no iterator value) and
  `new.set` (no set — and its operand shape is the registered defect **F25**). They are refused, not
  reinterpreted: a module that reaches one of them gets a code and no result, which D1 will see as
  the object model's boundary rather than as a decode error. (c) `import idx` (0x68) is a load‑time
  claim, so executing it is the δ=(0,0) no‑op the shared table declares. (d) `seq.len` and
  `dyn.truthy` have δ (0,1): they *peek* and push, which is what makes the height pass and the
  interpreter agree on the same image. (e) `dyn.identity` is pointer identity, not `cmpeq`.
  (f) `MAX_SWITCH_CASES` is now the same 48 on both sides — the loader's old `OPS_MAX*16` was 64, so
  a 56‑case switch would have been scannable here and unemittable there.
* **Capability limits (honest):** ints are fixed 64‑bit — **no bignum promotion** (Python/Lua
  integers beyond i64 overflow to R‑0001).
* **Family widths this VM executes** are `i64` and `f64` only (§3). Any other numeric kind — and
  `f32`, which would silently compute at double precision — loads and is refused with R‑0706 at
  execution. Reference **cycles are not collected** (§4.8 forbids a tracing GC) so a `dyn` cycle
  leaks until exit. **Coroutines are single‑shot**: `coro.resume` runs a body to its first
  `yield`/`frame.exit` and returns that value; suspend/resume state is not persisted across separate
  resumes. `tailcall.stat` is validated for frame‑size legality but executed as an ordinary call (no
  native tail‑frame reuse), so deep tail recursion is bounded by the call‑depth cap. Exception
  dispatch uses the module `exception_table`; `finally` ordering is limited to the `depth` field and
  uncaught throws surface their `kind` rather than longjmp. `call.virt` operand‑stack height is not
  provable and is not checked.
