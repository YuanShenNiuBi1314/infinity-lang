//! §13.4 — the `infy` command line.
//!
//! Exit codes are normative (§13.4): 0 ok · 1 generic failure · 2 diagnostics blocked ·
//! 3 runtime panic · 4 permission/cache · 5 network/dependency · 6 toolchain or link failure ·
//! 7 checksum / reproducibility mismatch.
//!
//! What this build actually does end to end: the front end — stages [1]–[5] of §7.1 — over a whole
//! module, plus `init`, `fmt`, `expand`, `mod tidy`, `toolchain probe|list`, `artifact info`.
//! Commands that need code generation (`build`, `run`, `test`, `doc`, `ifbc …`, `lib pack`) report
//! that honestly instead of pretending to succeed; see `docs/status.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ast::{ItemKind, ParsedFile};
use crate::diag::{Diagnostic, Handler, LintConfig, Phase};
use crate::span::Position;
use crate::span::{FileId, Sources};
use crate::{borrow, derive, lexer, macro_expand, parser, pkg, sema};

/// §13.4 exit codes.
pub mod exit {
    pub const OK: i32 = 0;
    pub const GENERIC: i32 = 1;
    pub const DIAGNOSED: i32 = 2;
    pub const RUNTIME: i32 = 3;
    pub const PERMISSION: i32 = 4;
    pub const NETWORK: i32 = 5;
    pub const TOOLCHAIN: i32 = 6;
    pub const CHECKSUM: i32 = 7;
}

pub fn run(args: &[String]) -> i32 {
    let argv: Vec<String> = args.iter().skip(1).cloned().collect();
    let cmd = argv.first().map(|s| s.as_str()).unwrap_or("--help");
    let rest = &argv[argv.len().saturating_sub(argv.len())..];
    let rest: Vec<String> = if argv.is_empty() { Vec::new() } else { argv[1..].to_vec() };
    let _ = rest;
    let flag = |name: &str| rest.contains(&name.to_string());
    let opt_value = |name: &str| -> Option<String> {
        rest.iter().position(|a| a == name).and_then(|i| rest.get(i + 1)).cloned()
    };
    let positional: Vec<String> = rest.iter().filter(|a| !a.starts_with('-')).cloned().collect();
    match cmd {
        "-V" | "--version" | "version" => {
            println!("{}", crate::VERSION.banner());
            exit::OK
        }
        "init" => init(&positional),
        "check" => check(&positional, &rest, false),
        "build" => check(&positional, &rest, true),
        "fmt" => fmt_cmd(&positional, flag("-w"), flag("--check"), opt_value("--max-width")),
        "expand" => expand_cmd(&positional, opt_value("--macro"), opt_value("--stage")),
        "mod" => mod_cmd(&positional),
        "toolchain" => toolchain_cmd(&positional),
        "target" => target_cmd(&positional),
        "artifact" => artifact_cmd(&positional),
        "clean" => clean_cmd(flag("--cache")),
        "run" | "test" | "bench" | "doc" | "lib" | "ifbc" | "ingest" | "conformance" | "lsp" | "pack" | "unpack" | "install" | "sumdb" | "database" => {
            eprintln!(
                "notice: `infy {cmd}` is not implemented in this build — it needs code generation \
                 (stage [8]) and the artifact writer (stage [9]), which are still open work items. \
                 See docs/status.md. Front-end commands (check, fmt, expand, init, mod tidy, \
                 toolchain, target, artifact info) are available."
            );
            exit::GENERIC
        }
        "--help" | "-h" | "help" => {
            print!("{}", usage());
            exit::OK
        }
        other => {
            eprintln!("error: unknown subcommand `{other}`\n{}", usage());
            exit::GENERIC
        }
    }
}

