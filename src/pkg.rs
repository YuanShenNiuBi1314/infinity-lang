//! §5 — module / package / library model: directory discovery, `MODULE.infy.toml`, import path
//! resolution, `internal` visibility, MVS dependency selection, `MODULE.infy.lock`, build
//! constraints, and the §5.8 dirhash integrity model.
//!
//! The three layers are exactly Go's (§5.1): **module** (a versioned distribution unit rooted at
//! a directory containing `MODULE.infy.toml`) > **package** (one directory, one namespace) >
//! **file**. Unlike Go, export is decided by an explicit `pub`, never by name case (§2.3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::codes;
use crate::diag::{Diagnostic, Handler, Level, Phase};
use crate::span::Position;
use crate::sha256;
use crate::toml::{self, Value as TomlValue};

pub const MANIFEST_NAME: &str = "MODULE.infy.toml";
pub const LOCK_NAME: &str = "MODULE.infy.lock";
pub const TOOLCHAIN_NAME: &str = "TOOLCHAIN.infy.toml";

/// §2.2: the closed set of build-constraint names, before module `features` are added. `ignore`
/// and `test` are missing from the spec's own list (line 96) but used by its examples — §2.2's
/// `//infy:build ignore` and §14.7's `//infy:build test` — so they are admitted here; `test` is the
/// §5.9 test-build scope, the only constraint whose value depends on which command is running.
pub const BASE_CONSTRAINTS: &[&str] = &[
    "windows", "linux", "macos", "x86_64", "aarch64", "msvc", "gnu", "a_track", "b_track", "debug",
    "release", "ignore", "test",
];

