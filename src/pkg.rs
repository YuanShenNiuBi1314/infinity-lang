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

/// §2.2: the closed set of build-constraint names, before module `features` are added.
pub const BASE_CONSTRAINTS: &[&str] = &[
    "windows", "linux", "macos", "x86_64", "aarch64", "msvc", "gnu", "a_track", "b_track", "debug", "release",
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
        let known = Manifest::KNOWN.iter().any(|p| *p == path || path.starts_with(&format!("{p}.")) || (matches!(k.as_str(), "path" | "version" | "infy" | "authors") && prefix == "module"))
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
    // A directory with no `.infy` files contributes no package but its children may.
    if !files.is_empty() {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let dir_name = rel_str.rsplit('/').next().unwrap_or("root").to_string();
        let mut pkg_files = Vec::new();
        let mut names: BTreeMap<String, ()> = BTreeMap::new();
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
            // §5.1 R1: package clause must equal the directory's last segment (cmd/* entry files
            // may omit it and inherit the directory name, §3.1).
            if declared_package(&text).is_none() && !rel_str.starts_with("cmd/") {
                handler.emit(
                    Diagnostic::new("E-0501", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!("{}: 缺少 package 声明", display_of(module_root, path)))
                        .note("只有 `cmd/*` 下的入口文件可以省略 package 子句（§3.1）")
                        .help(format!("补一行 `package {dir_name}`")),
                );
            }
            if pkg_name != dir_name && !(rel_str.is_empty()) {
                handler.emit(
                    Diagnostic::new("E-0501", Phase::Name, 0, Position { line: 1, column: 1 })
                        .titled(format!("包名与目录不符：`package {pkg_name}` 位于 `{rel_str}/`"))
                        .note("一个目录 = 一个包，包名必须等于目录末段名（§5.1 R1）")
                        .help(format!("改为 `package {dir_name}`")),
                );
            }
            if names.insert(pkg_name.clone(), ()).is_some() {
                // §5.1 R2: two package names in one directory is not allowed.
                continue;
            }
            pkg_files.push(PackageFile {
                abs: path.clone(),
                display: display_of(module_root, path),
                name: name.clone(),
                is_test: name.ends_with("_test.infy"),
                constraint: build_constraint_of(&text),
            });
        }
        if pkg_files.len() != files.len() {
            // duplicate package names: report once per directory
            handler.emit(
                Diagnostic::new("E-0501", Phase::Name, 0, Position { line: 1, column: 1 })
                    .titled(format!("`{rel_str}/` 内存在两个包名"))
                    .note("同一目录内不得同时存在两个包名（§5.1 R2）"),
            );
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

/// Evaluate a build-constraint expression against the active build configuration.
pub fn constraint_matches(expr: &str, os: &str, arch: &str, abi: &str, features: &[String], a_track: bool, b_track: bool, profile: &str) -> Result<bool, String> {
    let toks = constraint_tokens(expr)?;
    let mut p = ConstraintParser { toks, pos: 0 };
    let v = p.or()?;
    if p.pos != p.toks.len() {
        return Err(format!("构建约束有多余内容：`{expr}`"));
    }
    Ok(eval(&v, os, arch, abi, features, a_track, b_track, profile))
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
            _ => Err("构建约束表达式不完整".to_string()),
        }
    }
}

fn eval(n: &CNode, os: &str, arch: &str, abi: &str, features: &[String], a_track: bool, b_track: bool, profile: &str) -> bool {
    match n {
        CNode::Not(x) => !eval(x, os, arch, abi, features, a_track, b_track, profile),
        CNode::And(xs) => xs.iter().all(|x| eval(x, os, arch, abi, features, a_track, b_track, profile)),
        CNode::Or(xs) => xs.iter().any(|x| eval(x, os, arch, abi, features, a_track, b_track, profile)),
        CNode::Name(s) => match s.as_str() {
            "windows" => os == "windows",
            "linux" => os == "linux",
            "macos" => os == "macos",
            "x86_64" => arch == "x86_64",
            "aarch64" => arch == "aarch64",
            "msvc" => abi == "msvc",
            "gnu" => abi == "gnu",
            "a_track" => a_track,
            "b_track" => b_track,
            "debug" => profile == "debug",
            "release" => profile == "release" || profile == "release-small",
            "ignore" => false,
            other => features.iter().any(|f| f == other),
        },
    }
}

/// §2.2: names outside the closed set plus the module's `features` are `E-0012`.
pub fn unknown_constraint_names(expr: &str, features: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for tok in constraint_tokens(expr).unwrap_or_default() {
        if tok.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') && !tok.is_empty() {
            let known = BASE_CONSTRAINTS.contains(&tok.as_str()) || tok == "ignore" || features.iter().any(|f| *f == tok);
            if !known {
                out.push(tok);
            }
        }
    }
    out
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