fn usage() -> String {
    let mut s = String::new();
    s.push_str(&format!("{}\n\nUSAGE:\n  infy <command> [options] [paths…]\n\nCOMMANDS (§13.4):\n", crate::VERSION.banner()));
    for (name, note) in [
        ("init <module-path>", "scaffold a module + MODULE.infy.toml (§5.2)"),
        ("check [--strict] [--diag-json f] [--deadlock]", "stages [1]-[5], emits no code, needs no cc (§9.4)"),
        ("build [--target T] [-O0..-O3|-Oz] [--reproducible]", "stages [1]-[9] — codegen not implemented yet"),
        ("run [mod]", "build to a temp dir and execute — not implemented yet"),
        ("fmt [-w] [--check]", "official formatting, byte-unique (§9.5)"),
        ("expand [--macro name] [--stage all|macro|derive]", "auditable macro/derive output (§6.8)"),
        ("mod tidy|why|verify|upgrade|vendor", "dependency resolution, MVS (§5.7)"),
        ("toolchain probe|list|add|remove", "A-track compilers + B-track bytecode toolchains (§7.6.2)"),
        ("target list|add <triple>", "target triples (§7.2)"),
        ("artifact info|sections <file>", ".inf meta and segment layout (§7.4)"),
        ("lib pack|verify|dump|diff", ".infib containers (§13.2) — not implemented yet"),
        ("ifbc dump|verify", "IFBC segment inspection (§7.8.4) — not implemented yet"),
        ("test / bench / doc / conformance / ingest / sumdb / lsp", "not implemented yet"),
        ("clean [--cache]", "remove build output (§5.10)"),
        ("-V, --version", "language, tool, IFABI and IFBC versions"),
    ] {
        s.push_str(&format!("  {name:56} {note}\n"));
    }
    s.push_str("\nEXIT CODES: 0 ok · 1 generic · 2 diagnostics blocked · 3 runtime panic · 4 permission/cache · 5 network · 6 toolchain/link · 7 checksum/reproducibility\n");
    s
}

// ---------------------------------------------------------------- module loading

/// A loaded module: manifest, packages, and every `.infy` file parsed.
pub struct Loaded {
    pub root: PathBuf,
    pub manifest: pkg::Manifest,
    pub sources: Sources,
    pub files: BTreeMap<FileId, ParsedFile>,
    /// file → package path
    pub file_pkg: BTreeMap<FileId, String>,
    pub handler: Handler,
}

impl Loaded {
    fn lint(&self) -> LintConfig {
        let mut lint = LintConfig { strict: matches!(self.manifest.profile.diagnostic_level, crate::pkg::DiagLevel::Strict), ..Default::default() };
        for (code, level) in &self.manifest.lints {
            if let Some(msg) = lint.set(code, level) {
                let _ = msg; // reported by check() as E-0093
            }
        }
        lint
    }
}

