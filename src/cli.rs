//! §13.4 — the `infy` command line.
//!
//! Exit codes are normative (§13.4): 0 ok · 1 generic failure · 2 diagnostics blocked ·
//! 3 runtime panic · 4 permission/cache · 5 network/dependency · 6 toolchain or link failure ·
//! 7 checksum / reproducibility mismatch.
//!
//! What this build actually does end to end: the front end — stages [1]–[5] of §7.1 — over a whole
//! module, stages [7]–[9] (`hir` lowering + C11 codegen + link + the §7.4 container) behind
//! `infy build`, plus `init`, `fmt`, `expand`, `mod tidy`, `toolchain probe|list` and
//! `artifact info|sections`. `infy run`/`test`/`doc`/`ifbc …`/`lib pack` still report that the
//! stage behind them is open work rather than pretending to succeed (see `docs/status.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ast::{ItemKind, ParsedFile};
use crate::diag::{Diagnostic, Handler, LintConfig, Phase};
use crate::span::Position;
use crate::span::{FileId, Sources};
use crate::{artifact, borrow, codegen, derive, hir, lexer, macro_expand, parser, pkg, sema};

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
        "artifact" => artifact_cmd(&positional, &rest),
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
        ("build [--target T] [--cc P] [--reproducible] [--verify-reproducible]", "stages [1]-[9]: codegen, link, §7.4 container"),
        ("run [mod]", "executes the §7.4 artifact — not wired up yet"),
        ("fmt [-w] [--check]", "official formatting, byte-unique (§9.5)"),
        ("expand [--macro name] [--stage all|macro|derive]", "auditable macro/derive output (§6.8)"),
        ("mod tidy|why|verify|upgrade|vendor", "dependency resolution, MVS (§5.7)"),
        ("toolchain probe|list|add|remove", "A-track compilers + B-track bytecode toolchains (§7.6.2)"),
        ("target list|add <triple>", "target triples (§7.2)"),
        ("artifact info|sections <file> [--nodes]", ".inf meta and segment layout (§7.4)"),
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
pub fn analyse(l: &mut Loaded) -> sema::Program {
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
    prog
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
    let prog = analyse(&mut l);
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
        return build_c(&l, &prog, rest);
    }
    if building {
        eprintln!("error: `infy build` 中止于阶段 [1]-[5]：诊断里有阻断项（§7.1：任一阶段出现 `E-` 即中止后续阶段）");
        return code;
    }
    code
}