/// §5.1 R3: a directory name that can appear in an import path.
pub fn dir_name_is_importable(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// §5.1 R4: dotted directories, `_`-prefixed directories and `testdata` are invisible to the
/// build system.
pub fn dir_is_skipped(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('_') || name == "testdata"
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overflow {
    Panic,
    Saturate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagLevel {
    Strict,
    Lenient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
    Oz,
}

impl OptLevel {
    pub fn flag(&self) -> &'static str {
        match self {
            OptLevel::O0 => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
            OptLevel::Oz => "-Oz",
        }
    }
    pub fn parse(s: &str) -> Option<OptLevel> {
        Some(match s {
            "0" => OptLevel::O0,
            "1" => OptLevel::O1,
            "2" => OptLevel::O2,
            "3" => OptLevel::O3,
            "z" | "Z" => OptLevel::Oz,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanicRadius {
    Focus,
    Chain,
    Process,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lto {
    Off,
    Thin,
    Fat,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub overflow: Overflow,
    pub diagnostic_level: DiagLevel,
    pub opt_level: OptLevel,
    pub stack_bytes: u64,
    pub focus_stack_bytes: u64,
    pub focus_quota: u32,
    pub iter_guard_steps: u64,
    pub a_track: Option<String>,
    pub panic_radius: PanicRadius,
    pub reproducible: bool,
    pub fast_math: bool,
    pub lto: Lto,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            overflow: Overflow::Panic,
            diagnostic_level: DiagLevel::Strict,
            opt_level: OptLevel::O2,
            stack_bytes: 8 * 1024 * 1024,
            focus_stack_bytes: 256 * 1024,
            focus_quota: 1,
            iter_guard_steps: 1 << 32,
            a_track: None,
            panic_radius: PanicRadius::Focus,
            reproducible: false,
            fast_math: false,
            lto: Lto::Fat,
        }
    }
}

/// §5.2 `[dependencies]`: `"path" = "1.5.0"` or a `^1.5` semver range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub module: String,
    pub spec: String,
    /// Declared as `std` — the mandatory built-in dependency (§5.3 step 2).
    pub is_std: bool,
}

#[derive(Clone, Debug)]
pub struct BinTarget {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct LibTarget {
    pub path: String,
    pub editions: Vec<String>,
    pub targets: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub module_path: String,
    pub version: String,
    pub infy: String,
    pub authors: Vec<String>,
    pub dependencies: Vec<Requirement>,
    pub features: BTreeMap<String, Vec<String>>,
    pub profile: Profile,
    pub b_track: BTreeMap<String, String>,
    pub bins: Vec<BinTarget>,
    pub libs: Vec<LibTarget>,
    pub lints: Vec<(String, String)>,
    pub net_private: Vec<String>,
    pub net_nosumdb: Vec<String>,
    pub unknown_keys: Vec<String>,
}

impl Manifest {
    /// The known key paths; anything else in the manifest is `W-0500` (§5.2) so a typo cannot be
    /// silently ignored.
    const KNOWN: &'static [&'static str] = &[
        "module.path", "module.version", "module.infy", "module.authors", "dependencies", "features", "profile",
        "b_track", "bin", "lib", "lints", "net",
    ];

    pub fn feature_names(&self) -> Vec<String> {
        self.features.keys().cloned().collect()
    }

    pub fn declares_a_track(&self) -> bool {
        self.profile.a_track.as_deref() == Some("unsafe-trusted")
    }
}

/// Parse `MODULE.infy.toml`. Errors are `E-0506`, unknown keys are `W-0500`.
pub fn parse_manifest(text: &str, display_path: &str, handler: &mut Handler) -> Option<Manifest> {
    let v = match toml::parse(text) {
        Ok(v) => v,
        Err(e) => {
            let (line, col) = (e.line().max(1) as u32, e.column().max(1) as u32);
            let message = e.message().to_string();
            let file = file_id(handler, display_path);
            handler.emit(
                Diagnostic::new("E-0506", Phase::Name, file, Position { line, column: col })
                    .titled(format!("清单语法错误：{message}"))
                    .region(file, line, col, col + 1, "此处无法解析")
                    .help("按 §5.2 修正 MODULE.infy.toml"),
            );
            return None;
        }
    };
    let mut unknown = Vec::new();
    collect_unknown_keys(&v, "", &mut unknown);
    let file = file_id(handler, display_path);
    for k in &unknown {
        handler.emit(
            Diagnostic::new("W-0500", Phase::Name, file, Position { line: 1, column: 1 })
                .titled(format!("清单未知键 `{k}`"))
                .note("未知的清单键会被忽略，拼写错误因此会静默失效（§5.2）")
                .help("删除该键，或按 §5.2 使用已知键名"),
        );
    }

    let mut m = Manifest {
        module_path: string_field(&v, "module.path")?,
        version: string_field(&v, "module.version").unwrap_or_else(|| "0.0.0-dev".to_string()),
        infy: string_field(&v, "module.infy").unwrap_or_else(|| crate::VERSION.lang.to_string()),
        authors: string_list(&v, "module.authors"),
        dependencies: Vec::new(),
        features: BTreeMap::new(),
        profile: Profile::default(),
        b_track: BTreeMap::new(),
        bins: Vec::new(),
        libs: Vec::new(),
        lints: Vec::new(),
        net_private: Vec::new(),
        net_nosumdb: Vec::new(),
        unknown_keys: unknown,
    };

    // §5.3 step 2: a local module path may not squat on `std`.
    if m.module_path.starts_with("std") {
        handler.emit(
            Diagnostic::new("E-0507", Phase::Name, file, Position { line: 1, column: 1 })
                .titled(format!("module path 与 `std` 冲突：`{}`", m.module_path))
                .help("改用自有域名前缀（§5.3）"),
        );
    }

    if let Some(dep) = v.get("dependencies").and_then(|d| d.as_table()) {
        for (k, val) in dep {
            let spec = match val {
                TomlValue::Str(s) => s.clone(),
                other => {
                    let _ = other;
                    continue;
                }
            };
            m.dependencies.push(Requirement { module: k.clone(), spec, is_std: k == "std" });
        }
    }
    if let Some(f) = v.get("features").and_then(|f| f.as_table()) {
        for (k, vals) in f {
            let list = match vals {
                TomlValue::Array(a) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
                _ => Vec::new(),
            };
            m.features.insert(k.clone(), list);
        }
    }
    if let Some(p) = v.get("profile").and_then(|p| p.as_table()) {
        let get = |k: &str| p.get(k).cloned();
        if let Some(TomlValue::Str(s)) = get("overflow") {
            m.profile.overflow = match s.as_str() {
                "panic" => Overflow::Panic,
                "saturate" => Overflow::Saturate,
                "wrap" => {
                    // §4.4: `wrap` is not a legal policy; only `std.num.wrapping_add` may wrap.
                    handler.emit(
                        Diagnostic::new("E-0506", Phase::Name, file, Position { line: 1, column: 1 })
                            .titled("overflow = \"wrap\" 不被允许".to_string())
                            .note("回绕只能通过 std.num.wrapping_*() 显式表达（§4.4，UB-003）")
                            .help("改为 \"panic\" 或 \"saturate\""),
                    );
                    Overflow::Panic
                }
                _ => Overflow::Panic,
            };
        }
        if let Some(TomlValue::Str(s)) = get("diagnostic_level") {
            m.profile.diagnostic_level = if s == "lenient" { DiagLevel::Lenient } else { DiagLevel::Strict };
        }
        if let Some(v) = get("opt_level") {
            let s = match v {
                TomlValue::Int(i) => i.to_string(),
                TomlValue::Str(s) => s,
                _ => String::new(),
            };
            if let Some(o) = OptLevel::parse(s.trim()) {
                m.profile.opt_level = o;
            }
        }
        if let Some(TomlValue::Int(i)) = get("stack_bytes") {
            m.profile.stack_bytes = i.max(0) as u64;
        }
        if let Some(TomlValue::Int(i)) = get("focus_stack_bytes") {
            m.profile.focus_stack_bytes = i.max(0) as u64;
        }
        if let Some(TomlValue::Int(i)) = get("focus_quota") {
            m.profile.focus_quota = i.max(1) as u32;
        }
        if let Some(TomlValue::Int(i)) = get("iter_guard_steps") {
            m.profile.iter_guard_steps = i.max(1) as u64;
        }
        if let Some(TomlValue::Str(s)) = get("a_track") {
            m.profile.a_track = Some(s);
        }
        if let Some(TomlValue::Str(s)) = get("panic_radius") {
            m.profile.panic_radius = match s.as_str() {
                "chain" => PanicRadius::Chain,
                "process" => PanicRadius::Process,
                _ => PanicRadius::Focus,
            };
        }
        if let Some(TomlValue::Bool(b)) = get("reproducible") {
            m.profile.reproducible = b;
        }
        if let Some(TomlValue::Bool(b)) = get("fast_math") {
            m.profile.fast_math = b;
        }
        if let Some(TomlValue::Str(s)) = get("lto") {
            m.profile.lto = match s.as_str() {
                "thin" => Lto::Thin,
                "false" => Lto::Off,
                _ => Lto::Fat,
            };
        } else if let Some(TomlValue::Bool(b)) = get("lto") {
            m.profile.lto = if b { Lto::Fat } else { Lto::Off };
        }
    }
    if let Some(b) = v.get("b_track").and_then(|b| b.as_table()) {
        for (k, val) in b {
            if let Some(s) = val.as_str() {
                m.b_track.insert(k.clone(), s.to_string());
            }
        }
    }
    for bin in array_of_tables(&v, "bin") {
        m.bins.push(BinTarget {
            name: string_in(&bin, "name").unwrap_or_default(),
            path: string_in(&bin, "path").unwrap_or_default(),
        });
    }
    for lib in array_of_tables(&v, "lib") {
        let editions = match lib.get("edition") {
            Some(TomlValue::Str(s)) => vec![s.clone()],
            Some(TomlValue::Array(a)) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
            _ => vec!["source".to_string()],
        };
        let targets = match lib.get("targets") {
            Some(TomlValue::Array(a)) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
            _ => Vec::new(),
        };
        m.libs.push(LibTarget { path: string_in(&lib, "path").unwrap_or_default(), editions, targets });
    }
    if let Some(l) = v.get("lints").and_then(|l| l.as_table()) {
        for (k, val) in l {
            if let Some(s) = val.as_str() {
                m.lints.push((k.clone(), s.to_string()));
            }
        }
    }
    if let Some(n) = v.get("net").and_then(|n| n.as_table()) {
        if let Some(TomlValue::Array(a)) = n.get("private") {
            m.net_private = a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
        }
        if let Some(TomlValue::Array(a)) = n.get("nosumdb") {
            m.net_nosumdb = a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
        }
    }
    Some(m)
}

/// `handler` has no file table; positions are resolved by the caller, so we keep a stable pseudo
/// id for manifest diagnostics.
fn file_id(_h: &mut Handler, _path: &str) -> crate::span::FileId {
    // Manifests are not `.infy` sources; they live in their own id space starting above the
    // source files so that §9.2 renders `MODULE.infy.toml:LINE:COL`.
    u32::MAX - 1
}

fn string_field(v: &TomlValue, path: &str) -> Option<String> {
    v.get_path(path).and_then(|x| x.as_str()).map(|s| s.to_string())
}
fn string_in(t: &BTreeMap<String, TomlValue>, k: &str) -> Option<String> {
    t.get(k).and_then(|x| x.as_str()).map(|s| s.to_string())
}
fn string_list(v: &TomlValue, path: &str) -> Vec<String> {
    v.get_path(path).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}
fn array_of_tables<'a>(v: &'a TomlValue, key: &str) -> Vec<BTreeMap<String, TomlValue>> {
    match v.get(key) {
        Some(TomlValue::Array(a)) => a.iter().filter_map(|x| x.as_table().cloned()).collect(),
        Some(TomlValue::Table(t)) => vec![t.clone()],
        _ => Vec::new(),
    }
}

fn collect_unknown_keys(v: &TomlValue, prefix: &str, out: &mut Vec<String>) {
    let Some(t) = v.as_table() else { return };
    for (k, val) in t {
        let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        let known = Manifest::KNOWN.iter().any(|p| *p == path || path.starts_with(&format!("{p}.")) || p.starts_with(&format!("{path}.")))
            || (matches!(k.as_str(), "path" | "version" | "infy" | "authors") && prefix == "module")
            || (prefix == "profile" && matches!(k.as_str(), "overflow" | "diagnostic_level" | "opt_level" | "stack_bytes" | "focus_stack_bytes" | "focus_quota" | "iter_guard_steps" | "a_track" | "panic_radius" | "reproducible" | "fast_math" | "lto"))
            || (prefix == "net" && matches!(k.as_str(), "private" | "nosumdb" | "proxy" | "timeout_ms"))
            || (prefix.ends_with("[]") && matches!(k.as_str(), "name" | "path" | "edition" | "targets"))
            || (prefix == "lints" || prefix == "dependencies" || prefix == "features" || prefix == "b_track");
        if !known {
            out.push(path.clone());
        }
        if let Some(sub) = val.as_table() {
            if prefix != "dependencies" && prefix != "features" && prefix != "b_track" && prefix != "lints" {
                collect_unknown_keys_value(val, &path, out);
            }
            let _ = sub;
        }
    }
}

fn collect_unknown_keys_value(v: &TomlValue, prefix: &str, out: &mut Vec<String>) {
    let mut inner = Vec::new();
    collect_unknown_keys(v, prefix, &mut inner);
    out.extend(inner);
}

// ---------------------------------------------------------------- packages

#[derive(Clone, Debug)]
pub struct PackageFile {
    pub abs: PathBuf,
    /// Path as printed in diagnostics: relative to the module root, slash separated (§9.2/§7.3).
    pub display: String,
    pub name: String,
    pub is_test: bool,
    /// `//infy:build` constraint text (§2.2), if present.
    pub constraint: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub dir: PathBuf,
    /// Directory-relative path from the module root, slash separated (`core/build`).
    pub rel: String,
    /// Package name == last directory segment (§5.1 R1).
    pub name: String,
    pub files: Vec<PackageFile>,
    /// True when the path contains an `internal` segment; the parent of the *nearest* internal
    /// directory decides who may import it (§5.5).
    pub internal_under: Option<String>,
    /// Test-only package (`package foo_test`, §5.9).
    pub is_test_pkg: bool,
}

impl Package {
    /// Full import path: module path + `/` + rel (§5.1).
    pub fn import_path(&self, module_path: &str) -> String {
        if self.rel.is_empty() {
            module_path.to_string()
        } else {
            format!("{module_path}/{}", self.rel)
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModuleGraph {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub packages: Vec<Package>,
}

impl ModuleGraph {
    pub fn package_by_rel(&self, rel: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.rel == rel)
    }
    /// Map an import path inside this module to a package (§5.3 step 3).
    pub fn package_by_import(&self, import: &str) -> Option<&Package> {
        let mp = self.manifest.module_path.clone();
        let rest = import.strip_prefix(&mp)?;
        let rest = rest.strip_prefix('/')?;
        self.package_by_rel(rest)
    }
}

/// Walk the module tree and build the package list. Applies R1–R5 (§5.1) as diagnostics.
pub fn discover(root: &Path, manifest: &Manifest, handler: &mut Handler) -> ModuleGraph {
    let mut packages = Vec::new();
    discover_into(root, Path::new(""), root, manifest, handler, &mut packages);
    packages.sort_by(|a, b| a.rel.cmp(&b.rel));
    ModuleGraph { root: root.to_path_buf(), manifest: manifest.clone(), packages }
}

fn discover_into(
    abs: &Path,
    rel: &Path,
    module_root: &Path,
    manifest: &Manifest,
    handler: &mut Handler,
    out: &mut Vec<Package>,
) {
    let entries = match std::fs::read_dir(abs) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    let mut has_manifest_here = false;
    for ent in entries.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        let path = ent.path();
        if path.is_dir() {
            if dir_is_skipped(&name) {
                continue;
            }
            if !dir_name_is_importable(&name) {
                let display = display_of(module_root, &path);
                handler.emit(
                    Diagnostic::new("E-0505", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!("目录名不可导入：`{display}`"))
                        .note("导入路径中的一段必须是小写 ASCII（[a-z0-9_]+，首字符非数字，§5.1 R3）")
                        .help("把目录改名为小写 ASCII；中文标识符放在符号名而非路径里"),
                );
                continue;
            }
            subdirs.push((name, path));
        } else {
            if name == MANIFEST_NAME {
                has_manifest_here = true;
            }
            if name.ends_with(".infy") {
                files.push((name, path));
            }
        }
    }
    let _ = has_manifest_here;
    // §7.3: `read_dir` hands entries in arbitrary order, and this order decides `FileId` numbering
    // downstream (so it leaks into the emitted C text and the build-id). Sort both lists by name.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    subdirs.sort_by(|a, b| a.0.cmp(&b.0));
    // A directory with no `.infy` files contributes no package but its children may.
    if !files.is_empty() {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let dir_name = rel_str.rsplit('/').next().unwrap_or("root").to_string();
        let mut pkg_files = Vec::new();
        let mut names: BTreeMap<String, ()> = BTreeMap::new();
        let mut nontest_declares_test_pkg = false;
        for (name, path) in &files {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if text.is_empty() {
                handler.emit(
                    Diagnostic::new("E-0090", Phase::Lex, 0, Position { line: 1, column: 1 })
                        .titled(format!("空源码文件：{}", display_of(module_root, path)))
                        .help("删除该文件，或至少写入 `package`（§9.5 第 11 条）"),
                );
                continue;
            }
            let pkg_name = declared_package(&text).unwrap_or_else(|| dir_name.clone());
            let is_test = name.ends_with("_test.infy");
            // §5.1 R1: the package clause must equal the directory's last segment. §5.9 adds the
            // two shapes a test file may use instead: `package <dir>_test` living beside
            // `package <dir>`, and §14.1/§14.7's `tests/build_test.infy` = `package build_test`,
            // where the file stem names the package.
            let stem = name.strip_suffix(".infy").unwrap_or(name.as_str());
            let r1_ok = pkg_name == dir_name
                || rel_str.is_empty()
                || (is_test && (pkg_name == format!("{dir_name}_test") || pkg_name == stem));
            if pkg_name == format!("{dir_name}_test") && !is_test {
                nontest_declares_test_pkg = true;
            }
            if declared_package(&text).is_none() && !rel_str.starts_with("cmd/") {
                handler.emit(
                    Diagnostic::new("E-0501", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!("{}: 缺少 package 声明", display_of(module_root, path)))
                        .note("只有 `cmd/*` 下的入口文件可以省略 package 子句（§3.1）")
                        .help(format!("补一行 `package {dir_name}`")),
                );
            }
            if !r1_ok {
                handler.emit(
                    Diagnostic::new("E-0501", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!("包名与目录不符：`package {pkg_name}` 位于 `{rel_str}/`"))
                        .note("一个目录 = 一个包，包名必须等于目录末段名；测试文件另有两种形态 `\
                               package {dir_name}_test` 或 `package <文件名主干>`（§5.1 R1 + §5.9）")
                        .help(format!("改为 `package {dir_name}`")),
                );
            }
            names.insert(pkg_name.clone(), ());
            pkg_files.push(PackageFile {
                abs: path.clone(),
                display: display_of(module_root, path),
                name: name.clone(),
                is_test,
                constraint: build_constraint_of(&text),
            });
        }
        // §5.1 R2: one directory = one package. The only pair §5.9 allows is `package <dir>` next
        // to `package <dir>_test`, and the `_test` half may only be contributed by `_test.infy`
        // files. (The check is on the *set* of names — several files of the same package sharing a
        // directory is §14.1's `mod.infy` + `cache.infy`, not a violation.)
        if !rel_str.is_empty() {
            let distinct: Vec<&String> = names.keys().collect();
            let test_pair = format!("{dir_name}_test");
            let legal_pair = distinct.len() == 2
                && distinct.iter().any(|n| **n == dir_name)
                && distinct.iter().any(|n| **n == test_pair)
                && !nontest_declares_test_pkg;
            if distinct.len() > 1 && !legal_pair {
                handler.emit(
                    Diagnostic::new("E-0501", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!(
                            "`{rel_str}/` 内存在 {} 个包名：{}",
                            distinct.len(),
                            distinct.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ")
                        ))
                        .note("同一目录内不得同时存在两个包名（§5.1 R2）；例外只有 `package <目录>` 与 \
                               `package <目录>_test` 并存（§5.9）"),
                );
            }
        }
        let is_test_pkg = rel_str.starts_with("tests/") || pkg_files.iter().all(|f| f.is_test);
        let internal_under = internal_parent(&rel_str);
        out.push(Package {
            dir: abs.to_path_buf(),
            rel: rel_str,
            name: dir_name,
            files: pkg_files,
            internal_under,
            is_test_pkg,
        });
    }
    for (name, path) in subdirs {
        let child_rel = if rel.as_os_str().is_empty() { PathBuf::from(&name) } else { rel.join(&name) };
        // §5.5: `internal` may not contain `internal`
        let child_rel_str = child_rel.to_string_lossy().replace('\\', "/");
        if child_rel_str.matches("internal").count() > 1 {
            handler.emit(
                Diagnostic::new("E-0509", Phase::Name, 0, Position { line: 1, column: 1 })
                    .titled(format!("`internal` 嵌套 `internal`：{child_rel_str}"))
                    .help("删除内层 internal（§5.5）"),
            );
        }
        discover_into(&path, &child_rel, module_root, manifest, handler, out);
    }
}

fn internal_parent(rel: &str) -> Option<String> {
    let segs: Vec<&str> = rel.split('/').collect();
    let idx = segs.iter().rposition(|s| *s == "internal")?;
    // the directory that owns the internal subtree is its parent
    if idx == 0 {
        Some(String::new())
    } else {
        Some(segs[..idx].join("/"))
    }
}

fn display_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// First `package X` clause in a file, without needing the full parser.
pub fn declared_package(text: &str) -> Option<String> {
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("//") || l.starts_with("/*") {
            continue;
        }
        if let Some(rest) = l.strip_prefix("package ") {
            let name = rest.trim().trim_end_matches(';').split_whitespace().next()?;
            return Some(name.to_string());
        }
    }
    None
}

/// The `//infy:build …` line's text, or None. Position checks are the lexer's job (§2.2).
pub fn build_constraint_of(text: &str) -> Option<String> {
    for line in text.lines() {
        let l = line.trim_start();
        if let Some(rest) = l.strip_prefix("//infy:build") {
            return Some(rest.trim().to_string());
        }
        if !l.is_empty() && !l.starts_with("#!") {
            return None;
        }
    }
    None
}

// ---------------------------------------------------------------- §2.2 constraints

/// §2.2 + §5.9: the build configuration a file is selected against. `test` is the §5.9 scope: true
/// only under `infy test`, which is what makes `//infy:build test` mean "test build only" and
/// `//infy:build !test` mean "not in the test build".
#[derive(Clone, Debug)]
pub struct BuildConfig<'a> {
    pub os: &'a str,
    pub arch: &'a str,
    pub abi: &'a str,
    /// `[features]` names declared by the manifest (§5.2).
    pub features: &'a [String],
    pub a_track: bool,
    pub b_track: bool,
    /// `[profile]` name (§5.2): `debug` | `release` | `release-small`.
    pub profile: &'a str,
    pub test: bool,
}

/// Evaluate a build-constraint expression against the active build configuration.
pub fn constraint_matches(expr: &str, cfg: &BuildConfig) -> Result<bool, String> {
    let toks = constraint_tokens(expr)?;
    let mut p = ConstraintParser { toks, pos: 0 };
    let v = p.or()?;
    if p.pos != p.toks.len() {
        return Err(format!("构建约束有多余内容：`{expr}`"));
    }
    Ok(eval(&v, cfg))
}

#[derive(Clone, Debug)]
enum CNode {
    Name(String),
    Not(Box<CNode>),
    And(Vec<CNode>),
    Or(Vec<CNode>),
}

fn constraint_tokens(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' || c == '!' || c == ')' {
            out.push(c.to_string());
            i += 1;
            continue;
        }
        if c == '&' && i + 1 < cs.len() && cs[i + 1] == '&' {
            out.push("&&".to_string());
            i += 2;
            continue;
        }
        if c == '|' && i + 1 < cs.len() && cs[i + 1] == '|' {
            out.push("||".to_string());
            i += 2;
            continue;
        }
        if c == ',' {
            out.push(",".to_string());
            i += 1;
            continue;
        }
        let start = i;
        while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_' || cs[i] == '.') {
            i += 1;
        }
        if start == i {
            return Err(format!("构建约束中出现非法字符 `{c}`"));
        }
        out.push(cs[start..i].iter().collect());
    }
    Ok(out)
}

