//! The normative diagnostic code table (§9.1 interval allocation, §9.3 code set, §9.7 code
//! completion, plus the §9.3 "废止但不复用" note).
//!
//! Provenance of every field:
//!
//! * `title` / `note` / `help` are the 标题 / 触发场景 / 修复建议 columns of the §9.3 `E-` table and
//!   the five §9.7 tables, copied verbatim (markdown `**` emphasis markers stripped, CJK text and
//!   `§` references kept).
//! * The §9.3 `W-` list is inline prose with a single phrase per code and the §9.3 `R-` table has
//!   only a 含义 column, so for those 39 codes `title` is that phrase/含义 verbatim while `note` /
//!   `help` are condensed from the clause that actually defines the behaviour (§2.1, §4.9 UB list,
//!   §7.8.3, §11 …). `help` is left empty where the spec offers no repair.
//! * `E-0093` is defined only in §9.6 prose (it is missing from both normative tables);
//!   `E-0701` / `R-0702` come from the §9.3 reservation note and are kept with a `（废止）` title so
//!   their numbers stay reserved.
//! * `clause` is the section that defines the behaviour; `group` is the §9.1 interval topic, which
//!   is what `infy conformance --category` filters on.

use crate::diag::Level;

/// One row of the normative diagnostic table.
#[derive(Clone, Copy, Debug)]
pub struct CodeInfo {
    /// `E-0111`, `W-0801`, `R-0704`, `N-0800`, …
    pub code: &'static str,
    pub level: Level,
    /// Headline printed after `level[code]: ` (§9.2).
    pub title: &'static str,
    /// Default `= note:` line from the table's "触发场景" column.
    pub note: &'static str,
    /// Default `= help:` line from the table's "修复建议" column.
    pub help: &'static str,
    /// Spec clause that defines the code, e.g. `§4.2`.
    pub clause: &'static str,
    /// §9.1 interval topic, used by `infy conformance --category`.
    pub group: &'static str,
}

const LV_E: Level = Level::Error;
const LV_W: Level = Level::Warn;
const LV_R: Level = Level::Runtime;
const LV_N: Level = Level::Notice;

// §9.1 区间分配表 topics, verbatim (the trailing `（§x.y）` range is what `clause` carries instead).
const G_LEX: &str = "词法基础：编码、注释、标识符、关键字"; // 000x–003x
const G_LIT: &str = "字面量、字符串、Unicode 安全"; // 004x–005x
const G_CST: &str = "编译期常量求值 `const-eval`"; // 006x
const G_PAT: &str = "模式与穷尽性"; // 007x
const G_ORD: &str = "求值顺序"; // 008x
const G_FMT: &str = "`infy fmt` 自身故障"; // 009x
const G_TYOP: &str = "类型与运算符"; // 010x–011x
const G_MUT: &str = "可变性"; // 012x
const G_INTR: &str = "内置函数 / env / 字符串"; // 013x–015x
const G_INFER: &str = "类型推断与 trait 求解"; // 016x
const G_PROP: &str = "错误传播 `?`、单态化、`defer`"; // 017x
const G_ITER: &str = "格式化与迭代器"; // 018x
const G_CHAIN: &str = "语句与连携"; // 030x
const G_MAC: &str = "宏与属性"; // 040x–041x
const G_DRV: &str = "derive 生成失败"; // 042x
const G_MOD: &str = "模块/包/依赖/`.infib`/校验"; // 05xx
const G_BLD: &str = "构建/链接/工具链/可复现"; // 06xx
const G_EMB: &str = "嵌入：通用 070x，B 轨 071x，A 轨 072x，IFBC 073x"; // 07xx
const G_SCH: &str = "链、节点表、焦点调度"; // 08xx
const G_HOST: &str = "宿主事件、`HostApi`、握手"; // 09xx
const G_OWN: &str = "所有权/借用/生命周期/drop"; // 100x（101x–103x 未分配，就近落此）
const G_MEM: &str = "内存布局与栈"; // 104x