/// Stages [7], [8] and [9] of §7.1: lower to HIR, emit C11, write it under `.infy-build/`, then
/// link it against the runtime with the A-track `cc` and append the §7.4 segment container.
///
/// `--cc`/`$CC` select the compiler (default `gcc`); a missing one is `E-0602` (§7.6.2), not a
/// silent skip. `--verify-reproducible` re-runs stage [9] in two independent build directories and
/// compares bytes (§7.3 item 3, mismatch → `E-0603`/exit 7).
fn build_c(l: &Loaded, prog: &sema::Program, rest: &[String]) -> i32 {
    let triple = match rest.iter().position(|a| a == "--target").and_then(|i| rest.get(i + 1)) {
        Some(t) => t.clone(),
        None => host_triple(),
    };
    let (triple, target) = match codegen::target_from_triple(&triple) {
        Some(x) => x,
        None => {
            eprintln!(
                "error: E-0601 · target `{triple}` 不在 §7.2 的内建集里，也没有 TOOLCHAIN.infy.toml 可注册（内建集：{}）",
                codegen::BUILTIN_TARGETS.join(", ")
            );
            return exit::TOOLCHAIN;
        }
    };
    let entry_chain = l
        .files
        .values()
        .find_map(|pf| pf.file_attr("entry_chain"))
        .and_then(|a| a.positional.first())
        .and_then(|v| match v {
            crate::ast::AttrValue::Str(s) => Some(s.clone()),
            _ => None,
        });
    let cfg = hir::HirConfig { target, entry_chain };
    let hc = hir::lower(prog, &cfg);
    if !hc.errors.is_empty() {
        // §7.2: no silent degradation — an item that failed to lay out is a build failure.
        for e in hc.errors.iter() {
            eprintln!("error: 阶段 [7] 布局失败：{}（{}）", e.message(), e.spec_code().unwrap_or("§4.8"));
        }
        return exit::TOOLCHAIN;
    }
    let files: Vec<(FileId, String, ParsedFile)> = l
        .files
        .iter()
        .map(|(f, pf)| (*f, l.file_pkg.get(f).cloned().unwrap_or_default(), pf.clone()))
        .collect();
    let cfg = codegen::Config {
        target_triple: triple.clone(),
        target,
        header_include: "infy_rt.h".to_string(),
        overflow_panic: matches!(l.manifest.profile.overflow, pkg::Overflow::Panic),
        module: l.manifest.module_path.clone(),
    };
    let text = match codegen::emit(prog, &hc, &files, &l.sources, &cfg) {
        Ok(t) => t,
        Err(es) => {
            for e in es.iter() {
                eprintln!("{}", e.render());
            }
            eprintln!(
                "error: `infy build` 中止于阶段 [8]：codegen 覆盖了 §14 导览子集，上述构造在范围之外（拒绝而不是生成近似代码，见 docs/BACKLOG.md C2）"
            );
            return exit::TOOLCHAIN;
        }
    };
    let out = match rest.iter().position(|a| a == "--out").and_then(|i| rest.get(i + 1)) {
        Some(p) => PathBuf::from(p),
        None => {
            let stem = l.manifest.module_path.rsplit('/').next().unwrap_or("crate").to_string();
            l.root.join(".infy-build").join(&triple).join(format!("{stem}.c"))
        }
    };
    if let Some(dir) = out.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("error: 无法创建 {}: {e}", dir.display());
            return exit::PERMISSION;
        }
    }
    if let Err(e) = std::fs::write(&out, &text) {
        eprintln!("error: 无法写 {}: {e}", out.display());
        return exit::PERMISSION;
    }
    let chains = hc.items.iter().filter(|i| matches!(i, hir::HirItem::Chain(_))).count();
    let nodes = hc.items.iter().filter_map(|i| match i {
        hir::HirItem::Chain(c) => Some(c.nodes.len()),
        _ => None,
    }).sum::<usize>();
    println!(
        "codegen ok — {} byte(s) of C11 written to {} (stages [7]-[8]; {chains} chain(s), {nodes} node(s))",
        text.len(),
        out.display(),
    );
    let work = out.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let stem = l.manifest.module_path.rsplit('/').next().unwrap_or("crate").to_string();
    let unreproducible = rest.contains(&"--allow-toolchain-drift".to_string());
    let plan = match link_plan(rest, &l.manifest.profile) {
        Ok(p) => p,
        Err(m) => {
            eprintln!("error: E-0602 · {m}（§7.6.2：A 轨工具链缺失或未注册，绝不静默跳过链接）");
            return exit::TOOLCHAIN;
        }
    };
    let bid = compute_build_id(l, &plan, &triple);
    let bytes = match stage9(l, &hc, &out, &plan, target, &triple, &stem, &work, bid, unreproducible) {
        Ok(b) => b,
        Err(m) => {
            eprintln!("error: 阶段 [9] 失败：{m}");
            return exit::TOOLCHAIN;
        }
    };
    let art_path = work.join(host_file_name(&stem, &triple));
    if let Err(e) = std::fs::write(&art_path, &bytes) {
        eprintln!("error: 无法写产物 {}: {e}", art_path.display());
        return exit::PERMISSION;
    }
    let a = match artifact::read(&bytes, crate::VERSION.ifabi) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: 阶段 [9] 写出的容器读不回自己（{e}）—— 这是实现 bug，不是输入问题");
            return exit::TOOLCHAIN;
        }
    };
    let tr = a.trailer;
    println!(
        "artifact ok — {} byte(s) 写到 {}（§7.4：宿主镜像 {} + 段 blob {} + 段尾 {}，{} 段）",
        bytes.len(),
        art_path.display(),
        tr.blob_off,
        tr.blob_len,
        artifact::TRAILER_SIZE,
        tr.seg_count
    );
    println!("  build-id {}", a.meta.build_id);
    println!(
        "  = note: 容器已按冻结布局读回（.inf meta {} 字节 | 节点表 {} 行 | .inf sum {} 条依赖 dirhash | 入口 {}）",
        artifact::META_FIXED_BYTES,
        a.meta.n_nodes,
        a.meta.sum.len / artifact::SUM_REC_BYTES as u32,
        match a.entry() {
            Some((artifact::ENTRY_MAIN, name)) => format!("fn main `{name}`"),
            Some((_, name)) => format!("chain `{name}`"),
            None => "-".to_string(),
        }
    );
    println!(
        "  = note: .inf ifbc 为空（{} 个 B 轨模块）—— 字节码摄取 = D1 未做；\
               .inf rt 来自 {} 的 infy_rt.c + infy_dyn.c",
        a.meta.n_ifbc_modules,
        plan.runtime_dir.display()
    );
    if rest.contains(&"--verify-reproducible".to_string()) {
        let a = stage9(l, &hc, &out, &plan, target, &triple, &stem, &work.join("verify-a"), bid, unreproducible);
        let b = stage9(l, &hc, &out, &plan, target, &triple, &stem, &work.join("verify-b"), bid, unreproducible);
        let (a, b) = match (a, b) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("error: 阶段 [9] 复核失败：{e}");
                return exit::TOOLCHAIN;
            }
        };
        if a == b {
            println!("verify-reproducible ok — 两个独立构建目录产物字节相同（{} 字节）", a.len());
        } else {
            let off = a.iter().zip(b.iter()).position(|(x, y)| x != y).unwrap_or(0);
            eprintln!(
                "error: E-0603 · 可复现构建不一致：首个差异在偏移 {off}（{}）§7.3",
                artifact::describe_offset(&a, off)
            );
            return exit::CHECKSUM;
        }
    }
    exit::OK
}