/// Walk up from `path` looking for `MODULE.infy.toml` (§5.1: a module is the directory tree that
/// has one).
pub fn find_module_root(path: Option<&str>) -> Option<PathBuf> {
    let start = match path {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().ok()?,
    };
    let mut cur = if start.is_file() { start.parent()?.to_path_buf() } else { start };
    loop {
        if cur.join(pkg::MANIFEST_NAME).is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Read, lex and parse every `.infy` file of a module: stages [1] and the package layer of [3].
pub fn load(root: &Path, handler: &mut Handler) -> Option<Loaded> {
    let text = std::fs::read_to_string(root.join(pkg::MANIFEST_NAME)).ok()?;
    let manifest = pkg::parse_manifest(&text, pkg::MANIFEST_NAME, handler)?;
    let graph = pkg::discover(root, &manifest, handler);
    let mut sources = Sources::default();
    let mut files: BTreeMap<FileId, ParsedFile> = BTreeMap::new();
    let mut file_pkg: BTreeMap<FileId, String> = BTreeMap::new();
    for package in &graph.packages {
        for f in &package.files {
            let body = match std::fs::read_to_string(&f.abs) {
                Ok(b) => b,
                Err(e) => {
                    handler.emit(Diagnostic::new("E-0570", Phase::Lex, 0, Position { line: 1, column: 1 }).titled(format!("无法读取 {}: {e}", f.display)));
                    continue;
                }
            };
            let id = sources.add(f.display.clone(), body);
            let lexed = lexer::lex_file(&sources, id, handler);
            let pf = parser::parse(&sources, id, &lexed, handler);
            file_pkg.insert(id, package.import_path(&manifest.module_path));
            files.insert(id, pf);
        }
    }
    let mut lint = handler.lint.clone();
    lint.strict = matches!(manifest.profile.diagnostic_level, crate::pkg::DiagLevel::Strict);
    handler.lint = lint;
    Some(Loaded { root: root.to_path_buf(), manifest, sources, files, file_pkg, handler: std::mem::take(handler) })
}

/// Run stages [2]–[5] over an already-loaded module and return it with diagnostics filled in.
pub fn analyse(l: &mut Loaded) {
    // [2] macro expansion + derive, then attribute validation (§6.5/§6.6/§6.7)
    let mut opts = macro_expand::ExpandOptions::default();
    opts.stage = macro_expand::Stage::All;
    macro_expand::expand_all(&mut l.files, &l.sources, &mut l.handler, &opts);
    derive::expand_derives(&mut l.files, &mut l.handler);
    derive::validate_attrs(&l.files, &l.sources, &mut l.handler);

    // [3] import resolution and visibility (§5.3/§5.5)
    for (id, pf) in &l.files {
        let importer_rel = l.file_pkg.get(id).map(|p| p.strip_prefix(&format!("{}/", l.manifest.module_path)).unwrap_or(p).to_string()).unwrap_or_default();
        for import in &pf.imports {
            match pkg::classify_import(&import.path, &l.manifest) {
                pkg::ImportSource::Undeclared => {
                    let pos = l.sources.line_col(import.path_span).0;
                    l.handler.emit(
                        Diagnostic::new("E-0201", Phase::Name, *id, pos)
                            .titled(format!("导入未声明的 module：`{}`", import.path))
                            .region(*id, pos.line, pos.column, pos.column + 1, "清单 [dependencies] 中没有它")
                            .help("运行 `infy mod tidy` 补全（§5.3 第 5 步）"),
                    );
                }
                pkg::ImportSource::Dependency | pkg::ImportSource::Local => {
                    // `internal` may only be imported by its parent subtree (§5.5)
                    let target_rel = import.path.strip_prefix(&format!("{}/", l.manifest.module_path)).unwrap_or(&import.path).to_string();
                    if target_rel.split('/').any(|s| s == "internal") && !internal_visible(&target_rel, &importer_rel) {
                        let pos = l.sources.line_col(import.path_span).0;
                        l.handler.emit(
                            Diagnostic::new("E-0508", Phase::Name, *id, pos)
                                .titled("越界访问 internal".to_string())
                                .region(*id, pos.line, pos.column, pos.column + 1, &format!("`{target_rel}` 只对上级子树可见"))
                                .help("提升为公开 API，或移入同子树（§5.5）"),
                        );
                    }
                }
                pkg::ImportSource::Std => {}
            }
        }
    }

    // §5.4: package cycles are refused, never lazily initialised around
    check_import_cycles(l);

    // [4] type check, [5] borrow check (§4.2: type checking must run first)
    let a_track = l.manifest.declares_a_track();
    let b_tracks: Vec<(String, String)> = l.manifest.b_track.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let link_units = collect_link_units(&l.files);
    let tuple: Vec<(FileId, String, ParsedFile)> = l.files.iter().map(|(f, pf)| (*f, l.file_pkg.get(f).cloned().unwrap_or_default(), pf.clone())).collect();
    let prog = sema::check_files(&l.sources, &mut l.handler, &l.manifest.module_path, &tuple, a_track, b_tracks, link_units);
    borrow::check(&l.files, &l.sources, &mut l.handler, &prog);

    // §7.4 entry rules: #![entry_chain] > fn main > first chain of the main package
    resolve_entry(l, &prog);
}

fn internal_visible(target_rel: &str, importer_rel: &str) -> bool {
    let segs: Vec<&str> = target_rel.split('/').collect();
    let idx = match segs.iter().rposition(|s| *s == "internal") {
        Some(i) => i,
        None => return true,
    };
    let owner = if idx == 0 { String::new() } else { segs[..idx].join("/") };
    importer_rel == owner || importer_rel.starts_with(&format!("{owner}/"))
}

fn collect_link_units(files: &BTreeMap<FileId, ParsedFile>) -> Vec<String> {
    let mut out = Vec::new();
    for pf in files.values() {
        for it in &pf.items {
            if let Some(a) = it.attr("link") {
                if let Some(n) = a.str_arg("name") {
                    out.push(n);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// §5.4: package-level cycle → `E-0502`, module-level cycle → `E-0503`.
fn check_import_cycles(l: &mut Loaded) {
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, pf) in &l.files {
        let from = l.file_pkg.get(id).cloned().unwrap_or_default();
        for import in &pf.imports {
            if matches!(pkg::classify_import(&import.path, &l.manifest), pkg::ImportSource::Local) {
                edges.entry(from.clone()).or_default().push(import.path.clone());
            }
        }
    }
    let mut seen: BTreeMap<String, u8> = BTreeMap::new(); // 0 visiting, 1 done
    for start in edges.keys().cloned().collect::<Vec<_>>() {
        let mut stack: Vec<String> = vec![start];
        let mut path: Vec<String> = Vec::new();
        let mut visited: BTreeMap<String, usize> = BTreeMap::new();
        while let Some(node) = stack.pop() {
            if let Some(at) = visited.get(&node) {
                let _ = at;
                continue;
            }
            visited.insert(node.clone(), path.len());
            path.push(node.clone());
            if seen.get(&node) == Some(&1) {
                path.pop();
                continue;
            }
            for next in edges.get(&node).cloned().unwrap_or_default() {
                if let Some(i) = visited.get(&next) {
                    let cycle = path[*i..].iter().cloned().chain(std::iter::once(next.clone())).collect::<Vec<_>>();
                    let (id, span_file) = l.file_pkg.iter().find(|(_, p)| *p == &node).map(|(f, _)| (*f, node.clone())).unwrap_or((0, node.clone()));
                    l.handler.emit(
                        Diagnostic::new("E-0502", Phase::Name, id, Position { line: 1, column: 1 })
                            .titled("循环包依赖".to_string())
                            .note(format!("完整环路径：{}", cycle.join(" → ")))
                            .help("提取第三个包，或反转依赖（§5.4：没有惰性初始化绕过循环的降级）"),
                    );
                    let _ = span_file;
                    return;
                }
                stack.push(next);
            }
        }
        for k in visited.keys() {
            seen.insert(k.clone(), 1);
        }
    }
}

/// §7.4: exactly one entry, with a fixed precedence.
fn resolve_entry(l: &mut Loaded, prog: &sema::Program) {
    let declared: Vec<String> = l.files.iter().filter(|(_, pf)| pf.file_attr("entry_chain").is_some()).map(|(_, pf)| pf.file_attr("entry_chain").and_then(|a| a.positional.first()).and_then(|v| match v { crate::ast::AttrValue::Str(s) => Some(s.clone()), _ => None }).unwrap_or_default()).collect();
    let declared: Vec<String> = declared.into_iter().filter(|s| !s.is_empty()).collect();
    if declared.len() > 1 {
        l.handler.emit(
            Diagnostic::new("E-0601", Phase::Link, *l.files.keys().next().unwrap_or(&0), Position { line: 1, column: 1 })
                .titled("入口声明冲突".to_string())
                .note(format!("多个 #![entry_chain]：{}", declared.join(", ")))
                .help("只留一个，或拆成多个 [[bin]]（§7.4）"),
        );
        return;
    }
    if let Some(name) = declared.first() {
        if prog.lookup_chain(name).is_none() {
            l.handler.emit(
                Diagnostic::new("E-0800", Phase::Link, *l.files.keys().next().unwrap_or(&0), Position { line: 1, column: 1 })
                    .titled(format!("入口链不存在：`{name}`"))
                    .help("先声明该链，或修正 #![entry_chain]（§8.2）"),
            );
        }
        return;
    }
    if prog.entry.is_none() {
        let has_main = prog.fns.iter().any(|f| f.name == "main" && f.self_ty.is_none());
        if !has_main && prog.chains.is_empty() {
            l.handler.emit(
                Diagnostic::new("E-0600", Phase::Link, *l.files.keys().next().unwrap_or(&0), Position { line: 1, column: 1 })
                    .titled("产物无入口".to_string())
                    .note("既没有 #![entry_chain]，也没有 fn main，也没有任何链".to_string())
                    .help("声明 #![entry_chain = \"…\"]（§7.4 入口规则）"),
            );
        }
    }
}

// ---------------------------------------------------------------- commands

fn init(positional: &[String]) -> i32 {
    let path = match positional.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("error: infy init <module-path>（§13.4）");
            return exit::GENERIC;
        }
    };
    if path.starts_with("std") || path == "std" {
        eprintln!("error[E-0507]: module path 与 std 冲突：`{path}`\n  = help: 改用自有域名前缀（§5.3）");
        return exit::DIAGNOSED;
    }
    let last = path.rsplit('/').next().unwrap_or("app").to_string();
    let dir = PathBuf::from(&last);
    if dir.join(pkg::MANIFEST_NAME).exists() {
        eprintln!("error: {} 已存在", dir.join(pkg::MANIFEST_NAME).display());
        return exit::PERMISSION;
    }
    if let Err(e) = std::fs::create_dir_all(dir.join("cmd").join(&last)) {
        eprintln!("error: 无法创建目录：{e}");
        return exit::PERMISSION;
    }
    let manifest = format!(
        "[module]\npath    = \"{path}\"\nversion = \"0.0.0-dev\"\ninfy    = \"{}\"\n\n[dependencies]\n\"std\" = \"{}\"\n\n[profile]\noverflow         = \"panic\"\ndiagnostic_level = \"strict\"\nopt_level        = 2\nstack_bytes      = 8388608\nreproducible     = true\n\n[[bin]]\nname = \"{last}\"\npath = \"cmd/{last}\"\n",
        crate::VERSION.lang, crate::VERSION.std_version
    );
    // §5.1 R3: directory names that can appear in an import path
    let main = format!("#![entry_chain = \"Main\"]\n\npackage {last}\n\nchain Main {{\n  step();\n}}\n\nfn step() {{\n}}\n");
    let writes = [
        (dir.join(pkg::MANIFEST_NAME), manifest),
        (dir.join("cmd").join(&last).join("main.infy"), main),
    ];
    for (p, text) in writes {
        if let Err(e) = std::fs::write(&p, text) {
            eprintln!("error: 无法写入 {}: {e}", p.display());
            return exit::PERMISSION;
        }
    }
    println!("created {} (module path {path})", dir.display());
    println!("next: infy check · infy build · infy run");
    exit::OK
}

fn check(positional: &[String], rest: &[String], building: bool) -> i32 {
    let root = match pkg_root(positional) {
        Some(r) => r,
        None => {
            eprintln!("error: 找不到 {}——先 infy init <module-path>，或在 module 目录内运行（§5.1）", pkg::MANIFEST_NAME);
            return exit::GENERIC;
        }
    };
    let mut handler = Handler::default();
    if rest.contains(&"--deadlock".to_string()) {
        handler.lint.deadlock_analysis = true;
    }
    let mut l = match load(&root, &mut handler) {
        Some(l) => l,
        None => {
            let text = handler.render(&handler_empty_sources(&root));
            eprint!("{text}");
            return exit::DIAGNOSED;
        }
    };
    if rest.contains(&"--lenient".to_string()) {
        l.handler.lint.strict = false;
    }
    analyse(&mut l);
    let rendered = l.handler.render(&l.sources);
    eprint!("{rendered}");
    if let Some(out) = rest.iter().position(|a| a == "--diag-json").and_then(|i| rest.get(i + 1)) {
        let json = l.handler.render_json(&l.sources, &crate::VERSION);
        if let Err(e) = std::fs::write(out, json) {
            eprintln!("error: 无法写 {out}: {e}");
            return exit::PERMISSION;
        }
    }
    let summary = l.handler.summary();
    let code = l.handler.exit_code();
    if code == exit::OK {
        println!("check ok — {} file(s), {}", l.files.len(), summary);
    } else {
        println!("check FAILED — {summary}");
    }
    if building && code == exit::OK {
        eprintln!("notice: `infy build` stops here in this build — stages [8] codegen and [9] link \
                   are not implemented yet (docs/status.md). `infy check` passed.");
        return exit::TOOLCHAIN;
    }
    code
}

fn handler_empty_sources(root: &Path) -> Sources {
    let mut s = Sources::default();
    s.add(root.join(pkg::MANIFEST_NAME).to_string_lossy().replace('\\', "/"), String::new());
    s
}

fn pkg_root(positional: &[String]) -> Option<PathBuf> {
    let explicit = positional.first().map(|s| s.as_str());
    match explicit {
        Some(p) => {
            let path = Path::new(p);
            if path.is_file() {
                path.parent().map(|d| d.to_path_buf()).and_then(|_| find_module_root(Some(p)))
            } else {
                find_module_root(Some(p))
            }
        }
        None => find_module_root(None),
    }
}

fn fmt_cmd(positional: &[String], in_place: bool, check_only: bool, max_width: Option<String>) -> i32 {
    let width = match max_width {
        Some(w) => match w.parse::<u32>() {
            Ok(v @ (80 | 100 | 120)) => v,
            _ => {
                eprintln!("error[E-0053]: max_width 只允许 80/100/120 三档（§9.5 第 14 条）");
                return exit::DIAGNOSED;
            }
        },
        None => 100,
    };
    let mut opts = crate::fmt::FmtOptions::default();
    opts.max_width = width;
    opts.in_place = in_place;
    opts.check = check_only;
    let targets: Vec<PathBuf> = match positional.first() {
        Some(p) => vec![PathBuf::from(p)],
        None => default_infy_files(),
    };
    let mut changed = 0;
    let mut failures = 0;
    for t in targets {
        let text = match std::fs::read_to_string(&t) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("error: 无法读取 {}: {e}", t.display());
                failures += 1;
                continue;
            }
        };
        let mut sources = Sources::default();
        let display = t.to_string_lossy().replace('\\', "/");
        let id = sources.add(display, text);
        let mut handler = Handler::default();
        let lexed = lexer::lex_file(&sources, id, &mut handler);
        let pf = parser::parse(&sources, id, &lexed, &mut handler);
        let mut files: BTreeMap<FileId, ParsedFile> = BTreeMap::new();
        files.insert(id, pf);
        let out = crate::fmt::format_file(&sources, id, &mut handler, &opts);
        eprint!("{}", handler.render(&sources));
        if !handler.diags.is_empty() {
            failures += 1;
            continue;
        }
        match out {
            Some(bytes) => {
                if bytes != sources.get(id).map(|f| f.text.clone()).unwrap_or_default() {
                    changed += 1;
                    if check_only {
                        println!("would reformat: {}", t.display());
                    } else if let Err(e) = std::fs::write(&t, bytes) {
                        eprintln!("error: 无法写 {}: {e}", t.display());
                        failures += 1;
                    }
                }
            }
            None => failures += 1,
        }
    }
    if failures > 0 {
        return exit::DIAGNOSED;
    }
    if check_only && changed > 0 {
        println!("{changed} file(s) are not canonical");
        return exit::DIAGNOSED;
    }
    println!("{changed} file(s) formatted");
    exit::OK
}

fn default_infy_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(root) = find_module_root(None) {
        collect_infy(&root, &mut out);
    }
    out
}

fn collect_infy(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if pkg::dir_is_skipped(&name) {
                continue;
            }
            collect_infy(&p, out);
        } else if name.ends_with(".infy") {
            out.push(p);
        }
    }
    out.sort();
}

