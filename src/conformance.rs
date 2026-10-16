//! §12 — the conformance suite: the category table, the on-disk case format, and the runner.
//!
//! Three things live here, and only these three (the reporting layer is E2's `infy conformance
//! --report`):
//!
//! * [`CATEGORIES`] — §12.1/§12.2/§12.5's rows **verbatim**, including the per-category counts the
//!   spec adds up to 2300. `infy conformance --category` needs that table, and §12.5 makes "只报总数"
//!   itself non-conforming, so the table is data rather than prose.
//! * The case format. §12 names categories, counts and checks, but never says what a case looks like
//!   on disk — that gap is decided here and recorded as `docs/spec-deviations.md` D-47.
//! * [`run_case`] — one case in, the verdict `infy check` would give for it out.
//!
//! The corpus layout:
//!
//! ```text
//! conformance/cases/2-lexical/CATEGORY.toml                  # the §12 row, cross-checked against CATEGORIES
//! conformance/cases/2-lexical/2-lexical-0001/case.toml        # the expectation
//! conformance/cases/2-lexical/2-lexical-0001/main.infy        # the INFY source, byte for byte
//! ```
//!
//! One case is one directory, never one file with expectation comments inside the source: §12.1 tests
//! comments (§2.5), §12.5's `§9.5` category tests comment *retention*, and metadata baked into the
//! source would make both of them self-referential.
//!
//! The runner goes through [`crate::cli::check_verdict`] — the same functions `infy check` runs,
//! stages [1]-[5] plus [6a] — rather than a second front-end path. A corpus that exercised a private
//! re-implementation would stay green while the command itself is broken; this repo has been bitten
//! by exactly that shape twice (D1(2) and D2).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::span::{FileId, Sources};
use crate::diag::Handler;
use crate::{cli, codes, fmt, ifbc, pkg, toml};

/// The filename inside a case directory that carries the expectation.
pub const CASE_FILE: &str = "case.toml";
/// The filename inside a category directory that restates its §12 row.
pub const CATEGORY_FILE: &str = "CATEGORY.toml";

/// One row of §12's category tables. `title` and `check` are copied out of the spec so a reviewer
/// can diff them against `spec/infinity_lang_spec.md` without re-reading the tables.
#[derive(Clone, Copy, Debug)]
pub struct Category {
    /// Directory name under `conformance/cases/`, and the prefix of every case id in it.
    pub slug: &'static str,
    /// Which §12 table the row comes from.
    pub table: &'static str,
    /// The row's 分类 column, verbatim.
    pub title: &'static str,
    /// The row's 检查 column, verbatim (minus the table's markdown emphasis).
    pub check: &'static str,
    /// The row's 数量 column — the spec's own target, not this build's achievement.
    pub required: usize,
}

/// §12.1 (9 rows, 1244) + §12.2 (3 rows, 256) + §12.5 (8 rows, 800) = 2300.
pub const CATEGORIES: &[Category] = &[
    // ---------------------------------------------------------------- §12.1 前端与语义
    Category {
        slug: "2-lexical",
        table: "§12.1",
        title: "§2 词法（含 build 约束）",
        check: "编码、标识符、生命周期记法、字面量、注释、约束表达式",
        required: 96,
    },
    Category {
        slug: "3-grammar",
        table: "§12.1",
        title: "§3 文法",
        check: "每条 EBNF 规则正反例，含 §3.10 消歧",
        required: 288,
    },
    Category {
        slug: "4-types-ops",
        table: "§12.1",
        title: "§4.1–§4.4 类型与运算",
        check: "隐式转换禁令、溢出策略、`as` 位级语义",
        required: 120,
    },
    Category {
        slug: "4.7-ownership",
        table: "§12.1",
        title: "§4.7 所有权/借用/生命周期",
        check: "E-1001..E-1005 每类正反 20 例；elision 三规则；drop 顺序",
        required: 200,
    },
    Category {
        slug: "4.9-ub",
        table: "§12.1",
        title: "§4.9 UB 清单",
        check: "24 条禁令逐条必须被识别为编译错误或规范 panic",
        required: 120,
    },
    Category {
        slug: "5-modules",
        table: "§12.1",
        title: "§5 模块/包/`.infib`/校验",
        check: "目录=包、internal、MVS、循环依赖、dirhash、wheel/源码版等价性",
        required: 144,
    },
    Category {
        slug: "6-macros",
        table: "§12.1",
        title: "§6 宏与属性",
        check: "卫生性、深度、片段分类、属性封闭表",
        required: 96,
    },
    Category {
        slug: "8-chains",
        table: "§12.1",
        title: "§8 链与控制流",
        check: "`goto` 编译期解析、节点静态编号、FIFO、多焦点",
        required: 128,
    },
    Category {
        slug: "1.3-domain-neutral",
        table: "§12.1",
        title: "§1.3 领域中立",
        check: "静态扫描 `std.*` 与语言层导出符号表，命中领域词表黑名单（`domain_terms.txt`）即判不合规；同时校验 `env` 字段集合封闭",
        required: 52,
    },
    // ---------------------------------------------------------------- §12.2 嵌入
    Category {
        slug: "7.6-a-track",
        table: "§12.2",
        title: "§7.6 A 轨",
        check: "绑定生成、符号命名、所有权映射、缺失工具链必须编译期失败、`isolation=\"process\"` 语义等价",
        required: 96,
    },
    Category {
        slug: "7.7-b-track",
        table: "§12.2",
        title: "§7.7 B 轨",
        check: "能力扫描正反例（每类 `E-0712`）、`E-0711` 动态代码生成拒绝、`.pyc` magic 校验（`E-0713`）、行映射回原语言、`dyn` 边界、`infy ingest py-wheel` 端到端",
        required: 96,
    },
    Category {
        slug: "7.8-ifbc",
        table: "§12.2",
        title: "§7.8 IFBC",
        check: "校验器六项各自的正反例（必须拒绝畸形字节码，不得\"部分执行\"）",
        required: 64,
    },
    // ---------------------------------------------------------------- §12.5 分类扩展
    Category {
        slug: "2.8-literals",
        table: "§12.5",
        title: "§2.8–§2.12 字面量/字符串/const-eval",
        check: "分隔符、进制、字面量溢出、多行 raw 缩进剥离、转义封闭表、confusables",
        required: 96,
    },
    Category {
        slug: "3.11-expr-pat-stmt",
        table: "§12.5",
        title: "§3.11–§3.13 表达式/模式/语句",
        check: "自动引用阶梯顺序、结构体更新移动、穷尽性与可达性、`defer` 顺序",
        required: 128,
    },
    Category {
        slug: "4.10-eval-order",
        table: "§12.5",
        title: "§4.10–§4.16 求值顺序与推断",
        check: "每条求值顺序规则一个可观察副作用用例、`?` 展开式、`[lints]` 不可降级 `E-`",
        required: 144,
    },
    Category {
        slug: "6.7-derive",
        table: "§12.5",
        title: "§6.7–§6.8 derive",
        check: "每个 derive 的生成语义（含 `Ord` 字典序、`Hash` 字段判别式前缀）+ `infy expand` 字节稳定",
        required: 72,
    },
    Category {
        slug: "8.4-node-focus",
        table: "§12.5",
        title: "§8.4–§8.5 节点表与焦点",
        check: "节点编号跨实现/跨优化一致、取消传播矩阵、抢占点封闭性",
        required: 96,
    },
    Category {
        slug: "9.5-fmt",
        table: "§12.5",
        title: "§9.5 `infy fmt`",
        check: "幂等、AST 不变、注释保留、`@embed` 不透明、三档行宽各自黄金",
        required: 120,
    },
    Category {
        slug: "10.9-std",
        table: "§12.5",
        title: "§10.9–§10.14 std 补充",
        check: "无偏 shuffle 序列、`Map` 有序迭代、`Rng` 种子可复现、`ParseIntError.idx`",
        required: 84,
    },
    Category {
        slug: "11.1-host-protocol",
        table: "§12.5",
        title: "§11.1–§11.4 宿主协议",
        check: "三步握手失败码、串行化锁与 FIFO 不互锁、镜像表 ID 一致性",
        required: 60,
    },
];