/// §7.1 stage [9] inputs: the A-track `cc`, its version string (a §7.3 build-id input), the
/// runtime sources that make up `.inf rt`, and the module's optimization flag (§7.2).
struct LinkPlan {
    cc: String,
    cc_version: String,
    runtime_dir: PathBuf,
    opt: &'static str,
}

fn flag_value(rest: &[String], name: &str) -> Option<String> {
    rest.iter().position(|a| a == name).and_then(|i| rest.get(i + 1)).cloned()
}

fn link_plan(rest: &[String], prof: &pkg::Profile) -> Result<LinkPlan, String> {
    let cc = flag_value(rest, "--cc")
        .or_else(|| std::env::var("CC").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "gcc".to_string());
    let probe = std::process::Command::new(&cc).arg("-dumpversion").output();
    let probe = match probe {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            return Err(format!("`{cc} -dumpversion` 退出码 {:?}{err}", o.status.code()));
        }
        Err(e) => return Err(format!("`{cc}` 无法启动：{e}")),
    };
    let v = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    if v.is_empty() {
        return Err(format!("`{cc} -dumpversion` 没有打印版本字符串（§7.6.2 版本核对需要它）"));
    }
    let cc_version = format!("{cc} {v}");
    let runtime_dir = match flag_value(rest, "--runtime") {
        Some(d) => PathBuf::from(d),
        None => {
            let mut found = None;
            let mut cur = std::env::current_dir().ok();
            while let Some(dir) = cur {
                let cand = dir.join("runtime");
                if cand.join("infy_rt.h").is_file() {
                    found = Some(cand);
                    break;
                }
                cur = dir.parent().map(|p| p.to_path_buf());
            }
            found.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime"))
        }
    };
    for f in ["infy_rt.h", "infy_rt.c", "infy_dyn.c"] {
        if !runtime_dir.join(f).is_file() {
            return Err(format!("{} 里没有 {f}（§7.4 `.inf rt` 的来源）", runtime_dir.display()));
        }
    }
    Ok(LinkPlan { cc, cc_version, runtime_dir, opt: prof.opt_level.flag() })
}