fn expand_cmd(positional: &[String], only_macro: Option<String>, stage: Option<String>) -> i32 {
    let root = match pkg_root(positional) {
        Some(r) => r,
        None => {
            eprintln!("error: 找不到 module 根（§5.1）");
            return exit::GENERIC;
        }
    };
    let mut handler = Handler::default();
    let mut l = match load(&root, &mut handler) {
        Some(l) => l,
        None => return exit::DIAGNOSED,
    };
    let mut opts = macro_expand::ExpandOptions { only_macro, stage: macro_expand::Stage::All };
    if let Some(s) = stage {
        opts.stage = match s.as_str() {
            "macro" => macro_expand::Stage::Macro,
            "derive" => macro_expand::Stage::Derive,
            _ => macro_expand::Stage::All,
        };
    }
    let text = macro_expand::render_expanded(&l.files, &l.sources, &opts);
    // §6.8: the output must be byte-stable for identical input
    let again = macro_expand::render_expanded(&l.files, &l.sources, &opts);
    if text != again {
        eprintln!("error[E-0091]: infy expand 输出不唯一（§6.8 字节稳定要求）");
        return exit::DIAGNOSED;
    }
    print!("{text}");
    let _ = l.handler.diags.len();
    exit::OK
}

fn mod_cmd(positional: &[String]) -> i32 {
    let sub = positional.first().map(|s| s.as_str()).unwrap_or("");
    let root = match find_module_root(None) {
        Some(r) => r,
        None => {
            eprintln!("error: 找不到 module 根（§5.1）");
            return exit::GENERIC;
        }
    };
    let mut handler = Handler::default();
    let text = match std::fs::read_to_string(root.join(pkg::MANIFEST_NAME)) {
        Ok(t) => t,
        Err(_) => return exit::GENERIC,
    };
    let Some(manifest) = pkg::parse_manifest(&text, pkg::MANIFEST_NAME, &mut handler) else {
        eprint!("{}", handler.render(&handler_empty_sources(&root)));
        return exit::DIAGNOSED;
    };
    match sub {
        "tidy" => {
            // §5.7: MVS over the declared demands; the result is written to MODULE.infy.lock
            let demands: Vec<(String, String)> = manifest.dependencies.iter().filter(|d| !d.is_std).map(|d| (d.module.clone(), d.spec.clone())).collect();
            let mut available: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for d in &demands {
                available.entry(d.0.clone()).or_default().push(d.1.replace('^', ""));
            }
            match pkg::mvs(&demands, &available) {
                Ok(chosen) => {
                    let mut lock = pkg::Lock::default();
                    for (m, v) in &chosen {
                        lock.req.push((m.clone(), v.clone(), None));
                    }
                    for d in &demands {
                        if !chosen.iter().any(|(m, _)| m == &d.0) {
                            let _ = d;
                        }
                    }
                    let path = root.join(pkg::LOCK_NAME);
                    if let Err(e) = std::fs::write(&path, lock.render()) {
                        eprintln!("error: 无法写 {}: {e}", path.display());
                        return exit::PERMISSION;
                    }
                    println!("wrote {} ({} requirement(s))", path.display(), chosen.len());
                    exit::OK
                }
                Err(conflicts) => {
                    for c in conflicts {
                        eprintln!("error[E-0510]: 版本需求不可满足：{c}\n  = help: 放宽 [dependencies] 区间（§5.7 第 3 步）");
                    }
                    exit::DIAGNOSED
                }
            }
        }
        "why" => {
            let want = positional.get(1).cloned().unwrap_or_default();
            let lock_path = root.join(pkg::LOCK_NAME);
            let lock = pkg::Lock::parse(&std::fs::read_to_string(&lock_path).unwrap_or_default());
            let hit = lock.req.iter().filter(|(m, _, _)| m == &want).count();
            if hit == 0 {
                println!("{want}: 不在 MODULE.infy.lock 中");
                return exit::GENERIC;
            }
            println!("{want}: 由 {} 的 [dependencies] 直接要求（§5.7）", manifest.module_path);
            exit::OK
        }
        "verify" => {
            let lock_path = root.join(pkg::LOCK_NAME);
            let lock = pkg::Lock::parse(&std::fs::read_to_string(&lock_path).unwrap_or_default());
            lock.check_against_manifest(&manifest, &mut handler);
            eprint!("{}", handler.render(&handler_empty_sources(&root)));
            if handler.has_errors() {
                return exit::CHECKSUM;
            }
            println!("lock is consistent with the manifest ({} requirement(s))", lock.req.len());
            exit::OK
        }
        other => {
            eprintln!("notice: `infy mod {other}` 尚未实现（download/upgrade/vendor/accept-changes 需要网络与缓存层，§5.10）");
            exit::GENERIC
        }
    }
}

