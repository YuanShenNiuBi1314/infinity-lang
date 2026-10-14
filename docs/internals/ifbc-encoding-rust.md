# INFY v2.0 — IFBC 的 Rust 侧字节表（`src/ifbc.rs`）

这份文件是 `runtime/IFBC.md`（C 侧那份）的 **Rust 镜像**：两边现在描述同一份线格式，这是偏差
**D-38** 裁出来的结果（裁决前两侧各自实现过一套，且从未交换过一字节，因为 `.inf ifbc` 恒空）。
规范本身（§7.8.1 + 附录 D）固定的是语义与家族编号，容器头、三张表的记录形态、`line_map` /
`exception_table` / `string_pool` 的字节都留给实现（附录 D 的 spec hole `C-7 ①`）。

这里没有"实现细节"以外的主张：每一行都由 `src/ifbc.rs` 的同一份代码产生与消费，
表本身是一次性探针（`Op::from_byte` × `arity()` × `operand_kinds()` × `stack_effect()`）打出来的，
不是手抄。改代码请同时改这张表，`ifbc::tests` 里有逐字节的布局断言会替你把关。

## 1. 容器：各区自定界（D-38 (a)）

```text
header | type_table | import_table | func_table | code | line_map | exception_table | string_pool
```

顺序固定（§7.8.1）。**没有外层 `byte_len` 包装**——本模块以前给每个区加
`byte_len:u32 || body`，理由是"不包就分不出 `code` 与 `line_map`"，该理由不成立：`code` 自己的
前导 `u32` 就是它的长度，C 侧从来这么扫。整数全部大端（D.1）。

### 1.1 header（47 字节）

| 偏移 | 字段 | 宽度 | 约束 |
|---|---|---|---|
| 0 | `magic` = `"IFB1"` | 4 | 等于 `INFY_IFBC_MAGIC` |
| 4 | `ifbc_version` | u16 | 本工具链只认 1 |
| 6 | `ifabi` | u16 | 必须 = 2（IFABI-2）；行的 `ifabi` 字段已删，模块级只有这一处 |
| 8 | `lang_tag` | u16 | `(lang_id << 8) | format_id`，两侧封闭表见 `ifbc::{LANGS,FORMATS}` |
| 10 | `module_hash` | 32 | SHA-256 over §1.9 的规范化区体 |
| 42 | `func_count` | u32 | 必须等于 `func_table` 行数 |
| 46 | `frame_kind` | u8 | **模块级值模型**，只允许 0；协程标记在 `func_table`（D-23） |

## 1.2 `type_table`（合并形态）

```text
u32 n_types                       # 上限 2^20（与 C 的装载器同宽）
n_types × {
   u32 name_str
   u32 kind                        # §1.3 的统一 kind 空间
   u32 flags                       # bit0 = #[repr(C)]，field_off 的判据
   u32 size
   u32 align
   u32 nfields                     # 上限 2^20
   u32 payload_a                   # C 侧忽略：list/tuple 元素、dict 键
   u32 payload_b                   # C 侧忽略：dict 值 / 泛型实参
   nfields × { u32 name_str, u32 off, u32 ty }
}
```

`payload_a/b` 收 Rust 侧原有的容器元素信息（D-38：C 忽略它们，不是新语义）。
`size`/`align` 由生产者填：**自动置的标量行写 0**，因为没有任何规范位置给出
`sizeof(InfyDyn)` 之类的数，编一个上去比留空更糟；要用 `size_of.idx`/`align_of.idx`/`field_off`
的生产者必须自己填真布局（`src/hir.rs` 的布局引擎那份，由 `gcc` 探针校准）。

## 1.3 kind 空间 = IFABI 标量码（D-38 (a)）

`0..=17` 就是 `runtime/infy_ifbc.c` 的 `SIG_*` 枚举（也就是 `IFBC.md` §2 那张表），
`18..` 是扩展的静态类别。**构建器自动把前 18 行置为标量行**，于是
"类型索引 `0..=17` == 标量码"恒成立——这正是让 `import_table` 的 `u8` 签名码和
`type_table` 的 `u32` 索引能指同一空间的开关（`ifbc::N_SCALAR_ROWS = 18`）。