/// §13.4 / ABI §6: the host image keeps its platform's executable suffix.
fn host_file_name(stem: &str, triple: &str) -> String {
    if triple.contains("windows") {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// §5.4/§5.8: one dirhash per package, packages in path order (§7.3 names the dirhash set as a
/// build-id input, so the order itself is part of the identity).
fn package_dirhashes(l: &Loaded) -> Vec<String> {
    let mut by_pkg: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    for id in l.files.keys().copied() {
        let Some(sf) = l.sources.get(id) else { continue };
        let pkg = l.file_pkg.get(&id).cloned().unwrap_or_default();
        by_pkg.entry(pkg).or_default().push((sf.display_path.clone(), sf.text.clone().into_bytes()));
    }
    by_pkg.values_mut().map(|files| pkg::dirhash(files)).collect()
}

/// §7.2: the manifest records profile *fields*, not the selected section name, so the name goes
/// back out of the opt level (D-32).
fn profile_name(p: &pkg::Profile) -> &'static str {
    match p.opt_level {
        pkg::OptLevel::O0 => "debug",
        pkg::OptLevel::Oz => "release-small",
        _ => "release",
    }
}

/// §7.3 item 1: the build-id over the normalised inputs. Paths and timestamps are not in the set.
fn compute_build_id(l: &Loaded, plan: &LinkPlan, triple: &str) -> [u8; 32] {
    let dirh = package_dirhashes(l);
    let lock = std::fs::read_to_string(l.root.join(pkg::LOCK_NAME)).unwrap_or_default();
    let manifest = std::fs::read_to_string(l.root.join(pkg::MANIFEST_NAME)).unwrap_or_default();
    let mut tools = vec![plan.cc_version.clone()];
    for (k, v) in &l.manifest.b_track {
        tools.push(format!("{k} {v}"));
    }
    let compiler = format!("{} {}", crate::VERSION.name, crate::VERSION.version);
    artifact::build_id(&artifact::BuildInputs {
        dirhashes: &dirh,
        lock: &lock,
        manifest: &manifest,
        target: triple,
        opt_level: plan.opt,
        profile: profile_name(&l.manifest.profile),
        toolchains: &tools,
        ifabi: crate::VERSION.ifabi,
        compiler: &compiler,
    })
}

/// §7.1 stage [9]: link the emitted C against the runtime sources with the A-track `cc`, then
/// append the §7.4 blob + trailer to the image in place. Returns the whole container's bytes.
fn stage9(
    l: &Loaded,
    hc: &hir::HirCrate,
    gen_c: &Path,
    plan: &LinkPlan,
    target: hir::Target,
    triple: &str,
    stem: &str,
    work: &Path,
    build_id: [u8; 32],
    unreproducible: bool,
) -> Result<Vec<u8>, String> {
    if let Err(e) = std::fs::create_dir_all(work) {
        return Err(format!("无法创建构建目录 {}: {e}", work.display()));
    }
    let exe = work.join(host_file_name(stem, triple));
    let mut cmd = std::process::Command::new(&plan.cc);
    cmd.arg("-std=c11")
        .arg(plan.opt)
        .arg("-I")
        .arg(&plan.runtime_dir)
        .arg(gen_c)
        .arg(plan.runtime_dir.join("infy_rt.c"))
        .arg(plan.runtime_dir.join("infy_dyn.c"))
        // §7.3 forbids timestamps in the identity; GNU ld writes one into the COFF header unless
        // told not to, and a stamped image can never be byte-reproducible.
        .arg("-Wl,--no-insert-timestamp")
        .arg("-o")
        .arg(&exe);
    let out = cmd
        .output()
        .map_err(|e| format!("`{} …` 无法启动：{e}", plan.cc))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "`{} -std=c11 {} -I {} …` 退出码 {:?}：\n{}",
            plan.cc,
            plan.opt,
            plan.runtime_dir.display(),
            out.status.code(),
            err.trim()
        ));
    }
    let image = std::fs::read(&exe).map_err(|e| format!("无法读回 {}: {e}", exe.display()))?;

    let mut chains: Vec<artifact::ChainRec> = Vec::new();
    let mut nodes: Vec<artifact::NodeRec> = Vec::new();
    let mut chain_id = 0u32;
    for it in &hc.items {
        let hir::HirItem::Chain(c) = it else { continue };
        chains.push(artifact::ChainRec {
            name: c.name.clone(),
            package: c.package.clone(),
            node_count: c.nodes.len() as u32,
            frame_bytes: c.frame.size as u32,
        });
        for n in &c.nodes {
            nodes.push(artifact::NodeRec {
                chain_id,
                node_idx: n.idx,
                name: n
                    .anchor
                    .clone()
                    .or_else(|| n.host_event.clone())
                    .unwrap_or_else(|| format!("{}#{}", c.name, n.idx)),
                file: l.sources.get(n.file).map(|s| s.display_path.clone()).unwrap_or_default(),
                line: n.line,
                col: n.col,
                linkage_count: n.linkage_count,
                host_event_id: n.event_id,
            });
        }
        chain_id += 1;
    }
    // §11 EventTable: the row index *is* the host event id, so rows cannot be filtered out.
    let events: Vec<artifact::EventRec> = hc
        .events
        .iter()
        .map(|i| match hc.items.get(*i) {
            Some(hir::HirItem::Fn(f)) => artifact::EventRec {
                name: f.name.clone(),
                arity: (f.params.len() + usize::from(f.self_ty.is_some())) as u32,
            },
            _ => artifact::EventRec { name: String::new(), arity: 0 },
        })
        .collect();
    let entry = match hc.entry {
        Some(hir::HirEntry::Chain(i)) => hc.items.get(i).map(|it| (artifact::ENTRY_CHAIN, it.name().to_string())),
        Some(hir::HirEntry::Main(i)) => hc.items.get(i).map(|it| (artifact::ENTRY_MAIN, it.name().to_string())),
        None => None,
    };
    let lock = pkg::Lock::parse(&std::fs::read_to_string(l.root.join(pkg::LOCK_NAME)).unwrap_or_default());
    let sums = lock
        .dirh
        .iter()
        .map(|(m, v, d)| artifact::SumRec { package: m.clone(), version: v.clone(), dirhash: d.clone() })
        .collect();
    let p = &l.manifest.profile;
    let content = artifact::Content {
        host_image: &image,
        ifabi: crate::VERSION.ifabi,
        opt_level: artifact::opt_level_index(p.opt_level),
        overflow: if matches!(p.overflow, pkg::Overflow::Saturate) {
            artifact::OV_SATURATE
        } else {
            artifact::OV_PANIC
        },
        panic_radius: match p.panic_radius {
            pkg::PanicRadius::Focus => artifact::RADIUS_FOCUS,
            pkg::PanicRadius::Chain => artifact::RADIUS_CHAIN,
            pkg::PanicRadius::Process => artifact::RADIUS_PROCESS,
        },
        usize_bits: (target.pointer_width * 8) as u8,
        fast_math: p.fast_math,
        lto: artifact::lto_index(p.lto),
        stack_bytes: p.stack_bytes as u32,
        focus_stack_bytes: p.focus_stack_bytes as u32,
        focus_quota: p.focus_quota,
        iter_guard_steps: p.iter_guard_steps,
        target_triple: triple,
        build_id,
        platform: std::env::consts::OS,
        lang_version: crate::VERSION.lang,
        source_toolchain: &plan.cc_version,
        unreproducible,
        ifbc: &[],
        n_ifbc_modules: 0,
        chains,
        events,
        nodes,
        sums,
        resource_keys: Vec::new(),
        entry,
    };
    artifact::write(&content).map_err(|e| e.to_string())
}