/// What one case asks the toolchain to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Stages [1]-[5] (+ [6a] when [1]-[5] came back clean) over the case directory as a module.
    Check,
    /// §9.5: format the case's file, format the result again, both passes must be identical and
    /// produce the expected diagnostics.
    FmtIdempotent,
    /// §7.8.3: load an IFBC image and report the validator's codes.
    IfbcVerify,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "check" => Some(Kind::Check),
            "fmt-idempotent" => Some(Kind::FmtIdempotent),
            "ifbc-verify" => Some(Kind::IfbcVerify),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::FmtIdempotent => "fmt-idempotent",
            Kind::IfbcVerify => "ifbc-verify",
        }
    }
}

/// A case as written on disk.
#[derive(Clone, Debug)]
pub struct Case {
    /// Stable identifier: `conformance/cases/<category>/<id>/`. §12 forbids reusing a number or
    /// changing an existing case's expectation, so ids are append-only.
    pub id: String,
    pub category: &'static Category,
    /// The clause the case pins, e.g. `§2.2`.
    pub clause: String,
    pub title: String,
    pub kind: Kind,
    /// Expected diagnostic codes (compared as a sorted multiset).
    pub expect: Vec<String>,
    pub expect_exit: i32,
    /// The file the kind operates on, relative to the case directory (`fmt-idempotent` only).
    pub file: Option<String>,
    /// The image to validate, relative to the repository root (`ifbc-verify` only). Paths are
    /// shared with `runtime/fixtures` deliberately — a copy of a fixture would be a second truth.
    pub image: Option<String>,
    /// Build-time tools the case is meaningless without (§7.7.3's interpreter, §7.6.2's `cc`).
    pub requires: Vec<String>,
    /// A BACKLOG item for a known-unfixed defect. Such a case is **never** counted as passing:
    /// while it still misses its (spec-derived) expectation it reports `Blocked`, and the moment it
    /// passes the marker is stale and the test suite says so.
    pub defect: Option<String>,
    /// The case directory.
    pub path: PathBuf,
}

/// What happened when a case ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    /// The case could not be judged here: a required build-time tool is missing.
    Blocked,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub status: Status,
    /// Codes the toolchain actually produced, sorted.
    pub got: Vec<String>,
    /// Exit code the toolchain actually produced (`-1` = never got that far).
    pub exit: i32,
    pub detail: String,
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The corpus root (§12's `conformance/cases/**`).
pub fn corpus_root() -> PathBuf {
    repo_root().join("conformance").join("cases")
}

/// Where a case is materialized before it runs. §5.1 makes a module a directory tree, so a case
/// becomes a real module under `target/scratch/conformance/<id>`; nothing outside `target/` is
/// written by the runner.
fn scratch_root() -> PathBuf {
    let dir = repo_root().join("target").join("scratch").join("conformance");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn category_by_slug(slug: &str) -> Option<&'static Category> {
    CATEGORIES.iter().find(|c| c.slug == slug)
}

/// §12's own arithmetic, as an assertion the tests can call.
pub fn required_total(table: &str) -> usize {
    CATEGORIES.iter().filter(|c| c.table == table).map(|c| c.required).sum()
}

/// Read every case under `root`. All problems are collected, never short-circuited: a corpus that
/// silently skips a malformed case would report a pass rate over fewer cases than exist, which is
/// the failure mode §12.5 is aimed at.
pub fn load_cases(root: &Path) -> Result<Vec<Case>, Vec<String>> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    if !root.is_dir() {
        return Err(vec![format!("用例根目录不存在：{}", root.display())]);
    }
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let entries = sorted_entries(root);
    let mut found: Vec<&'static str> = Vec::new();
    for e in entries {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !path.is_dir() {
            errs.push(format!("{}: 分类目录下只允许目录（这里是文件 `{name}`）", root.display()));
            continue;
        }
        let cat = match category_by_slug(&name) {
            Some(c) => c,
            // A typo in a category directory name would otherwise drop every case inside it.
            None => {
                errs.push(format!("{}: 未知分类目录 `{name}`（§12.1/§12.2/§12.5 之外的分类不存在）", path.display()));
                continue;
            }
        };
        found.push(cat.slug);
        verify_category_file(&path, cat, &mut errs);
        for sub in sorted_entries(&path) {
            let sub_path = sub.path();
            let sub_name = sub.file_name().to_string_lossy().to_string();
            if sub_path.is_file() {
                if sub_name == CATEGORY_FILE {
                    continue;
                }
                if sub_name == "README.md" {
                    continue;
                }
                errs.push(format!("{}: 分类目录里只允许 `{CATEGORY_FILE}`、`README.md` 与用例目录（这里是 `{sub_name}`）", path.display()));
                continue;
            }
            match load_case(&sub_path, cat) {
                Ok(c) => {
                    if let Some(prev) = seen.get(&c.id) {
                        errs.push(format!("{}: 用例编号 `{}` 与 {prev} 重复（§12：编号不得复用）", sub_path.display(), c.id));
                        continue;
                    }
                    seen.insert(c.id.clone(), sub_path.display().to_string());
                    out.push(c);
                }
                Err(e) => errs.push(e),
            }
        }
    }
    for cat in CATEGORIES {
        if !found.iter().any(|s| *s == cat.slug) {
            errs.push(format!("{}: 缺 §12 分类目录 `{}`（{}，规范要求 {} 个用例）", root.display(), cat.slug, cat.title, cat.required));
        }
    }
    if errs.is_empty() {
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    } else {
        Err(errs)
    }
}

/// The row on disk must say the same thing as the compiled row — otherwise a category's target
/// count can drift from the spec while both files still look self-consistent.
fn verify_category_file(dir: &Path, cat: &Category, errs: &mut Vec<String>) {
    let path = dir.join(CATEGORY_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            errs.push(format!("{}: 无法读取 {CATEGORY_FILE}：{e}", dir.display()));
            return;
        }
    };
    let v = match toml::parse(&text) {
        Ok(v) => v,
        Err(e) => {
            errs.push(format!("{}: {CATEGORY_FILE} 解析失败：{}", path.display(), e.message()));
            return;
        }
    };
    let want: [(&str, String); 5] = [
        ("slug", cat.slug.to_string()),
        ("table", cat.table.to_string()),
        ("title", cat.title.to_string()),
        ("check", cat.check.to_string()),
        ("required", cat.required.to_string()),
    ];
    for (key, expect) in want {
        let got = v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string()).or_else(|| v.get(key).and_then(|x| x.as_int()).map(|i| i.to_string()));
        match got {
            None => errs.push(format!("{}: {CATEGORY_FILE} 缺 `{key}`（应为 {expect}）", path.display())),
            Some(g) if g != expect => errs.push(format!("{}: {CATEGORY_FILE} 的 `{key}` = `{g}`，与 §12 表里的 `{expect}` 不一致", path.display())),
            Some(_) => {}
        }
    }
}