pub static ALL: &[CodeInfo] = &[
    // ---------------------------------------------------------------- §9.3 `E-` 主表 + §9.7 补全
    CodeInfo {
        code: "E-0001",
        level: LV_E,
        title: "语言版本不兼容",
        note: "`#![infy_version = \"1.0\"]` 在 v2.0 编译器上",
        help: "升级源码声明或删除该行",
        clause: "§1.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0002",
        level: LV_E,
        title: "源码非 UTF-8",
        note: "非法字节序列",
        help: "转为 UTF-8",
        clause: "§2.1",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0003",
        level: LV_E,
        title: "CR 换行",
        note: "Mac 老式 CR",
        help: "改为 LF（`infy fmt` 自动修复）",
        clause: "§2.1",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0004",
        level: LV_E,
        title: "未闭合块注释",
        note: "`/*` 无匹配 `*/`",
        help: "补全闭合",
        clause: "§2.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0005",
        level: LV_E,
        title: "注释嵌套过深",
        note: ">255 层",
        help: "简化注释",
        clause: "§2.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0006",
        level: LV_E,
        title: "混淆标识符",
        note: "NFKC 归一冲突",
        help: "更换标识符",
        clause: "§2.10",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0007",
        level: LV_E,
        title: "保留前缀标识符",
        note: "用户定义 `__foo`",
        help: "删除前置 `__`",
        clause: "§2.3",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0008",
        level: LV_E,
        title: "歧义整数字面量",
        note: "未加后缀且无法推断",
        help: "加后缀 `123u32`",
        clause: "§2.6",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0009",
        level: LV_E,
        title: "未闭合字符串",
        note: "缺少 `\"`",
        help: "补全",
        clause: "§2.9",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0010",
        level: LV_E,
        title: "未闭合原字符串",
        note: "`r#\"` 定界不匹配",
        help: "增加 `#` 数量",
        clause: "§2.9",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0011",
        level: LV_E,
        title: "构建约束位置错误",
        note: "`//infy:build` 不在首个非空行",
        help: "上移到首行",
        clause: "§2.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0012",
        level: LV_E,
        title: "未知构建约束名",
        note: "`//infy:build riscv`",
        help: "用 §2.2 封闭集或在 `[features]` 声明",
        clause: "§2.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0013",
        level: LV_E,
        title: "重复构建约束注释",
        note: "同一文件出现两条 `//infy:build`",
        help: "合并为一条布尔表达式",
        clause: "§2.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0020",
        level: LV_E,
        title: "保留字作标识符",
        note: "`let use = 1;`、`fn mod() {}`",
        help: "改名（§2.4 保留集）",
        clause: "§2.4",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0021",
        level: LV_E,
        title: "`let mut` 非法",
        note: "`let mut x = 1;`",
        help: "改 `var x = 1;`（§4.3）",
        clause: "§4.3",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0022",
        level: LV_E,
        title: "`var` 用于形参",
        note: "`fn f(var x: u32)`",
        help: "改 `fn f(mut x: u32)`（§4.3）",
        clause: "§4.3",
        group: G_LEX,
    },
    CodeInfo {
        code: "E-0030",
        level: LV_E,
        title: "`auto` 出现在非法位置",
        note: "`let x = auto;`、`return auto;`",
        help: "只用于具名实参（§2.6）",
        clause: "§2.6",
        group: G_LEX,
    },
    // ---- 004x–005x：字面量、字符串、Unicode 安全（§9.7 词法与编译期求值）
    CodeInfo {
        code: "E-0040",
        level: LV_E,
        title: "非法数字分隔符",
        note: "`_` 连用 / 首尾 `_`",
        help: "移除多余分隔符",
        clause: "§2.8",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0041",
        level: LV_E,
        title: "分隔符位置非法",
        note: "`_` 贴近 `.` 或 `e`",
        help: "移到数字之间",
        clause: "§2.8",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0042",
        level: LV_E,
        title: "字面量超出可表示范围",
        note: "无后缀整型超 `i128`",
        help: "加后缀或改常量",
        clause: "§2.8",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0043",
        level: LV_E,
        title: "带后缀字面量溢出",
        note: "`129i8`、`300u8`",
        help: "换更宽后缀（不回绕、不饱和）",
        clause: "§2.8",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0044",
        level: LV_E,
        title: "浮点字面量写法非法",
        note: "`1.`",
        help: "写 `1.0`",
        clause: "§2.8",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0045",
        level: LV_E,
        title: "浮点字面量为无穷",
        note: "`1.0e999f32`",
        help: "用 `std.f32.INF` 显式表达",
        clause: "§2.8",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0046",
        level: LV_E,
        title: "未知转义序列",
        note: "`\\\\q`",
        help: "查 §2.9 封闭表",
        clause: "§2.9",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0047",
        level: LV_E,
        title: "`str_lit` 内使用字节转义",
        note: "`\"\\\\x41\"`",
        help: "改 `b\"\\\\x41\"` 或 `\\\\u{41}`",
        clause: "§2.9",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0048",
        level: LV_E,
        title: "多行 raw 缩进混用制表符与空格",
        note: "公共前缀含两类",
        help: "统一缩进字符",
        clause: "§2.9",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0049",
        level: LV_E,
        title: "`char_lit` 含图符簇",
        note: "`'e\\\\u{301}'`",
        help: "改用 `str`",
        clause: "§2.9",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0050",
        level: LV_E,
        title: "混淆标识符（confusables）",
        note: "西里尔 `а` 混拉丁 `a`",
        help: "更换字符",
        clause: "§2.10",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0051",
        level: LV_E,
        title: "脚本混用非法",
        note: "Han 与谚文同标识符",
        help: "拆分命名",
        clause: "§2.10",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0052",
        level: LV_E,
        title: "零宽/方向控制字符",
        note: "`\\\\u{200B}` 出现在源码",
        help: "删除；确需则放字符串字面量",
        clause: "§2.10",
        group: G_LIT,
    },
    CodeInfo {
        code: "E-0053",
        level: LV_E,
        title: "语句缺少终结符",
        note: "非块/非尾表达式却无 `;`",
        help: "补 `;`（无自动插入，§2.11）",
        clause: "§2.11",
        group: G_LIT,
    },
    // ---- 006x：const-eval
    CodeInfo {
        code: "E-0060",
        level: LV_E,
        title: "编译期求值遇禁止构造",
        note: "const 内 IO / `env.*` / 堆分配",
        help: "移出 const 上下文（§2.12）",
        clause: "§2.12",
        group: G_CST,
    },
    CodeInfo {
        code: "E-0061",
        level: LV_E,
        title: "const-eval 深度超限",
        note: ">512 层调用",
        help: "拆分常量表达式",
        clause: "§2.12",
        group: G_CST,
    },
    CodeInfo {
        code: "E-0062",
        level: LV_E,
        title: "编译期常量溢出",
        note: "`const` 算术越界",
        help: "显式 `wrapping_*` 或改值",
        clause: "§2.12",
        group: G_CST,
    },
    CodeInfo {
        code: "E-0063",
        level: LV_E,
        title: "`const_assert` 失败",
        note: "条件为假",
        help: "修正断言或值；可 `[lints] allow`",
        clause: "§3.13",
        group: G_CST,
    },
    // ---- 007x：模式与穷尽性
    CodeInfo {
        code: "E-0070",
        level: LV_E,
        title: "`match` 不穷尽",
        note: "存在未覆盖值",
        help: "补臂或用 `_`（枚举建议列全）",
        clause: "§3.12",
        group: G_PAT,
    },
    CodeInfo {
        code: "E-0071",
        level: LV_E,
        title: "不可达匹配臂",
        note: "与前面各臂差集为空",
        help: "删除该臂",
        clause: "§3.12",
        group: G_PAT,
    },
    CodeInfo {
        code: "E-0072",
        level: LV_E,
        title: "或模式绑定集不一致",
        note: "`A(x) \\\\| B(_)`",
        help: "两侧同名同类型",
        clause: "§3.12",
        group: G_PAT,
    },
    CodeInfo {
        code: "E-0073",
        level: LV_E,
        title: "模式嵌套过深",
        note: ">32 层",
        help: "拆分为嵌套 `match`",
        clause: "§3.12",
        group: G_PAT,
    },
    CodeInfo {
        code: "E-0074",
        level: LV_E,
        title: "`defer` 内非法控制流",
        note: "`defer { return; }`、`defer` 内 `?`",
        help: "移到普通语句",
        clause: "§3.13",
        group: G_PAT,
    },
    // ---- 008x / 009x
    CodeInfo {
        code: "E-0080",
        level: LV_E,
        title: "求值顺序无法确定",
        note: "两个副作用无序列点且写重叠",
        help: "拆成语句（§4.10 第 10 条）",
        clause: "§4.10",
        group: G_ORD,
    },
    CodeInfo {
        code: "E-0090",
        level: LV_E,
        title: "空源码文件",
        note: "0 字节的 `.infy`",
        help: "删除该文件或写入 `package`",
        clause: "§9.5",
        group: G_FMT,
    },
    CodeInfo {
        code: "E-0091",
        level: LV_E,
        title: "`infy fmt` 不幂等",
        note: "内部二次格式化不一致",
        help: "编译器 bug，拒绝写出",
        clause: "§9.5",
        group: G_FMT,
    },
    CodeInfo {
        code: "E-0092",
        level: LV_E,
        title: "`infy fmt` 改变 AST",
        note: "格式化前后不同构",
        help: "编译器 bug，保持原文件",
        clause: "§9.5",
        group: G_FMT,
    },
    // E-0093 is defined only in §9.6 prose; §9.3/§9.7 never tabulate it.
    CodeInfo {
        code: "E-0093",
        level: LV_E,
        title: "`[lints]` 试图降级 `E-`/`R-`",
        note: "只有 `W-` 与 `N-` 可被调整级别；实现若接受降级 `E-`/`R-` 的配置即 UB（`E-0063` 是规范明示的唯一例外）",
        help: "删除该覆盖项；调整只在主 module 生效，不传递给依赖",
        clause: "§9.6",
        group: G_FMT,
    },
    // ---- 010x–011x：类型与运算符
    CodeInfo {
        code: "E-0101",
        level: LV_E,
        title: "未初始化变量读取",
        note: "读取未定型路径上的绑定",
        help: "初始化",
        clause: "§4.1",
        group: G_TYOP,
    },
    CodeInfo {
        code: "E-0110",
        level: LV_E,
        title: "隐式数字转换",
        note: "`u32 + u8`",
        help: "显式 `as`",
        clause: "§4.2",
        group: G_TYOP,
    },
    CodeInfo {
        code: "E-0111",
        level: LV_E,
        title: "隐式浮点/整数互转",
        note: "同上",
        help: "显式 `roundN` / `as`",
        clause: "§4.2",
        group: G_TYOP,
    },
    // ---- 012x：可变性
    CodeInfo {
        code: "E-0120",
        level: LV_E,
        title: "修改 `let` 不可变绑定",
        note: "`let x = 1; x = 2;`",
        help: "改 `var`",
        clause: "§4.3",
        group: G_MUT,
    },
    CodeInfo {
        code: "E-0121",
        level: LV_E,
        title: "跨链可变共享无同步",
        note: "多焦点写同一绑定",
        help: "`Arc<Mutex<T>>` 或 `Channel<T>`",
        clause: "§4.3",
        group: G_MUT,
    },
    // ---- 013x–015x：内置函数 / env / 字符串
    CodeInfo {
        code: "E-0130",
        level: LV_E,
        title: "`roundN` 的 N<10",
        note: "`round3(x)`",
        help: "用 ≥10 的 N",
        clause: "§4.5",
        group: G_INTR,
    },
    CodeInfo {
        code: "E-0131",
        level: LV_E,
        title: "内置函数无法常量求值",
        note: "`std.mem.size_of<dyn Unknown>()`",
        help: "给定具体类型",
        clause: "§4.5",
        group: G_INTR,
    },
    CodeInfo {
        code: "E-0140",
        level: LV_E,
        title: "写入 `env` 只读变量",
        note: "`env.gen = 5`",
        help: "禁止写入",
        clause: "§4.6",
        group: G_INTR,
    },
    CodeInfo {
        code: "E-0150",
        level: LV_E,
        title: "字符串含 NUL 注入",
        note: "`\"a\\0b\"` 跨 A 轨边界",
        help: "`std.str.strip_nul` / `to_cstr()`",
        clause: "§7.6.4",
        group: G_INTR,
    },
    // ---- 016x：类型推断与 trait 求解
    CodeInfo {
        code: "E-0160",
        level: LV_E,
        title: "推断展开深度超限",
        note: "类型变量互推 >8 层",
        help: "显式标注其一",
        clause: "§4.13",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0161",
        level: LV_E,
        title: "闭包形参不可推断",
        note: "存储后再调用",
        help: "写 `\\|x: u32\\|`",
        clause: "§4.13",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0162",
        level: LV_E,
        title: "方法名歧义",
        note: "同层两个 trait 提供同名方法",
        help: "`Trait::f(x)` 消歧",
        clause: "§4.14",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0163",
        level: LV_E,
        title: "关联类型缺失",
        note: "`impl` 未给非 default 项",
        help: "补 `type X = ..`",
        clause: "§4.14",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0164",
        level: LV_E,
        title: "关联类型重复提供",
        note: "同一 `impl` 内两次",
        help: "删其一",
        clause: "§4.14",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0165",
        level: LV_E,
        title: "trait bound 求解过深",
        note: "搜索 >64 层",
        help: "加显式 bound 或拆 trait",
        clause: "§4.14",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0166",
        level: LV_E,
        title: "bound 推导成环",
        note: "`impl<T: Foo<T>> Bar for T`",
        help: "打破自依赖",
        clause: "§4.14",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0167",
        level: LV_E,
        title: "trait 非对象安全",
        note: "泛型方法 / `Self` 返回",
        help: "改泛型参数或拆 trait",
        clause: "§4.14",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0168",
        level: LV_E,
        title: "违反孤儿规则",
        note: "外部 trait 对 外部 type 做 impl",
        help: "包 newtype",
        clause: "§4.14",
        group: G_INFER,
    },
    CodeInfo {
        code: "E-0169",
        level: LV_E,
        title: "impl 重叠不可判定",
        note: "两个 impl 交集无法判空",
        help: "收窄其中一侧",
        clause: "§4.14",
        group: G_INFER,
    },
    // ---- 017x：错误传播 `?`、单态化、`defer`
    CodeInfo {
        code: "E-0170",
        level: LV_E,
        title: "`?` 缺 `From` 实现",
        note: "错误类型无法转换",
        help: "写 `impl From<A> for B` 或显式 `map_err`",
        clause: "§4.11",
        group: G_PROP,
    },
    CodeInfo {
        code: "E-0171",
        level: LV_E,
        title: "`Option` 在返回 `Result` 处用 `?`",
        note: "类型族不匹配",
        help: "`.ok_or(..)`",
        clause: "§4.11",
        group: G_PROP,
    },
    CodeInfo {
        code: "E-0172",
        level: LV_E,
        title: "`?` 所在函数返回类型非法",
        note: "非 `Result`/`Option`",
        help: "改签名",
        clause: "§4.11",
        group: G_PROP,
    },
    CodeInfo {
        code: "E-0173",
        level: LV_E,
        title: "`?` 出现在链顶层",
        note: "无函数返回边界",
        help: "改连携处理（§8.1）",
        clause: "§4.11",
        group: G_PROP,
    },
    CodeInfo {
        code: "E-0174",
        level: LV_E,
        title: "泛型实例化总数超限",
        note: ">65536",
        help: "改 `dyn` 或收窄单态化",
        clause: "§4.15",
        group: G_PROP,
    },
    CodeInfo {
        code: "E-0175",
        level: LV_E,
        title: "泛型递归尺寸无限",
        note: "`struct L<T>{ t:T, next:L<T> }`",
        help: "插入 `Box`",
        clause: "§4.15",
        group: G_PROP,
    },
    // ---- 018x：格式化与迭代器
    CodeInfo {
        code: "E-0180",
        level: LV_E,
        title: "格式串实参不匹配",
        note: "`fmt(\"{}\",)` 数量/具名不符",
        help: "对齐占位符与实参（§10.11）",
        clause: "§10.11",
        group: G_ITER,
    },
    CodeInfo {
        code: "E-0181",
        level: LV_E,
        title: "格式串非常量",
        note: "`fmt(var_str, ..)`",
        help: "改字面量，或显式 `fmt_dynamic`（放弃编译期校验）",
        clause: "§10.11",
        group: G_ITER,
    },
    // ---- 05xx：模块/包/依赖/.infib/校验
    CodeInfo {
        code: "E-0200",
        level: LV_E,
        title: "lock 与清单不同步",
        note: "`MODULE.infy.lock` 含清单已不要求的依赖，或缺当前所需项",
        help: "`infy mod tidy` 重建 lock",
        clause: "§5.7",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0201",
        level: LV_E,
        title: "导入未声明的 module",
        note: "清单无该 `[dependencies]` 项",
        help: "`infy mod tidy`",
        clause: "§5.3",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0202",
        level: LV_E,
        title: "导入目标不存在",
        note: "路径 404 / 缓存缺失且离线",
        help: "修正路径或 `infy mod download`",
        clause: "§5.3",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0210",
        level: LV_E,
        title: "访问 package 私有项",
        note: "跨目录引用非 `pub` 项",
        help: "加 `pub` 或改 API",
        clause: "§5.5",
        group: G_MOD,
    },
    // ---- 030x：语句与连携
    CodeInfo {
        code: "E-0300",
        level: LV_E,
        title: "连携语句独立出现（UB-011）",
        note: "`goto(\"x\");` 直接写在 chain 顶层",
        help: "包进宿主语句的 `{ ... }` 连携块",
        clause: "§8.1",
        group: G_CHAIN,
    },
    CodeInfo {
        code: "E-0301",
        level: LV_E,
        title: "`goto` 目标编译期不可解析",
        note: "锚点不存在 / 常量偏移越界",
        help: "检查 `label` 拼写与节点范围（UB-013）",
        clause: "§3.5",
        group: G_CHAIN,
    },
    // ---- 040x–041x：宏与属性
    CodeInfo {
        code: "E-0400",
        level: LV_E,
        title: "宏作用域泄漏",
        note: "卫生宏展开命名冲突",
        help: "重命名宏内标识符",
        clause: "§6.3",
        group: G_MAC,
    },
    CodeInfo {
        code: "E-0401",
        level: LV_E,
        title: "宏展开深度超限",
        note: ">128",
        help: "简化递归",
        clause: "§6.4",
        group: G_MAC,
    },
    CodeInfo {
        code: "E-0402",
        level: LV_E,
        title: "宏展开产物非法",
        note: "展开后不是合法 AST 片段",
        help: "修正展开器（诊断指向展开器）",
        clause: "§6.5",
        group: G_MAC,
    },
    CodeInfo {
        code: "E-0410",
        level: LV_E,
        title: "未知属性",
        note: "`#[myattr]`",
        help: "用 §6.6 封闭列表内属性",
        clause: "§6.6",
        group: G_MAC,
    },
    // ---- 042x：derive 生成失败
    CodeInfo {
        code: "E-0420",
        level: LV_E,
        title: "`#[skip_clone]` 字段无 `Default`",
        note: "derive Clone 无法填值",
        help: "实现 Default 或去掉 skip",
        clause: "§6.7",
        group: G_DRV,
    },
    CodeInfo {
        code: "E-0421",
        level: LV_E,
        title: "`Copy` derive 非法",
        note: "含 `Drop`/`dyn`/非 Copy 字段",
        help: "只 derive Clone",
        clause: "§6.7",
        group: G_DRV,
    },
    CodeInfo {
        code: "E-0422",
        level: LV_E,
        title: "`Default` derive 于 enum 无零值变体",
        note: "首变体字段缺 Default",
        help: "加 `#[default]`",
        clause: "§6.7",
        group: G_DRV,
    },
    CodeInfo {
        code: "E-0423",
        level: LV_E,
        title: "多个 `#[default]` 变体",
        note: "enum 上重复",
        help: "保留一个",
        clause: "§6.7",
        group: G_DRV,
    },
    CodeInfo {
        code: "E-0424",
        level: LV_E,
        title: "含浮点字段却 derive `Eq`",
        note: "`NaN != NaN`",
        help: "加 `#[allow_nan_equality]`",
        clause: "§6.7",
        group: G_DRV,
    },
    CodeInfo {
        code: "E-0425",
        level: LV_E,
        title: "derive `Eq` 但字段非 `Eq`",
        note: "混合类型",
        help: "先满足约束",
        clause: "§6.7",
        group: G_DRV,
    },
    CodeInfo {
        code: "E-0426",
        level: LV_E,
        title: "重复 derive",
        note: "`#[derive(Clone, Clone)]`",
        help: "合并",
        clause: "§6.7",
        group: G_DRV,
    },
    CodeInfo {
        code: "E-0427",
        level: LV_E,
        title: "derive 需要 `unsafe`",
        note: "生成代码触及裸指针",
        help: "手写该 impl 并写 safety 注释",
        clause: "§6.7",
        group: G_DRV,
    },
    // ---- 05xx 续
    CodeInfo {
        code: "E-0501",
        level: LV_E,
        title: "包名与目录不符",
        note: "`package a` 在 `core/b/` 下",
        help: "改为 `package b`",
        clause: "§5.1",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0502",
        level: LV_E,
        title: "循环包依赖",
        note: "`a→b→a`",
        help: "提取第三包或反转依赖（诊断打印完整环）",
        clause: "§5.4",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0503",
        level: LV_E,
        title: "循环 module 依赖",
        note: "module 级环",
        help: "同上，跨 module 层",
        clause: "§5.4",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0504",
        level: LV_E,
        title: "非法导入路径",
        note: "相对路径 / 非字符串 / 通配",
        help: "用 module path 起算的完整路径",
        clause: "§5.3",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0505",
        level: LV_E,
        title: "目录名不可导入",
        note: "大写 / 中文 / 首字符数字目录名",
        help: "目录名改小写 ASCII（§5.1 R3）",
        clause: "§5.1",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0506",
        level: LV_E,
        title: "清单语法错误",
        note: "`MODULE.infy.toml` 非法 TOML",
        help: "按 §5.2 修正",
        clause: "§5.2",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0507",
        level: LV_E,
        title: "module path 与 `std` 冲突",
        note: "`path = \"std_x\"` 前缀违规",
        help: "改用自有域名前缀",
        clause: "§5.3",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0508",
        level: LV_E,
        title: "越界访问 `internal`",
        note: "`a/x/internal/c` 被 `a/y` 导入",
        help: "提升为公开 API 或移入同子树",
        clause: "§5.5",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0509",
        level: LV_E,
        title: "`internal` 嵌套 `internal`",
        note: "`internal/internal/`",
        help: "删除内层",
        clause: "§5.5",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0510",
        level: LV_E,
        title: "版本需求不可满足",
        note: "MVS 无解",
        help: "放宽 `[dependencies]` 区间",
        clause: "§5.7",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0551",
        level: LV_E,
        title: "库文件不可执行",
        note: "`infy run foo.infib`",
        help: "在调用方 module 内 `import` 它",
        clause: "§5.6",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0552",
        level: LV_E,
        title: "库文件含入口",
        note: "`.infib` 内有 `fn main`/`entry_chain`/`[[bin]]`",
        help: "拆成 `[[bin]]` module",
        clause: "§5.6",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0553",
        level: LV_E,
        title: "库文件夹带未声明动态库",
        note: "wheel 版出现游离 `[[link]]` 路径",
        help: "在清单显式声明 target 与库",
        clause: "§5.6",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0554",
        level: LV_E,
        title: "`.infib` 清单缺必需字段",
        note: "无 `ifabi`/`abi_hash`/`deps.dirhash`",
        help: "用 `infy lib pack` 重产",
        clause: "附录 E.2",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0560",
        level: LV_E,
        title: "校验和不匹配",
        note: "dirhash ≠ `MODULE.infy.lock`",
        help: "回滚或 `accept-changes`（§5.8 第 2 项）",
        clause: "§5.8",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0561",
        level: LV_E,
        title: "校验数据库不可达",
        note: "首次解析该版本且离线",
        help: "配 `INFY_NOSUMDB` 或先 `infy mod vendor`",
        clause: "§5.8",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0562",
        level: LV_E,
        title: "校验数据库与本地记录冲突",
        note: "公开日志已改写该版本",
        help: "立即冻结该依赖，视为供应链事件",
        clause: "§5.8",
        group: G_MOD,
    },
    CodeInfo {
        code: "E-0570",
        level: LV_E,
        title: "缓存目录不可写",
        note: "权限/磁盘满",
        help: "设 `INFY_CACHE`（不阻断，仅无缓存）",
        clause: "§5.10",
        group: G_MOD,
    },
    // ---- 06xx：构建/链接/工具链/可复现
    CodeInfo {
        code: "E-0600",
        level: LV_E,
        title: "产物无入口",
        note: "既无 `entry_chain` 也无 `fn main`",
        help: "声明 `#![entry_chain]`",
        clause: "§7.4",
        group: G_BLD,
    },
    CodeInfo {
        code: "E-0601",
        level: LV_E,
        title: "入口声明冲突",
        note: "多个 `entry_chain`/`[[bin]]` 争抢",
        help: "只留一个或分 `[[bin]]`",
        clause: "§7.4",
        group: G_BLD,
    },
    CodeInfo {
        code: "E-0602",
        level: LV_E,
        title: "A 轨工具链缺失/未注册",
        note: "`@c` 但无 `cc` 记录",
        help: "`infy toolchain probe` / `add`",
        clause: "§7.6.2",
        group: G_BLD,
    },
    CodeInfo {
        code: "E-0603",
        level: LV_E,
        title: "可复现构建不一致",
        note: "`--verify-reproducible` 两次产物不同",
        help: "查 §7.3 差异段并上报实现 bug",
        clause: "§7.3",
        group: G_BLD,
    },
    CodeInfo {
        code: "E-0604",
        level: LV_E,
        title: "工具链版本漂移 / 符号未导出",
        note: "wheel 记录版本 ≠ 实际；或链接缺符号",
        help: "对齐编译器版本 / 补源码导出（UB-023）",
        clause: "§7.6.2",
        group: G_BLD,
    },
    CodeInfo {
        code: "E-0605",
        level: LV_E,
        title: "ABI 或布局不一致",
        note: "`IFABI` 冲突、跨边界类型缺 `#[repr(C)]`",
        help: "统一 `IFABI` 或标注布局（UB-021）",
        clause: "§4.8",
        group: G_BLD,
    },
    CodeInfo {
        code: "E-0606",
        level: LV_E,
        title: "依赖间编译策略冲突",
        note: "`overflow`/`fast_math` 混用",
        help: "统一到主 module 策略",
        clause: "§5.2",
        group: G_BLD,
    },
    CodeInfo {
        code: "E-0607",
        level: LV_E,
        title: "wheel 未覆盖本机 target 且无源码版",
        note: "`targets` 列表不含本机 triple",
        help: "发布含本机 target 的 wheel 或附源码版",
        clause: "§5.6",
        group: G_BLD,
    },
    // ---- 07xx：嵌入（通用 070x，B 轨 071x，A 轨 072x）
    CodeInfo {
        code: "E-0700",
        level: LV_E,
        title: "嵌入块位置非法",
        note: "`@py` 出现在模式/类型位置",
        help: "移到 `event`/`fn` 体或语句位",
        clause: "§3.6",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0701",
        level: LV_E,
        title: "（废止）编译型语言嵌入禁令",
        note: "v1.0 的\"编译型语言嵌入禁令\"码，v2.0 废止；编号永久保留、不得赋予新语义（§9.3 注）",
        help: "历史诊断日志与 §12 回归用例仍可能引用；该编号不得复用，A 轨的对应新码见 `E-0602`/`E-0722`",
        clause: "§9.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0702",
        level: LV_E,
        title: "未知或不支持的嵌入前缀",
        note: "`@forth`、`@ruby`、`@php`、`@java`(v2.0)",
        help: "用 §7.5 白名单内前缀",
        clause: "§7.5",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0703",
        level: LV_E,
        title: "跨轨直接调用",
        note: "A 轨单元直调 B 轨单元",
        help: "经 INFY 侧 `event` 中转",
        clause: "§7.9",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0711",
        level: LV_E,
        title: "动态代码生成",
        note: "`eval` / `exec` / 运行时构造 code object",
        help: "静态展开，或改 A 轨 / `isolation=\"process\"`（§7.7.2）",
        clause: "§7.7.2",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0712",
        level: LV_E,
        title: "字节码能力缺失",
        note: "摄取到的 code object 含未映射 opcode / 未实现 builtin / 未声明 import",
        help: "按诊断列出的具体项等待能力表扩容，或改写该处",
        clause: "§7.7.2",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0713",
        level: LV_E,
        title: "字节码版本不匹配",
        note: "`.pyc` magic 或实际解释器版本 ≠ 清单 `[b_track] py` 声明",
        help: "`infy toolchain add py --version <声明版本>` 或改声明",
        clause: "§7.7.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0714",
        level: LV_E,
        title: "wheel 含原生二进制",
        note: "`infy ingest py-wheel` 输入非 `py3-none-any`",
        help: "走 A 轨链接其 C API，或 `isolation=\"process\"`",
        clause: "§7.7.4",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0715",
        level: LV_E,
        title: "未声明 B 轨工具链版本",
        note: "使用 `@py` / `@lua` 但清单无 `[b_track]` 条目",
        help: "声明 `[b_track] py = \"3.13\"`（§7.7.3）",
        clause: "§7.7.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0716",
        level: LV_E,
        title: "进程隔离前缀缺 `isolation`",
        note: "`@js { ... }` 未写 `isolation=\"process\"`",
        help: "补参数，或改用 `@py`/`@lua`（§7.7.5 R-JS）",
        clause: "§7.7.5",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0720",
        level: LV_E,
        title: "A 轨 `name` 不匹配",
        note: "`#[link(name=\"a\")]` 对不上 `@c(name=\"b\")`",
        help: "统一 name",
        clause: "§7.6.1",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0721",
        level: LV_E,
        title: "A 轨调用缺 `unsafe`",
        note: "直接调 `extern` 函数",
        help: "包 `unsafe { }` + `// infy-safety:`",
        clause: "§7.6.1",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0722",
        level: LV_E,
        title: "未声明 A 轨信任策略",
        note: "用了 `@c` 但清单无 `a_track`",
        help: "`[profile] a_track = \"unsafe-trusted\"`",
        clause: "§7.6.6",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0723",
        level: LV_E,
        title: "跨边界指针移交未声明",
        note: "`extern` 返回裸指针无 `#[owns]`/`#[borrows]`",
        help: "二选一显式写出（UB-010，无默认）",
        clause: "§7.6.4",
        group: G_EMB,
    },
    CodeInfo {
        code: "E-0724",
        level: LV_E,
        title: "回调指针未校验即解引用",
        note: "`#[export]` 侧直接信任外部指针",
        help: "走 §7.6.5 thunk",
        clause: "§7.6.5",
        group: G_EMB,
    },
    // ---- 08xx：链、节点表、焦点调度
    CodeInfo {
        code: "E-0800",
        level: LV_E,
        title: "链不存在",
        note: "`start(chain = NoSuch)`",
        help: "修正链名 / 先声明",
        clause: "§8.2",
        group: G_SCH,
    },
    CodeInfo {
        code: "E-0801",
        level: LV_E,
        title: "链锚点不存在",
        note: "`start(chain=C, node=\"anchor\")` 不在 C 节点表",
        help: "检查锚点属于该链",
        clause: "§8.2",
        group: G_SCH,
    },
    // ---- 09xx：宿主事件 / HostApi / 握手
    CodeInfo {
        code: "E-0900",
        level: LV_E,
        title: "宿主事件参数不匹配",
        note: "调用 native `event` 缺必填参数",
        help: "补参数或给默认值",
        clause: "§10.8",
        group: G_HOST,
    },
    // ---- 100x–103x：所有权/借用/生命周期/drop（含 §4.7.5 信任边界）
    CodeInfo {
        code: "E-1001",
        level: LV_E,
        title: "使用已移出的绑定",
        note: "`let a = v; use(v);`",
        help: "用 `clone()` 或改传 `&v`",
        clause: "§4.7.1",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1002",
        level: LV_E,
        title: "借用冲突",
        note: "`&mut` 与 `&` 共存 / 双 `&mut`",
        help: "缩短作用域或改 `RefCell`",
        clause: "§4.7.2",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1003",
        level: LV_E,
        title: "生命周期无法唯一确定",
        note: "elision 三规则均不适用",
        help: "显式 `'a` 标注（§4.7.3）",
        clause: "§4.7.3",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1004",
        level: LV_E,
        title: "返回局部引用",
        note: "`&x` 逃逸",
        help: "返回所有权类型 / `Box<T>`",
        clause: "§4.7.2",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1005",
        level: LV_E,
        title: "借用已移出的值",
        note: "`let r = &moved`",
        help: "先借用后移动",
        clause: "§4.7.2",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1006",
        level: LV_E,
        title: "从引用移动全部字段",
        note: "`S { ..&base }`",
        help: "改 `clone()` 或取所有权",
        clause: "§3.11",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1030",
        level: LV_E,
        title: "裸指针解引用缺 `unsafe`",
        note: "`*p`",
        help: "包 `unsafe` 并写前提",
        clause: "§4.7.5",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1031",
        level: LV_E,
        title: "`static mut` 访问缺 `unsafe`",
        note: "读写全局可变",
        help: "改 `Mutex<..>`/`Arc<..>`",
        clause: "§4.7.5",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1032",
        level: LV_E,
        title: "`unsafe` 项未包 `unsafe`",
        note: "调 `unsafe fn`",
        help: "包 `unsafe`",
        clause: "§4.7.5",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1033",
        level: LV_E,
        title: "`union` 非活跃变体访问",
        note: "读未写过的变体",
        help: "记录活跃变体或用 `enum`",
        clause: "§4.7.5",
        group: G_OWN,
    },
    CodeInfo {
        code: "E-1034",
        level: LV_E,
        title: "`transmute` 尺寸/对齐不匹配",
        note: "`transmute::<u32,u64>`",
        help: "显式 `as` 或包结构体",
        clause: "§4.7.5",
        group: G_OWN,
    },
    // ---- 104x：内存布局与栈
    CodeInfo {
        code: "E-1040",
        level: LV_E,
        title: "非法对齐",
        note: "`#[align(1)]` 于 8 对齐类型",
        help: "只允许增大对齐",
        clause: "§4.8",
        group: G_MEM,
    },
    // ---------------------------------------------------------------- 警告 `W-`（§9.3 内联清单）
    CodeInfo {
        code: "W-0001",
        level: LV_W,
        title: "CRLF",
        note: "源码使用 CRLF 换行；规范要求强制 LF",
        help: "改为 LF（`infy fmt` 自动修复）",
        clause: "§2.1",
        group: G_LEX,
    },
    CodeInfo {
        code: "W-0002",
        level: LV_W,
        title: "缺 trailing LF",
        note: "文件结尾缺一个 trailing LF",
        help: "`infy fmt` 自动补齐（§9.5 第 11 条）",
        clause: "§2.1",
        group: G_LEX,
    },
    CodeInfo {
        code: "W-0003",
        level: LV_W,
        title: "构建约束恒 false",
        note: "约束恒 false（文件必然被排除）",
        help: "删除该约束或改为其取值在 §2.2 封闭集内的条件",
        clause: "§2.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "W-0070",
        level: LV_W,
        title: "枚举 `_` 掩盖新变体",
        note: "下游可能漏处理",
        help: "列全变体",
        clause: "§3.12",
        group: G_PAT,
    },
    CodeInfo {
        code: "W-0071",
        level: LV_W,
        title: "匹配臂部分不可达",
        note: "与前臂重叠",
        help: "合并或删除",
        clause: "§3.12",
        group: G_PAT,
    },
    CodeInfo {
        code: "W-0072",
        level: LV_W,
        title: "模式绑定遮蔽已知名",
        note: "`case x` 想比常量？",
        help: "用 `Const::X` 或 `x @ _`",
        clause: "§3.12",
        group: G_PAT,
    },
    CodeInfo {
        code: "W-0160",
        level: LV_W,
        title: "实例化次数偏高",
        note: "单函数 >512 实例",
        help: "改 `dyn`/`Box` 参数",
        clause: "§4.15",
        group: G_INFER,
    },
    CodeInfo {
        code: "W-0170",
        level: LV_W,
        title: "丢弃 `Result`/`Option`",
        note: "`f();` 未消费",
        help: "`let _ = f();` 或处理",
        clause: "§4.11",
        group: G_PROP,
    },
    CodeInfo {
        code: "W-0185",
        level: LV_W,
        title: "依赖 `HashMap` 迭代顺序",
        note: "顺序影响输出",
        help: "改 `Map`（有序）",
        clause: "§10.13",
        group: G_ITER,
    },
    CodeInfo {
        code: "W-0400",
        level: LV_W,
        title: "使用已弃用项",
        note: "使用点触及被 `#[deprecated(\"note\")]` 标注的任何 `pub` 项",
        help: "改用替代项；strict 下阻断",
        clause: "§6.6",
        group: G_MAC,
    },
    CodeInfo {
        code: "W-0500",
        level: LV_W,
        title: "清单未知键",
        note: "清单出现未知键（防拼写失效的静默忽略）",
        help: "按 §5.2 校正键名",
        clause: "§5.2",
        group: G_MOD,
    },
    CodeInfo {
        code: "W-0604",
        level: LV_W,
        title: "允许工具链漂移（产物标 `unreproducible`）",
        note: "`--allow-toolchain-drift` 把 `E-0604` 的版本核对降为警告",
        help: "该产物不再获得 §7.3 可复现承诺，`.inf meta` 标 `unreproducible` 旗",
        clause: "§7.6.2",
        group: G_BLD,
    },
    CodeInfo {
        code: "W-0700",
        level: LV_W,
        title: "使用 `env.return` 旧写法",
        note: "`env.return(x)` 作为 v1.0 兼容写法保留，等价实现期 `return`",
        help: "改 `return`；多返回值 → 元组",
        clause: "§7.7.1",
        group: G_EMB,
    },
    CodeInfo {
        code: "W-0705",
        level: LV_W,
        title: "IFBC 行映射缺失",
        note: "行映射不完备：并非每条可执行指令都有 `line_map` 覆盖（校验项 6，不阻断）",
        help: "重新生成 IFBC 并保留调试信息；不得手写",
        clause: "§7.8.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "W-0706",
        level: LV_W,
        title: "声明纯函数但存在宿主副作用",
        note: "声明为纯（无 `#[effectful]`）的 B 轨函数无法从 code object 的 `IMPORT_NAME`/`co_names` 证明不触达文件、网络、进程与 `os`/`sys` 类内建",
        help: "拒绝内联优化并提示；确需副作用则标 `#[effectful]`",
        clause: "§7.9",
        group: G_EMB,
    },
    CodeInfo {
        code: "W-0720",
        level: LV_W,
        title: "A 轨导出符号未被任何签名声明使用",
        note: "源码导出侧的签名未被签名表使用",
        help: "对齐 `#[export]` 与签名表后重生成绑定",
        clause: "§7.6.1",
        group: G_EMB,
    },
    CodeInfo {
        code: "W-0800",
        level: LV_W,
        title: "静态锁序环",
        note: "`infy check --deadlock` 的静态锁序图分析发现环",
        help: "固定资源获取顺序（FIFO 只保证同资源互斥顺序，不提供锁序检查）",
        clause: "§8.3",
        group: G_SCH,
    },
    CodeInfo {
        code: "W-0801",
        level: LV_W,
        title: "长循环无抢占点",
        note: "纯计算循环超配额",
        help: "插入 `std.chain.yield_now()`",
        clause: "§8.5",
        group: G_SCH,
    },
    CodeInfo {
        code: "W-1010",
        level: LV_W,
        title: "未使用的生命周期",
        note: "`infy check` 对未使用的生命周期参数报警（`infy fmt` 不重命名生命周期）",
        help: "删除该生命周期参数",
        clause: "§2.5",
        group: G_OWN,
    },
    CodeInfo {
        code: "W-1011",
        level: LV_W,
        title: "`dyn` 出现在 `pub` API",
        note: "`dyn` 只能出现在 B 轨 `@embed` 的形参/返回/局部以及 `std.embed.Value`；出现在 `pub` API 签名会污染下游静态性",
        help: "改具体类型，或 `std.embed.Value`",
        clause: "§7.7.7",
        group: G_OWN,
    },
    CodeInfo {
        code: "W-1012",
        level: LV_W,
        title: "`std.host.event` 出现在 `pub` API",
        note: "`std.host.event` 只用于宿主事件动态增删的场景，出现在 `pub` API 即违反静态优先原则",
        help: "在 INFY 侧声明为无体 `event f(...) -> T;`（编译期参数检查）",
        clause: "§10.8",
        group: G_OWN,
    },
    CodeInfo {
        code: "W-1030",
        level: LV_W,
        title: "`unsafe` 块缺 `infy-safety` 注释",
        note: "每个 `unsafe` 必须紧邻一行 `// infy-safety: <一句话前提>` 注释（缺失即报，strict 下阻断）",
        help: "补注释，并留在其 `unsafe` 块同一行或紧邻上一行（§9.5 第 8 条）",
        clause: "§4.7.5",
        group: G_OWN,
    },
    // ---------------------------------------------------------------- 运行时 panic `R-`（§9.3 主表）
    CodeInfo {
        code: "R-0001",
        level: LV_R,
        title: "整数溢出（`overflow=panic`）",
        note: "整数溢出不得未定义（UB-003）：必须 panic 或 saturate（module 配置），不能 wrap 除非显式 `wrapping_add()`",
        help: "显式 `wrapping_*`，或按 module 策略 saturate",
        clause: "§4.4",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0002",
        level: LV_R,
        title: "除 0",
        note: "除 0 静默忽略是 UB-004：整数 `/`、`%` 与 `f % 0.0` 必须 panic",
        help: "先判除数非零",
        clause: "§4.12",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0003",
        level: LV_R,
        title: "`str` 运行时检出 NUL",
        note: "NUL 注入：`\\0` 出现在 `str` 中间，构造时 `E-0150`，跨 A 轨边界运行时检查须 `R-0003`（UB-007）",
        help: "`std.str.strip_nul` / `to_cstr()`",
        clause: "§7.6.4",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0004",
        level: LV_R,
        title: "数组越界",
        note: "数组越界访问静默是 UB-008，必须运行时 panic",
        help: "访问前显式判界",
        clause: "§4.9",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0005",
        level: LV_R,
        title: "`Option/Result::unwrap()` 于 None/Err",
        note: "INFY 无 `null`（UB-009）：`Option::unwrap()` 在 None 时必须 panic",
        help: "显式处理 `None`/`Err`，或用 `?` 传播",
        clause: "§4.9",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0006",
        level: LV_R,
        title: "`bool` 非法位模式",
        note: "读到的 `bool` 位模式不是规范定义的合法表示",
        help: "跨边界布尔须经 `#[repr(C)]` 映射规则读取",
        clause: "§4.1",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0007",
        level: LV_R,
        title: "移位量 ≥ 类型宽度",
        note: "`1u8 << 8`",
        help: "掩码或加宽",
        clause: "§4.12",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0008",
        level: LV_R,
        title: "`inf % x`",
        note: "浮点无穷取模",
        help: "先判 `is_finite`",
        clause: "§4.12",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0009",
        level: LV_R,
        title: "`ilog2/ilog10` 于 0",
        note: "定义域",
        help: "判零后调用",
        clause: "§10.9",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0010",
        level: LV_R,
        title: "`goto` 运行时目标不存在",
        note: "仅当节点集依赖构建配置而无法静态确定时才允许运行时判定（UB-013）",
        help: "优先编译期消解：命名锚点 / 常量偏移 → `E-0301`",
        clause: "§8.2",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0011",
        level: LV_R,
        title: "drop 中 panic（双 panic）",
        note: "`Drop` 内 `unwrap` 失败",
        help: "drop 不得失败",
        clause: "§4.16",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0012",
        level: LV_R,
        title: "运行时 radix 越界",
        note: "`from_str_radix(.., 37)` 非常量",
        help: "编译期常量传参",
        clause: "§10.9",
        group: G_LEX,
    },
    CodeInfo {
        code: "R-0701",
        level: LV_R,
        title: "嵌入执行超时（B 轨回边计数 / A 轨边界点）",
        note: "B 轨靠 `tickloop` 计数；A 轨的 `timeout_ms` 只在该单元回到 INFY 边界时生效",
        help: "需可抢占的 A 轨单元改 `isolation=\"process\"`（§7.9 已列为 A 轨已知代价）",
        clause: "§7.9",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0702",
        level: LV_R,
        title: "（废止）嵌入 runtime 缺失",
        note: "v1.0 的\"嵌入 runtime 缺失\"码，v2.0 废止；编号永久保留、不得赋予新语义（§9.3 注）",
        help: "历史诊断日志与 §12 回归用例仍可能引用；该编号不得复用，A 轨的对应新码见 `E-0602`/`E-0722`",
        clause: "§9.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0703",
        level: LV_R,
        title: "IFBC 装载校验失败（总类）",
        note: "IFBC 字节码未校验即执行（UB-019）：装载期必须逐函数通过 §7.8.3 校验器",
        help: "禁止\"部分执行\"；失败即整体拒绝该模块",
        clause: "§7.8.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0704",
        level: LV_R,
        title: "IFBC 非线性指令流",
        note: "校验项 1 线性性：每条指令边界可判定，不存在落入指令中部的跳转",
        help: "",
        clause: "§7.8.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0705",
        level: LV_R,
        title: "IFBC 栈高度不一致",
        note: "校验项 2 栈深度一致：每个基本块入口栈高度必须唯一可推",
        help: "",
        clause: "§7.8.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0706",
        level: LV_R,
        title: "IFBC 操作数类型与后缀不符",
        note: "校验项 3 类型一致：每个操作数类型与指令后缀匹配；`div/rem` 须紧邻 `guard.nonzero`，`shl/shr` 须紧邻 `guard.shift`",
        help: "不能删检查（UB-003/UB-004 的字节码侧对应）",
        clause: "§7.8.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0707",
        level: LV_R,
        title: "协程指令出现在非协程帧",
        note: "校验项 4 帧合法：`ret` 的类型与 `func_table` 声明一致；`yield` 只出现在协程帧",
        help: "",
        clause: "§7.8.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0708",
        level: LV_R,
        title: "`call.host` 签名与 `IFABI` 不符",
        note: "校验项 5 host 调用签名：`call.host` 目标必须在 `import_table` 且签名逐元匹配 `IFABI`",
        help: "跨 A 轨一律走 `call.host` 到运行时的 thunk，不允许直接跳外部地址",
        clause: "§7.8.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0709",
        level: LV_R,
        title: "嵌入单元超出 `max_mem_bytes`",
        note: "校验项 6 边界：`@embed(max_mem_bytes=…)` 的独立 arena 超限",
        help: "",
        clause: "§7.9",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0710",
        level: LV_R,
        title: "`dyn` 标签与声明类型不符（UB-020）",
        note: "B 轨动态值越界后按静态类型解释；动态标签不得被优化删除",
        help: "除 `#[assume_typed]` 显式声明且 `-O3` 下仍保留检查",
        clause: "§7.7.7",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0712",
        level: LV_R,
        title: "动态调用目标越界",
        note: "`invoke.dynamic` 指向模块外函数",
        help: "改走 `call.host`",
        clause: "§7.8.2",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0713",
        level: LV_R,
        title: "非 `repr(C)` 求字段偏移",
        note: "`field_off` 于默认布局类型",
        help: "标注 `#[repr(C)]`",
        clause: "§7.8.2",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0720",
        level: LV_R,
        title: "A 轨跨边界指针调用期无效",
        note: "边界 thunk 的调用期借用有效性检查失败；A 轨返回裸指针为 0 且未声明 `Option`（UB-009）",
        help: "按 §7.6.3 的 thunk 清单补长度/对齐/null→`Option` 检查",
        clause: "§7.6.3",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0721",
        level: LV_R,
        title: "异常跨 `\"C\"` ABI 边界展开",
        note: "A 轨边界处外部语言的异常/信号（SIGSEGV 除外）",
        help: "`isolation=\"process\"` 下按 `R-0721`；同进程下规范不承诺可恢复（§7.6.6 的代价）",
        clause: "§7.6.1",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0722",
        level: LV_R,
        title: "A 轨 `Result` 出参未写",
        note: "`Result<T, i32>` 映射为 `T ret, int32_t* err` 出参形式，外部忘写 `err`",
        help: "由自动生成的 thunk 补齐出参约定",
        clause: "§7.6.4",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0723",
        level: LV_R,
        title: "回调 `Arc` 卸载时计数非 0（泄漏）",
        note: "闭包被装箱进 `Arc`，外部释放未经 `#[export]` 的 `free` 入口，卸载时计数不为 0",
        help: "外部释放必须经 `#[export]` 的 `free` 入口",
        clause: "§7.6.4",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0730",
        level: LV_R,
        title: "丢弃必释放值",
        note: "`pop` 掉 `#[must_drop]` 类型",
        help: "显式 `drop(..)`",
        clause: "§7.8.2",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0731",
        level: LV_R,
        title: "读取已移出的局部",
        note: "`ldl i` 于 `move.out` 之后",
        help: "重新赋值后再读",
        clause: "§7.8.2",
        group: G_EMB,
    },
    CodeInfo {
        code: "R-0802",
        level: LV_R,
        title: "焦点 ID 回绕",
        note: "`focus_id` 超过 `u32`",
        help: "回收长驻焦点 / 重启进程",
        clause: "§8.5",
        group: G_SCH,
    },
    CodeInfo {
        code: "R-0803",
        level: LV_R,
        title: "无限迭代器消费",
        note: "`collect` 非 `ExactSizeIterator` 超 `iter_guard_steps`",
        help: "改 `take(n)`",
        clause: "§10.10",
        group: G_SCH,
    },
    CodeInfo {
        code: "R-0900",
        level: LV_R,
        title: "宿主事件返回未映射错误",
        note: "宿主返回的错误未携带可映射的 INFY 诊断码，运行时按 `R-0900` 归类并打印 `unmapped host error`；宿主不认识该 `EventId` 时同样归此类",
        help: "宿主错误必须映射到 `E-0900` 系诊断码（§11）",
        clause: "§11",
        group: G_HOST,
    },
    CodeInfo {
        code: "R-0901",
        level: LV_R,
        title: "`std.host.query` 键类型与宿主定义不符",
        note: "`std.host.query` 的键类型与宿主侧定义不一致（§10.8 静态优先原则的运行时对应）",
        help: "按宿主定义修正键类型，或改声明为无体 `event`",
        clause: "§10.8",
        group: G_HOST,
    },
    CodeInfo {
        code: "R-0902",
        level: LV_R,
        title: "宿主回调嵌套过深",
        note: ">8 层互递归",
        help: "改为事件队列",
        clause: "§11.1",
        group: G_HOST,
    },
    CodeInfo {
        code: "R-0903",
        level: LV_R,
        title: "`IFABI` 不匹配",
        note: "产物与宿主 ABI 版本不同",
        help: "同步重建",
        clause: "§11.3",
        group: G_HOST,
    },
    CodeInfo {
        code: "R-0904",
        level: LV_R,
        title: "宿主事件缺失/签名不符",
        note: "`EventTable` 项在宿主不存在",
        help: "补注册或重编",
        clause: "§11.3",
        group: G_HOST,
    },
    CodeInfo {
        code: "R-0905",
        level: LV_R,
        title: "`EnvSnapshot` 布局不符",
        note: "字段偏移表不一致",
        help: "升级宿主绑定",
        clause: "§11.3",
        group: G_HOST,
    },
    CodeInfo {
        code: "R-1010",
        level: LV_R,
        title: "drop 期间被跨链访问",
        note: "drop 时机必须是 §4.7.4 的确定性顺序且与 `-O` 级别无关（UB-017/UB-018）",
        help: "把跨链访问移到 drop 之前，或改 `Arc` 共享所有权",
        clause: "§4.7.4",
        group: G_OWN,
    },
    CodeInfo {
        code: "R-1040",
        level: LV_R,
        title: "链栈溢出",
        note: "每链独立栈默认 8 MiB，溢出必须终止该链，不得静默破坏相邻链",
        help: "由 module 清单 `[profile] stack_bytes` 调整，或改堆分配递归",
        clause: "§4.8",
        group: G_MEM,
    },
    // ---------------------------------------------------------------- 通知 `N-`
    CodeInfo {
        code: "N-0800",
        level: LV_N,
        title: "`goto` 用绝对索引",
        note: "源码改动会移动节点",
        help: "改命名锚点（§8.4）",
        clause: "§8.4",
        group: G_SCH,
    },
    CodeInfo {
        code: "N-0801",
        level: LV_N,
        title: "焦点创建被压力限流",
        note: "宿主 `set_pressure` 生效",
        help: "非错误；用于定位迟滞",
        clause: "§11.4",
        group: G_SCH,
    },
];