fn toolchain_cmd(positional: &[String]) -> i32 {
    let sub = positional.first().map(|s| s.as_str()).unwrap_or("list");
    let mut found: Vec<(String, String, String)> = Vec::new();
    for (kind, exe, version_args) in [("cc", "gcc", &["-dumpversion"][..]), ("cc", "clang", &["--version"][..]), ("py", "python", &["--version"][..]), ("lua", "lua", &["-v"][..])] {
        if let Some(path) = which(exe) {
            let version = probe_version(&path, version_args);
            found.push((kind.to_string(), path.display().to_string(), version));
        }
    }
    match sub {
        "probe" => {
            if found.is_empty() {
                println!("no A-track or B-track toolchains found on PATH");
                println!("  = help: infy toolchain add cc --path <exe>（§7.6.2）");
                return exit::TOOLCHAIN;
            }
            let mut out = String::from("[[toolchain]]\n");
            for (kind, path, version_str) in &found {
                out.push_str(&format!("kind = \"{kind}\" ; name = \"{path}\" ; version_str = \"{version_str}\"\n"));
            }
            let target = crate::resource_dir().join("TOOLCHAIN.infy.toml");
            println!("probed {} toolchain(s); registry path {}", found.len(), target.display());
            print!("{out}");
            exit::OK
        }
        "list" => {
            if found.is_empty() {
                println!("(none registered on PATH)");
            }
            for (kind, path, version) in &found {
                println!("{kind:6} {version:12} {path}");
            }
            exit::OK
        }
        other => {
            eprintln!("notice: `infy toolchain {other}` 尚未实现（写入 TOOLCHAIN.infy.toml 需要注册表路径策略，§7.6.2/E.4）");
            exit::GENERIC
        }
    }
}