struct ConstraintParser {
    toks: Vec<String>,
    pos: usize,
}

impl ConstraintParser {
    fn peek(&self) -> Option<&String> {
        self.toks.get(self.pos)
    }
    fn or(&mut self) -> Result<CNode, String> {
        let mut parts = vec![self.and()?];
        while self.peek().map(|s| s == "||").unwrap_or(false) {
            self.pos += 1;
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 { parts.remove(0) } else { CNode::Or(parts) })
    }
    fn and(&mut self) -> Result<CNode, String> {
        let mut parts = vec![self.unary()?];
        loop {
            match self.peek().map(|s| s.as_str()) {
                Some("&&") | Some(",") => {
                    self.pos += 1;
                    parts.push(self.unary()?);
                }
                _ => break,
            }
        }
        Ok(if parts.len() == 1 { parts.remove(0) } else { CNode::And(parts) })
    }
    fn unary(&mut self) -> Result<CNode, String> {
        match self.peek().map(|s| s.as_str()) {
            Some("!") => {
                self.pos += 1;
                Ok(CNode::Not(Box::new(self.unary()?)))
            }
            Some("(") => {
                self.pos += 1;
                let v = self.or()?;
                if self.peek().map(|s| s == ")").unwrap_or(false) {
                    self.pos += 1;
                } else {
                    return Err("构建约束括号不配对".to_string());
                }
                Ok(v)
            }
            Some(name) => {
                let name = name.to_string();
                self.pos += 1;
                Ok(CNode::Name(name))
            }
            None => Err("构建约束表达式不完整".to_string()),
        }
    }
}

