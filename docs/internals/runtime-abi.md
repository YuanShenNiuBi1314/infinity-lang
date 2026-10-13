# INFY v2.0 — codegen / runtime ABI (IFABI-2)

Normative for this implementation. The spec fixes *observable* behaviour (§4, §7.4, §7.8, §8,
§11, §12.3) and leaves the value representation to `IFABI` (§4.1: "vtable 布局由 `IFABI` 固定",
§1.2: IFABI is versioned independently of the language). This document is that fixing.
Anything a `.infib` wheel could observe — sizes, alignment, vtable order, `capacity()` growth
factors, `Debug`/`Ord`/`Hash` output, node numbering, drop order — is listed here.

Target: C11 + `<stdatomic.h>`, compiled by `cc` (gcc/clang/cl). 64-bit little-endian is the
reference target; `usize`/`isize` width is taken from the target and recorded in `.inf meta`
(§2.8). 128-bit integers use `__int128` where the host compiler provides it, otherwise
`InfyU128/InfyI128` software pairs (the compiler probes this and records it in `.inf meta`).

## 1. Scalar and aggregate representation

| INFY type | C type | size/align @64 |
| --- | --- | --- |
| `bool` | `uint8_t` (only `0x00`/`0x01` legal — other bit patterns are `R-0006`, §4.1) | 1/1 |
| `u8`…`u64`, `i8`…`i64` | `uintN_t` / `intN_t` | N/8 |
| `u128`, `i128` | `unsigned __int128` / `__int128` | 16/8 (16/16 on GCC ≥ 13 for `__int128`; the compiler emits the probed value) |
| `usize` / `isize` | `size_t` / `ptrdiff_t` | target width |
| `f32` / `f64` | `float` / `double`, IEEE-754, `-ffp-contract=off` unless `#[fast_math]` (§4.4) | 4/8 |
| `char` | `uint32_t` (a Unicode code point; grapheme clusters are rejected at lex time, `E-0049`) | 4/4 |
| `()` | empty struct `typedef struct { uint8_t _0; } InfyUnit;` — zero-sized types are represented as `InfyUnit`, never as nothing, so that `&T` on a ZST stays well-formed | 1/1 |
| `str` | `typedef struct { const uint8_t* ptr; uint64_t len; } InfyStr;` — UTF-8, **not** NUL-terminated (§7.6.4) | 16/8 |
| `String` | `typedef struct { uint8_t* ptr; uint64_t len; uint64_t cap; } InfyString;` | 24/8 |
| `&T` / `&mut T` | `const T*` / `T*` | 8/8 |
| `*const T` / `*mut T` | `T const*` / `T*`, dereference only inside `unsafe` (§4.7.5) | 8/8 |
| `[T; N]` | `T[N]` | `sizeof(T)*N` |
| `[T]` | `typedef struct { const T* ptr; uint64_t len; } InfySlice_T;` (fat pointer, §4.1) | 16/8 |
| `Vec<T>` | `typedef struct { T* ptr; uint64_t len; uint64_t cap; } InfyVec_T;` | 24/8 |
| `Box<T>` | `T*` (heap, freed by the owning scope's cleanup entry) | 8/8 |
| `Rc<T>` / `Arc<T>` | `typedef struct { T* data; uint64_t* strong; } InfyRc_T;` (`Arc` uses `_Atomic uint64_t*`) | 16/8 |
| `Option<T>` (scalar/`str`) | `typedef struct { T val; uint8_t some; } InfyOption_T;` | C padding rules (see below) — `Option<u32>` = 8/4, `Option<str>` = 24/8 |
| `Option<T>` where `T` is a pointer (`&T`, `Box<U>`, `*const U`, fn ptr) | the pointer itself; `NULL == None` (§4.1 niche — source must not depend on this beyond `.is_some()`) | 8/8 |
| `Result<T, E>` | `typedef struct { T ok; E err; uint8_t is_ok; } InfyResult_T_E;` | C padding rules — `Result<u32, Fail>` = **20**/4, `Result<u32, u64>` = **24**/8 |
| `enum` | `typedef struct { uint32_t tag; union { ... } v; } InfyE_name;` — explicit tag, never niche-dependent (§4.1); **if no variant carries a payload the union is elided** and the enum is exactly its tag | `FailWhy` 8/4, `Phase` **4/4** |
| `struct` | C struct, fields in declaration order; `#[repr(Rust)]` default layout is **the same order** in this implementation (legal: §4.8 forbids source from depending on the default layout, and field reordering is an optimisation this backend does not perform) | sum |
| `#[repr(C)] struct` | identical, and additionally the layout exported to A-track code | sum |
| `(A, B)` tuple | C struct, elements in order, fields named `a0`, `a1`, … (deviation D-24: §4.1 fixes no tuple representation) | `(u32, bool)` 8/4, `(u64, (u32, bool))` 16/8 |
| `fn(A,B) -> R` | `R (*)(A, B)` (non-capturing only, §4.1) | 8/8 |
| closure (capturing) | `typedef struct { void* fn; void* env; } InfyCloseret_args;` | 16/8 |
| `dyn Trait` | `typedef struct { void* data; const InfyVtable_Trait* vt; } InfyDyn_Trait;` | 16/8 |

`dyn` (B-track boxed dynamic value) is `InfyDyn` — a reference-counted tagged box (§7.7.7).

**The exact rule.** The arithmetic shorthands this table used to carry are not what the backend does, so
they are gone: `Result<u32, Fail>` measures **20** (ok@0, err@4, is_ok@16) and `Result<u32, u64>` measures
**24** (ok@0 + 4 bytes of padding, err@8, is_ok@16, tail-padded to 8), while `sizeof(T)+sizeof(E)+1, aligned`
would have said 16 and 16. What is implemented is the record rule: every record is laid out by C
rules — each field starts at `align_up(offset, field.align)`, the record's alignment is the maximum field
alignment, the size is padded to that alignment — and a union is `{ max size, max align }` with every
member at offset 0. Implemented and measured in one place: `src/hir.rs::record_layout` / `union_layout`,
goldens `hir::tests::scalar_rows_of_abi_section_1`,
`hir::tests::option_and_result_rows_of_abi_section_1` and `hir::tests::tour_declarations_carry_the_measured_layouts`
(each number cross-checked against `gcc -std=c11` `sizeof`/`_Alignof`/`offsetof`, `docs/RUNLOG.md` cron-07).
`#[align(N)]` raises a record's alignment and therefore its tail padding.

### vtable layout (`IFABI-2`)

`InfyVtable_T` is emitted in this order, with no padding holes:

```
void   (*drop)(void* self);
uint64_t size;                 // sizeof of the concrete type
uint64_t align;
Trait method pointers in trait declaration order, then inherited traits in the order
the `: Base` clause lists them; generic methods make the trait not object-safe (E-0167),
so the pointer list is always monomorphic.
```

### Growth factors (§10.13 — observable through `capacity()`, therefore ABI)

`Vec`/`String`: start at 4; `×2` while the new capacity ≤ 1024; `×3/2` while ≤ 1 MiB; `×5/4`
above that. Growth never shrinks; `reserve(n)` uses the same sequence applied until `≥ n`.

## 2. Names, symbols, sections

* Every INFY item becomes a C symbol `infy_<mangled package path>_<item>`; mangled package path
  is the package path with non-alphanumerics replaced by `_`. A-track wrapper symbols are
  `infy_e_<name>_<8hex>` where `<8hex>` is the first 4 bytes of `SHA-256` of the normalised
  `extern` signature block (§7.6.3 item 2).
* Local variables keep SSA-ish numbered names (`x_17`) to make the generated C readable and
  reproducible (§7.3): the emitted C text must be byte-stable for identical inputs.
* Generic functions are monomorphised (§4.15) with the concrete type list appended as a hash;
  instantiation counts are checked against `W-0160` (>512 per function) and `E-0174` (>65536).

## 3. Allocation and drop (§4.7.4, §4.8)

* `infy_alloc` / `infy_free` wrap the platform allocator and maintain per-process counters for
  `std.rt.mem_stats()`. Every heap block carries a 16-byte header `{chain_id, must_drop flag}`.
* Address stability (§4.8): no compaction, no GC — the backend never relocates an object.
* Drop order is emitted, not inferred: each scope pushes an entry onto the focus's cleanup
  deque at the point a value with a destructor comes into scope, and pops it at scope end.
  `infy_panic` unwinds by draining that deque LIFO, which reproduces §4.7.4 exactly
  (`impl Drop` body first, then fields in declaration order, locals in reverse declaration
  order, statement temporaries at statement end). `-O` level never changes this (UB-018).
* `#[no_drop]` suppresses the cleanup entry (§4.7.4); `defer { … }` pushes a closure entry that
  runs after the scope's variable drops, in reverse declaration order (§3.13).

## 4. Panics, radius, and the `R-` codes (§4.9, §4.16)

`infy_panic(uint16_t code, const char* detail, const InfyNodeLoc*)` never returns. It renders
the §9.2 block for `R-xxxx` **including** the focus/chain/node triple (mandatory, §12.4), then
applies `panic_radius` from `.inf meta`:

* `focus` (default): `longjmp` to the current focus's try point; the scheduler drains the
  cleanup deque, marks that focus `Failed`, reports to the host, and leaves the other focuses
  and the process running.
* `chain`: as `focus`, plus every focus of that chain is marked `Failed` and the chain's
  `on_fail` linkage (when declared) runs.
* `process`: run all deferred drops, then `exit(3)` (§13.4 exit code 3).

A panic raised while unwinding (double panic) switches immediately to `process` and reports
`R-0011` (§4.16). Boundary checks are **never** compiled out: array index (`R-0004`), divide by
zero (`R-0002`), shift width (`R-0007`), integer overflow (`R-0001` or saturate per the module
profile, §4.4), `Option::unwrap` on `None` (`R-0005`), `bool` bit pattern (`R-0006`), NUL in a
`str` crossing an A-track boundary (`R-0003`), IFBC load validation (`R-0703`…`R-0708`, plus
`R-0712`/`R-0713` for `invoke.dynamic` and `field_off`), `dyn` tag mismatch (`R-0710`).

## 5. Chains, nodes, focuses (§8)

A `chain` compiles to

```
typedef struct ChainFrame_<name> { InfyLocals locals; uint32_t node_idx; ... } Frame;
static void node_<name>_<i>(Frame* f);      // one function per §8.4 node
static const InfyNodeDesc nodes_<name>[];   // file:line:col, linkage_count, host_event_id
```

`ChainFrame_<name>` exactly: the chain's `let`/`var` bindings as a record **in declaration order**, then
`node_idx: uint32_t`, then tail padding to the frame's alignment. A chain with no bindings is therefore
4/4 (just `node_idx`); `let a: u32` + `var b: u64` is 24/8 (`a`@0, `b`@8, `node_idx`@16). The runtime never
inspects the internals — it allocates `InfyChainDesc.frame_size` bytes with `frame_align` and zeroes them
(`runtime/infy_rt.c`, `infy_alloc(d->frame_size, ...)` + `memset`), so these offsets are the whole contract
between codegen and the scheduler. `InfyLocals` is that prefix, not a separate struct.
Goldens: `hir::tests::chain_frame_is_locals_then_node_idx`, `hir::tests::abi_table_rows_match_infy_rt_h`
(`InfyNodeLoc` 24/8 = 0/8/12/16/20, `InfyChainDesc` 48/8 = 0/8/12/16/20/24/32/40, `InfyFocusId3` 12/4).

Node numbering follows §8.4 to the letter: only top-level `expr_stmt`s (including `@embed`) open
a new node; `let`/`var`/plain assignment/`label` are the *preamble* of the preceding node;
`if`/`match` bodies never open a node. The table is emitted into `.inf meta` and is identical at
every `-O` level and on every target (§8.4 rule 6, §12.3 golden).

A **focus** is one independent advance position (`chain_id`, `node_idx`, its own `Frame`, its own
cleanup deque). The scheduler is a cooperative, work-item pool:

* a work item is "run one node of focus F"; at the node boundary the item returns to the pool,
  which is exactly preemption point ① of §8.5;
* preemption point ② (after every A/B-track return) is honoured by re-entering the scheduler
  from the boundary thunk; ③ `std.chain.yield_now()` sets a flag consumed at the next node
  boundary; ④ belongs to IFBC's `tickloop`; ⑤ is a real blocking wait on a `Mutex`/`Channel`;
* fairness: bounded round-robin with `focus_quota` work items per node (default 1), FIFO over
  ready focuses — never "newest first", never hash order (§8.5);
* OS threads: `min(logical_cpus, 8)`, overridable by `INFY_THREADS`; focus stacks are thread
  stacks sized by `[profile] focus_stack_bytes` (default 256 KiB), chain stacks 8 MiB default
  (§4.8) with overflow reported as `R-1040` terminating only that focus.

Documented deviation: §8.5 describes focuses as user-space coroutine stacks on a thread pool.
This backend keeps the *observable* semantics (advance position, independent stack and ownership
scope, closed preemption point set, fairness, failure radius) but realises the stack as a
thread stack, because portable C has no standard coroutine primitive. `env.focus_id`,
cancellation, `pause`/`resume` and the §8.5 matrix are unaffected.

`start(chain = X)` allocates a new focus (id monotonically increasing; on `u32` wraparound the
runtime refuses new focuses with `R-0802` rather than reuse an id, §8.5); `stop` sets the global
flag so all focuses finish the current node's plain **and** linkage actions and then stop,
dropping per §4.7.4 and running the leak check at that point (`R-0723`); `pause` suspends the
focus after the current node while retaining its frame; `resume` re-enqueues it.

`ResourceLock` (§8.3) is a process-global FIFO: `name → { mutex, condvar, queue }`;
`std.chain.lock(name)` appends, sleeps until it is at the head, and returns a `Guard` whose
cleanup entry releases and pops. No lock-order analysis is promised; `infy check --deadlock`
emits `W-0800` from a static lock-set graph built by the front end.

## 6. Artifact container (`.inf` segments, §7.4)

`infy build` produces one executable file: the host image (PE/ELF/Mach-O) followed by the INFY
segment blob and a fixed 64-byte trailer:

```
magic "INFYART2" | hdr_ver u16 | ifabi u16 | seg_count u16 | flags u16
blob_off u64 | blob_len u64
meta_off u64 | meta_len u64 | ifbc_off u64 | ifbc_len u64
sum_off  u64 | sum_len  u64 | rodata_off u64 | rodata_len u64
build_id [u8;32] | trailer_crc u32 | pad
```

Mapping of §7.4's six segments onto the file:

| §7.4 | where it lives |
| --- | --- |
| `.inf meta` | trailer + `meta` region of the blob (IFABI, target triple, `usize` width, build-id, dependency closure with dirhashes, overflow/opt/stack/panic-radius policy, entry description, `EventTable`, `ChainTable`, `NodeTable`, `ResourceKeys`, `EnvOffsets`, export table, `source_toolchain`) |
| `.inf text` | the host image's native code (INFY codegen output plus A-track objects, linked in §7.4 order: reverse dependency topological, then name bytewise) |
| `.inf rodata` | string literals and the type mirror table are in the image's native read-only data; the blob keeps a `rodata` **index** (offset + length per item) so `infy artifact sections` can report them and `std.meta` can resolve names |
| `.inf ifbc` | `ifbc` region of the blob, one IFBC module per B-track unit (§7.8.1), validated at load |
| `.inf rt` | statically linked `infy_rt` inside the image |
| `.inf sum` | `sum` region: dependency dirhash list + sumdb records (§5.8) |

The runtime finds its own path (`/proc/self/exe`, `GetModuleFileNameW`, `_NSGetExecutablePath`),
seeks the trailer from the end, and validates `trailer_crc`; a missing/duplicate trailer means the
file is not an INFY artifact. `infy run` on a `.infib` is refused with `E-0551` before anything
executes. Appending a trailing blob is legal for the PE, ELF and Mach-O loaders; this is the same
"archive-after-executable" trick used by self-extracting installers, and `--verify-reproducible`
(§7.3) rebuilds into a second directory and compares bytes.

## 7. Build identity (§7.3)

`build_id = SHA-256` over, in this order, each length-prefixed: normalised source dirhash per
package in §5.4 order, the dependency lock, the module manifest, target triple, opt level,
profile name, toolchain version strings (including the B-track interpreter version, which is why
changing Python changes the build-id), `IFABI`, and the compiler version. No timestamps, no
absolute paths, no usernames. Any hash-table iteration or symbol-suffix randomisation in the
compiler must be seeded from the `build_id`, never from system entropy.

## 8. Host interface (§11)

`runtime/infy_host.h` declares the C ABI mirror of the spec's Rust trait:

```
typedef struct InfyHostApi {
  const InfyEnvSnapshot* (*env_snapshot)(void* self);
  int8_t (*call_native_event)(void* self, uint32_t id, const InfyArgFrame* args, InfyOutVal* out);
  uint64_t (*acquire_resource)(void* self, uint32_t res, uint64_t ts);
  void (*release_resource)(void* self, uint64_t guard);
  uint32_t (*spawn_focus)(void* self, uint32_t chain, uint32_t start_node);
  void (*request_pause)(void* self, uint32_t focus);
  void (*request_stop)(void* self);
  void (*set_pressure)(void* self, float p);
} InfyHostApi;
```

`src/host.rs` exposes the spec's Rust-shaped builder
(`Builder::host(impl).entry(ChainId).build()`) on top of it. `acquire_resource` /
`release_resource` / `set_pressure` are called outside the serialising host lock (§11.1);
`call_native_event` is serialised through a `Mutex<Host>`, so hosts need not lock themselves but
must not assume any ordering beyond same-focus serialisation. Host callbacks into INFY (`#[export]`)
and host→INFY event nesting deeper than 8 levels is `R-0902`. The three-step handshake
(`IFABI`, event table presence + signature digest, `EnvSnapshot` layout) is checked at startup
and reports `R-0903` / `R-0904` / `R-0905`, and `infy check --host-signature f.json` performs the
same check at build time.

## 9. IFBC (§7.8, appendix D)

Big-endian, fixed-width `opcode: u8` + 0..3 `u32` operands; module layout
`header | type_table | import_table | func_table | code | line_map | exception_table | string_pool`
with header `magic "IFB1" | ifbc_version u16 | ifabi u16 | lang_tag u16 | module_hash[32]
| func_count u32 | frame_kind u8`.

Appendix D leaves `line_map` and `exception_table` byte formats unspecified (spec hole C-7 ①).
This implementation fixes them, and records the choice in `docs/spec-deviations.md`:

```
line_map        : count u32, then count × { code_off u32, line u32 } sorted by code_off
exception_table : count u32, then count × { start u32, end u32, handler u32, depth u16, kind u16 }
```

The loader runs §7.8.3's six validations **per function in `func_table` order, wholly before any
instruction executes**; the first failure rejects the entire module (D.10) with `R-0704`
(linearity), `R-0705` (stack height), `R-0706` (operand type), `R-0707` (frame legality — including
D.10's "boundary" row: tail-call frame size and `closure.new` outside `func_table`; deviation D-22
keeps `R-0709` for the §7.9 `max_mem_bytes` arena check alone), `R-0708` (host signature).
`infy ifbc verify --all-errors` lists every violation instead of only the first.

`frame_kind` has two levels with different value sets (deviation D-23): the header u8 is the
module-wide value model and only `0` (single-address operand stack + local window, §7.8.1) is legal,
while D.8's coroutine test is a **per-function** field in `func_table` — `0` = stack frame,
`1` = coroutine (`INFY_FRAME_STACK` / `INFY_FRAME_COROUTINE` in `infy_rt.h`, `FRAME_KIND_STACK` /
`FRAME_KIND_COROUTINE` in `src/ifbc.rs`); any other value fails the load. The spec never numbers
D.8's values, so `1` is this implementation's fixed choice.

## 10. `env` (§4.6)

`EnvSnapshot` is a fixed-layout struct; `env.x` compiles to a load at a compile-time offset
recorded in `.inf meta`'s `EnvOffsets`. The layout is `runtime/infy_rt.h`'s `InfyEnvSnapshot`, and §4.6's
`&str` entries are stored **inline as `InfyStr`** (the host publishes the whole block; a pointer into host
memory would not survive the node boundary):

| field | type | offset @64 | size/align |
| --- | --- | --- | --- |
| `iter` | `u32` | 0 | 4/4 |
| `runtime_s` | `u32` | 4 | 4/4 |
| `realtime` | `u64` | 8 | 8/8 |
| `ram_mb` | `u32` | 16 | 4/4 |
| `cpu_use` | `f32` | 20 | 4/4 |
| `gpu_use` | `f32` | 24 | 4/4 |
| `chain_id` | `u32` | 28 | 4/4 |
| `node_idx` | `u32` | 32 | 4/4 |
| `focus_id` | `u32` | 36 | 4/4 |
| `platform` | `&str` → `InfyStr` | 40 | 16/8 |
| `build_id` | `&str` → `InfyStr` | 56 | 16/8 |

Total **72/8**. Golden: `hir::tests::env_offset_table_matches_the_frozen_header` (the same 11 offsets measured
with `offsetof` in `gcc`); a host that disagrees answers `R-0905` (§11.2). The field set is closed and domain-neutral (§1.3):
`iter u32`, `runtime_s u32`, `realtime u64`, `ram_mb u32`, `cpu_use f32`, `gpu_use f32`,
`chain_id u32`, `node_idx u32`, `focus_id u32`, `platform &str`, `build_id &str`. Anything else
goes through `std.host.query`. The host refreshes the snapshot between node dispatches; writes to
`env.*` are `E-0140` at compile time.