| kind | 名称 | kind | 名称 | kind | 名称 |
|---|---|---|---|---|---|
| 0 | void | 9 | u16 | 18 | list |
| 1 | unit | 10 | u32 | 19 | tuple |
| 2 | bool | 11 | u64 | 20 | dict |
| 3 | char | 12 | f32 | 21 | func |
| 4 | i8 | 13 | f64 | 22 | coro |
| 5 | i16 | 14 | str | 23 | instance |
| 6 | i32 | 15 | bytes | 24 | ref |
| 7 | i64 | 16 | ptr | 25 | box |
| 8 | u8 | 17 | dyn | 26 | rc |
| | | | | 27 | arc |
| | | | | 28 | option |
| | | | | 29 | result |
| | | | | 30 | never |
| | | | | 31 | usize |
| | | | | 32 | isize |

签名码折叠：`18..` 的类别在 IFABI 线上就是一个装箱值，所以 `ret_sig`/`param_sig` 写 `dyn`(17)
（`TyKind::sig_code()`）。`InfyDynTag` 的对应关系自 D-38 重编号后不再是"数值相等"，改由
`TyKind::dyn_tag()` 这张显式表给出（`unit→DY_NONE 0`、`i64→DY_INT 1`、…、`dyn→DY_INSTANCE 11`）。

## 1.4 `import_table`（C 形状 + 尾部摘要）

```text
u32 n_imports                     # 上限 2^20
n_imports × {
   u32 name_str
   u8  kind                       # 0=host 1=embed 2=event
   u8  nparams
   u8  ret_sig                    # §1.3 的标量码
   u8  flags                      # bit0 = IFABI-signed（编译器核对 thunk 后盖）
   u32 sig_hash                   # 校验项 5 的快速比对值
   nparams × u8 param_sig         # 标量码
}
```

Rust 原来的 `ImportKind::{PyModule, Infib, Entry}` 折叠掉：源语言在 header 的 `lang_tag` 里，
入口点在 `func_table.flags` 的 `F_ENTRY` 里，两者都不需要 import 行。每行原来的 `ifabi u16`
也删了——模块级只有 header 那一份。签名文本（`(dyn, dyn) -> dyn`）的 SHA-256 前 4 字节是
`sig_hash`，文本本身仍是真相，可由 `name_str` + 标量码重建。

## 1.5 `func_table`（48 字节/条）

```text
u32 n_funcs                       # 必须 == header.func_count
u64 max_mem_bytes                 # §7.9 单元级配额；0 = 不限（Rust 侧新增的模块字段）
n_funcs × {                       # 12 个 u32
   u32 code_off                   u32 code_len
   u32 locals                     u32 max_stack     # 声明的操作数栈上限
   u32 nargs                      u32 ret_arity     # 0=unit 1=标量 n=元组
   u32 ret_sig                   u32 name_str
   u32 sig_str                   u32 sig_hash
   u32 flags                     u32 frame_kind     # 0=stack 1=coroutine（D-23）
}
```

`max_stack` 由 `CodeBuilder` 在发射时跟踪运行栈高的**峰值**得到：`call.*` 只按 opcode 隐含效果
计入（构建器不知道被调元数），所以这是一个**保守上界**——永不低于装载器自己高度分析出的数，
因此不会造成假失败。C 侧读的是它自己那份 10 个字段的**超集**，D-38 之后两侧同形。

## 1.6 / 1.7 / 1.8 `line_map`、`exception_table`、`string_pool`

两侧本就逐字节一致，未做合并：

```text
line_map:        u32 count; count × { u32 code_off, u32 line }        # 按 code_off 升序
exception_table: u32 count; count × { u32 start, u32 end, u32 handler, u16 depth, u16 kind }
string_pool:     u32 n_strings; n_strings × { u32 len; len × byte }   # 不要求 NUL
```

## 2. 指令编码（D-38 (b)：家族字节 + 操作数 0 = 类型索引）

一条指令 = `opcode:u8` + 其操作码隐含的 `u32` 操作数，长度 `1 + 4*nops`；
唯一的例外是 `switch`：长度 `1 + 4*(1+n)`，`n` 个目标**内联在指令里**（D-39 把
"固定宽度"读作"长度静态可算"，单遍前向扫描这个规范性理由不被破坏）。
函数入口偏移与 code 区末端 4 字节对齐。

`0x20..0x36` 是 D.3 的**家族**字节。D-38 之前 C 侧在这里放的是"具体 `(op,type)` 字节表"
（`add.i64`…`cmpeq.f64`），两侧整块错位；现在 C 侧向家族形态对齐：exec 按
`types[operand0].kind` 选 i64/f64 路径，其它 kind → `R-0706`。