fn eval(n: &CNode, cfg: &BuildConfig) -> bool {
    match n {
        CNode::Not(x) => !eval(x, cfg),
        CNode::And(xs) => xs.iter().all(|x| eval(x, cfg)),
        CNode::Or(xs) => xs.iter().any(|x| eval(x, cfg)),
        CNode::Name(s) => match s.as_str() {
            "windows" => cfg.os == "windows",
            "linux" => cfg.os == "linux",
            "macos" => cfg.os == "macos",
            "x86_64" => cfg.arch == "x86_64",
            "aarch64" => cfg.arch == "aarch64",
            "msvc" => cfg.abi == "msvc",
            "gnu" => cfg.abi == "gnu",
            "a_track" => cfg.a_track,
            "b_track" => cfg.b_track,
            "debug" => cfg.profile == "debug",
            "release" => cfg.profile == "release" || cfg.profile == "release-small",
            "ignore" => false,
            "test" => cfg.test,
            other => cfg.features.iter().any(|f| f == other),
        },
    }
}

/// §2.2: names outside the closed set plus the module's `features` are `E-0012`.
pub fn unknown_constraint_names(expr: &str, features: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for tok in constraint_tokens(expr).unwrap_or_default() {
        if tok.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') && !tok.is_empty() {
            let known = BASE_CONSTRAINTS.contains(&tok.as_str()) || features.iter().any(|f| *f == tok);
            if !known {
                out.push(tok);
            }
        }
    }
    out
}

/// §5.9 items 1-2 + §2.2: does this file take part in the build [`BuildConfig`] describes? Two
/// independent gates, both of which must pass:
///
/// * §5.9 — a `*_test.infy` file, or any file of a test-only package (`tests/…`), is out of the
///   ordinary build and in only for `infy test`;
/// * §2.2 — the file's own `//infy:build` expression evaluated against the configuration, so
///   `//infy:build ignore` is out of every build and `//infy:build !test` is out of `infy test`.
///
/// `Err` means the constraint text does not parse; the caller reports it, it is never a silent
/// "include the file anyway" (§7.1: no silent degradation).
pub fn file_selected(f: &PackageFile, pkg: &Package, cfg: &BuildConfig) -> Result<bool, String> {
    if (f.is_test || pkg.is_test_pkg) && !cfg.test {
        return Ok(false);
    }
    match &f.constraint {
        Some(text) => constraint_matches(text, cfg),
        None => Ok(true),
    }
}

// ---------------------------------------------------------------- §5.7 MVS

/// A `^1.5` / `>=x <y` / exact requirement, semver-interpreted (§5.7 step 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub lower: (u64, u64, u64),
    pub upper: Option<(u64, u64, u64)>,
    pub spec: String,
}

pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut p = s.trim_start_matches('v').splitn(3, '.');
    let a = p.next()?.parse().ok()?;
    let b = p.next().unwrap_or("0").parse().unwrap_or(0);
    let c = p.next().unwrap_or("0");
    let c = c.split(|ch: char| !ch.is_ascii_digit()).next().unwrap_or("0").parse().unwrap_or(0);
    Some((a, b, c))
}

pub fn parse_range(spec: &str) -> Range {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix('^') {
        let (a, b, c) = parse_semver(rest).unwrap_or((0, 0, 0));
        // caret: [a.b.c, next breaking) — breaking means the first non-zero component increments
        let upper = if a != 0 {
            (a + 1, 0, 0)
        } else if b != 0 {
            (0, b + 1, 0)
        } else {
            (0, 0, c + 1)
        };
        return Range { lower: (a, b, c), upper: Some(upper), spec: spec.to_string() };
    }
    if let Some(idx) = spec.find(">= ") {
        let lower = parse_semver(&spec[idx + 3..]).unwrap_or((0, 0, 0));
        let upper = spec.find("< ").map(|j| parse_semver(&spec[j + 2..]).unwrap_or((0, 0, 0)));
        return Range { lower, upper, spec: spec.to_string() };
    }
    let v = parse_semver(spec).unwrap_or((0, 0, 0));
    Range { lower: v, upper: Some((v.0, v.1, v.2 + 1)), spec: spec.to_string() }
}

pub fn range_contains(r: &Range, v: (u64, u64, u64)) -> bool {
    v >= r.lower && r.upper.map(|u| v < u).unwrap_or(true)
}