/// The §7.2 triple of the machine running the compiler.
fn host_triple() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    };
    match std::env::consts::OS {
        "windows" => format!("{arch}-pc-windows-gnu"),
        "macos" => format!("{arch}-apple-darwin"),
        "linux" => format!("{arch}-unknown-linux-gnu"),
        other => format!("{arch}-unknown-{other}-gnu"),
    }
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

/// §13.4 `infy artifact info|sections <artifact>`: read the §7.4 container back. §9.7's 06xx table
/// has no dedicated code for "this file is not an INFY artifact", so the container refusals borrow
/// `E-0605` (ABI/布局不一致) and exit 7 (§13.4 校验和不一致) — recorded as D-32.
fn artifact_cmd(positional: &[String], rest: &[String]) -> i32 {
    // `positional` already excludes the `artifact` word itself (§13.4 dispatch), so [0] is the
    // subcommand and [1] the file.
    let sub = positional.first().map(|s| s.as_str()).unwrap_or("info");
    if !matches!(sub, "info" | "sections") {
        eprintln!("error: infy artifact 子命令不认识 `{sub}`（可用：info | sections）");
        return exit::GENERIC;
    }
    let path = match positional.get(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("error: infy artifact info|sections <file> [--nodes]（§13.4）");
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
    let a = match artifact::read(&bytes, crate::VERSION.ifabi) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error[E-0605]: {} 不是可读的 INFY 产物：{e}", path.display());
            return exit::CHECKSUM;
        }
    };
    match sub {
        "info" => {
            print!("{}", artifact::dump_info(&a));
            if rest.contains(&"--nodes".to_string()) {
                print!("{}", artifact::dump_nodes(&a));
            }
        }
        "sections" => {
            print!("{}", artifact::dump_sections(&a));
            println!("string pool ({} 项)", a.pool.len());
        }
        _ => unreachable!("the two accepted subcommands are checked above"),
    }
    println!("  = note: 校验通过项：INFYART2 段尾魔数/追加区唯一性、trailer_crc、hdr_ver={}、ifabi={}、区段范围、INFYMETA 记录",
        artifact::HDR_VER, crate::VERSION.ifabi);
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

    /// §13.4 的位置参数索引（F5 那一族的现场）：`infy artifact info <f>` 里 positional = ["info","f"]，
    /// 不是 ["artifact","info","f"]。缺文件 / 未知子命令 / 非产物三条都要各有其退出码。
    #[test]
    fn artifact_cmd_indexes_positionals_and_refuses_non_artifacts() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("scratch")
            .join("cli-artifact");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let junk = dir.join("junk.bin");
        std::fs::write(&junk, vec![0u8; 256]).expect("junk");
        let j = junk.to_string_lossy().to_string();
        assert_eq!(artifact_cmd(&["info".to_string(), j.clone()], &[]), exit::CHECKSUM, "非产物 = E-0605/7");
        assert_eq!(artifact_cmd(&["sections".to_string(), j.clone()], &[]), exit::CHECKSUM);
        assert_eq!(artifact_cmd(&["info".to_string()], &[]), exit::GENERIC, "缺文件参数不该被当成路径");
        assert_eq!(artifact_cmd(&["bogus".to_string(), j.clone()], &[]), exit::GENERIC);
        assert_eq!(artifact_cmd(&[], &[]), exit::GENERIC);
        let missing = dir.join("does-not-exist.bin");
        assert_eq!(
            artifact_cmd(&["info".to_string(), missing.to_string_lossy().to_string()], &[]),
            exit::GENERIC,
            "读不到文件是通用失败，不是校验失败"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