fn load_case(dir: &Path, cat: &'static Category) -> Result<Case, String> {
    let path = dir.join(CASE_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: 无法读取 {CASE_FILE}：{e}", dir.display()))?;
    let v = toml::parse(&text).map_err(|e| format!("{}: {CASE_FILE} 解析失败：{}", path.display(), e.message()))?;
    let dir_name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let str_field = |key: &str, required: bool| -> Result<String, String> {
        match v.get(key).and_then(|x| x.as_str()) {
            Some(s) => Ok(s.to_string()),
            None if required => Err(format!("{}: {CASE_FILE} 缺 `{key}`", path.display())),
            None => Ok(String::new()),
        }
    };
    let id = str_field("id", true)?;
    if id != dir_name {
        return Err(format!("{}: `id = \"{id}\"` 与目录名 `{dir_name}` 不一致（编号即路径，§12 要求编号不复用）", path.display()));
    }
    let category = str_field("category", true)?;
    if category != cat.slug {
        return Err(format!("{}: `category = \"{category}\"` 与所在目录 `{}` 不一致", path.display(), cat.slug));
    }
    let clause = str_field("clause", true)?;
    if !clause.starts_with('§') {
        return Err(format!("{}: `clause = \"{clause}\"` 必须以 `§` 开头（规范条款是用例的身份）", path.display()));
    }
    let title = str_field("title", true)?;
    let kind = match Kind::parse(&str_field("kind", true)?) {
        Some(k) => k,
        None => return Err(format!("{}: 未知 `kind`（可用：check / fmt-idempotent / ifbc-verify）", path.display())),
    };
    let expect = match v.get("expect") {
        None => return Err(format!("{}: 缺 `expect`（正例也要写 `expect = []`，空是声称而不是漏）", path.display())),
        Some(x) => {
            let mut codes = Vec::new();
            for item in x.as_array().ok_or_else(|| format!("{}: `expect` 必须是字符串数组", path.display()))? {
                let c = item.as_str().ok_or_else(|| format!("{}: `expect` 的元素必须是字符串", path.display()))?;
                // A typo in an expected code would ask the runner for a code that can never appear.
                if codes::lookup(c).is_none() {
                    return Err(format!("{}: 期望码 `{c}` 不在 §9.1–§9.8 的码表里（codes::lookup）", path.display()));
                }
                codes.push(c.to_string());
            }
            codes
        }
    };
    let expect_exit = v.get("exit").and_then(|x| x.as_int()).ok_or_else(|| format!("{}: 缺整数 `exit`（§13.4）", path.display()))?;
    if !(0..=7).contains(&expect_exit) {
        return Err(format!("{}: `exit = {expect_exit}` 不在 §13.4 的 0–7 里", path.display()));
    }
    let requires = string_array(&v, "requires", &path)?;
    for t in &requires {
        if !TOOLS.contains(&t.as_str()) {
            return Err(format!("{}: `requires` 里的 `{t}` 不是已知构建期工具（{:?}）", path.display(), TOOLS));
        }
    }
    let defect = str_field("defect", false)?;
    let file = str_field("file", false)?;
    let image = str_field("image", false)?;
    match kind {
        Kind::Check => {
            if !has_infy(dir)? {
                return Err(format!("{}: kind = check 的用例至少要有一个 `.infy` 源文件", path.display()));
            }
        }
        Kind::FmtIdempotent => {
            if file.is_empty() {
                return Err(format!("{}: kind = fmt-idempotent 需要 `file`（相对用例目录的 .infy 路径）", path.display()));
            }
            if !dir.join(&file).is_file() {
                return Err(format!("{}: `file = \"{file}\"` 指向的文件不存在", path.display()));
            }
        }
        Kind::IfbcVerify => {
            if image.is_empty() {
                return Err(format!("{}: kind = ifbc-verify 需要 `image`（相对仓库根的镜像路径）", path.display()));
            }
            if !repo_root().join(&image).is_file() {
                return Err(format!("{}: `image = \"{image}\"` 指向的镜像不存在", path.display()));
            }
        }
    }
    Ok(Case {
        id,
        category: cat,
        clause,
        title,
        kind,
        expect,
        expect_exit: expect_exit as i32,
        file: (!file.is_empty()).then_some(file),
        image: (!image.is_empty()).then_some(image),
        requires,
        defect: (!defect.is_empty()).then_some(defect),
        path: dir.to_path_buf(),
    })
}

fn string_array(v: &toml::Value, key: &str, path: &Path) -> Result<Vec<String>, String> {
    match v.get(key) {
        None => Ok(Vec::new()),
        Some(x) => {
            let mut out = Vec::new();
            for item in x.as_array().ok_or_else(|| format!("{}: `{key}` 必须是字符串数组", path.display()))? {
                out.push(item.as_str().ok_or_else(|| format!("{}: `{key}` 的元素必须是字符串", path.display()))?.to_string());
            }
            Ok(out)
        }
    }
}

fn has_infy(dir: &Path) -> Result<bool, String> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in sorted_entries(&d) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.to_string_lossy().ends_with(".infy") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn sorted_entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    let mut v: Vec<std::fs::DirEntry> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return Vec::new(),
    };
    v.sort_by_key(|e| e.file_name());
    v
}

/// Build-time tools a case can name in `requires`.
pub const TOOLS: &[&str] = &["cc", "python", "lua"];

fn tool_present(name: &str) -> bool {
    match name {
        "python" => crate::btrack::find_python().is_some(),
        "lua" => runs("lua"),
        "cc" => std::env::var("CC").ok().filter(|p| runs(p)).is_some() || ["gcc", "cc", "clang"].iter().any(|c| runs(c)),
        _ => false,
    }
}