/// §5.7: minimal version selection — for each module path take the greatest lower bound of all
/// demands, then choose the smallest available version that satisfies every demand. No backtracking,
/// no SAT solving.
pub fn mvs(
    demands: &[(String, String)],
    available: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<(String, String)>, Vec<String>> {
    let mut by_module: BTreeMap<String, Vec<Range>> = BTreeMap::new();
    for (m, spec) in demands {
        by_module.entry(m.clone()).or_default().push(parse_range(spec));
    }
    let mut chosen = Vec::new();
    let mut conflicts = Vec::new();
    for (m, ranges) in by_module {
        let mut best: Option<(u64, u64, u64)> = None;
        let mut candidates: Vec<(u64, u64, u64)> = available
            .get(&m)
            .map(|vs| vs.iter().filter_map(|v| parse_semver(v)).collect())
            .unwrap_or_else(|| ranges.iter().map(|r| r.lower).collect());
        if candidates.is_empty() {
            candidates = ranges.iter().map(|r| r.lower).collect();
        }
        candidates.sort();
        for c in candidates {
            if ranges.iter().all(|r| range_contains(r, c)) {
                best = Some(c);
                break;
            }
        }
        match best {
            Some(v) => chosen.push((m.clone(), format!("{}.{}.{}", v.0, v.1, v.2))),
            None => conflicts.push(format!(
                "{m}: 需要 {} 但无版本同时满足全部需求",
                ranges.iter().map(|r| r.spec.clone()).collect::<Vec<_>>().join(" 且 ")
            )),
        }
    }
    if conflicts.is_empty() {
        chosen.sort();
        Ok(chosen)
    } else {
        Err(conflicts)
    }
}

// ---------------------------------------------------------------- §5.8 dirhash

/// §5.8 item 1: dirhash over a module@version's logical file tree.
pub fn dirhash(files: &mut Vec<(String, Vec<u8>)>) -> String {
    files.sort_by(|a, b| a.0.cmp(&b.0));
    sha256::merkle_dirhash(files)
}

/// §5.8 / appendix E.1: `MODULE.infy.lock` — a line-oriented file whose order is tool-decided but
/// must be stably emitted (§7.3), and which must remain `git diff` friendly (§5.1 R? E.1 note).
#[derive(Clone, Debug, Default)]
pub struct Lock {
    pub req: Vec<(String, String, Option<String>)>,
    pub dirh: Vec<(String, String, String)>,
    pub dirh_m: Vec<(String, String, String)>,
    pub sumdb: Vec<(String, String, String, u64, String)>,
    pub priv_globs: Vec<String>,
    pub nosum_globs: Vec<String>,
}

impl Lock {
    pub fn parse(text: &str) -> Lock {
        let mut l = Lock::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.split_whitespace();
            match it.next() {
                Some("req") => {
                    let m = it.next().unwrap_or("").to_string();
                    let v = it.next().unwrap_or("").to_string();
                    let resolved = if it.next() == Some("resolved-from") { it.next().map(|s| s.to_string()) } else { None };
                    l.req.push((m, v, resolved));
                }
                Some("dirh") => l.dirh.push((it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())),
                Some("dirh-m") => l.dirh_m.push((it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())),
                Some("sumdb") => {
                    let m = it.next().unwrap_or("").to_string();
                    let v = it.next().unwrap_or("").to_string();
                    let rest: Vec<String> = it.map(|s| s.to_string()).collect();
                    let mut tree_head = String::new();
                    let mut inset = 0u64;
                    let mut proof = String::new();
                    for kv in rest {
                        if let Some(x) = kv.strip_prefix("tree_head=") {
                            tree_head = x.to_string();
                        } else if let Some(x) = kv.strip_prefix("inset=") {
                            inset = x.parse().unwrap_or(0);
                        } else if let Some(x) = kv.strip_prefix("proof=") {
                            proof = x.to_string();
                        }
                    }
                    l.sumdb.push((m, v, tree_head, inset, proof));
                }
                Some("priv") => l.priv_globs.push(it.next().unwrap_or("").to_string()),
                Some("nosum") => l.nosum_globs.push(it.next().unwrap_or("").to_string()),
                _ => {}
            }
        }
        l
    }

    /// Stable, sorted output (§7.3) — the file must be byte-identical for identical inputs.
    pub fn render(&self) -> String {
        let mut o = String::new();
        o.push_str("# Generated by `infy mod tidy` (§5.7). Do not hand-edit without `infy mod accept-changes`.\n");
        let mut req = self.req.clone();
        req.sort();
        for (m, v, r) in req {
            match r {
                Some(r) => o.push_str(&format!("req {m} {v} resolved-from {r}\n")),
                None => o.push_str(&format!("req {m} {v}\n")),
            }
        }
        let mut d = self.dirh.clone();
        d.sort();
        for (m, v, h) in d {
            o.push_str(&format!("dirh {m} {v} {h}\n"));
        }
        let mut dm = self.dirh_m.clone();
        dm.sort();
        for (m, v, h) in dm {
            o.push_str(&format!("dirh-m {m} {v} {h}\n"));
        }
        let mut s = self.sumdb.clone();
        s.sort();
        for (m, v, th, inset, proof) in s {
            o.push_str(&format!("sumdb {m} {v} tree_head={th} inset={inset} proof={proof}\n"));
        }
        let mut p: Vec<String> = self.priv_globs.clone();
        p.sort();
        for g in p {
            o.push_str(&format!("priv {g}\n"));
        }
        let mut n: Vec<String> = self.nosum_globs.clone();
        n.sort();
        for g in n {
            o.push_str(&format!("nosum {g}\n"));
        }
        o
    }

    pub fn checksum_of(&self, module: &str, version: &str) -> Option<&str> {
        self.dirh.iter().find(|(m, v, _)| m == module && v == version).map(|(_, _, h)| h.as_str())
    }

    /// §5.8 item 2: a lock that no longer matches the manifest is `E-0200`.
    pub fn check_against_manifest(&self, manifest: &Manifest, handler: &mut Handler) {
        let mut need: Vec<(String, String)> = manifest
            .dependencies
            .iter()
            .filter(|d| !d.is_std)
            .map(|d| (d.module.clone(), parse_range(&d.spec).lower))
            .map(|(m, v)| (m, format!("{}.{}.{}", v.0, v.1, v.2)))
            .collect();
        need.sort();
        for (m, v) in need {
            if !self.req.iter().any(|(rm, rv, _)| *rm == m && *rv == v) {
                handler.emit(
                    Diagnostic::new("E-0200", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!("lock 与清单不同步：缺 `{m}@{v}`"))
                        .note("锁定文件缺少清单所要求的依赖（§5.7 第 4 步）")
                        .help("运行 `infy mod tidy` 重建 MODULE.infy.lock"),
                );
            }
        }
        for (rm, rv, _) in &self.req {
            if !manifest.dependencies.iter().any(|d| &d.module == rm) {
                handler.emit(
                    Diagnostic::new("E-0200", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!("lock 与清单不同步：多余 `{rm}@{rv}`"))
                        .note("清单已不再要求该依赖（§5.7）")
                        .help("运行 `infy mod tidy` 重建 MODULE.infy.lock"),
                );
            }
        }
    }
}

/// §5.3: the resolution order for one import path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportSource {
    /// `std*` prefix — the built-in virtual module (§5.3 step 2).
    Std,
    /// A package of the module currently being built (§5.3 step 3).
    Local,
    /// A declared dependency, resolved via MVS / cache / vendor (§5.3 step 4).
    Dependency,
    /// Not declared at all — `E-0201`.
    Undeclared,
}

pub fn classify_import(import: &str, manifest: &Manifest) -> ImportSource {
    if import == "std" || import.starts_with("std/") {
        return ImportSource::Std;
    }
    if import == manifest.module_path || import.starts_with(&format!("{}/", manifest.module_path)) {
        return ImportSource::Local;
    }
    if manifest.dependencies.iter().any(|d| import == d.module || import.starts_with(&format!("{}/", d.module))) {
        return ImportSource::Dependency;
    }
    ImportSource::Undeclared
}

/// §5.5: may `importer_rel` import `target`?  `a/b/internal/c` is visible only to `a/b/...`.
pub fn internal_allows(importer_rel: &str, target: &Package) -> bool {
    let Some(owner) = &target.internal_under else { return true };
    let owner_prefix = if owner.is_empty() { String::new() } else { format!("{owner}/") };
    importer_rel == owner || importer_rel.starts_with(&owner_prefix)
}

/// §5.3 default local name = last path segment of the directory sequence.
pub fn default_local_name(import: &str) -> String {
    import.trim_end_matches('/').rsplit('/').next().unwrap_or(import).to_string()
}

/// §12.1: the domain-neutrality scan of `std.*` and language-level exports against a
/// spec-maintained blacklist (§1.3). The list lives in `std/domain_terms.txt`.
pub fn domain_neutrality_violations<'a>(symbols: impl Iterator<Item = &'a str>, blacklist: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut terms: Vec<String> = blacklist
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_ascii_lowercase())
        .collect();
    terms.sort();
    for s in symbols {
        // §2.10: match with NFKCCaseFold so `Numpy` and `numpy` cannot slip past.
        let folded = crate::stdlib::nfkc_case_fold(s);
        if terms.iter().any(|t| folded == *t || folded.contains(&format!("{t}_")) || folded.ends_with(t)) {
            out.push(s.to_string());
        }
    }
    out
}

pub fn code_table_size() -> usize {
    codes::count()
}