下表由 `src/ifbc.rs` 的 `Op::from_byte / arity / operand_kinds / stack_effect` 打印。
"操作数"列里的 `ty@0` 表示 D.3 家族的类型在操作数 0，反汇编把它打印成助记符后缀
（`add.i64`），因此不重复出现在操作数列表里。`new.list`/`new.dict`/`closure.new`/`call.*`
的栈效果由表决定，见"表定"注。

| byte | 助记符 | nops | 操作数 | 栈效果 (pop, push) |
|---|---|---|---|---|
| 0x00 | `nop` | 0 | — | (0, 0) |
| 0x01 | `ldl i` | 1 | slot | (0, 1) |
| 0x02 | `stl i` | 1 | slot | (1, 0) |
| 0x03 | `ldw i,ty` | 2 | slot, ty | (0, 1) |
| 0x04 | `push k,ty` | 2 | pool, ty | (0, 1) |
| 0x05 | `push.b k` | 1 | imm | (0, 1) |
| 0x06 | `dup` | 0 | — | (1, 2) |
| 0x07 | `pop` | 0 | — | (1, 0) |
| 0x08 | `ldnull` | 0 | — | 禁用：装载期 `R-0706` |
| 0x09 | `ld.frame` | 0 | — | (0, 0) |
| 0x0A | `dup2` | 0 | — | (2, 4) |
| 0x0B | `swap` | 0 | — | (2, 2) |
| 0x0C/0x0D | *(空)* | | D-38 (c)：这两个字节曾是 C 的 `guard.*`，现已归位 0x27/0x2F | |
| 0x0E | `ldg` | 1 | slot | (0, 1) |
| 0x0F | `stg` | 1 | slot | (1, 0) |
| 0x20 | `add` | 1 | ty@0 | (2, 1) |
| 0x21 | `sub` | 1 | ty@0 | (2, 1) |
| 0x22 | `mul` | 1 | ty@0 | (2, 1) |
| 0x23 | `div` | 1 | ty@0 | (2, 1) 需紧邻 `guard.nonzero` |
| 0x24 | `rem` | 1 | ty@0 | (2, 1) 需紧邻 `guard.nonzero` |
| 0x25 | `neg` | 1 | ty@0 | (1, 1) |
| 0x26 | `abs` | 1 | ty@0 | (1, 1) |
| 0x27 | `guard.nonzero` | **0** | — | (0, 0) |
| 0x28 | `and` | 1 | ty@0 | (2, 1) |
| 0x29 | `or` | 1 | ty@0 | (2, 1) |
| 0x2A | `xor` | 1 | ty@0 | (2, 1) |
| 0x2B | `not` | 1 | ty@0 | (1, 1)（`bool` 合法） |
| 0x2C | `shl` | 1 | ty@0 | (2, 1) 需紧邻 `guard.shift` |
| 0x2D | `shr` | 1 | ty@0 | (2, 1) 需紧邻 `guard.shift` |
| 0x2E | `shr.a` | 1 | ty@0 | (2, 1) 需紧邻 `guard.shift` |
| 0x2F | `guard.shift` | **0** | — | (0, 0) |
| 0x30 | `cmplt` | 1 | ty@0 | (2, 1) 推 bool |
| 0x31 | `cmple` | 1 | ty@0 | (2, 1) |
| 0x32 | `cmpgt` | 1 | ty@0 | (2, 1) |
| 0x33 | `cmpge` | 1 | ty@0 | (2, 1) |
| 0x34 | `cmpeq` | 1 | ty@0 | (2, 1) |
| 0x35 | `cmpne` | 1 | ty@0 | (2, 1) |
| 0x36 | `cmp` | 1 | ty@0 | (2, 1) 三态 |
| 0x40 | `jmp off` | 1 | code | (0, 0) 终止块 |
| 0x41 | `br off` | 1 | code | (1, 0) |
| 0x42 | `brf off` | 1 | code | (1, 0) |
| 0x43 | `switch n` | 1(+n) | imm + 内联 n×code | (1, 0) |
| 0x44 | `tickloop n` | 1 | imm | (0, 0) |
| 0x45 | `leave` | 0 | — | (0, 0) |
| 0x46 | `throw.inf k` | 1 | pool | (1, 0) 终止 |
| 0x47 | `enter.try h,c` | 2 | code, code | (0, 0) |
| 0x60 | `call.stat f` | 1 | func | 表定：pop `nargs`，push 1（`ret_arity==0` 时 0） |
| 0x61 | `call.virt idx` | 1 | ty | 本侧拒绝（见 §4） |
| 0x62 | `call.host idx` | 1 | import | 表定：pop `nparams`，push 1（`ret_sig==unit/void` 时 0） |
| 0x63 | `call.embed idx` | 1 | import | 本侧拒绝（见 §4），但行的 kind 仍在校验项 5 里核对 |
| 0x64 | `ret` | 0 | — | (1, 0) 终止 |
| 0x65 | `ret.multi n` | 1 | imm | (n, 0) 终止，且 `n == ret_arity` |
| 0x66 | `closure.new ci,capn` | 2 | func, imm | 表定：pop `capn`，push 1 |
| 0x67 | `tailcall.stat f` | 1 | func | 表定：pop `nargs`；帧尺寸五项与调用者相同，否则 `R-0707`（D-22） |
| 0x68 | `import idx` | 1 | import | (0, 0) |
| 0x69 | `import.attr k` | 1 | pool | (1, 1) |
| 0x80 | `new.str k` | 1 | pool | (0, 1) |
| 0x81 | `new.list ty,count` | 2 | ty, imm | 表定：pop `count`，push 1 |
| 0x82 | `new.dict kt,vt,count` | 3 | ty, ty, imm | 表定：pop `2*count`，push 1 |
| 0x83 | `new.frame f` | 1 | func | (0, 1) |
| 0x84 | `seq.push` | 1 | ty | (2, 1) |
| 0x85 | `seq.len` | 1 | ty | (0, 1) |
| 0x86 | `seq.next` | 1 | ty | (1, 2) |
| 0x87 | `seq.iter` | 1 | ty | (1, 1) |
| 0x88 | `idx.get` | 1 | ty | (2, 1) |
| 0x89 | `idx.set` | 1 | ty | (3, 0) |
| 0x8A | `slice` | 1 | ty | (3, 1) |
| 0x8B | `inst.get k` | 1 | pool | (1, 1) |
| 0x8C | `inst.set k` | 1 | pool | (2, 0) |
| 0x8D | `map.insert` | 1 | ty | (3, 1) |
| 0x8E | `str.concat` | 1 | ty | (2, 1) |
| 0x8F | `seq.concat` | 1 | ty | (2, 1) |
| 0x90 | `ref.inc` | 0 | — | (0, 0) |
| 0x91 | `ref.dec` | 0 | — | (0, 0) |
| 0x92 | `new.tuple n` | 1 | imm | 表定：pop `n`，push 1 |
| 0x93 | `new.set ty` | 1 | ty | 表定：pop 操作数 0，push 1（**已知缺陷**，见 §4） |
| 0x94 | `move.out i` | 1 | slot | (1, 0) |
| 0xA0 | `tag.get` | 0 | — | (1, 1) |
| 0xA1 | `cast.dyn T` | 1 | ty | (1, 1) 需先前有 tag 检查点，否则 `R-0710` |
| 0xA2 | `is.dyn T` | 1 | ty | (1, 1) |
| 0xA3 | `box.dyn` | **0** | — | 恒等（D-38 (f)） |
| 0xA4 | `unbox.dyn` | 0 | — | 需紧邻 `is.dyn`，否则 `R-0710` |
| 0xA5 | `dyn.truthy` | 0 | — | (0, 1) |
| 0xA6 | `dyn.identity` | 0 | — | (2, 1) |
| 0xA8 | `invoke.dynamic f` | 1 | imm | 本侧拒绝（见 §4）；目标越界 `R-0712` |
| 0xC0 | `frame.enter` | 0 | — | 协程帧专属，否则 `R-0707` |
| 0xC1 | `yield` | 0 | — | 协程帧专属 |
| 0xC2 | `yield.from` | 0 | — | 协程帧专属 |
| 0xC3 | `frame.exit` | 0 | — | 协程帧专属，终止 |
| 0xC4 | `coro.resume` | 1 | imm | (0, 0)（与 C 的 `sim_delta = -1` 不同，见 §4） |
| 0xC5 | `coro.throw` | 1 | imm | (0, 0)（同上） |
| 0xE0 | `typeof.idx T` | 1 | ty | (0, 1) |
| 0xE1 | `size_of.idx T` | 1 | ty | (0, 1) |
| 0xE2 | `align_of.idx T` | 1 | ty | (0, 1) |
| 0xE3 | `field_off idx,fld` | 2 | ty, imm | (0, 1)；非 `repr(C)` → `R-0713`，字段越界 → `R-0706` |