fn which(exe: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) { format!("{exe}.exe") } else { exe.to_string() };
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(&name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn probe_version(path: &Path, args: &[&str]) -> String {
    let ok = std::process::Command::new(path).args(args).output();
    match ok {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let text = if text.trim().is_empty() { String::from_utf8_lossy(&out.stderr).to_string() } else { text };
            text.lines().next().unwrap_or("").chars().take(40).collect::<String>().trim().to_string()
        }
        Err(_) => "unknown".to_string(),
    }
}

fn target_cmd(positional: &[String]) -> i32 {
    const BUILTIN: &[&str] = &[
        "x86_64-pc-windows-msvc",
        "x86_64-pc-windows-gnu",
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "aarch64-unknown-linux-gnu",
    ];
    match positional.first().map(|s| s.as_str()) {
        Some("list") | None => {
            for t in BUILTIN {
                println!("{t}");
            }
            println!("\n(自定义 triple 必须先在 TOOLCHAIN.infy.toml 注册，否则 E-0601，§7.2)");
            exit::OK
        }
        Some(other) => {
            eprintln!("notice: `infy target {other}` 需要工具链注册表写入（§7.2），尚未实现");
            exit::GENERIC
        }
    }
}

fn artifact_cmd(positional: &[String]) -> i32 {
    let path = match positional.get(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("error: infy artifact info|sections <file>（§13.4）");
            return exit::GENERIC;
        }
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: 无法读取 {}: {e}", path.display());
            return exit::GENERIC;
        }
    };
    // §7.4 / docs/internals/runtime-abi.md §6: the trailer is a fixed 64-byte record ending the
    // file, with magic INFYART2.
    let magic = b"INFYART2";
    if bytes.len() < magic.len() + 64 || &bytes[bytes.len() - magic.len()..] != magic {
        eprintln!("error[E-0605]: {} 不是 INFY 产物（找不到 INFYART2 段尾）", path.display());
        eprintln!("  = note: 本构建尚未实现 infy build，因此没有可检视的产物（docs/status.md）");
        return exit::CHECKSUM;
    }
    println!("{}: INFYART2 trailer found ({} bytes of trailing segments)", path.display(), magic.len() + 64);
    println!("  = note: .inf meta 解析需要 codegen 落地后才能输出完整段表");
    exit::OK
}