/// Level mapping for `--diag-json` consumers.
pub fn level_of(code: &str) -> Option<Level> {
    codes::lookup(code).map(|c| c.level)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5.2 in full, written out here so this module owns its own fixture.
    const FULL: &str = "\
[module]
path    = \"github.com/user/infinity\"
version = \"1.2.3\"
infy    = \"2.0\"
authors = [\"Alice <a@b.com>\"]

[dependencies]
\"std\"                   = \"1.0\"
\"github.com/user/mathx\" = \"^1.5\"
\"github.com/user/decoder\" = \"2.0.0\"

[features]
gpu = []
a_track_c = [\"gpu\"]

[profile]
overflow         = \"saturate\"
diagnostic_level = \"lenient\"
opt_level        = 2
stack_bytes      = 8388608
a_track          = \"unsafe-trusted\"
reproducible     = true

[b_track]
py  = \"3.13\"
lua = \"5.4\"

[[bin]]
name = \"zhukong\"
path = \"cmd/host\"

[[lib]]
path    = \"core/schedule\"
edition = [\"source\", \"wheel\"]
targets = [\"x86_64-pc-windows-msvc\", \"x86_64-unknown-linux-gnu\"]

[lints]
W-0070 = \"allow\"

[net]
private = [\"git.internal/*\"]
nosumdb = [\"internal.example\"]
";

    const MINIMAL: &str = "[module]\npath = \"github.com/user/m\"\n";

    fn parse(text: &str) -> (Option<Manifest>, Vec<String>) {
        let mut h = Handler::default();
        let m = parse_manifest(text, MANIFEST_NAME, &mut h);
        (m, h.diags.iter().map(|d| d.code.clone()).collect())
    }

    fn full() -> Manifest {
        let (m, c) = parse(FULL);
        assert!(c.iter().all(|x| !x.starts_with('E')), "{c:?}");
        m.expect("§5.2 fixture must parse")
    }

    // ---------------------------------------------------------------- §5.2 manifest

    /// §5.2: every documented section of `MODULE.infy.toml` reaches the `Manifest` struct — a key
    /// that silently fails to bind is how a typo becomes an ignored setting.
    #[test]
    fn manifest_reads_every_5_2_section() {
        let m = full();
        assert_eq!(m.module_path, "github.com/user/infinity");
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.infy, "2.0");
        assert_eq!(m.authors, vec!["Alice <a@b.com>".to_string()]);
        assert_eq!(m.dependencies.len(), 3);
        assert_eq!(m.feature_names(), vec!["a_track_c".to_string(), "gpu".to_string()]);
        assert_eq!(m.profile.overflow, Overflow::Saturate);
        assert_eq!(m.profile.diagnostic_level, DiagLevel::Lenient);
        assert_eq!(m.profile.stack_bytes, 8_388_608);
        assert!(m.profile.reproducible);
        assert_eq!(m.b_track.get("py").map(|s| s.as_str()), Some("3.13"));
        assert_eq!(m.bins.len(), 1);
        assert_eq!(m.bins[0].name, "zhukong");
        assert_eq!(m.libs[0].editions, vec!["source".to_string(), "wheel".to_string()]);
        assert_eq!(m.libs[0].targets.len(), 2);
        assert_eq!(m.lints, vec![("W-0070".to_string(), "allow".to_string())]);
        assert_eq!(m.net_private, vec!["git.internal/*".to_string()]);
        assert_eq!(m.net_nosumdb, vec!["internal.example".to_string()]);
    }

    /// §5.2: the defaults a development module gets when it writes nothing else.
    #[test]
    fn manifest_defaults_match_5_2() {
        let m = full_minimal();
        assert_eq!(m.version, "0.0.0-dev");
        assert_eq!(m.infy, crate::VERSION.lang);
        assert_eq!(m.profile.overflow, Overflow::Panic, "UB-003: panic is the default");
        assert_eq!(m.profile.diagnostic_level, DiagLevel::Strict, "§9.6: strict by default");
        assert_eq!(m.profile.opt_level, OptLevel::O2);
        assert_eq!(m.profile.focus_stack_bytes, 256 * 1024, "§8.5 focus stack");
        assert_eq!(m.profile.focus_quota, 1, "§8.5 one scheduling slice per node");
        assert_eq!(m.profile.panic_radius, PanicRadius::Focus, "§4.16");
        assert!(!m.declares_a_track(), "§4.7.5: @c/@py need an explicit a_track declaration");
    }

    fn full_minimal() -> Manifest {
        let (m, c) = parse(MINIMAL);
        assert!(c.is_empty(), "{c:?}");
        m.expect("minimal manifest must parse")
    }

    /// §5.2: an unknown manifest key is `W-0500`, so a misspelled section cannot be silently
    /// ignored (the same class of failure as an unset `overflow`).
    #[test]
    fn manifest_unknown_key_is_w0500() {
        let (_, c) = parse("[module]\npath = \"github.com/u/m\"\n\n[profle]\noverflow = \"saturate\"\n");
        assert!(c.contains(&"W-0500".to_string()), "{c:?}");
    }

    /// §5.2: malformed TOML is `E-0506` and yields no manifest at all — never a default-filled one.
    #[test]
    fn manifest_syntax_error_is_e0506() {
        let (m, c) = parse("[module\npath = ");
        assert!(m.is_none());
        assert!(c.contains(&"E-0506".to_string()), "{c:?}");
    }

    /// §5.2: `module.path` is mandatory. A manifest without it must be refused, not half-built.
    #[test]
    fn manifest_without_module_path_is_refused() {
        let (m, _) = parse("[module]\nversion = \"1.0.0\"\n");
        assert!(m.is_none(), "a manifest with no module path cannot resolve imports (§5.3)");
    }

    /// §5.3 step 2: a local module may not squat on the `std` virtual module.
    #[test]
    fn manifest_path_squatting_on_std_is_e0507() {
        let (_, c) = parse("[module]\npath = \"std/whatever\"\n");
        assert!(c.contains(&"E-0507".to_string()), "{c:?}");
    }

    /// §4.4 / UB-003: `overflow = "wrap"` is not a legal policy; the profile falls back to `panic`
    /// and the request is reported rather than quietly honoured.
    #[test]
    fn manifest_rejects_wrap_overflow_policy() {
        let (m, c) = parse("[module]\npath = \"github.com/u/m\"\n\n[profile]\noverflow = \"wrap\"\n");
        assert!(c.contains(&"E-0506".to_string()), "{c:?}");
        assert_eq!(m.expect("still yields a manifest").profile.overflow, Overflow::Panic);
    }

    /// §5.3 step 2: `std` is the mandatory built-in dependency and must be distinguishable.
    #[test]
    fn std_dependency_is_flagged() {
        let m = full();
        assert_eq!(m.dependencies.iter().filter(|d| d.is_std).count(), 1);
        assert!(m.dependencies.iter().any(|d| d.module == "github.com/user/mathx" && d.spec == "^1.5" && !d.is_std));
    }

    // ---------------------------------------------------------------- §5.3 import resolution order

    /// §5.3: the resolution order for an import path is fixed — `std*`, then a local package, then
    /// a declared dependency, otherwise undeclared.
    #[test]
    fn classify_import_follows_the_5_3_order() {
        let m = full();
        assert_eq!(classify_import("std", &m), ImportSource::Std);
        assert_eq!(classify_import("std/collections", &m), ImportSource::Std);
        assert_eq!(classify_import("github.com/user/infinity/core", &m), ImportSource::Local);
        assert_eq!(classify_import("github.com/user/infinity", &m), ImportSource::Local);
        assert_eq!(classify_import("github.com/user/mathx/spline", &m), ImportSource::Dependency);
        assert_eq!(classify_import("github.com/user/nobody", &m), ImportSource::Undeclared);
        // §5.3 step 1/4: a prefix that only matches mid-segment is not a dependency.
        assert_eq!(classify_import("github.com/user/mathx2", &m), ImportSource::Undeclared);
    }

    // ---------------------------------------------------------------- §5.7 semver ranges and MVS

    /// §5.7 step 2: `^` is the first non-zero component's compatibility window.
    #[test]
    fn caret_range_is_the_compatibility_window() {
        let r = parse_range("^1.5");
        assert_eq!(r.lower, (1, 5, 0));
        assert_eq!(r.upper, Some((2, 0, 0)));
        assert!(range_contains(&r, (1, 5, 0)));
        assert!(range_contains(&r, (1, 99, 0)));
        assert!(!range_contains(&r, (2, 0, 0)), "major bump is breaking");
        assert!(!range_contains(&r, (1, 4, 9)));
        // 0.x: the minor is the breaking component.
        assert_eq!(parse_range("^0.3").upper, Some((0, 4, 0)));
        assert_eq!(parse_range("^0.0.7").upper, Some((0, 0, 8)));
    }

    /// §5.7 step 2: an exact spec selects exactly one version.
    #[test]
    fn exact_spec_selects_one_version() {
        let r = parse_range("2.0.0");
        assert!(range_contains(&r, (2, 0, 0)));
        assert!(!range_contains(&r, (2, 0, 1)));
        assert_eq!(parse_semver("v2.0"), Some((2, 0, 0)));
    }

    /// §5.7: minimal version selection picks the *smallest* version satisfying every demand — no
    /// "highest wins", no backtracking, no SAT solving.
    #[test]
    fn mvs_selects_the_minimal_satisfying_version() {
        let available: BTreeMap<String, Vec<String>> = [(
            "github.com/user/mathx".to_string(),
            vec!["1.4.0".to_string(), "1.5.0".to_string(), "1.5.3".to_string(), "1.9.0".to_string()],
        )]
        .into_iter()
        .collect();
        let demands = vec![
            ("github.com/user/mathx".to_string(), "^1.0".to_string()),
            ("github.com/user/mathx".to_string(), ">= 1.5".to_string()),
        ];
        let got = mvs(&demands, &available).expect("MVS must succeed");
        assert_eq!(got, vec![("github.com/user/mathx".to_string(), "1.5.0".to_string())]);
    }

    /// §5.7: an unsatisfiable demand set is an error naming the module — MVS must not invent a
    /// version or quietly drop one of the demands.
    #[test]
    fn mvs_reports_conflicts_without_backtracking() {
        let available: BTreeMap<String, Vec<String>> = [("github.com/u/a".to_string(), vec!["1.0.0".to_string(), "2.0.0".to_string()])].into_iter().collect();
        let demands = vec![
            ("github.com/u/a".to_string(), "^1.0".to_string()),
            ("github.com/u/a".to_string(), "^2.0".to_string()),
        ];
        let err = mvs(&demands, &available).expect_err("disjoint ranges cannot be satisfied");
        assert_eq!(err.len(), 1, "{err:?}");
        assert!(err[0].contains("github.com/u/a"), "{}", err[0]);
        assert!(err[0].contains("^1.0") && err[0].contains("^2.0"), "the conflict must name both demands: {}", err[0]);
    }

    /// §5.7: MVS output is sorted by module path, because the lock file must be byte-stable (§7.3).
    #[test]
    fn mvs_output_is_sorted_by_module_path() {
        let available: BTreeMap<String, Vec<String>> = [
            ("github.com/u/b".to_string(), vec!["1.0.0".to_string()]),
            ("github.com/u/a".to_string(), vec!["1.0.0".to_string()]),
        ]
        .into_iter()
        .collect();
        let demands = vec![("github.com/u/b".to_string(), "1.0.0".to_string()), ("github.com/u/a".to_string(), "1.0.0".to_string())];
        assert_eq!(mvs(&demands, &available).unwrap(), vec![("github.com/u/a".to_string(), "1.0.0".to_string()), ("github.com/u/b".to_string(), "1.0.0".to_string())]);
    }

    // ---------------------------------------------------------------- §5.8 dirhash

    /// §5.8 item 1: dirhash sorts by path, folds `path` and `content` together, and is `h1:<base64>`.
    #[test]
    fn dirhash_is_order_free_and_binds_paths_to_contents() {
        let mut a = vec![("core/mod.infy".to_string(), b"package core".to_vec()), ("cmd/main.infy".to_string(), b"package main".to_vec())];
        let mut b = vec![("cmd/main.infy".to_string(), b"package main".to_vec()), ("core/mod.infy".to_string(), b"package core".to_vec())];
        let ha = dirhash(&mut a);
        let hb = dirhash(&mut b);
        assert_eq!(ha, hb, "§5.8: enumeration order must not change the hash");
        assert!(ha.starts_with("h1:"), "{ha}");
        // same content set under swapped paths is a *different* tree: path is part of the digest
        let mut c = vec![("core/mod.infy".to_string(), b"package main".to_vec()), ("cmd/main.infy".to_string(), b"package core".to_vec())];
        assert_ne!(ha, dirhash(&mut c));
        let mut d = vec![("core/mod.infy".to_string(), b"package coreX".to_vec()), ("cmd/main.infy".to_string(), b"package main".to_vec())];
        assert_ne!(ha, dirhash(&mut d));
    }

    /// §5.8 / §7.3: the empty tree has a fixed hash (a build with no files must still be comparable).
    #[test]
    fn dirhash_of_empty_tree_is_stable() {
        let mut e: Vec<(String, Vec<u8>)> = Vec::new();
        let mut f: Vec<(String, Vec<u8>)> = Vec::new();
        assert_eq!(dirhash(&mut e), dirhash(&mut f));
        assert!(dirhash(&mut e).starts_with("h1:"));
    }

    // ---------------------------------------------------------------- §5.8 / appendix E.1 lock

    /// §5.8 item 2 / appendix E.1: `MODULE.infy.lock` round-trips, and rendering is sorted so two
    /// runs over the same graph produce identical bytes (§7.3 reproducibility).
    #[test]
    fn lock_round_trips_and_renders_stably() {
        let text = "\
# comment line
req github.com/user/decoder 2.0.0
req github.com/user/mathx 1.5.0 resolved-from github.com/user/mathx@1.5.0
dirh github.com/user/mathx 1.5.0 h1:AAA
dirh-m github.com/user/mathx 1.5.0 h1:BBB
sumdb github.com/user/mathx 1.5.0 tree_head=h1:CCC inset=7 proof=h1:DDD
priv git.internal/*
nosum internal.example
";
        let l = Lock::parse(text);
        assert_eq!(l.req.len(), 2);
        assert_eq!(l.req[1].2.as_deref(), Some("github.com/user/mathx@1.5.0"));
        assert_eq!(l.dirh[0].2, "h1:AAA");
        assert_eq!(l.dirh_m[0].2, "h1:BBB");
        assert_eq!(l.sumdb[0].3, 7);
        assert_eq!(l.priv_globs, vec!["git.internal/*".to_string()]);
        assert_eq!(l.nosum_globs, vec!["internal.example".to_string()]);
        assert_eq!(l.checksum_of("github.com/user/mathx", "1.5.0"), Some("h1:AAA"));
        assert_eq!(l.checksum_of("github.com/user/mathx", "9.9.9"), None);

        // rendering is order-insensitive: build the same lock from shuffled input and compare bytes
        let shuffled = "\
priv git.internal/*
req github.com/user/mathx 1.5.0 resolved-from github.com/user/mathx@1.5.0
req github.com/user/decoder 2.0.0
nosum internal.example
dirh github.com/user/mathx 1.5.0 h1:AAA
dirh-m github.com/user/mathx 1.5.0 h1:BBB
sumdb github.com/user/mathx 1.5.0 tree_head=h1:CCC inset=7 proof=h1:DDD
";
        assert_eq!(Lock::parse(text).render(), Lock::parse(shuffled).render());
        assert_eq!(Lock::parse(&l.render()).req.len(), 2, "render output must re-parse");
    }

    /// §5.7 step 4 / §5.8 item 2: a lock that no longer matches the manifest is `E-0200` — both for
    /// a missing requirement and for one the manifest has dropped.
    #[test]
    fn lock_out_of_sync_with_manifest_is_e0200() {
        let m = full();
        let codes_for = |lock: &str| {
            let l = Lock::parse(lock);
            let mut h = Handler::default();
            l.check_against_manifest(&m, &mut h);
            h.diags.iter().map(|d| (d.code.clone(), d.title.clone())).collect::<Vec<_>>()
        };
        let in_sync = "\"std\" is built in, so only declared dependencies need a req line\n\
req github.com/user/mathx 1.5.0\n\
req github.com/user/decoder 2.0.0\n";
        assert!(codes_for(in_sync).is_empty(), "{:?}", codes_for(in_sync));
        let missing = codes_for("req github.com/user/mathx 1.5.0\n");
        assert!(missing.iter().any(|(c, t)| c == "E-0200" && t.contains("decoder")), "{missing:?}");
        let extra = codes_for("req github.com/user/mathx 1.5.0\nreq github.com/user/decoder 2.0.0\nreq github.com/user/gone 1.0.0\n");
        assert!(extra.iter().any(|(c, t)| c == "E-0200" && t.contains("gone")), "{extra:?}");
    }

    /// §5.7 step 4: the `req` line for a `^1.5` demand is its lower bound, which is what MVS chose.
    #[test]
    fn lock_records_the_lower_bound_of_a_range_demand() {
        let (m, _) = parse("[module]\npath = \"github.com/u/m\"\n\n[dependencies]\n\"github.com/u/a\" = \"^1.5\"\n");
        let mut h = Handler::default();
        Lock::parse("req github.com/u/a 1.5.0\n").check_against_manifest(&m.expect("manifest"), &mut h);
        assert!(h.diags.is_empty(), "{:?}", h.diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>());
    }

    // ---------------------------------------------------------------- §2.2 / §5.9 build scope

    fn build_cfg<'a>(features: &'a [String], a_track: bool, profile: &'a str, test: bool) -> BuildConfig<'a> {
        BuildConfig { os: "windows", arch: "x86_64", abi: "gnu", features, a_track, b_track: false, profile, test }
    }

    /// §2.2's expression grammar evaluated against the configuration — including the two names the
    /// spec uses but leaves out of its own list (`ignore`, `test`), and a half-written expression,
    /// which must be an error rather than a silent "include the file anyway".
    #[test]
    fn constraint_expression_evaluates_the_build_config() {
        let none: Vec<String> = Vec::new();
        let normal = build_cfg(&none, false, "release", false);
        let testb = build_cfg(&none, false, "release", true);
        assert_eq!(constraint_matches("windows && x86_64", &normal), Ok(true));
        assert_eq!(constraint_matches("!(linux, aarch64)", &normal), Ok(true), "§2.2: `,` means `&&`");
        assert_eq!(constraint_matches("a_track", &normal), Ok(false));
        assert_eq!(constraint_matches("!a_track", &normal), Ok(true));
        assert_eq!(constraint_matches("debug", &normal), Ok(false), "`debug` is the [profile] name");
        assert_eq!(constraint_matches("release", &build_cfg(&none, false, "release-small", false)), Ok(true));
        assert_eq!(constraint_matches("ignore", &testb), Ok(false), "§2.2: `ignore` is out of every build");
        assert_eq!(constraint_matches("test", &normal), Ok(false));
        assert_eq!(constraint_matches("test", &testb), Ok(true), "§14.7: `//infy:build test` is the test build");
        assert_eq!(constraint_matches("!test", &testb), Ok(false), "and `!test` keeps a file out of it");
        let gpu = vec!["gpu".to_string()];
        assert_eq!(constraint_matches("gpu", &build_cfg(&gpu, false, "release", false)), Ok(true), "[features] name");
        assert_eq!(constraint_matches("gpu", &build_cfg(&none, false, "release", false)), Ok(false));
        assert!(constraint_matches("linux &&", &normal).is_err(), "incomplete expression must not default to include");
    }

    /// §5.9 items 1-2: a `*_test.infy` file, and every file of a `tests/…` package, take part only
    /// in the test build. The file's own `//infy:build` text is a second, independent gate.
    #[test]
    fn test_files_enter_only_the_test_build() {
        let none: Vec<String> = Vec::new();
        let pick = |name: &str, constraint: Option<&str>, in_tests: bool| -> (bool, bool) {
            let f = PackageFile {
                abs: PathBuf::from(name),
                display: name.to_string(),
                name: name.to_string(),
                is_test: name.ends_with("_test.infy"),
                constraint: constraint.map(|s| s.to_string()),
            };
            let p = Package {
                dir: PathBuf::from("."),
                rel: if in_tests { "tests".to_string() } else { "core/build".to_string() },
                name: "build".to_string(),
                files: vec![f.clone()],
                internal_under: None,
                is_test_pkg: in_tests,
            };
            (
                file_selected(&f, &p, &build_cfg(&none, false, "release", false)).expect("no constraint error"),
                file_selected(&f, &p, &build_cfg(&none, false, "release", true)).expect("no constraint error"),
            )
        };
        assert_eq!(pick("mod.infy", None, false), (true, true), "an ordinary file is in both builds");
        assert_eq!(pick("mod_test.infy", None, false), (false, true), "§5.9: out of the ordinary build");
        assert_eq!(pick("report.infy", None, true), (false, true), "§5.9: a `tests/…` package is test-only");
        assert_eq!(pick("mod.infy", Some("ignore"), false), (false, false), "§2.2 `ignore` wins in both");
        assert_eq!(pick("mod.infy", Some("!test"), false), (true, false), "§2.2 `!test` gates the test build");
        assert_eq!(pick("mod_test.infy", Some("linux"), false), (false, false), "both gates must pass");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("scratch").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    }

    fn write_in(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent dir")).expect("mkdir");
        std::fs::write(p, text).expect("write");
    }

    fn discover_codes(root: &Path) -> (ModuleGraph, Vec<String>) {
        let mut h = Handler::default();
        let g = discover(root, &full(), &mut h);
        let codes = h
            .diags
            .iter()
            .filter(|d| d.code.starts_with('E'))
            .map(|d| format!("{} · {}", d.code, d.title))
            .collect();
        (g, codes)
    }

    /// §5.1 R1/R2 read together with §14.1 and §5.9: one directory is one package, but that package
    /// may be spread over several files (`mod.infy` + `cache.infy` — §14.1's own layout), a
    /// `_test.infy` file may add `package <dir>_test`, and inside `tests/` the file stem names the
    /// package (§14.1's `tests/build_test.infy` = `package build_test`). Before C5 the R2 check was
    /// keyed on the wrong thing — a *repeated* package name was treated as the violation, so one
    /// file of a legal two-file package was dropped and `E-0501` fired.
    #[test]
    fn discover_accepts_the_package_shapes_the_spec_shows() {
        let root = scratch("pkg-discover-shapes");
        write_in(&root, "core/build/mod.infy", "package build\n\npub fn a() -> u32 { 1u32 }\n");
        write_in(&root, "core/build/cache.infy", "package build\n\npub fn b() -> u32 { 2u32 }\n");
        write_in(&root, "core/build/build_test.infy", "package build_test\n\n#[test]\nfn t() { }\n");
        write_in(&root, "tests/report_test.infy", "package report_test\n\n#[test]\nfn u() { }\n");
        let (g, codes) = discover_codes(&root);
        assert!(codes.is_empty(), "the spec's own shapes must discover clean: {codes:?}");
        let build = g.packages.iter().find(|p| p.rel == "core/build").expect("core/build package");
        assert_eq!(build.files.len(), 3, "every file of the package survives, §14.1");
        assert!(!build.is_test_pkg, "`core/build` is an ordinary package");
        assert!(build.files.iter().filter(|f| f.is_test).count() == 1, "`build_test.infy` is the test file");
        let tests = g.packages.iter().find(|p| p.rel == "tests").expect("tests package");
        assert!(tests.is_test_pkg, "§5.9: `tests/…` is test-only");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// §5.1 R2 is still enforced where §5.9 does not carve an exception: two unrelated package names
    /// in one directory, or a test file naming something the directory cannot host.
    #[test]
    fn discover_reports_genuinely_two_package_directories() {
        let root = scratch("pkg-discover-two");
        write_in(&root, "core/build/mod.infy", "package build\n\npub fn a() -> u32 { 1u32 }\n");
        write_in(&root, "core/build/other.infy", "package other\n\npub fn c() -> u32 { 3u32 }\n");
        let (_, codes) = discover_codes(&root);
        assert!(
            codes.iter().any(|c| c.starts_with("E-0501") && c.contains("2 个包名")),
            "R2 must name both packages: {codes:?}"
        );
        let root2 = scratch("pkg-discover-bogus-test");
        write_in(&root2, "core/build/wibble_test.infy", "package zzz\n\n#[test]\nfn t() { }\n");
        let (_, codes2) = discover_codes(&root2);
        assert!(
            codes2.iter().any(|c| c.starts_with("E-0501") && c.contains("包名与目录不符")),
            "a `_test.infy` file may not name an arbitrary package: {codes2:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    /// §7.3: `read_dir` hands entries in arbitrary order, and that order used to leak into the
    /// package's file list — which decides `FileId` numbering downstream, so the emitted C text and
    /// the build-id became a function of the filesystem. Files are now sorted by name.
    #[test]
    fn discover_orders_files_by_name() {
        let root = scratch("pkg-discover-order");
        for (i, n) in ["zeta.infy", "mid.infy", "alpha.infy"].iter().enumerate() {
            write_in(&root, &format!("core/build/{n}"), &format!("package build\n\npub fn f{i}() -> u32 {{ {}u32 }}\n", i + 1));
        }
        let (g, codes) = discover_codes(&root);
        assert!(codes.is_empty(), "{codes:?}");
        let p = g.packages.iter().find(|p| p.rel == "core/build").expect("core/build package");
        let names: Vec<String> = p.files.iter().map(|f| f.name.clone()).collect();
        assert_eq!(names, vec!["alpha.infy".to_string(), "mid.infy".to_string(), "zeta.infy".to_string()], "§7.3: name order, not readdir order");
        let _ = std::fs::remove_dir_all(&root);
    }
}