未列出的字节没有条目：装载期是未知 opcode。

## 3. 附录 D 的分组/漏项决定（`ifbc::GROUPING` 的镜像）

* `D.2 0x06` — dup / dup2 / swap → 0x06 dup, 0x0A dup2, 0x0B swap
* `D.2 0x0A..0x1F` — 空档 → 0x0E ldg, 0x0F stg；0x0C/0x0D 在 `guard.*` 归位 0x27/0x2F 后空出
* `D.2 0x03` — `ldw i,ty` 2 操作数；操作数 1 是静态类型注解，执行器不读（D-38 (e)）
* `D.2 0x04` — `push k,ty` 2 操作数：池常量必须带类型（D-38 (d) / DEV-IFBC-13）
* `D.3 all rows` — 家族字节 + 操作数 0 = `type_table` 索引，不做每个 `(op,type)` 一字节（D-38 (b) / DEV-IFBC-1）
* `D.3 0x27, 0x2F` — `guard.nonzero`/`guard.shift` 0 操作数：D.3 的邻接校验只看前一字节（D-38 (c)）
* `D.4 0x43` — `switch n` + 内联 `n×u32`，长度 `1+4(1+n)`（D-38 (h) / D-39）
* `D.5 0x68, 0x69` — `import`, `import.attr`：DEV-IFBC-6，D-38 (i) 转正
* `D.6 0x81, 0x82` — `new.list elem_ty,count` / `new.dict k_ty,v_ty,count`：D.6 只点了类型操作数，
  元素个数是"栈形态可判定"的必然操作数（D-38 补充裁定；与 D-39 的 `field_off` 同一类漏项）