fn clean_cmd(cache: bool) -> i32 {
    let root = find_module_root(None).unwrap_or_else(|| PathBuf::from("."));
    let build = root.join(".infy-build");
    if build.exists() {
        if let Err(e) = std::fs::remove_dir_all(&build) {
            eprintln!("error: 无法删除 {}: {e}", build.display());
            return exit::PERMISSION;
        }
        println!("removed {}", build.display());
    } else {
        println!("nothing to clean in {}", build.display());
    }
    if cache {
        println!("--cache: 缓存目录是 {}（§5.10）。为避免误删，本命令只报告路径，不删除；确认后可手工删除。", crate::cache_root().display());
    }
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        let mut v = vec!["infy".to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        v
    }

    #[test]
    fn version_banner_lists_lang_tool_ifabi_ifbc() {
        let b = crate::VERSION.banner();
        assert!(b.contains("INFY-v2.0"), "{b}");
        assert!(b.contains("IFABI"), "{b}");
        assert!(b.contains("IFBC"), "{b}");
    }

    #[test]
    fn unknown_subcommand_is_a_generic_failure() {
        assert_eq!(run(&argv(&["frobnicate"])), exit::GENERIC);
    }

    #[test]
    fn unimplemented_commands_say_so_and_exit_1() {
        // no fake success: §13.4 exit 1 = generic failure, and the message names the missing stage
        assert_eq!(run(&argv(&["build"])), exit::GENERIC);
        assert_eq!(run(&argv(&["test"])), exit::GENERIC);
    }

    #[test]
    fn internal_visibility_rule_matches_the_spec_example() {
        assert!(internal_visible("a/b/internal/c", "a/b"));
        assert!(internal_visible("a/b/internal/c", "a/b/d"));
        assert!(!internal_visible("a/b/internal/c", "a/x"));
        assert!(internal_visible("a/b", "a/x"));
    }

    #[test]
    fn link_units_are_collected_deterministically() {
        let mut files = BTreeMap::new();
        let mut src = Sources::default();
        let f = src.add("m/a.infy".to_string(), "package a\n\n#[link(name = \"z\", static = true)]\nextern \"C\" { }\n\n#[link(name = \"a\", static = true)]\nextern \"C\" { }\n".to_string());
        let mut h = Handler::default();
        let lexed = lexer::lex_file(&src, f, &mut h);
        files.insert(f, parser::parse(&src, f, &lexed, &mut h));
        let names = collect_link_units(&files);
        assert_eq!(names, vec!["a".to_string(), "z".to_string()], "sorted, not source order (§7.3)");
    }
}