pub fn lookup(code: &str) -> Option<&'static CodeInfo> {
    ALL.iter().find(|c| c.code == code)
}

pub fn count() -> usize {
    ALL.len()
}

pub fn codes_with_prefix(prefix: char) -> Vec<&'static CodeInfo> {
    ALL.iter().filter(|c| c.code.starts_with(prefix)).collect()
}

/// The sorted distinct §9.1 interval topics present in the table — the value domain of
/// `infy conformance --category`.
pub fn known_groups() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = ALL.iter().map(|c| c.group).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §9.3/§9.7 must not hand out one number twice.
    #[test]
    fn no_duplicate_codes() {
        let mut seen: Vec<&'static str> = ALL.iter().map(|c| c.code).collect();
        let n = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "诊断码重复");
    }

    /// The prefix letter is the level (§9.1); a mismatch would break §9.2 rendering and §9.6.
    #[test]
    fn level_matches_prefix() {
        for c in ALL {
            let letter = c.code.chars().next().unwrap();
            assert_eq!(
                Level::from_prefix(letter),
                Some(c.level),
                "{} 的 level 与前缀不符",
                c.code
            );
            assert_eq!(c.level.prefix(), letter, "{} 前缀", c.code);
        }
    }

    /// Every code is `<EWN R>-` + exactly four digits, and the four fields are populated.
    #[test]
    fn codes_are_well_formed() {
        for c in ALL {
            let b = c.code.as_bytes();
            assert_eq!(b.len(), 6, "{} 长度", c.code);
            assert!(matches!(b[0], b'E' | b'W' | b'R' | b'N'), "{} 前缀", c.code);
            assert_eq!(&c.code[1..2], "-", "{} 分隔符", c.code);
            assert!(c.code[2..].bytes().all(|x| x.is_ascii_digit()), "{} 数字", c.code);
            assert!(!c.title.is_empty(), "{} 缺标题", c.code);
            assert!(!c.clause.is_empty(), "{} 缺条款", c.code);
            assert!(!c.group.is_empty(), "{} 缺主题", c.code);
        }
    }

    /// §9.1 claims 210 codes; the tables carry 209 and `E-0093` comes from §9.6 prose.
    #[test]
    fn table_is_complete() {
        assert!(count() >= 200, "码表不完整：{}", count());
        assert_eq!(count(), 210, "§9.1 声称 210 码");
        assert_eq!(codes_with_prefix('E').len(), 146);
        assert_eq!(codes_with_prefix('W').len(), 22);
        assert_eq!(codes_with_prefix('R').len(), 40);
        assert_eq!(codes_with_prefix('N').len(), 2);
    }

    /// `lookup` is the only accessor `diag` uses; it must round-trip for every row.
    #[test]
    fn lookup_finds_every_row() {
        for c in ALL {
            let hit = lookup(c.code).unwrap_or_else(|| panic!("{} 查不到", c.code));
            assert_eq!(hit.clause, c.clause, "{}", c.code);
        }
        assert!(lookup("E-9999").is_none());
        assert!(lookup("").is_none());
    }

    /// The §9.2 sample: `error[E-0111]: 隐式浮点/整数互转：期望 u32，实际 f32`.
    #[test]
    fn e0111_title_is_the_implicit_conversion() {
        let info = lookup("E-0111").expect("E-0111 必须在表中");
        assert!(info.title.contains("隐式"), "E-0111 标题: {}", info.title);
        assert_eq!(info.level, Level::Error);
        assert_eq!(info.clause, "§4.2");
    }

    /// Reserved-but-deprecated numbers stay in the table so they cannot be reused.
    #[test]
    fn deprecated_codes_stay_reserved() {
        for code in ["E-0701", "R-0702"] {
            let info = lookup(code).unwrap_or_else(|| panic!("{code} 必须保留"));
            assert!(info.title.starts_with("（废止）"), "{code} 标题应标注废止");
            assert!(info.note.contains("永久保留"), "{code} 应说明编号永久保留");
            assert!(info.help.contains("不得复用"), "{code} 应说明不得复用");
        }
        // E-0093 exists only in §9.6 prose; §13.4/§9.6 both depend on it being emittable.
        assert!(lookup("E-0093").is_some(), "E-0093 必须收录");
    }

    /// `--category` filters on the §9.1 interval topics.
    #[test]
    fn groups_cover_the_spec_topics() {
        let g = known_groups();
        assert!(!g.is_empty());
        for topic in [
            "字面量、字符串、Unicode 安全",
            "所有权/借用/生命周期/drop",
            "编译期常量求值 `const-eval`",
            "模式与穷尽性",
            "词法基础：编码、注释、标识符、关键字",
            "类型与运算符",
            "构建/链接/工具链/可复现",
            "宿主事件、`HostApi`、握手",
            "内存布局与栈",
        ] {
            assert!(g.contains(&topic), "缺 §9.1 主题: {topic}");
        }
        assert_eq!(g.len(), 22, "§9.1 有 23 个区间行，105x 无码");
    }

    /// Every §9.3/§9.7 interval must be represented by the codes that actually exist.
    #[test]
    fn every_interval_is_populated() {
        for c in ALL {
            let n: u32 = c.code[2..].parse().expect("数字区间");
            assert!(n <= 1999, "{} 越出 E/W/R 区间（§9.1）", c.code);
            if c.code.starts_with('N') {
                assert!(n <= 999, "{} 越出 N- 区间（§9.1）", c.code);
            }
        }
    }
}