* `D.6 0x83` — `new.frame f`：D.6 没写操作数，但帧记录必须指名协程函数（C 侧本就带 `f`）
* `D.6 0x84..0x87` — `seq.push/len/next/iter`：DEV-IFBC-7
* `D.6 0x8B..0x8F` — `inst.get/inst.set/map.insert/str.concat/seq.concat`：DEV-IFBC-7
* `D.6 0x92, 0x93` — `new.tuple`, `new.set`：DEV-IFBC-7
* `D.7 0xA3` — `box.dyn` 0 操作数（恒等）；`D.7 0xA5/0xA6` = `dyn.truthy`/`dyn.identity`：DEV-IFBC-7
* `D.9 0xE0/0xE3` — `typeof.idx T`、`field_off idx,fld`：D.9 的助记符列漏了校验/执行必需的操作数
  （D-38 补充裁定 / D-39）

## 4. 这一侧还没做到的（不用假成功掩盖）

* `call.virt` / `call.embed` / `invoke.dynamic`：本侧的装载期校验直接判 `R-0705`
  （"被调元数无法静态确定，校验项 2 无法完成"），而 C 侧会校验并执行它们；
  `coro.resume`/`coro.throw`/`throw.inf` 的栈效果表与 C 的 `sim_delta` 也不同。
  D4 的第 (3) 步只交叉验证**反汇编**，所以这不挡路，但登记为 BACKLOG **F26**。
* `new.set ty` 没有元素个数的落点：`effect()` 把类型索引当作弹出个数用 —— 真实缺陷，
  登记为 BACKLOG **F25**（本轮不改语义，只在 D-38 补充裁定里点名）。
* `ResourceKeys`/`EnvOffsets` 之类是产物容器的事（F17），不在本文件。
* 间隙指令（`ldg/stg/import*/seq.*/inst.*/map.insert/*concat/new.tuple/new.set/dyn.truthy/dyn.identity`）
  在 C 侧还没有实现，是 D4 余项 (2) 的内容。

## 5. 确定性（§7.3）

字符串池按**首次出现**序号化；编码器走 `BTreeMap`；`module_hash` 是 §1 里 header 之后全部区体
（含各区自定界前导）的 SHA-256，`seal()` 盖章、`decode()` 用镜像自身重算并拒绝不符。
`infy ifbc dump` 的文本不含地址/时间戳/哈希序，同一镜像两次打印逐字节相同
（`ifbc::tests::disassembly_is_byte_stable`）。