fn runs(prog: &str) -> bool {
    std::process::Command::new(prog)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The manifest a case gets when it does not ship its own: the smallest thing §5.1 accepts, with
/// §4.9's `overflow = "panic"` so an arithmetic case cannot quietly wrap.
pub const DEFAULT_MANIFEST: &str = "[module]\npath    = \"example/conformance\"\nversion = \"0.1.0\"\ninfy    = \"2.0\"\n\n[dependencies]\n\"std\" = \"1.0\"\n\n[profile]\noverflow = \"panic\"\n";

/// `run_case` keys its materialized module by case id alone (`target/scratch/conformance/<id>`), and
/// materializing an id deletes that directory first — so two corpus runs inside one process pull each
/// other's tree out from under a read. The symptom is not a lock error but a *semantic* one: a bogus
/// `E-0600 产物无入口`, or os error 3 on the manifest path, on whichever case another thread happens
/// to be re-materializing. E3 pinned this down with a lock inside `conformance::tests`, which did not
/// hold: `cli::tests` runs the same corpus through the public `run_case`, and a `mod tests` lock
/// cannot reach it. One lock, taken where the shared path is touched, is the only shape no caller can
/// forget — so it lives here, not in a test.
static SCRATCH_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run one case. Never panics, never prints, and never reports `Pass` for something it did not do:
/// a missing tool or a case that misses its expectation both come back with a reason.
pub fn run_case(c: &Case) -> Outcome {
    let _scratch = SCRATCH_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for t in &c.requires {
        if !tool_present(t) {
            return Outcome {
                status: Status::Blocked,
                got: Vec::new(),
                exit: -1,
                detail: format!("本机没有构建期工具 `{t}`（§7.7.3/§7.6.2 把工具在场列为前提，缺它不算通过也不算失败）"),
            };
        }
    }
    let ran = match c.kind {
        Kind::Check => run_check(c),
        Kind::FmtIdempotent => run_fmt(c),
        Kind::IfbcVerify => run_ifbc(c),
    };
    let (got, exit, detail) = match ran {
        Ok(r) => r,
        Err(e) => return Outcome { status: Status::Fail, got: Vec::new(), exit: -1, detail: e },
    };
    let mut want = c.expect.clone();
    want.sort();
    let mut gots = got.clone();
    gots.sort();
    let matched = want == gots && exit == c.expect_exit;
    let mismatch = format!("期望 {want:?} / exit {}，实得 {gots:?} / exit {exit}", c.expect_exit);
    match (matched, &c.defect) {
        (true, None) => Outcome { status: Status::Pass, got: gots, exit, detail: String::new() },
        (true, Some(d)) => Outcome {
            status: Status::Fail,
            got: gots,
            exit,
            detail: format!("用例通过了，但仍挂着 `defect = \"{d}\"`：{d} 已修的话请删掉这个标记（§12 的期望不得改，标记也不能留）"),
        },
        (false, Some(d)) => Outcome {
            status: Status::Blocked,
            got: gots,
            exit,
            detail: format!("{mismatch} —— 已知缺陷 {d} 未修，记为阻塞而不是通过\n{detail}"),
        },
        (false, None) => Outcome { status: Status::Fail, got: gots, exit, detail: format!("{mismatch}\n{detail}") },
    }
}

fn run_check(c: &Case) -> Result<(Vec<String>, i32, String), String> {
    let dir = materialize(c)?;
    let v = cli::check_verdict(&dir, &cli::Scope::default()).ok_or_else(|| format!("{}: 落成 module 后仍找不到 {}", c.path.display(), pkg::MANIFEST_NAME))?;
    Ok((v.codes, v.exit, v.rendered))
}

fn run_fmt(c: &Case) -> Result<(Vec<String>, i32, String), String> {
    let rel = c.file.as_ref().ok_or("`file` 缺失")?;
    let text = std::fs::read_to_string(c.path.join(rel)).map_err(|e| format!("无法读取 {}: {e}", rel))?;
    let (once, h1) = format_once(&text, rel);
    let once = once.ok_or_else(|| "§9.5 第一轮格式化就失败（`format_file` 返回 None）".to_string())?;
    let (twice, h2) = format_once(&once, rel);
    match twice {
        None => return Err("§9.5 对自身的输出再格式化一次失败（幂等性的第二遍不该读不回）".to_string()),
        Some(t) if t != once => {
            return Err(format!("§9.5 幂等性失败：二次格式化与一次不同（{} → {} 字节）", once.len(), t.len()))
        }
        Some(_) => {}
    }
    let codes = h1.visible().iter().map(|d| d.code.clone()).collect();
    let exit = h1.exit_code();
    let (src, _) = one_file(rel, &text);
    let mut detail = h1.render(&src);
    if h2.visible().len() != h1.visible().len() {
        detail.push_str(&format!("\n第二遍格式化的诊断数也不同：{} vs {}", h2.visible().len(), h1.visible().len()));
    }
    Ok((codes, exit, detail))
}

fn format_once(text: &str, display: &str) -> (Option<String>, Handler) {
    let (src, id) = one_file(display, text);
    let mut h = Handler::default();
    let opts = fmt::FmtOptions::default();
    let out = fmt::format_file(&src, id, &mut h, &opts);
    (out, h)
}

/// A one-file [`Sources`], with the id [`Sources::add`] handed out rather than an assumed `0`.
fn one_file(display: &str, text: &str) -> (Sources, FileId) {
    let mut s = Sources::default();
    let id = s.add(display.replace('\\', "/"), text.to_string());
    (s, id)
}

fn run_ifbc(c: &Case) -> Result<(Vec<String>, i32, String), String> {
    let rel = c.image.as_ref().ok_or("`image` 缺失")?;
    let bytes = std::fs::read(repo_root().join(rel)).map_err(|e| format!("无法读取 {rel}: {e}"))?;
    let m = ifbc::decode(&bytes).map_err(|e| format!("镜像无法装载（§7.8.3 项 1 的静态对应）：{e}"))?;
    let vs = ifbc::verify(&m, &ifbc::HostSigTable::std_default(), true);
    let codes: Vec<String> = vs.iter().map(|v| v.code.to_string()).collect();
    let blocking = vs.iter().filter(|v| !v.code.starts_with('W')).count();
    let detail = vs.iter().map(|v| v.render()).collect::<Vec<_>>().join("\n");
    // Same exit attribution as `infy ifbc verify` (D-45(c)): a §7.8.3 blocker is exit 2.
    Ok((codes, if blocking > 0 { 2 } else { 0 }, detail))
}

/// Copy the case directory into a scratch module. `case.toml`/`CATEGORY.toml` stay behind: the
/// module a case becomes must contain only what an author would ship.
fn materialize(c: &Case) -> Result<PathBuf, String> {
    let dest = scratch_root().join(&c.id);
    let _ = std::fs::remove_dir_all(&dest);
    copy_tree(&c.path, &dest).map_err(|e| format!("落成 module 失败：{e}"))?;
    if !dest.join(pkg::MANIFEST_NAME).is_file() {
        std::fs::write(dest.join(pkg::MANIFEST_NAME), DEFAULT_MANIFEST).map_err(|e| format!("写默认清单失败：{e}"))?;
    }
    // §7.7.3 makes the declared `py = "<minor>"` an agreement with the interpreter this build
    // actually finds, so a case that wants a *matching* declaration cannot hard-code one — the file
    // says `@PY_MINOR@` and the runner fills in what the probe reported. Without this the same
    // committed case would be green on 3.14 and `E-0713`-red on 3.13, i.e. the corpus would be
    // measuring the machine. (The negative shape needs no substitution: declaring `3.9` is stable.)
    if c.requires.iter().any(|t| t == "python") {
        let minor = python_minor()?;
        substitute_in_tree(&dest, PY_MINOR_TOKEN, &minor)?;
    }
    Ok(dest)
}

/// The token a B-track case writes where it means "this interpreter's minor version".
pub const PY_MINOR_TOKEN: &str = "@PY_MINOR@";

fn python_minor() -> Result<String, String> {
    let py = crate::btrack::find_python().ok_or("阶段 [6a] 需要构建期 CPython（§7.7.3），本机没找到")?;
    let full = crate::btrack::probe_version(&py).ok_or_else(|| "CPython 在场但探针读不出版本".to_string())?;
    // §7.7.3 agrees on the *minor*; a patch-level declaration is what D-44(f) settled.
    Ok(full.split('.').take(2).collect::<Vec<_>>().join("."))
}

fn substitute_in_tree(dir: &Path, token: &str, value: &str) -> Result<(), String> {
    for e in sorted_entries(dir) {
        let p = e.path();
        if p.is_dir() {
            substitute_in_tree(&p, token, value)?;
            continue;
        }
        // Binary inputs (an IFBC image a case ships) are not text and are never rewritten.
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        if !text.contains(token) {
            continue;
        }
        std::fs::write(&p, text.replace(token, value)).map_err(|e| format!("替换 {token} 失败：{e}"))?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(to)?;
    for e in sorted_entries(from) {
        let name = e.file_name().to_string_lossy().to_string();
        if name == CASE_FILE || name == CATEGORY_FILE || name == "README.md" {
            continue;
        }
        let src = e.path();
        let dst = to.join(&name);
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// §12.5's per-category tally: the numbers `infy conformance --report` must print (E2). Kept here
/// because the corpus tests need the same arithmetic to say what is missing.
#[derive(Clone, Debug)]
pub struct Tally {
    pub category: &'static Category,
    pub cases: usize,
    pub pass: usize,
    pub fail: usize,
    pub blocked: usize,
}

impl Tally {
    /// §12.1/§12.2/§12.5 counts are the spec's target, so a shortfall is a fact to print, not a
    /// rounding to hide.
    pub fn shortfall(&self) -> usize {
        self.category.required.saturating_sub(self.cases)
    }
}

/// Group `cases`/`outcomes` by category, in `CATEGORIES` order (every declared category appears,
/// including the empty ones — dropping them is how "只报总数" happens by accident).
pub fn tally(cases: &[(Case, Outcome)]) -> Vec<Tally> {
    let mut out = Vec::new();
    for cat in CATEGORIES {
        let mut t = Tally { category: cat, cases: 0, pass: 0, fail: 0, blocked: 0 };
        for (c, o) in cases {
            if c.category.slug != cat.slug {
                continue;
            }
            t.cases += 1;
            match o.status {
                Status::Pass => t.pass += 1,
                Status::Fail => t.fail += 1,
                Status::Blocked => t.blocked += 1,
            }
        }
        out.push(t);
    }
    out
}

/// Convenience for E2 and the tests: load and run the whole corpus.
pub fn run_corpus(root: &Path) -> Result<Vec<(Case, Outcome)>, Vec<String>> {
    let cases = load_cases(root)?;
    Ok(cases.into_iter().map(|c| {
        let o = run_case(&c);
        (c, o)
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_category_table_adds_up_to_the_specs_own_totals() {
        // §12 states 1244 + 256 + 800 = 2300. If a row's count is mistyped, or a row is added or
        // dropped, the arithmetic stops matching the spec's own sentence.
        assert_eq!(1244, required_total("§12.1"), "§12.1 的合计");
        assert_eq!(256, required_total("§12.2"), "§12.2 的合计");
        assert_eq!(800, required_total("§12.5"), "§12.5 的合计");
        assert_eq!(2300, CATEGORIES.iter().map(|c| c.required).sum::<usize>(), "§12 的总数");
        assert_eq!(20, CATEGORIES.len(), "§12.1 的 9 + §12.2 的 3 + §12.5 的 8");
    }

    #[test]
    fn category_slugs_and_directories_are_unique_and_complete() {
        let mut seen: Vec<&str> = CATEGORIES.iter().map(|c| c.slug).collect();
        let n = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(n, seen.len(), "分类 slug 有重复");
        for c in CATEGORIES {
            assert!(corpus_root().join(c.slug).is_dir(), "缺分类目录 conformance/cases/{}", c.slug);
        }
    }

    #[test]
    fn the_corpus_loads_without_a_single_format_error() {
        if let Err(errs) = load_cases(&corpus_root()) {
            panic!("用例集格式错误 {} 条：\n{}", errs.len(), errs.join("\n"));
        }
    }

    #[test]
    fn every_case_in_the_corpus_meets_its_expectation_or_is_marked_blocked() {
        let cases = match load_cases(&corpus_root()) {
            Ok(c) => c,
            Err(errs) => panic!("用例集加载失败：\n{}", errs.join("\n")),
        };
        let ran: Vec<(Case, Outcome)> = cases.iter().map(|c| (c.clone(), run_case(c))).collect();
        let mut bad = Vec::new();
        for (c, o) in &ran {
            if o.status == Status::Fail {
                // First line of the detail is the `期望 … / 实得 …` diff; the §9.2 rendering that
                // follows is what a human fixing the case needs, so keep it but never truncated.
                bad.push(format!(
                    "{} ({} · {} · kind={}): {}",
                    c.id, c.category.slug, c.clause, c.kind.as_str(), o.detail
                ));
            }
        }
        assert!(bad.is_empty(), "用例集里有 {} 条不达期望：\n{}", bad.len(), bad.join("\n"));
        let t = tally(&ran);
        let pass: usize = t.iter().map(|x| x.pass).sum();
        let blocked: usize = t.iter().map(|x| x.blocked).sum();
        assert_eq!(ran.len(), pass + blocked, "既不是通过也不是阻塞的用例被静默吞掉了");
        assert!(ran.len() >= 20, "E1 的首批搬运至少要有 20 个用例，实得 {}", ran.len());
    }

    #[test]
    fn a_case_that_misses_its_expectation_fails_loudly_instead_of_being_skipped() {
        // The corpus's own credibility: prove the runner can go red. Two shapes of "wrong" are
        // refused at different layers — a code outside §9's tables is a *format* error the loader
        // rejects (so it can never be misread as "did not match"), while a real code this source
        // cannot produce must come back `Fail`, not `Pass` and not skipped.
        let dir = selftest_corpus("wrong-expectation");
        let case_dir = dir.join("2-lexical").join("2-lexical-9000");
        std::fs::create_dir_all(&case_dir).unwrap();
        std::fs::write(case_dir.join("main.infy"), "fn f() -> u32 { 1 }\n").unwrap();
        std::fs::write(case_dir.join(CASE_FILE), case_toml("2-lexical-9000", "2-lexical", "§4.1", "check", "[\"E-0999\"]", "2", "")).unwrap();
        let errs = load_cases(&dir).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("不在 §9.1")), "未知期望码没被拒：{errs:?}");
        // `E-0505` is a real code (目录名不可导入) that a root-level `main.infy` cannot produce.
        std::fs::write(case_dir.join(CASE_FILE), case_toml("2-lexical-9000", "2-lexical", "§4.1", "check", "[\"E-0505\"]", "2", "")).unwrap();
        let cases = load_cases(&dir).expect("selftest 用例应能加载");
        assert_eq!(1, cases.len());
        let o = run_case(&cases[0]);
        assert_eq!(Status::Fail, o.status, "一个不可能报 E-0505 的用例被记成 {:?}", o.status);
        assert!(o.detail.contains("期望"), "{:?}", o.detail);
    }

    #[test]
    fn an_unknown_category_directory_is_a_load_error_not_a_silent_drop() {
        // §2.2's lesson (C5): filtering a file set silently is how "the case never existed" happens.
        let dir = selftest_corpus("unknown-category");
        std::fs::create_dir_all(dir.join("2-lexicali")).unwrap();
        let errs = load_cases(&dir).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("未知分类目录 `2-lexicali`")), "{errs:?}");
        assert!(!errs.iter().any(|e| e.contains("缺 §12 分类目录")), "selftest_corpus 已建齐 20 个分类：{errs:?}");
    }

    #[test]
    fn a_case_id_that_does_not_match_its_directory_is_a_load_error() {
        let dir = selftest_corpus("id-drift");
        let case_dir = dir.join("2-lexical").join("2-lexical-0001");
        std::fs::create_dir_all(&case_dir).unwrap();
        std::fs::write(case_dir.join("main.infy"), "fn f() { }\n").unwrap();
        std::fs::write(case_dir.join(CASE_FILE), case_toml("2-lexical-0002", "2-lexical", "§3", "check", "[]", "0", "")).unwrap();
        let errs = load_cases(&dir).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("与目录名")), "{errs:?}");
    }

    #[test]
    fn tally_reports_every_declared_category_including_the_empty_ones() {
        // §12.5: 只报总数视为不合规 — so the per-category view must not be able to lose a row.
        let ran: Vec<(Case, Outcome)> = Vec::new();
        let t = tally(&ran);
        assert_eq!(CATEGORIES.len(), t.len());
        assert!(t.iter().all(|x| x.cases == 0));
        assert!(t.iter().any(|x| x.shortfall() == 288), "§3 文法的缺口应可见");
    }

    /// Authoring aid: `INFY_CONFORMANCE_PROBE=1 cargo test --offline --lib -- --nocapture
    /// conformance::tests::probe_corpus_dump_verdicts` prints one line per case
    /// (`id \t 实得码 \t exit`) without judging anything. A new case's expectation is measured here
    /// and then *reviewed* against the clause it claims to pin — never copied blind.
    #[test]
    fn probe_corpus_dump_verdicts() {
        if std::env::var("INFY_CONFORMANCE_PROBE").is_err() {
            return;
        }
        let cases = load_cases(&corpus_root()).expect("用例集应能加载");
        let mut lines = Vec::new();
        for c in &cases {
            let o = run_case(c);
            lines.push(format!("{}\t{}\t{}\t{}", c.id, o.got.join(","), o.exit, format!("{:?}", o.status)));
        }
        println!("{}", lines.join("\n"));
    }

    #[test]
    fn the_default_manifest_is_the_one_the_runner_writes() {
        // A case that ships no manifest still has to be a module (§5.1), and the manifest it gets is
        // part of the corpus's meaning — pin it rather than let it float.
        assert!(DEFAULT_MANIFEST.contains("path    = \"example/conformance\""));
        assert!(DEFAULT_MANIFEST.contains("overflow = \"panic\""));
        let dir = scratch_root().join("selftest-materialize-src");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.infy"), "fn f() { }\n").unwrap();
        let c = Case {
            id: "selftest-materialize-case".to_string(),
            category: category_by_slug("2-lexical").unwrap(),
            clause: "§3".to_string(),
            title: "x".to_string(),
            kind: Kind::Check,
            expect: Vec::new(),
            expect_exit: 0,
            file: None,
            image: None,
            requires: Vec::new(),
            defect: None,
            path: dir,
        };
        let m = materialize(&c).unwrap();
        assert!(m.join(pkg::MANIFEST_NAME).is_file());
        assert!(!m.join(CASE_FILE).is_file(), "case.toml 不该进 module");
    }

    fn category_file_text(c: &Category) -> String {
        format!("slug = \"{}\"\ntable = \"{}\"\ntitle = \"{}\"\ncheck = \"{}\"\nrequired = \"{}\"\n", c.slug, c.table, c.title, c.check.replace('"', "\\\""), c.required)
    }

    /// The `CATEGORY.toml` files are a *rendering* of [`CATEGORIES`], never a hand-written second
    /// copy — the same direction split the IFBC fixtures use (`.bin` only the Rust writer writes,
    /// `.can.txt` only the C side writes). [`verify_category_file`] locks the other direction: edit
    /// the table without re-rendering and the corpus stops loading.
    ///
    /// `INFY_REGENERATE_CATEGORIES=1 cargo test --offline --lib conformance` rewrites them.
    #[test]
    fn category_files_are_rendered_from_the_compiled_table() {
        if std::env::var("INFY_REGENERATE_CATEGORIES").is_err() {
            return;
        }
        for c in CATEGORIES {
            let dir = corpus_root().join(c.slug);
            std::fs::create_dir_all(&dir).expect("分类目录");
            std::fs::write(dir.join(CATEGORY_FILE), category_file_text(c)).expect("CATEGORY.toml");
        }
    }

    /// A corpus root under `target/scratch` that holds **every** declared category directory with a
    /// matching `CATEGORY.toml` — so the "missing category" rule can never mask the error a
    /// selftest is actually about.
    fn selftest_corpus(name: &str) -> PathBuf {
        let dir = scratch_root().join(format!("selftest-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        for c in CATEGORIES {
            let cat = dir.join(c.slug);
            std::fs::create_dir_all(&cat).unwrap();
            std::fs::write(cat.join(CATEGORY_FILE), category_file_text(c)).unwrap();
        }
        dir
    }

    fn case_toml(id: &str, category: &str, clause: &str, kind: &str, expect: &str, exit: &str, tail: &str) -> String {
        format!("id = \"{id}\"\ncategory = \"{category}\"\nclause = \"{clause}\"\ntitle = \"selftest\"\nkind = \"{kind}\"\nexpect = {expect}\nexit = {exit}\n{tail}")
    }

    // ------------------------------------------------------------ E3: the documented table and the refusal
    //
    // Two separate failures are locked here, and conflating them is what let both happen elsewhere:
    //
    // * A pass-rate table copied out of `infy conformance --report` into a doc goes stale in
    //   silence — the command moves, the prose does not. So `docs/coverage.md` §6's rows are
    //   re-derived from `tally` here, cell by cell, instead of being trusted.
    // * §12.4 forbids *calling* this implementation compatible before the suite is at 100%. As a
    //   sentence in a doc that is a promise; here it is a check. The name may appear inside a
    //   prohibition (the rule's own text, quoted in BACKLOG / RUNLOG / every doc's red line), which
    //   is exactly what makes it searchable — what must never appear is a line that asserts it.

    const COMPAT_NAME: &str = "INFY v2.0 兼容";
    const COMPAT_NEGATIONS: &[&str] = &["不得", "不声称", "未声称", "不许", "才能声称", "拒绝"];

    fn doc_text(rel: &str) -> String {
        let path = repo_root().join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {rel}（{}）: {e}", path.display()))
    }

    /// Every markdown file the repo owns, except `spec/` (the normative text states the rule; it is
    /// not this implementation's claim) and build output.
    fn markdown_docs() -> Vec<String> {
        const SKIP: &[&str] = &["spec", "target", ".git", ".infy-build", "node_modules"];
        fn walk(dir: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else { continue };
                if file_type.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if SKIP.contains(&name) {
                            continue;
                        }
                    }
                    walk(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    let rel = path.strip_prefix(repo_root()).unwrap().to_string_lossy().replace('\\', "/");
                    out.push(rel);
                }
            }
        }
        let mut out = Vec::new();
        walk(&repo_root(), &mut out);
        out.sort();
        out
    }

    /// The `Fxx` defect ids mentioned by a piece of text, sorted and deduped.
    fn defect_ids(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == 'F' {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    out.push(chars[i..j].iter().collect());
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        out.sort();
        out.dedup();
        out
    }

    fn category_slugs_in_doc_order(doc: &str) -> Vec<String> {
        doc.lines()
            .filter_map(|line| {
                let after = line.strip_prefix("| `")?;
                let end = after.find('`')?;
                let slug = &after[..end];
                CATEGORIES.iter().any(|c| c.slug == slug).then(|| slug.to_string())
            })
            .collect()
    }

    #[test]
    fn the_pass_rate_table_in_coverage_md_matches_the_corpus() {
        let doc = doc_text("docs/coverage.md");
        let ran = match run_corpus(&corpus_root()) {
            Ok(r) => r,
            Err(errs) => panic!("用例集加载失败：\n{}", errs.join("\n")),
        };
        let rows = tally(&ran);

        // Same rows, same order, none added: §12.5's "逐分类" is what a doc table must not shave.
        assert_eq!(
            CATEGORIES.iter().map(|c| c.slug.to_string()).collect::<Vec<String>>(),
            category_slugs_in_doc_order(&doc),
            "docs/coverage.md §6 的 20 行必须与 `CATEGORIES` 逐一对应、按同一顺序，且不多不少"
        );

        let mut bad: Vec<String> = Vec::new();
        for r in &rows {
            let slug = r.category.slug;
            let line = doc
                .lines()
                .find(|l| l.starts_with(&format!("| `{slug}` |")))
                .unwrap_or_else(|| panic!("docs/coverage.md §6 里没有 `{slug}` 这一行"));
            let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(|c| c.trim()).collect();
            if cells.len() != 11 {
                bad.push(format!("{slug}: 该行 {n} 列，应为 11 列", n = cells.len()));
                continue;
            }
            let want = [
                format!("`{slug}`"),
                r.category.table.to_string(),
                r.category.title.to_string(),
                r.category.required.to_string(),
                r.cases.to_string(),
                r.pass.to_string(),
                r.fail.to_string(),
                r.blocked.to_string(),
                cli::conformance_rate(r.pass, r.cases),
                r.shortfall().to_string(),
            ];
            for (i, (got, exp)) in cells.iter().zip(want.iter()).enumerate() {
                if got != exp {
                    bad.push(format!("{slug} 第 {} 列 = `{got}`，命令与 `tally` 给的是 `{exp}`", i + 1));
                }
            }
            let mut defects: Vec<String> = ran
                .iter()
                .filter(|(c, _)| c.category.slug == slug)
                .filter_map(|(c, _)| c.defect.clone())
                .collect();
            defects.sort();
            defects.dedup();
            let note = cells[10];
            if r.cases == 0 {
                if !note.starts_with("结构性空分类") {
                    bad.push(format!("{slug}: 0 条用例的分类必须逐行写明是结构性空分类，实得 `{note}`"));
                }
            } else if r.blocked > 0 {
                if !note.contains("Blocked") || defect_ids(note) != defects {
                    bad.push(format!("{slug}: 非通过那一列要点名 Blocked 与它挂的缺陷号 {:?}", defects));
                }
            } else if !note.starts_with("覆盖缺口") {
                bad.push(format!("{slug}: 全通过的行只欠用例数，应写覆盖缺口，实得 `{note}`"));
            }
        }
        assert!(bad.is_empty(), "docs/coverage.md §6 的通过率表与用例集不符：\n{}", bad.join("\n"));

        // The three subtotals and the total are the report's own arithmetic, spelled the same way.
        for table in ["§12.1", "§12.2", "§12.5"] {
            let sub: Vec<&Tally> = rows.iter().filter(|r| r.category.table == table).collect();
            let cases: usize = sub.iter().map(|r| r.cases).sum();
            let pass: usize = sub.iter().map(|r| r.pass).sum();
            let fail: usize = sub.iter().map(|r| r.fail).sum();
            let blocked: usize = sub.iter().map(|r| r.blocked).sum();
            let req = required_total(table);
            let line = format!(
                "小计 {table}：规范目标 {req} · 用例 {cases} · Pass {pass} · Fail {fail} · Blocked {blocked} · 缺口 {}",
                req - cases
            );
            assert!(doc.contains(&line), "docs/coverage.md §6 少了这条小计行：`{line}`");
        }
        let req_all: usize = CATEGORIES.iter().map(|c| c.required).sum();
        let pass: usize = rows.iter().map(|r| r.pass).sum();
        let fail: usize = rows.iter().map(|r| r.fail).sum();
        let blocked: usize = rows.iter().map(|r| r.blocked).sum();
        let total_line = format!(
            "**合计：用例 {} · Pass {pass} · Fail {fail} · Blocked {blocked} · 通过率 {} · 规范目标 {req_all}（覆盖 {}，缺口 {}）**",
            ran.len(),
            cli::conformance_rate(pass, ran.len()),
            ran.len(),
            req_all - ran.len()
        );
        assert!(doc.contains(&total_line), "docs/coverage.md §6 少了这条合计行：`{total_line}`");

        // And the gap split into its three kinds, because "还差 2217 条" alone hides that three of
        // those rows are not a missing-cases problem at all.
        let short_rows = rows.iter().filter(|r| r.shortfall() > 0).count();
        let short_all: usize = rows.iter().map(|r| r.shortfall()).sum();
        let empty: Vec<&str> = rows.iter().filter(|r| r.cases == 0).map(|r| r.category.slug).collect();
        let blocked_cases = ran.iter().filter(|(_, o)| o.status == Status::Blocked).count();
        let mut all_defects: Vec<String> = ran.iter().filter_map(|(c, _)| c.defect.clone()).collect();
        all_defects.sort();
        all_defects.dedup();
        let gap_line = format!(
            "**缺口分三类**：覆盖缺口 = {short_rows}/{} 行 `short` > 0，合计 {short_all} 条；结构性空分类 = {} 行（{}）；已知未修的 Blocked = {blocked_cases} 条用例（defect：{}）",
            CATEGORIES.len(),
            empty.len(),
            empty.join(" · "),
            all_defects.join(" · ")
        );
        assert!(doc.contains(&gap_line), "docs/coverage.md §6 少了这条缺口分类：`{gap_line}`");
    }

    #[test]
    fn no_markdown_doc_claims_the_compatibility_name() {
        let mut offenders = Vec::new();
        let mut hits = 0usize;
        let files = markdown_docs();
        assert!(files.len() >= 8, "只扫到 {} 个 .md ⇒ `markdown_docs` 的遍历写错了", files.len());
        for rel in &files {
            for (n, line) in doc_text(rel).lines().enumerate() {
                if !line.contains(COMPAT_NAME) {
                    continue;
                }
                hits += 1;
                if !COMPAT_NEGATIONS.iter().any(|marker| line.contains(marker)) {
                    offenders.push(format!("{rel}:{}", n + 1));
                }
            }
        }
        assert!(hits > 0, "一个 .md 都没有出现这个名字 ⇒ 扫描对象写错了，这条测试在空转");
        assert!(
            offenders.is_empty(),
            "§12.4：合规率 100% 之前不得作出该声称。这些行出现了 `{COMPAT_NAME}` 而没有否定标记：\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn the_front_facing_docs_restate_the_refusal() {
        // Absence of a claim is not the same as stating the limit: deleting the red line would
        // satisfy the test above and make E3's deliverable worse.
        for rel in ["README.md", "docs/coverage.md", "conformance/README.md", "docs/status.md"] {
            let text = doc_text(rel);
            assert!(
                text.lines().any(|l| l.contains(COMPAT_NAME) && COMPAT_NEGATIONS.iter().any(|m| l.contains(m))),
                "{rel} 里没有那句红线（§12.4 的 100% 之前不得声称），而 E3 要求它写在面向读者的文档里"
            );
        }
    }

    /// F1：CI 是"这些测试与两份 C harness 每次推送都真跑一遍"的唯一机器凭据。命令面少一条，那一环
    /// 从此没人跑 —— 而 yaml 不是代码，没有任何编译器会替我们发现它被删了一行（D1(2)/D2 那族"上游
    /// 全绿对『这一步存不存在』没有预测力"的第一次应用恰好发生在这种文件上）。
    #[test]
    fn the_ci_workflow_covers_the_build_the_tests_and_both_c_harnesses() {
        let y = doc_text(".github/workflows/ci.yml");
        for want in [
            "cargo build --offline --release",
            "cargo test --offline",
            "for f in runtime/infy_rt.c runtime/infy_ifbc.c; do",
            "gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c",
            "gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/test_ifbc.c runtime/test_ifbc_stubs.c",
            "gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/infy_rt.c runtime/test_ifbc.c",
            "cmp stubs.out real.out",
            "\"$INFY\" fmt --check examples/tour",
            "\"$INFY\" check examples/tour",
            "\"$INFY\" build examples/tour --verify-reproducible",
            "\"$INFY\" artifact info \"$exe\" --nodes",
        ] {
            assert!(y.contains(want), "CI 里没有 `{want}` ⇒ 那一环没有步骤覆盖它");
        }
    }

    /// 这四个开关重写的是**提交进仓库**的夹具、双侧文本与 `CATEGORY.toml`。CI 一旦设了它们，
    /// "双侧逐字节比较"就变成"两侧各写一遍再比一遍自己"，红绿都失去意义。
    #[test]
    fn the_ci_workflow_never_sets_the_regeneration_switches() {
        let y = doc_text(".github/workflows/ci.yml");
        for name in [
            "INFY_REGENERATE_FIXTURE",
            "INFY_REGENERATE_CANONICAL",
            "INFY_REGENERATE_CATEGORIES",
            "INFY_CONFORMANCE_PROBE",
        ] {
            for (n, line) in y.lines().enumerate() {
                let t = line.trim();
                if t.starts_with('#') {
                    continue; // 注释里点名正是"为什么不设"的说明，允许。
                }
                for form in [format!("{name}="), format!("{name}:"), format!("export {name}")] {
                    assert!(
                        !t.contains(&form),
                        ".github/workflows/ci.yml:{} 设了重写夹具的开关：`{t}`",
                        n + 1
                    );
                }
            }
        }
    }

    /// 三条"验不到但可以钉死"的形状：runner 只有 Windows、检出改写工作树就红、rt harness 的 CWD
    /// 锁在 `target/scratch`。第三条还顺带钉住 `.gitattributes` 里给 workflow 自己钉的 LF ——
    /// `core.autocrlf=true` 的 runner 上，多行 run 段的 `do\r`/`fi\r` 是 bash 语法错误。
    #[test]
    fn the_ci_workflow_is_windows_only_and_keeps_the_three_anti_false_red_guards() {
        let y = doc_text(".github/workflows/ci.yml");
        let runs_on: Vec<String> = y
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("runs-on:"))
            .map(|l| l.to_string())
            .collect();
        assert!(!runs_on.is_empty(), "CI 里没有 `runs-on:` ⇒ 一个 job 都没有");
        for line in &runs_on {
            assert!(
                line.contains("windows"),
                "runner 是 `{line}`：阶段 [9] 产 PE + 追加 INFYART2 段、std.fs 走 CreateFileW，\
                 别的平台上跑的不是这 530 项测过的形状"
            );
        }
        assert!(y.contains("git status --porcelain"), "行尾/改写守卫没了");
        assert!(
            y.contains("grep -rlU"),
            "CR 字节守卫没了 —— porcelain 为空证明不了字节形状：第一次真 run 就是 `.infy` 被检出成 CRLF，\
             三个 cli 测试红在 exit 2（W-0001），而 git status 全程干净"
        );
        assert!(
            doc_text(".gitattributes").contains("*.infy"),
            "INFY 源码的行尾没钉成 LF ⇒ Windows runner 上 autocrlf 会把输入面改成 CRLF"
        );
        assert!(
            y.contains("cd target/scratch"),
            "rt harness 的 CWD 守卫没了 —— 它按 ../../runtime/fixtures 取夹具，换目录是两条假失败 + exit 139"
        );
        assert!(
            doc_text(".gitattributes").contains(".github/workflows/*.yml"),
            "workflow 的行尾没钉成 LF"
        );
    }

    /// cron-26：`.gitattributes` 是**后匹配优先**，而 F1 为修 run #1 的假红加的那条全局
    /// `*.infy text eol=lf` 恰好盖掉了写在它前面的 `conformance/cases/** -text` ⇒
    /// 故意带 CRLF 的那条用例（`2-lexical-0002`，blob 里 6 个 CRLF 对）在 `core.autocrlf=true` 的
    /// 检出里**永久 dirty**（clean 过滤器把 6 个 CR 抹掉 ⇒ 归一化结果 ≠ blob），run #3 就红在 CI
    /// 步 3 的工作树守卫上。**字节其实没被改写**（`eol=lf` 的 smudge 不往工作树写 CR）⇒ 这是
    /// 「守卫红而用例语义还活着」那一族；反过来 run #2 对同一条规则**没红**，因为 `git status`
    /// 命中 racy-clean 时只信 stat 缓存不比内容 ⇒ **「守卫绿」从来不等价于「树干净」**。
    /// 两头都钉：① 语料例外必须排在任何 `text eol=lf` 之后；② 那两条行尾正反例在工作树上还是
    /// 它声称的形状（把语料「顺手规范化」掉 = 用例失效，而且守卫从此空转）。
    #[test]
    fn the_corpus_eol_exception_is_declared_after_the_global_infy_rule() {
        let attrs_text = doc_text(".gitattributes");
        let attrs: Vec<&str> = attrs_text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        let global_eol = attrs
            .iter()
            .rposition(|l| l.split_whitespace().nth(1) == Some("text") && l.contains("eol=lf"))
            .expect("没有把 INFY 源码钉成 LF 的全局规则了 —— 那是 run #1 的修法，不能删");
        let corpus = attrs
            .iter()
            .rposition(|l| l.starts_with("conformance/cases") && l.split_whitespace().last() == Some("-text"))
            .expect("语料例外 `-text` 没了 ⇒ 故意带 CRLF 的用例会被 clean 过滤器改坏");
        assert!(
            corpus > global_eol,
            "第 {corpus} 行的 `conformance/cases` `-text` 必须排在第 {global_eol} 行的 `text eol=lf` \
             **之后**：`.gitattributes` 后匹配优先，反过来的次序会让全局规则盖掉语料例外 ⇒ \
             `2-lexical-0002/main.infy` 在 autocrlf=true 的检出里永久 dirty（run #3 红在 CI 步 3）"
        );

        let (Some(crlf), Some(lone)) = (
            std::fs::read(repo_root().join("conformance/cases/2-lexical/2-lexical-0002/main.infy")).ok(),
            std::fs::read(repo_root().join("conformance/cases/2-lexical/2-lexical-0001/main.infy")).ok(),
        ) else {
            panic!("读不到那两条行尾正反例");
        };
        let pairs = crlf.windows(2).filter(|w| *w == b"\r\n").count();
        assert!(
            pairs >= 6,
            "2-lexical-0002 的工作树副本只剩 {pairs} 个 CRLF 对（blob 有 6 个）⇒ 检出改写了语料的行尾，\
             这条用例（CRLF 应报 W-0001）测的已经不是它声称的那件事"
        );
        assert!(
            lone.contains(&b'\r') && !lone.windows(2).any(|w| w == b"\r\n"),
            "2-lexical-0001 必须是**裸 CR**（0 个 CRLF 对）—— 它是「clean 过滤器对它无事可做、所以它不脏」\
             的对照组；它一变脏或一变干净，说明上面那条次序断言的判据本身漂了"
        );
    }

    /// F3：仓库里只留 `docs/RUNLOG.md` 这一份审计线。
    ///
    /// 一次性产物（手工 `gcc` 的落点、`push*.log` 那一族碎片）从 `725042a` 起一路被跟踪，因为
    /// `.gitignore` 只写了 `push.log` 这一条具体名字 —— 换轮次换个名字就又是一条漏网的脏文件。
    /// 这条测试钉的是**名单形状**（按"谁产生它"列），不是磁盘上今天有什么；同时钉住反向红线：
    /// 名单不得宽到把真值源（夹具 `.bin`、文档 `.md`、源码 `.rs`、整个目录）一起吞掉 ——
    /// 那种"更干净"的写法会让跨语言逐字节锁点静默脱钩，比脏树坏得多。
    #[test]
    fn the_gitignore_lists_the_one_off_families_and_never_a_truth_source() {
        let lines: Vec<String> = doc_text(".gitignore")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_string())
            .collect();

        // 谁产生它：`*.log` + `logs/` = 三个脚本的审计日志；`*.tmp` = 写回时的中转文件；
        // `*.o` 与无扩展名那一族 = 手工 gcc 的输出（cleanup.ps1 的白名单里有它们）。
        for required in ["*.log", "logs/", "*.tmp", "*.o", "/rt_tests", "/ifbc_tests", "/a.out"] {
            assert!(
                lines.iter().any(|l| l == required),
                ".gitignore 少了 `{required}` —— 它对应的产物一产生就变成未跟踪的脏树，\
                 而 CI 的工作树守卫正是拿脏树当红"
            );
        }

        // 反向红线：整目录或整个源文件后缀一旦进名单，真值源就悄悄脱离跟踪。
        for forbidden in [
            "*.bin", "*.md", "*.rs", "*.c", "*.h", "*.txt", "*.infy", "*.toml", "*.ps1",
            "docs/", "docs", "src/", "runtime/", "spec/", "conformance/", "scripts/",
            "capabilities/", "examples/", ".github/", "*",
        ] {
            assert!(
                !lines.iter().any(|l| l == forbidden),
                ".gitignore 里出现了 `{forbidden}` —— 这条会连真值源一起吞掉 \
                 （fixtures 的 .bin 锁点、逐条款文档、脚本自身）"
            );
        }

        // 脚本的审计 log 必须落在 logs/ 下，否则 `*.log` 只救了仓库根、救不了子目录。
        let scripts = repo_root().join("scripts");
        let mut n_scripts = 0;
        for entry in std::fs::read_dir(&scripts).expect("scripts/ 目录读不到") {
            let path = entry.expect("scripts/ 条目").path();
            if path.extension().and_then(|e| e.to_str()) != Some("ps1") {
                continue;
            }
            n_scripts += 1;
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
            assert!(
                text.contains("Join-Path $repo 'logs'"),
                "{} 把审计日志写在 logs/ 外面 —— F3 之后仓库只该有一份审计线（docs/RUNLOG.md），\
                 log 落在仓库根就是重新长出 push2.log 那一族",
                path.file_name().unwrap().to_string_lossy()
            );
        }
        assert!(n_scripts >= 3, "scripts/ 里只剩 {n_scripts} 个 .ps1？风险操作走脚本是 §0.0 的硬规矩");
    }
}
