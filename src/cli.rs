//! §13.4 — the `infy` command line.
//!
//! Exit codes are normative (§13.4): 0 ok · 1 generic failure · 2 diagnostics blocked ·
//! 3 runtime panic · 4 permission/cache · 5 network/dependency · 6 toolchain or link failure ·
//! 7 checksum / reproducibility mismatch.
//!
//! What this build actually does end to end: the front end — stages [1]–[5] of §7.1 — over a whole
//! module, stages [7]–[9] (`hir` lowering + C11 codegen + link + the §7.4 container) behind
//! `infy build`, `infy run` = the same pipeline into a temporary directory plus execution
//! (§13.4), `infy test` = the §5.9 test build plus the generated `#[test]` harness, plus `init`,
//! `fmt`, `expand`, `mod tidy`, `toolchain probe|list` and `artifact info|sections`.
//! `infy ifbc dump|verify` (§7.8.4) inspects IFBC images — a §7.4 artifact's `.inf ifbc` region or a
//! bare module image — as the human-readable disassembly, the machine-comparable canonical text both
//! sides of the cross-check render, or the §7.8.3 load-time verdicts.
//! `infy doc`/`lib pack`/`conformance` still report that the stage behind them is open work rather
//! than pretending to succeed (see `docs/status.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ast::{ItemKind, ParsedFile};
use crate::diag::{Diagnostic, Handler, LintConfig, Phase};
use crate::span::Position;
use crate::span::{FileId, Sources};
use crate::{artifact, atrack, borrow, codegen, derive, hir, ifbc, lexer, macro_expand, parser, pkg, sema};

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

/// `infy run` builds into a fresh temporary directory per invocation (§13.4: 开发态), so two runs
/// inside one process (a test suite, a script) must not share or overwrite one another's image.
static RUN_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
        "ifbc" => ifbc_cmd(&positional, &rest),
        "clean" => clean_cmd(flag("--cache")),
        "run" => run_cmd(&positional, &rest),
        "test" => test_cmd(&positional, &rest),
        "bench" | "doc" | "lib" | "ingest" | "conformance" | "lsp" | "pack" | "unpack" | "install" | "sumdb" | "database" => {
            eprintln!(
                "notice: `infy {cmd}` is not implemented in this build — stages [7]-[9] are wired \
                 (`infy build`, `infy run`), `infy test` runs the §5.9 `#[test]` harness and \
                 `infy ifbc dump|verify` inspects IFBC images (§7.8.4), but this command still \
                 needs its own stage: `ingest` = the §7.7.4 wheel→`.infib` path (the stage-[6a] \
                 ingest itself is wired and reachable from `infy check`), `bench` = §13.4 has no \
                 such subcommand (it is `infy test --bench`), \
                 `doc`/`lib`/`conformance`/`lsp`/… = see docs/status.md. Commands \
                that do work: check, build, run, test, fmt, expand, init, mod tidy, toolchain, target, \
                 artifact info, ifbc dump|verify."
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
        ("run [mod] [--target T] [--cc P]", "stages [1]-[9] into a temporary directory, then executes the artifact (§13.4)"),
        ("test [mod] [--bench] [--target T] [--cc P]", "§5.9 test build (`*_test.infy` + `tests/…` in scope) + run the `#[test]` harness (§13.4)"),
        ("fmt [-w] [--check]", "official formatting, byte-unique (§9.5)"),
        ("expand [--macro name] [--stage all|macro|derive]", "auditable macro/derive output (§6.8)"),
        ("mod tidy|why|verify|upgrade|vendor", "dependency resolution, MVS (§5.7)"),
        ("toolchain probe|list|add|remove", "A-track compilers + B-track bytecode toolchains (§7.6.2)"),
        ("target list|add <triple>", "target triples (§7.2)"),
        ("artifact info|sections <file> [--nodes]", ".inf meta and segment layout (§7.4)"),
        ("ifbc dump|verify <file> [--canonical] [--all-errors]", "IFBC image / `.inf ifbc` inspection: disassembly + §7.8.3 load-time checks (§7.8.4/§12.3)"),
        ("lib pack|verify|dump|diff", ".infib containers (§13.2) — not implemented yet"),
        ("bench / doc / conformance / ingest / sumdb / lsp", "not implemented yet (`--cov` is refused outright: no instrumentation stage)"),
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
    /// §5.9: the build configuration this module was loaded for (the test build sees more files).
    pub scope: Scope,
    /// §7.1 [6a]: the IFBC images stage [6a] ingested for this build, one per B-track embed unit.
    pub embed_images: Vec<crate::btrack::IngestedImage>,
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

/// §5.9 + §13.4: which build a command is asking for. Only `infy test` sets `test`; that one flag
/// decides both file selection (`*_test.infy` and `tests/…` are out of the ordinary build) and
/// what the generated `main` does (run the `#[test]` harness instead of the §7.4 entry).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// §5.9: the test build.
    pub test: bool,
    /// §13.4's `infy test --bench`: run `#[bench]` functions too.
    pub bench: bool,
}

/// §2.2: the parts a build constraint tests against a target triple (`arch-vendor-os[-abi]`).
/// `darwin` is reported as `macos` because `macos` is the name §2.2's closed set uses.
fn triple_parts(triple: &str) -> (String, String, String) {
    let segs: Vec<&str> = triple.split('-').collect();
    let arch = segs.first().copied().unwrap_or("").to_string();
    let os = match segs.get(2).copied().unwrap_or("") {
        "darwin" => "macos".to_string(),
        s => s.to_string(),
    };
    (arch, os, segs.get(3).copied().unwrap_or("").to_string())
}

/// Read, lex and parse every `.infy` file of a module: stages [1] and the package layer of [3].
pub fn load(root: &Path, handler: &mut Handler) -> Option<Loaded> {
    load_scoped(root, handler, &Scope::default(), &host_triple())
}

/// [`load`] for one build configuration: §5.9's test scope plus §2.2's constraint evaluation, so
/// `infy test` sees `*_test.infy` / `tests/…` and `infy check|build|run` do not.
pub fn load_scoped(root: &Path, handler: &mut Handler, scope: &Scope, triple: &str) -> Option<Loaded> {
    let text = std::fs::read_to_string(root.join(pkg::MANIFEST_NAME)).ok()?;
    let manifest = pkg::parse_manifest(&text, pkg::MANIFEST_NAME, handler)?;
    let graph = pkg::discover(root, &manifest, handler);
    let (arch, os, abi) = triple_parts(triple);
    let features = manifest.feature_names();
    let cfg = pkg::BuildConfig {
        os: &os,
        arch: &arch,
        abi: &abi,
        features: &features,
        a_track: manifest.declares_a_track(),
        b_track: !manifest.b_track.is_empty(),
        profile: profile_name(&manifest.profile),
        test: scope.test,
    };
    let mut sources = Sources::default();
    let mut files: BTreeMap<FileId, ParsedFile> = BTreeMap::new();
    let mut file_pkg: BTreeMap<FileId, String> = BTreeMap::new();
    // §2.2: a name outside the closed set plus `[features]` is `E-0012`, checked for **every**
    // discovered file — including the ones this configuration excludes, or a typo in a constraint
    // would silently drop the file instead of diagnosing. (`lexer::lex_file_with_features` can also
    // report it, but only for files that reach the lexer, and twice is worse than once.)
    for package in &graph.packages {
        for f in &package.files {
            if let Some(text) = &f.constraint {
                for name in pkg::unknown_constraint_names(text, &features) {
                    handler.emit(
                        Diagnostic::new("E-0012", Phase::Lex, 0, Position { line: 1, column: 1 })
                            .titled(format!("{}: 未知构建约束名 `{name}`", f.display))
                            .note("§2.2 的封闭集之外，只有 module 清单 `[features]` 里声明的名字可用")
                            .help("改用封闭集里的名字，或在 `MODULE.infy.toml` 的 `[features]` 声明它"),
                    );
                }
            }
        }
    }
    for package in &graph.packages {
        for f in &package.files {
            match pkg::file_selected(f, package, &cfg) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(m) => {
                    // §2.2 names no code for a constraint expression that does not parse; it is
                    // reported with the other constraint errors (docs/spec-deviations.md D-37) and
                    // the file is *not* silently included.
                    handler.emit(
                        Diagnostic::new("E-0012", Phase::Lex, 0, Position { line: 1, column: 1 })
                            .titled(format!("{}: 构建约束表达式无法解析", f.display))
                            .note(format!("{m}（§2.2 的表达式文法：{}`）", "//infy:build <约束>"))
                            .help("ident / `!` / 括号 / `&&`|`,` / `||`；约束名限 §2.2 封闭集加 `[features]`"),
                    );
                    continue;
                }
            }
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
    Some(Loaded { root: root.to_path_buf(), manifest, sources, files, file_pkg, scope: *scope, embed_images: Vec::new(), handler: std::mem::take(handler) })
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

    // §7.6.1 pairing, the other direction: `sema` reports "@c 找不到配对的签名块" (it holds the
    // source block's own span), and this reports "#[link] 落空 / ABI 不受支持 / 前缀未注册". Both
    // halves read `atrack::pair`, so "what counts as a pair" exists in exactly one place.
    let blocks = atrack::signature_blocks(&l.files);
    let sources = atrack::atrack_sources(&l.files);
    let pairing = atrack::pair(&blocks, &sources);
    if !pairing.1.is_empty() {
        emit_atrack_gaps(l, &pairing.1);
    }

    // §7.4 entry rules: #![entry_chain] > fn main > first chain of the main package
    resolve_entry(l, &prog);
    prog
}

/// Render an `atrack::Gap` as a diagnostic. Stage [4] has its own emitter inside `sema`; the CLI
/// needs one because pairing is a whole-module question (the signature block and the `@c` source
/// may live in different files, so no per-item checker owns the verdict).
fn emit_atrack_gaps(l: &mut Loaded, gaps: &[atrack::Gap]) {
    for g in gaps {
        let p = l.sources.line_col(g.span).0;
        let mut d = Diagnostic::new(g.code, Phase::Name, g.span.file, p)
            .titled(g.title.clone())
            .region(
                g.span.file,
                p.line,
                p.column,
                p.column + (g.span.end - g.span.start).max(1),
                g.label.clone(),
            );
        if let Some(n) = &g.note {
            d = d.note(n.clone());
        }
        if let Some(h) = &g.help {
            d = d.help(h.clone());
        }
        l.handler.emit(d);
    }
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
        // §5.9: the test build's entry is the generated `#[test]` harness, not a chain or `fn
        // main`, so "the artifact has no entry" is not a diagnosis here — `infy test` on a module
        // with no `#[test]` function runs zero tests and exits 0 (§13.4: 0 = success).
        if l.scope.test {
            return;
        }
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

/// Stages [1]-[5] (§7.1) plus the §9.2 report: shared by `infy check`, `infy build`, `infy run`
/// and `infy test` so all four agree on what a module is and on the exit code the diagnostics
/// give. `Err(code)` is the exit code the caller must return without proceeding.
fn frontend(positional: &[String], rest: &[String], scope: &Scope) -> Result<(Loaded, sema::Program, String, i32), i32> {
    let root = match pkg_root(positional) {
        Some(r) => r,
        None => {
            eprintln!("error: 找不到 {}——先 infy init <module-path>，或在 module 目录内运行（§5.1）", pkg::MANIFEST_NAME);
            return Err(exit::GENERIC);
        }
    };
    let mut handler = Handler::default();
    if rest.contains(&"--deadlock".to_string()) {
        handler.lint.deadlock_analysis = true;
    }
    let triple = flag_value(rest, "--target").unwrap_or_else(host_triple);
    let mut l = match load_scoped(&root, &mut handler, scope, &triple) {
        Some(l) => l,
        None => {
            let text = handler.render(&handler_empty_sources(&root));
            eprint!("{text}");
            return Err(exit::DIAGNOSED);
        }
    };
    if rest.contains(&"--lenient".to_string()) {
        l.handler.lint.strict = false;
    }
    let prog = analyse(&mut l);
    // §7.1's stage note puts [6a] inside `infy check` as well ("[6a] 在 `infy check` 下需要源语言
    // 工具链在场才能做能力扫描"), but §7.1 also aborts every later stage on a blocking diagnostic,
    // so the ingest only runs when [1]-[5] came back clean.
    if l.handler.exit_code() == exit::OK {
        ingest_b_track(&mut l, &prog);
    }
    let rendered = l.handler.render(&l.sources);
    eprint!("{rendered}");
    if let Some(out) = rest.iter().position(|a| a == "--diag-json").and_then(|i| rest.get(i + 1)) {
        let json = l.handler.render_json(&l.sources, &crate::VERSION);
        if let Err(e) = std::fs::write(out, json) {
            eprintln!("error: 无法写 {out}: {e}");
            return Err(exit::PERMISSION);
        }
    }
    let summary = l.handler.summary();
    let code = l.handler.exit_code();
    Ok((l, prog, summary, code))
}

/// §12: the front-end verdict for one module directory, in process — the diagnostic **codes** and
/// the exit code `infy check` would give, without rendering to a stream.
pub struct Verdict {
    /// Visible diagnostics in `Handler::sorted` order (suppressed-by-`[lints]` ones excluded, so a
    /// case that `[lints]`-allows a code is not credited with reporting it).
    pub codes: Vec<String>,
    pub exit: i32,
    pub rendered: String,
}

/// [`Verdict`] for `root` under build configuration `scope`. `None` only when there is no module to
/// load at all (no `MODULE.infy.toml`); a manifest that *fails to parse* still yields a verdict,
/// because those diagnostics are what such a case is about.
///
/// This deliberately calls the same functions `frontend()` calls rather than a re-implementation: a
/// corpus run against a second pipeline would stay green while `infy check` itself is broken.
pub fn check_verdict(root: &Path, scope: &Scope) -> Option<Verdict> {
    if !root.join(pkg::MANIFEST_NAME).is_file() {
        return None;
    }
    let mut handler = Handler::default();
    let mut l = match load_scoped(root, &mut handler, scope, &host_triple()) {
        Some(l) => l,
        None => {
            let codes = handler.visible().iter().map(|d| d.code.clone()).collect();
            return Some(Verdict {
                codes,
                exit: handler.exit_code(),
                rendered: handler.render(&handler_empty_sources(root)),
            });
        }
    };
    let prog = analyse(&mut l);
    // §7.1 puts [6a] inside `check`, and §7.1 also aborts later stages on a blocking diagnostic —
    // same rule as `frontend()`, so a B-track case's `E-0711`/`E-0712`/`E-0713` arrive exactly the
    // way a user sees them.
    if l.handler.exit_code() == exit::OK {
        ingest_b_track(&mut l, &prog);
    }
    Some(Verdict {
        codes: l.handler.visible().iter().map(|d| d.code.clone()).collect(),
        exit: l.handler.exit_code(),
        rendered: l.handler.render(&l.sources),
    })
}

/// §7.1 stage **[6a]**: every B-track embed unit in this build, as the **INFY side** declares it
/// (§7.7.1 rule 1 — the parameter table is built from the types the item's arguments were inferred
/// to, never from the code object). The refusals that belong to the front end rather than to the
/// ingest are reported here and nowhere else:
///
/// * a B-track prefix this build cannot lower (`@lua`: §7.7.1's ingest table names `luac`, the
///   probe only drives CPython) — `E-0712` naming the prefix, so the unit cannot be forgotten;
/// * a bound argument sema recorded no type for, or a type with no row in §2 of
///   `runtime/IFBC.md` (`u128`/`i128`, `auto`, an unresolved name) — `E-0712` naming the concrete
///   item, never a boxed guess (P3: 红着退出，不留到运行时).
///
/// §3.6 allows an embed block in two positions (the whole body, or a statement expression); both
/// are collected from every nesting of blocks. A-track `@c` is D3 and process-only prefixes are
/// §7.7.5 — neither is a B-track ingest unit, so neither appears here.
fn embed_sites(
    files: &BTreeMap<FileId, ParsedFile>,
    sources: &Sources,
    prog: &sema::Program,
    handler: &mut Handler,
) -> Vec<crate::btrack::EmbedSite> {
    let mut out = Vec::new();
    for (file, pf) in files {
        for item in &pf.items {
            sites_of_item(sources, prog, handler, *file, item, &mut out);
        }
    }
    out
}

fn sites_of_item(
    sources: &Sources,
    prog: &sema::Program,
    handler: &mut Handler,
    file: FileId,
    item: &crate::ast::Item,
    out: &mut Vec<crate::btrack::EmbedSite>,
) {
    use crate::ast::ItemKind;
    const META: &[&str] = &["name", "timeout_ms", "isolation"];
    let (sig, body) = match &item.kind {
        ItemKind::Fn { sig, body } | ItemKind::Event { sig, body } => (sig, body),
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } => {
            for inner in items {
                sites_of_item(sources, prog, handler, file, inner, out);
            }
            return;
        }
        ItemKind::ExternBlock { decls, .. } => {
            for inner in decls {
                sites_of_item(sources, prog, handler, file, inner, out);
            }
            return;
        }
        _ => return,
    };
    let mut blocks = Vec::new();
    embeds_in_body(body, &mut blocks);
    if blocks.is_empty() {
        return;
    }
    // The declared return type comes from stage [4]'s record; an item sema never collected has no
    // declared type to read, and `()` is the §3.3 reading of "no `->` written".
    let ret = prog
        .fns
        .iter()
        .find(|f| f.file == file && f.name == sig.name)
        .or_else(|| prog.fns.iter().find(|f| f.name == sig.name && f.self_ty.is_none()))
        .map(|f| f.ret.clone())
        .unwrap_or(sema::Ty::Unit);
    // §7.7.1 rule 2: the unit's `return` is the item's return value, so an unmappable return type is
    // a gap in the parameter table too — falling back to `()` here would silently change the
    // calling convention the loader validates (item 5 compares the digest of exactly this table).
    let ret_kind = match crate::btrack::embed_ty_kind(&ret) {
        Some(k) => k,
        None => {
            for e in &blocks {
                if !e.is_b_track() || e.lang != "py" {
                    continue;
                }
                handler.emit(embed_gap(
                    file,
                    sources.line_col(e.span).0,
                    format!("`{}` 的返回类型无法表达", sig.name),
                    format!(
                        "§7.7.1 规则 2：声明的返回类型 `{ty}` 在 `runtime/IFBC.md` §2 的封闭签名集里没有对应行",
                        ty = ret.text()
                    ),
                ));
            }
            return;
        }
    };
    for e in blocks {
        if !e.is_b_track() {
            continue;
        }
        let pos = sources.line_col(e.span).0;
        if e.lang != "py" {
            handler.emit(embed_gap(
                file,
                pos,
                format!("@{} 的字节码摄取未实现", e.lang),
                format!(
                    "§7.7.1 的摄取路径里 {lang} 一列写的是 `{probe}`，本实现只驱动 CPython；该单元不会静默丢弃",
                    lang = e.lang,
                    probe = if e.lang == "lua" { "luac -o" } else { "该语言的字节码编译器" }
                ),
            ));
            continue;
        }
        let unit_name = e.unit_name().unwrap_or_else(|| sig.name.clone());
        let mut params = Vec::new();
        let mut refused = false;
        for a in &e.args {
            if META.contains(&a.name.as_str()) {
                continue;
            }
            let Some(t) = prog.ty(a.value.span) else {
                handler.emit(embed_gap(
                    file,
                    pos,
                    format!("`{unit_name}` 的形参表无法构造"),
                    format!("§7.7.1 规则 1 要求形参表来自 INFY 侧类型，而实参 `{arg}` 在阶段 [4] 没有类型（未声明即已被拒绝）", arg = a.name),
                ));
                refused = true;
                break;
            };
            let Some(k) = crate::btrack::embed_ty_kind(t) else {
                handler.emit(embed_gap(
                    file,
                    pos,
                    format!("`{unit_name}` 的形参表无法构造"),
                    format!(
                        "§7.7.1 规则 1：实参 `{arg}` 的类型 `{ty}` 在 `runtime/IFBC.md` §2 的封闭签名集里没有对应行（无 128 位宽度、`auto` 与未定型者皆然）",
                        arg = a.name,
                        ty = t.text()
                    ),
                ));
                refused = true;
                break;
            };
            params.push((a.name.clone(), k));
        }
        if refused {
            continue;
        }
        out.push(crate::btrack::EmbedSite {
            lang: e.lang.clone(),
            unit_name,
            body: e.body.clone(),
            params,
            ret: ret_kind,
            file,
            pos,
        });
    }
}

/// `E-0712` (§7.7.2's "字节码能力缺失", the family the §9.4 table gives an ingest that cannot
/// proceed) with the concrete item named in the title, per P3.
fn embed_gap(file: FileId, pos: Position, title: String, note: String) -> Diagnostic {
    Diagnostic::new("E-0712", Phase::Embed, file, pos)
        .with_origin(crate::diag::Origin::EmbedPy)
        .titled(title)
        .note(note)
        .help("等能力表 / §2 签名集扩容，或改写该单元（§7.7.1、§7.7.2）")
}

fn embeds_in_body<'a>(body: &'a crate::ast::Body, out: &mut Vec<&'a crate::ast::EmbedBlock>) {
    match body {
        crate::ast::Body::Embed(e) => out.push(e),
        crate::ast::Body::Block(b) => embeds_in_block(b, out),
        crate::ast::Body::Foreign => {}
    }
}

fn embeds_in_block<'a>(b: &'a crate::ast::Block, out: &mut Vec<&'a crate::ast::EmbedBlock>) {
    use crate::ast::ExprKind;
    for st in &b.stmts {
        embeds_in_stmt(st, out);
    }
    if let Some(t) = &b.tail {
        if let ExprKind::Embed(e) = &t.kind {
            out.push(e);
        }
    }
}

fn embeds_in_stmt<'a>(st: &'a crate::ast::Stmt, out: &mut Vec<&'a crate::ast::EmbedBlock>) {
    use crate::ast::{ArmBody, ElseBranch, ExprKind, StmtKind};
    match &st.kind {
        StmtKind::Expr { expr, .. } => {
            if let ExprKind::Embed(e) = &expr.kind {
                out.push(e);
            }
        }
        StmtKind::If { then, else_, .. } => {
            embeds_in_block(then, out);
            if let Some(branch) = else_ {
                match &**branch {
                    ElseBranch::Block(b) => embeds_in_block(b, out),
                    ElseBranch::If(inner) => embeds_in_stmt(inner, out),
                }
            }
        }
        StmtKind::While { body, .. } | StmtKind::Loop { body, .. } | StmtKind::For { body, .. } => {
            embeds_in_block(body, out)
        }
        StmtKind::Unsafe(b) | StmtKind::Defer(b) => embeds_in_block(b, out),
        StmtKind::Match { arms, .. } => {
            for a in arms {
                match &a.body {
                    ArmBody::Block(b) => embeds_in_block(b, out),
                    ArmBody::Expr(e) => {
                        if let ExprKind::Embed(emb) = &e.kind {
                            out.push(emb);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// §7.1 [6a] over the whole module. A module with no B-track unit never touches the source-language
/// toolchain — §7.1's note that `infy check` needs it is about modules that **use** `@py`.
///
/// Where the images go: §7.4's `.inf ifbc` region is the artifact's home for them, but reaching
/// stage [9] requires stage [8] to emit the *call* into the IFBC VM, and a `@py` body is exactly
/// the host event §3.3/§7.6.5 describes — which `codegen` refuses by name (F29). So `.inf ifbc`
/// stays empty for every module this build can compile, and the committed consumer of a real
/// stage-[6a] image is `runtime/test_ifbc.c`, which loads `ifbc_py_unit.bin` with the production
/// VM. No fake success: nothing claims the artifact carries B-track bytecode.
fn ingest_b_track(l: &mut Loaded, prog: &sema::Program) {
    let Loaded { files, sources, handler, manifest, embed_images, .. } = l;
    let sites = embed_sites(files, sources, prog, handler);
    if sites.is_empty() {
        return;
    }
    let python = match crate::btrack::find_python() {
        Some(p) => p,
        None => {
            // §7.1's stage note and §7.7.3: the missing build-time interpreter is `E-0713`.
            for s in &sites {
                handler.emit(
                    crate::btrack::IngestErr::ToolchainMissing {
                        lang: s.lang.clone(),
                        declared: manifest.b_track.get(&s.lang).cloned().unwrap_or_default(),
                    }
                    .diagnostic(s.file, s.pos),
                );
            }
            return;
        }
    };
    let mut images = Vec::new();
    for s in &sites {
        if let Ok(img) = crate::btrack::ingest_embed_site(s, manifest, &python, handler) {
            images.push(img);
        }
    }
    *embed_images = images;
}

fn check(positional: &[String], rest: &[String], building: bool) -> i32 {
    let (l, prog, summary, code) = match frontend(positional, rest, &Scope::default()) {
        Ok(x) => x,
        Err(c) => return c,
    };
    if code == exit::OK {
        println!("check ok — {} file(s), {}", l.files.len(), summary);
        for img in &l.embed_images {
            println!(
                "ingest ok — 阶段 [6a] `{}`：IFBC 镜像 {} 字节 / {} 函数（{}，§7.7.1）",
                img.unit,
                img.image.len(),
                img.n_funcs,
                img.source_toolchain
            );
        }
    } else {
        println!("check FAILED — {summary}");
    }
    if building && code == exit::OK {
        return build_c(&l, &prog, rest, "infy build");
    }
    if building {
        eprintln!("error: `infy build` 中止于阶段 [1]-[5]：诊断里有阻断项（§7.1：任一阶段出现 `E-` 即中止后续阶段）");
        return code;
    }
    code
}

/// §13.4 `infy run`: what the executed program produced, kept byte-exact so a test can assert on
/// it (`code` is the §13.4 exit code `infy run` itself returns).
#[derive(Debug)]
pub struct RunOutcome {
    pub code: i32,
    pub exe: PathBuf,
    pub program_stdout: Vec<u8>,
    pub program_stderr: Vec<u8>,
}

/// §13.4: `infy run` = build to a temporary directory, then execute. The stages are the same
/// [1]-[5] + [7]-[9] that `infy build` runs; only the output directory and the execution differ.
/// `infy test` shares this path with a test [`Scope`] — §5.9/§13.4 define it as "build + execute"
/// as well, only with a different entry (the `#[test]` harness).
fn run_module(positional: &[String], rest: &[String], scope: &Scope) -> RunOutcome {
    let label = if scope.test { "infy test" } else { "infy run" };
    for f in ["--arg", "--feature"] {
        if rest.iter().any(|a| a == f) {
            eprintln!(
                "error: `{label} {f}` 未实现：§13.4 列了这两个开关，但 {f} 在当前实现里没有任何消费者 \
                 （`env` 的字段集合 §4.6 是封闭的、不含参数；`[features]` 也没有从命令行接进 §2.2 的约束求值）。\
                 拒绝而不是静默忽略 —— 见 docs/BACKLOG.md F19。"
            );
            return RunOutcome { code: exit::GENERIC, exe: PathBuf::new(), program_stdout: Vec::new(), program_stderr: Vec::new() };
        }
    }
    let (l, prog, summary, diag_code) = match frontend(positional, rest, scope) {
        Ok(x) => x,
        Err(c) => return RunOutcome { code: c, exe: PathBuf::new(), program_stdout: Vec::new(), program_stderr: Vec::new() },
    };
    if diag_code != exit::OK {
        println!("check FAILED — {summary}");
        eprintln!("error: `{label}` 中止于阶段 [1]-[5]：诊断里有阻断项（§7.1）");
        return RunOutcome { code: diag_code, exe: PathBuf::new(), program_stdout: Vec::new(), program_stderr: Vec::new() };
    }
    println!("check ok — {} file(s), {}", l.files.len(), summary);

    let triple = flag_value(rest, "--target").unwrap_or_else(host_triple);
    let stem = l.manifest.module_path.rsplit('/').next().unwrap_or("crate").to_string();
    let seq = RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let work = std::env::temp_dir().join(format!("infy-run-{}-{seq}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&work) {
        eprintln!("error: 无法创建临时构建目录 {}: {e}（§13.4 退出码 4：缓存/写入错误）", work.display());
        return RunOutcome { code: exit::PERMISSION, exe: PathBuf::new(), program_stdout: Vec::new(), program_stderr: Vec::new() };
    }
    let mut build_args = rest.to_vec();
    build_args.push("--out".to_string());
    build_args.push(work.join(format!("{stem}.c")).to_string_lossy().into_owned());
    // `--verify-reproducible` is a build-time audit, not something `run` promises; two extra
    // link passes would only slow the dev loop down.
    build_args.retain(|a| a != "--verify-reproducible");
    let code = build_c(&l, &prog, &build_args, label);
    let exe = work.join(host_file_name(&stem, &triple));
    if code != exit::OK {
        return RunOutcome { code, exe, program_stdout: Vec::new(), program_stderr: Vec::new() };
    }
    let out = match std::process::Command::new(&exe).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: 产物 {} 无法启动：{e}（§13.4 退出码 6：工具链/链接一类）", exe.display());
            let _ = std::fs::remove_dir_all(&work);
            return RunOutcome { code: exit::TOOLCHAIN, exe, program_stdout: Vec::new(), program_stderr: Vec::new() };
        }
    };
    let _ = std::fs::remove_dir_all(&work);
    // §13.4: 3 = runtime panic, and `infy_runtime_start` returns exactly that for a failed focus.
    // A child that died from a signal has no exit code; it is a runtime failure, so 3 as well
    // (the spec has no separate code — recorded in docs/spec-deviations.md D-35).
    let code = match out.status.code() {
        Some(c) => c,
        None => {
            eprintln!("error: 程序被信号/异常终止（无退出码），按 §13.4 记为 3");
            exit::RUNTIME
        }
    };
    RunOutcome { code, exe, program_stdout: out.stdout, program_stderr: out.stderr }
}

/// §13.4: `infy run` and `infy test` both forward the program's two streams byte-for-byte and then
/// print their own status line. §13.4 does not say which stream the tool's line belongs to, so it
/// goes to stderr and the program's bytes stay undiluted (docs/spec-deviations.md D-35(a)).
fn forward_program(out: &RunOutcome, label: &str, fail_note: &str) -> i32 {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut so = stdout.lock();
    let _ = so.write_all(&out.program_stdout);
    let _ = so.flush();
    let stderr = std::io::stderr();
    let mut se = stderr.lock();
    let _ = se.write_all(&out.program_stderr);
    let _ = se.flush();
    drop(se);
    if out.code == exit::OK {
        eprintln!(
            "{label} ok — 程序退出码 0（stdout {} 字节 / stderr {} 字节，§13.4）",
            out.program_stdout.len(),
            out.program_stderr.len()
        );
    } else if out.program_stderr.is_empty() && out.exe.as_os_str().is_empty() {
        eprintln!("{label} FAILED — 退出码 {}", out.code);
    } else {
        eprintln!("{label} FAILED — 程序退出码 {}（§13.4{fail_note}）", out.code);
    }
    out.code
}

fn run_cmd(positional: &[String], rest: &[String]) -> i32 {
    let out = run_module(positional, rest, &Scope::default());
    let note = match out.code {
        exit::RUNTIME => "：3 = 运行时 panic",
        _ => "",
    };
    forward_program(&out, "run", note)
}

/// §13.4 + §5.9: `infy test [pkg] [--bench] [--cov]` builds the **test** scope — which is the only
/// build that sees `*_test.infy` and `tests/…` (§5.9 items 1-2) — and runs the generated `#[test]`
/// harness; the exit code is §13.4's. `--cov` is refused rather than ignored: §12.4 takes coverage
/// from `-O0` node-table instrumentation that no stage of this build emits (BACKLOG F22).
fn test_cmd(positional: &[String], rest: &[String]) -> i32 {
    if rest.iter().any(|a| a == "--cov") {
        eprintln!(
            "error: `infy test --cov` 未实现：§12.4 要求覆盖率来自 `-O0` 构建的节点表插桩，而阶段 [7]-[9] \
             都不产出插桩，`--cov` 在本实现里没有任何消费者。拒绝而不是静默忽略出一个假的覆盖率 —— \
             见 docs/BACKLOG.md F22。"
        );
        return exit::GENERIC;
    }
    let scope = Scope { test: true, bench: rest.iter().any(|a| a == "--bench") };
    let out = run_module(positional, rest, &scope);
    let note = match out.code {
        exit::RUNTIME => "：3 = 运行时 panic（某个 `#[test]` 里 panic，harness 之外没有 catch 点 ⇒ 整条套件中止）",
        exit::GENERIC => "：1 = 一般性失败 = 有测试返回 Err",
        _ => "",
    };
    forward_program(&out, "test", note)
}

/// Stages [7], [8] and [9] of §7.1: lower to HIR, emit C11, write it under `.infy-build/`, then
/// link it against the runtime with the A-track `cc` and append the §7.4 segment container.
///
/// `--cc`/`$CC` select the compiler (default `gcc`); a missing one is `E-0602` (§7.6.2), not a
/// silent skip. `--verify-reproducible` re-runs stage [9] in two independent build directories and
/// compares bytes (§7.3 item 3, mismatch → `E-0603`/exit 7).
fn build_c(l: &Loaded, prog: &sema::Program, rest: &[String], cmd: &str) -> i32 {
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
        test_harness: l.scope.test,
        bench: l.scope.bench,
    };
    let text = match codegen::emit(prog, &hc, &files, &l.sources, &cfg) {
        Ok(t) => t,
        Err(es) => {
            for e in es.iter() {
                eprintln!("{}", e.render());
            }
            eprintln!(
                "error: `{cmd}` 中止于阶段 [8]：codegen 覆盖了 §14 导览子集，上述构造在范围之外（拒绝而不是生成近似代码，见 docs/BACKLOG.md C2）"
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
        "  = note: .inf ifbc 为空（{} 个 B 轨模块）—— 阶段 [6a] 已能摄取 `@py` 单元，缺的是阶段 [8] 通往 IFBC VM 的调用通道\
               （docs/BACKLOG.md F29）；.inf rt 来自 {} 的 infy_rt.c + infy_dyn.c",
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
/// §5.8 item 1: the dirhash covers the module's **全部文件**. It therefore cannot be derived from the
/// selected file set — §5.9 keeps `*_test.infy` out of a build, but those files are still part of
/// what a published module is, and a hash that depends on which command ran would break §7.3 too.
/// So this re-discovers the tree instead of reading `Loaded::files`.
fn package_dirhashes(l: &Loaded) -> Vec<String> {
    let mut throwaway = Handler::default();
    let graph = pkg::discover(&l.root, &l.manifest, &mut throwaway);
    let mut by_pkg: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    for package in &graph.packages {
        let pkg = package.import_path(&l.manifest.module_path);
        for f in &package.files {
            let body = std::fs::read(&f.abs).unwrap_or_default();
            by_pkg.entry(pkg.clone()).or_default().push((f.display.clone(), body));
        }
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
/// §7.6.3 项 1/2：把绑定头与每个 A 轨单元的翻译单元写进 `<build>/a_track/`，返回要交给编译器的
/// `.c` 列表（按单元名字典序 —— §7.4 的段顺序要求“依赖拓扑逆序 + 名称字典序”，而链接命令本身也必须
/// 定序，否则 §7.3 的可复现承诺会在第二次构建里断掉）。
///
/// 写文件放在 `stage9` 里而不是 `build_c`，因为 `--verify-reproducible` 用**两个**构建目录各跑一次
/// 阶段 [9]：只有一份的话第二个目录里就没有这些输入。
fn a_track_units(l: &Loaded, hc: &hir::HirCrate, work: &Path) -> Result<Vec<PathBuf>, String> {
    let blocks = atrack::signature_blocks(&l.files);
    if blocks.is_empty() {
        return Ok(Vec::new());
    }
    let sources = atrack::atrack_sources(&l.files);
    let (units, gaps) = atrack::pair(&blocks, &sources);
    if !gaps.is_empty() {
        // Stage [4] already rendered these; reaching here means an A-track declaration survived into
        // the link stage, which would produce an unresolved reference instead of a diagnosis.
        return Err(format!(
            "A 轨配对失败：{}",
            gaps.iter().map(|g| g.summary()).collect::<Vec<_>>().join(" / ")
        ));
    }
    let dir = work.join("a_track");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(format!("无法创建 A 轨目录 {}: {e}", dir.display()));
    }
    let plan = atrack::wrapper_plan(&hc.items, &blocks);
    let mut out: Vec<PathBuf> = Vec::new();
    let mut all: Vec<atrack::Wrapper> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for u in &units {
        let (ws, refusals) = plan.get(&u.name).cloned().unwrap_or_default();
        if !refusals.is_empty() {
            return Err(format!(
                "A 轨单元 `{}` 被 §7.6.4 的封闭表拒绝：{}",
                u.name,
                refusals.iter().map(|(n, t)| format!("{n}：{t}")).collect::<Vec<_>>().join(" / ")
            ));
        }
        let path = dir.join(format!("{}.c", atrack::unit_file_stem(&u.name)));
        let text = atrack::unit_c_source(&u.name, &u.source, &ws);
        if let Err(e) = std::fs::write(&path, &text) {
            return Err(format!("无法写 {}: {e}", path.display()));
        }
        out.push(path);
        names.push(u.name.clone());
        all.extend(ws);
    }
    if !out.is_empty() {
        let header = dir.join("infy_binding.h");
        if let Err(e) = std::fs::write(&header, atrack::binding_header(&names, &all)) {
            return Err(format!("无法写 {}: {e}", header.display()));
        }
        println!(
            "a_track ok — 阶段 [8] 绑定：{} 个单元（{}）、{} 个包装符号、{} 字节写入 {}（§7.6.3）",
            units.len(),
            names.join(", "),
            all.len(),
            std::fs::read(&header).map(|v| v.len()).unwrap_or(0),
            dir.display()
        );
    }
    Ok(out)
}

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
    // §7.6.3 项 4：A 轨的翻译单元与生成码一起进链接命令（规范不许用户手写胶水，也不许把它留给
    // `dlopen`）。没有 A 轨单元时链接输入一字不变 —— tour 的 199975 字节 / `build-id 4d8250b5…55ba`
    // 因此不受本条影响。
    let a_track_c = a_track_units(l, hc, work)?;
    let mut cmd = std::process::Command::new(&plan.cc);
    cmd.arg("-std=c11")
        .arg(plan.opt)
        .arg("-I")
        .arg(&plan.runtime_dir)
        .arg(gen_c)
        .arg(plan.runtime_dir.join("infy_rt.c"))
        .arg(plan.runtime_dir.join("infy_dyn.c"));
    if !a_track_c.is_empty() {
        // `@c` 源码里的 `#include "infy_binding.h"` 按同目录规则就能命中，但 `ci.c` 在上一层，
        // 所以显式给一条 `-I`，让两种包含写法都成立。
        cmd.arg("-I").arg(work.join("a_track"));
        for p in &a_track_c {
            cmd.arg(p);
        }
    }
    // §7.3 forbids timestamps in the identity; GNU ld writes one into the COFF header unless
    // told not to, and a stamped image can never be byte-reproducible.
    cmd.arg("-Wl,--no-insert-timestamp")
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

// ---------------------------------------------------------------- §7.8.4 IFBC inspection

/// What `infy ifbc dump|verify` was handed: a §7.4 container whose `.inf ifbc` region carries the
/// B-track module images, or a file that *is* one IFBC module image (a committed `runtime/fixtures`
/// `.bin`, an image written out of stage [6a]).
#[derive(Clone, Debug)]
struct IfbcInput {
    /// The images, in `.inf ifbc` order. Empty when a container's region is empty — which is every
    /// artifact this build produces, because stage [8] has no call channel into the VM (F29).
    images: Vec<Vec<u8>>,
    /// `Some((n_ifbc_modules, region_len))` when the bytes parsed as a §7.4 container.
    container: Option<(u32, u32)>,
}

/// Why the command cannot name a single image to inspect.
#[derive(Debug)]
enum IfbcOpenErr {
    /// Neither reading worked. Both reasons print, so a wrong `--out` path explains itself.
    Neither(String, String),
    /// `.inf ifbc` is non-empty but holds something other than exactly one module: §7.4 defines no
    /// framing across several modules and neither reader implements one, so the command says so
    /// instead of inventing a delimiter (F29; deviation D-45).
    Framing { n: u32, len: u32 },
}

/// Read a file as one of the two accepted shapes. The container is tried first: `artifact::read`
/// only inspects the trailer at end-of-file (D-33), so a bare image is never mistaken for one.
fn classify_ifbc(bytes: &[u8]) -> Result<IfbcInput, IfbcOpenErr> {
    let container = artifact::read(bytes, crate::VERSION.ifabi);
    if let Ok(a) = &container {
        let (n, len) = (a.meta.n_ifbc_modules, a.meta.ifbc.len);
        if len == 0 {
            return Ok(IfbcInput { images: Vec::new(), container: Some((n, len)) });
        }
        if n != 1 {
            return Err(IfbcOpenErr::Framing { n, len });
        }
        return Ok(IfbcInput {
            images: vec![a.region_bytes(a.meta.ifbc).to_vec()],
            container: Some((n, len)),
        });
    }
    match ifbc::decode(bytes) {
        Ok(_) => Ok(IfbcInput { images: vec![bytes.to_vec()], container: None }),
        Err(e) => Err(IfbcOpenErr::Neither(
            container.err().map(|x| x.to_string()).unwrap_or_default(),
            e.to_string(),
        )),
    }
}

/// What `infy ifbc dump|verify` prints for a container whose `.inf ifbc` region is empty. It is a
/// function rather than inline `println!` because the wording is the point: exit 0 here means
/// "nothing to inspect", and the note says so and names F29, so an empty region can never be read
/// as "the artifact's bytecode passed §7.8.3".
fn ifbc_empty_region_text(sub: &str, n: u32) -> String {
    format!(
        "ifbc {sub} — .inf ifbc 为空（{n} 个 B 轨模块 / 区域 0 字节）：没有可{}/§7.8.3 的字节码\n  \
         = note: 阶段 [6a] 已能从 CLI 摄取 `@py` 单元（`infy check` 打 `ingest ok —` 行），缺的是阶段 \
         [8] 通往 IFBC VM 的调用通道，所以含 B 轨单元的 module 在 [8] 就 exit 6、走不到 [9]\
         （BACKLOG F29）。[6a] 今天的镜像由 `runtime/fixtures/ifbc_py_unit.bin` 双侧锁点覆盖，不是这个\
         产物；本命令因此**不声称**校验过任何产物里的 B 轨字节码。\n",
        if sub == "dump" { "反汇编" } else { "校验" }
    )
}

/// The text `infy ifbc dump` writes for one image: the human-readable disassembly, or with
/// `--canonical` the machine-comparable text whose line grammar `runtime/IFBC.md` §6.1 defines and
/// `runtime/infy_ifbc.c` renders from the same bytes. Canonical mode returns that text with **no**
/// header and no decoration, because the committed `.can.txt` fixtures are the C side's rendering
/// and the comparison is a byte diff, not a claim.
fn ifbc_dump_text(image: &[u8], canonical: bool) -> Result<String, String> {
    let m = ifbc::decode(image).map_err(|e| e.to_string())?;
    if canonical {
        m.dump_canonical()
            .map_err(|e| format!("镜像可解码、指令流却不线性（{e}）—— 两侧没有共用文本可差"))
    } else {
        Ok(m.disassemble())
    }
}

/// §7.8.3's six load-time checks for one image, against this side's std host-signature table.
/// `all_errors == false` stops at the first failing function, exactly like the runtime (D.10).
fn ifbc_verify_image(image: &[u8], all_errors: bool) -> Result<Vec<ifbc::VerifyViolation>, String> {
    let m = ifbc::decode(image).map_err(|e| e.to_string())?;
    Ok(ifbc::verify(&m, &ifbc::HostSigTable::std_default(), all_errors))
}

/// The same verdicts as the machine-comparable text: sorted `V <code> f=<func> off=<off>` lines,
/// byte for byte what `infy_ifbc_verify_canonical` renders on the C side (`docs/internals/
/// ifbc-encoding-rust.md` §6). There is no shared text for an image either side cannot load, so
/// that is reported as an error rather than rendered as a partial match.
fn ifbc_verify_canonical_text(images: &[Vec<u8>]) -> Result<String, String> {
    let mut out = String::new();
    for img in images {
        let m = ifbc::decode(img).map_err(|e| e.to_string())?;
        out.push_str(&ifbc::verify_canonical(&m, &ifbc::HostSigTable::std_default()));
    }
    Ok(out)
}

/// §13.4/§7.8.4: `infy ifbc dump|verify <artifact|image> [--canonical] [--all-errors]`.
///
/// §13.4 names the two subcommands and §12.3 makes `infy ifbc dump` a golden-file target, but says
/// nothing about the accepted file shapes, the `--canonical` text or which exit code a §7.8.3
/// rejection carries — those choices are recorded in `docs/spec-deviations.md` D-45.
fn ifbc_cmd(positional: &[String], rest: &[String]) -> i32 {
    let sub = positional.first().map(|s| s.as_str()).unwrap_or("");
    if !matches!(sub, "dump" | "verify") {
        eprintln!("error: infy ifbc 子命令不认识 `{sub}`（可用：dump | verify，§13.4/§7.8.4）");
        return exit::GENERIC;
    }
    let path = match positional.get(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("error: infy ifbc dump|verify <artifact|镜像文件>（§13.4）");
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
    let canonical = rest.iter().any(|a| a == "--canonical");
    let all_errors = rest.iter().any(|a| a == "--all-errors");
    let input = match classify_ifbc(&bytes) {
        Ok(i) => i,
        Err(IfbcOpenErr::Neither(art, image)) => {
            eprintln!(
                "error[E-0605]: {} 既不是可读的 §7.4 产物，也不是可解码的 IFBC 镜像。\n  \
                 = 按产物读：{art}\n  = 按镜像读：{image}",
                path.display()
            );
            return exit::CHECKSUM;
        }
        Err(IfbcOpenErr::Framing { n, len }) => {
            eprintln!(
                "error: {} 的 `.inf ifbc` 区有 {len} 字节而 meta.n_ifbc_modules = {n} —— §7.4 没有定义\
                 一个区里多个模块如何分帧，两侧读取器也都没实现，本命令不猜分隔符（BACKLOG F29 / 偏差 D-45）。",
                path.display()
            );
            return exit::GENERIC;
        }
    };
    if input.images.is_empty() {
        // Not a fake pass: the region is empty, so there is no bytecode to disassemble or check.
        let (n, _) = input.container.unwrap_or((0, 0));
        print!("{}", ifbc_empty_region_text(sub, n));
        return exit::OK;
    }
    if sub == "dump" {
        for (i, img) in input.images.iter().enumerate() {
            match ifbc_dump_text(img, canonical) {
                Ok(text) => {
                    if !canonical {
                        println!(
                            "; IFBC 镜像 #{i} —— {} 字节（{}）",
                            img.len(),
                            if input.container.is_some() { "来自 .inf ifbc 区" } else { "裸镜像" }
                        );
                    }
                    use std::io::Write;
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
                Err(e) => {
                    println!("R-0704: 镜像 #{i} 无法装载：{e}");
                    println!("ifbc dump FAILED — 镜像 #{i} 不是可反汇编的 IFBC 镜像");
                    return exit::DIAGNOSED;
                }
            }
        }
        return exit::OK;
    }
    let mut blocking = 0usize;
    let mut warnings = 0usize;
    if canonical {
        // stdout stays pure in canonical mode: the text is the diff target against the C side.
        match ifbc_verify_canonical_text(&input.images) {
            Ok(out) => {
                print!("{out}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                blocking = out.lines().filter(|l| l.starts_with("V R")).count();
                warnings = out.lines().filter(|l| l.starts_with("V W")).count();
            }
            Err(e) => {
                eprintln!(
                    "error: 镜像无法解码（{e}）—— --canonical 只在两侧都能装载的镜像上有共用文本，\
                     这里不渲染半份。"
                );
                return exit::DIAGNOSED;
            }
        }
    } else {
        for (i, img) in input.images.iter().enumerate() {
            let vs = match ifbc_verify_image(img, all_errors) {
                Ok(v) => v,
                Err(e) => {
                    // A module that will not decode never reaches the six items; item 1's static
                    // counterpart is what the loader refused with, so this is a rejection (R-0704),
                    // not a tool error.
                    println!("R-0704: 镜像 #{i} 无法装载：{e}");
                    blocking += 1;
                    continue;
                }
            };
            println!("; §7.8.3 校验 —— 镜像 #{i}（{} 字节，按 func_table 顺序逐函数）", img.len());
            for v in &vs {
                if v.code.starts_with('W') {
                    warnings += 1;
                    if all_errors {
                        println!("  {}", v.render());
                    }
                } else {
                    blocking += 1;
                    println!("{}", v.render());
                }
            }
        }
    }
    // Canonical mode has already written its (pure) text, so the status lines go to stderr there.
    let say = |m: String| {
        if canonical {
            eprintln!("{m}")
        } else {
            println!("{m}")
        }
    };
    if blocking > 0 {
        say(format!(
            "ifbc verify FAILED — {blocking} 条阻断违规（R-07xx）/ {warnings} 条 W-0705\
             （§7.8.3 第 6 项，不阻断）"
        ));
        if !all_errors && !canonical {
            say(String::from(
                "  = note: 按附录 D.10 默认只报首个失败函数，保持与运行时一致；`--all-errors` 一次列全。",
            ));
        }
        return exit::DIAGNOSED;
    }
    say(format!(
        "ifbc verify ok — {} 个镜像通过 §7.8.3 的 1→5 与结构 mandate（{warnings} 条 W-0705 不阻断）",
        input.images.len()
    ));
    say(String::from(
        "  = note: 这是**本侧**装载期校验的结论。它与 C 侧虚拟机不等价（`call.virt`/`call.embed`/\
         `invoke.dynamic` 本侧判 R-0705、`coro.*`/`throw.inf` 的 δ 不同、`W-0705` 粒度不同 —— \
         不可比清单 = 偏差 D-42 (a)–(e)，BACKLOG F26）。两侧一致的只有逐 (code, func, off) 的\
         规范化文本与反汇编文本。",
    ));
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

    /// The C4 acceptance strings, read off the real processes in docs/RUNLOG.md cron-10 rather
    /// than written first. `infy run` forwards the program's streams byte-for-byte; the panic
    /// report line is §9.2's own bytes as `infy_rt.c` renders them.
    const TOUR_EXPECTED_STDOUT: &str = "";
    const TOUR_EXPECTED_STDERR: &str = "";
    const PANIC_EXPECTED_STDOUT: &str = "";
    const PANIC_EXPECTED_LINE: &str = "runtime[R-0001]: 整数溢出（`overflow=panic`）";

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
        // no fake success: §13.4 exit 1 = generic failure, and the message names the missing stage.
        // `infy test` left this list in the C5 round (§5.9 harness); `infy bench` stayed, because
        // §13.4 has no `bench` subcommand at all — it is `infy test --bench`.
        assert_eq!(run(&argv(&["build"])), exit::GENERIC);
        assert_eq!(run(&argv(&["bench"])), exit::GENERIC);
        assert_eq!(run(&argv(&["doc"])), exit::GENERIC);
    }

    /// §13.4: `--arg K=V` and `--feature f` are listed for `infy run` but have no consumer in this
    /// implementation (`env` is a closed field set without arguments, §4.6, and `[features]` never
    /// reaches §2.2 constraint evaluation from the command line). They must fail loudly, not be
    /// ignored — the check runs before the front end, so this costs no compiler.
    #[test]
    fn run_refuses_flags_it_cannot_honour() {
        assert_eq!(run(&argv(&["run", "--arg", "K=V", "examples/tour"])), exit::GENERIC);
        assert_eq!(run(&argv(&["run", "--feature", "gpu", "examples/tour"])), exit::GENERIC);
    }

    /// §13.4 + §7.4 + §8.2: `infy run examples/tour` runs stages [1]-[9] into a temporary
    /// directory, executes the artifact, and the chain *terminates* — `goto("done")` in node 3's
    /// linkage block moves the focus, so the `goto(+1)` after it never overwrites the target.
    /// Exit 0 (§13.4: success). These three constants are the expected strings the C4 acceptance
    /// asks for; they were read off the real process output (docs/RUNLOG.md cron-10).
    ///
    /// `TOUR_EXPECTED_STDOUT` is empty because **no INFY construct in the codegen subset can write
    /// to stdout yet** — §14.3's tour prints nothing and §7.7.1 item 4 sends B-track `print()` to
    /// the log sink, not to stdout (registered as BACKLOG F19, not hidden). The byte-exact
    /// *forwarding* is pinned by the panic test below. Empty stderr is itself an assertion here:
    /// a container the runtime cannot read prints `artifact rejected: …` / `runtime: no INFY
    /// artifact container …` (§7.4), so this pins "the profile came from `.inf meta`".
    #[test]
    fn run_executes_the_tour_and_exits_zero() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples").join("tour");
        let p = root.to_string_lossy().to_string();
        let out = run_module(&[p], &[], &Scope::default());
        assert_eq!(out.code, exit::OK, "infy run examples/tour: {:?}", String::from_utf8_lossy(&out.program_stderr));
        assert_eq!(String::from_utf8_lossy(&out.program_stdout), TOUR_EXPECTED_STDOUT);
        assert_eq!(String::from_utf8_lossy(&out.program_stderr), TOUR_EXPECTED_STDERR);
    }

    /// §13.4 exit code 3 = runtime panic, and the report is the §9.2 bytes. `overflow = "panic"`
    /// (§4.4 / UB-003) turns `u32::MAX + 1` in a chain node into `R-0001`; the focus fails, so
    /// `infy_sched_run_until_idle` returns 3 and `infy run` forwards it. A missing `cc` makes the
    /// build step fail this test instead of silently skipping it.
    #[test]
    fn run_forwards_the_program_streams_and_a_runtime_panic_maps_to_exit_3() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("scratch")
            .join("cli-run-boom");
        std::fs::create_dir_all(dir.join("cmd").join("boom")).expect("scratch module dir");
        std::fs::write(
            dir.join(pkg::MANIFEST_NAME),
            "[module]\npath    = \"example/boom\"\nversion = \"0.1.0\"\ninfy    = \"2.0\"\n\n\
             [profile]\noverflow = \"panic\"\nopt_level = 0\n\n[[bin]]\nname = \"boomer\"\npath = \"cmd/boom\"\n",
        )
        .expect("manifest");
        std::fs::write(
            dir.join("cmd").join("boom").join("main.infy"),
            "#![entry_chain = \"Boom\"]\n\npackage boom\n\nchain Boom {\n    tick();\n}\n\n\
             fn tick() -> u32 {\n    let a: u32 = 4294967295u32;\n    a + 1u32\n}\n",
        )
        .expect("source");
        let out = run_module(&[dir.to_string_lossy().to_string()], &[], &Scope::default());
        let err = String::from_utf8_lossy(&out.program_stderr).replace("\r\n", "\n");
        assert_eq!(out.code, exit::RUNTIME, "stderr was: {err}");
        assert_eq!(String::from_utf8_lossy(&out.program_stdout), PANIC_EXPECTED_STDOUT);
        assert!(err.starts_with(PANIC_EXPECTED_LINE), "first §9.2 line must be the panic report: {err}");
        assert!(err.contains("infy_add_u32 overflowed"), "the note names the operation: {err}");
        assert!(err.contains("(node_idx 0)"), "§12.4: the report carries the node: {err}");
    }


    // ---------------------------------------------------------------- §5.9 `infy test`

    /// §5.9/§13.4: `infy test`'s expected strings, read off the real processes in
    /// `docs/RUNLOG.md` cron-11 rather than written first. The harness is generated C writing in
    /// text mode, so the report is compared after `\r\n` → `\n` (same convention as the `run`
    /// tests' §9.2 comparison).
    const TEST_GREEN_REPORT: &str = "test example/ctest/cmd/ctest::calls_work ... ok\n\
                                     test example/ctest/cmd/ctest::result_ok ... ok\n\
                                     test result: 2 passed, 0 failed, 2 total\n";
    const TEST_RED_REPORT: &str = "test example/ctest/cmd/ctest::a_ok ... ok\n\
                                   test example/ctest/cmd/ctest::b_err ... FAILED\n\
                                   test result: 1 passed, 1 failed, 2 total\n";
    const TEST_NO_TESTS_REPORT: &str = "test result: 0 passed, 0 failed, 0 total\n";

    /// A scratch module for `infy test`: one ordinary package plus §5.9's `mod_test.infy`, whose
    /// import path is `example/ctest/cmd/ctest`. `fn main` is there so the *ordinary* build of the
    /// same module is a legal §7.4 artifact and the test build is the only thing under test.
    fn ctest_module(name: &str, sources: &[(&str, &str)]) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("scratch").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cmd").join("ctest")).expect("scratch module dir");
        std::fs::write(
            dir.join(pkg::MANIFEST_NAME),
            "[module]\npath    = \"example/ctest\"\nversion = \"0.1.0\"\ninfy    = \"2.0\"\n\n\
             [profile]\noverflow = \"panic\"\nopt_level = 0\n\n[[bin]]\nname = \"ctest\"\npath = \"cmd/ctest\"\n",
        )
        .expect("manifest");
        for (rel, text) in sources {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent dir")).expect("source dir");
            std::fs::write(&p, text).expect("source");
        }
        dir
    }

    const CTEST_MOD: &str = "package ctest\n\nfn main() { }\n\npub fn double(x: u32) -> u32 { x + x }\n\nfn work() { }\n";

    /// Same scratch-module builder, with the `[profile] a_track = "unsafe-trusted"` declaration §7.6.6
    /// makes a precondition of using the A track at all (`E-0722` without it).
    fn a_track_module(name: &str, sources: &[(&str, &str)]) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("scratch").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cmd").join("ctest")).expect("scratch module dir");
        std::fs::write(
            dir.join(pkg::MANIFEST_NAME),
            "[module]\npath    = \"example/ctest\"\nversion = \"0.1.0\"\ninfy    = \"2.0\"\n\n\
             [profile]\noverflow = \"panic\"\nopt_level = 0\na_track = \"unsafe-trusted\"\n\n[[bin]]\nname = \"ctest\"\npath = \"cmd/ctest\"\n",
        )
        .expect("manifest");
        for (rel, text) in sources {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent dir")).expect("source dir");
            std::fs::write(&p, text).expect("source");
        }
        dir
    }

    /// The minimal A-track unit for D3's acceptance. The observable is the C side's own `printf`:
    /// INFY has no construct that writes stdout (F19), while A-track C code legitimately can — so a
    /// printed line proves the unit was compiled, linked into the image **and** called from it.
    /// A chain entry rather than `fn main`, because `emit_driver` gives a chain-less module
    /// `return 1;` without reaching the function.
    const ATRACK_MOD: &str = "#![entry_chain = \"Dec\"]\n\npackage ctest\n\n#[link(name = \"decoder\", static = true, toolchain = \"cc\")]\nextern \"C\" {\n    fn dec_mark(v: u32) -> u32;\n}\n\nchain Dec {\n    step();\n}\n\nfn step() -> u32 {\n    @c(name = \"decoder\") {\n        #include <stdio.h>\n        uint32_t dec_mark(uint32_t v) {\n            printf(\"dec_mark(%u) -> %u\\n\", (unsigned)v, (unsigned)(v + 1u));\n            return v + 1;\n        }\n    }\n    unsafe { dec_mark(41u32) }\n    // infy-safety: dec_mark 只做算术与打印，不持有跨界指针\n}\n";

    /// D3 的验收：一个最小 `@c` 单元被 gcc 编译、链接、在产物里被调通。
    #[test]
    fn an_a_track_unit_is_compiled_linked_and_called() {
        let dir = a_track_module("cli-atrack", &[("cmd/ctest/main.infy", ATRACK_MOD)]);
        let p = dir.to_string_lossy().to_string();
        let out = run_module(&[p], &[], &Scope::default());
        let so = String::from_utf8_lossy(&out.program_stdout).to_string();
        let se = String::from_utf8_lossy(&out.program_stderr).to_string();
        assert_eq!(out.code, exit::OK, "infy run on the A-track module must exit 0\n--- stderr ---\n{se}\n--- stdout ---\n{so}");
        assert_eq!(
            so.replace("\r\n", "\n"),
            "dec_mark(41) -> 42\n",
            "the wrapper must have been called through, from inside the linked image\n--- stdout ---\n{so}\n--- stderr ---\n{se}"
        );
    }

    /// §7.6.3 项 1/2/4：绑定头与包装 TU 是**编译器产出**，落在 `<build>/a_track/`（§7.6.1 点名的路径），
    /// 且头文件里的包装原型必须与 `ci.c` 调用的符号逐字符相同 —— 不一致是**链接期**错误，
    /// 任何一侧的单测都看不见（`docs/spec-deviations.md` D-46(a)）。
    #[test]
    fn the_build_writes_the_binding_header_and_the_wrapper_unit() {
        let dir = a_track_module("cli-atrack-files", &[("cmd/ctest/main.infy", ATRACK_MOD)]);
        let p = dir.to_string_lossy().to_string();
        // `cli::run` takes the whole argv including argv[0].
        let code = run(&["infy".to_string(), "build".to_string(), p]);
        assert_eq!(code, exit::OK, "infy build must accept the A-track module");
        let at = dir.join(".infy-build").join(host_triple()).join("a_track");
        let header = std::fs::read_to_string(at.join("infy_binding.h")).unwrap_or_else(|e| panic!("{}: {e}", at.display()));
        let unit = std::fs::read_to_string(at.join("decoder.c")).expect("a_track/decoder.c");
        let ci = std::fs::read_to_string(dir.join(".infy-build").join(host_triple()).join("ctest.c")).expect("generated C");
        // 头文件里两半都在：用户必须定义的符号 + 包装符号。
        assert!(header.contains("uint32_t dec_mark(uint32_t v);"), "{header}");
        // §7.6.1：用户源码逐字节进 a_track/<name>.c。
        assert!(unit.contains("printf(\"dec_mark(%u) -> %u"), "{unit}");
        assert!(unit.contains("#include \"infy_binding.h\""), "{unit}");
        // 同一份包装符号：头、包装本体、生成码的调用点三处必须一字不差。
        let (_, after) = unit.split_once("uint32_t infy_e_dec_mark_").unwrap_or_else(|| panic!("{unit}"));
        let (hash, _) = after.split_once("(uint32_t v)").unwrap_or_else(|| panic!("{unit}"));
        let sym = format!("infy_e_dec_mark_{hash}");
        assert!(header.contains(&sym), "the header must declare {sym}:\n{header}");
        assert!(ci.contains(&format!("extern uint32_t {sym}")), "the generated C must declare {sym}:\n{ci}");
        assert!(ci.contains(&format!("{sym}(UINT32_C(41))")), "and call it:\n{ci}");
    }

    /// §7.6.1: `#[link]` 有块而 `@c` 落空 → `E-0720`，且 `infy check` 以诊断退出码收。
    /// 这一半此前**从未被报过**：`sema` 只查“源码块找不到签名块”那个方向。
    #[test]
    fn a_signature_block_without_a_source_block_is_e0720() {
        let dir = a_track_module(
            "cli-atrack-unpaired",
            &[("cmd/ctest/main.infy", "package ctest\n\n#[link(name = \"decoder\", static = true)]\nextern \"C\" {\n    fn dec_mark(v: u32) -> u32;\n}\n\nfn main() { }\n")],
        );
        let mut h = Handler::default();
        let mut l = load(&dir, &mut h).expect("loads");
        analyse(&mut l);
        let codes: Vec<String> = l.handler.diags.iter().map(|d| d.code.clone()).collect();
        assert!(codes.iter().any(|c| c == "E-0720"), "the other half of §7.6.1 pairing must fire, got {codes:?}");
        let text = l.handler.render(&l.sources);
        assert!(text.contains("decoder") && text.contains("@c"), "{text}");
    }

    /// UB-010: 返回裸指针而不写移交方式 → `E-0723`。这条码在本仓库**此前全仓库零发点**
    /// （`grep -rn "E-0723" src/` 只剩 `codes.rs` 的码表行），所以这一条是 D3 验收里唯一真正空的判据。
    #[test]
    fn a_pointer_return_without_ownership_is_e0723_from_the_cli() {
        let dir = a_track_module(
            "cli-atrack-ownership",
            &[("cmd/ctest/main.infy", "package ctest\n\n#[link(name = \"decoder\", static = true)]\nextern \"C\" {\n    fn dec_open(v: u32) -> *mut u8;\n}\n\n@c(name = \"decoder\") {\n    struct H { int x; };\n    struct H* _dec_open(unsigned v) { (void)v; return 0; }\n}\n\nfn main() { }\n")],
        );
        let mut h = Handler::default();
        let mut l = load(&dir, &mut h).expect("loads");
        analyse(&mut l);
        let hits: Vec<String> = l.handler.diags.iter().filter(|d| d.code == "E-0723").map(|d| d.title.clone()).collect();
        assert_eq!(hits.len(), 1, "exactly one UB-010 finding, got {hits:?}");
        assert_eq!(l.handler.exit_code(), exit::DIAGNOSED, "E-0723 must be a blocking diagnostic");
    }

    /// §4.7.5: 调用 A 轨 `extern` 函数不在 `unsafe` 内 → `E-0721`。原来的判据写的是
    /// `self.link_names().iter().any(|_| true)`（等价于“模块里存在过任何一个 `#[link]`”），
    /// 这里钉的是修好后的语义：调用点自己决定的那一条。
    #[test]
    fn an_unsafe_less_a_track_call_is_e0721() {
        let dir = a_track_module(
            "cli-atrack-unsafe",
            &[("cmd/ctest/main.infy", "package ctest\n\n#[link(name = \"decoder\", static = true)]\nextern \"C\" {\n    fn dec_mark(v: u32) -> u32;\n}\n\nfn main() {\n    dec_mark(1u32);\n}\n")],
        );
        let mut h = Handler::default();
        let mut l = load(&dir, &mut h).expect("loads");
        analyse(&mut l);
        let codes: Vec<String> = l.handler.diags.iter().map(|d| d.code.clone()).collect();
        assert!(codes.iter().any(|c| c == "E-0721"), "a bare A-track call must be E-0721, got {codes:?}");
    }

    /// §5.9 items 1-2 read against the file set: the `mod_test.infy` file is in the test build and
    /// out of `infy check|build|run`. Asserted on `Loaded` directly, because the `run`/`check`
    /// paths print the same count and must agree with it.
    #[test]
    fn test_files_are_out_of_the_ordinary_build() {
        let dir = ctest_module("cli-test-scope", &[("cmd/ctest/mod.infy", CTEST_MOD), ("cmd/ctest/mod_test.infy", "package ctest\n\n#[test]\nfn one() { work(); }\n")]);
        let mut h = Handler::default();
        let normal = load(&dir, &mut h).expect("the module loads");
        assert_eq!(normal.files.len(), 1, "§5.9: `*_test.infy` is not in the ordinary build");
        assert!(normal.handler.diags.iter().all(|d| d.code != "E-0501"), "one package over two files is legal (§5.1 R2), got {:?}", normal.handler.diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>());
        let mut h2 = Handler::default();
        let scoped = load_scoped(&dir, &mut h2, &Scope { test: true, bench: false }, &host_triple()).expect("test build loads");
        assert_eq!(scoped.files.len(), 2, "§5.9: `infy test` compiles the test file");
        assert_eq!(scoped.scope.test, true, "the scope is recorded on the module");
    }

    /// §5.8 item 1 + §7.3: the dirhash is over the module's **whole** file set, so it must not move
    /// with the build scope. §5.9 keeps `*_test.infy` out of the ordinary build, but the file is
    /// still part of what gets published — and a hash that depended on which command ran would make
    /// the build-id depend on it too.
    #[test]
    fn dirhash_covers_files_the_build_excludes() {
        let dir = ctest_module("cli-test-dirhash", &[("cmd/ctest/mod.infy", CTEST_MOD), ("cmd/ctest/mod_test.infy", "package ctest\n\n#[test]\nfn one() { work(); }\n")]);
        let mut h = Handler::default();
        let normal = load(&dir, &mut h).expect("ordinary build loads");
        let mut h2 = Handler::default();
        let scoped = load_scoped(&dir, &mut h2, &Scope { test: true, bench: false }, &host_triple()).expect("test build loads");
        assert_eq!(normal.files.len(), 1, "the test file is out of the ordinary build");
        assert_eq!(scoped.files.len(), 2, "and in for `infy test`");
        assert_eq!(package_dirhashes(&normal), package_dirhashes(&scoped), "§5.8: same hashes for both scopes");
        assert_eq!(package_dirhashes(&normal).len(), 1, "one package in this module");
    }

    /// §2.2's constraint expression is a second, independent gate on the file set, and it is
    /// checked for every file — including excluded ones, so an unknown name diagnoses instead of
    /// quietly dropping the file. `//infy:build ignore` stays out of both builds, which is what lets
    /// a draft that cannot type-check sit in the tree.
    #[test]
    fn build_constraints_gate_the_file_set_from_the_cli() {
        let dir = ctest_module(
            "cli-test-constraints",
            &[
                ("cmd/ctest/mod.infy", CTEST_MOD),
                ("cmd/ctest/draft.infy", "//infy:build ignore\n\npackage ctest\n\npub fn never(x: NoSuch) -> u32 { x }\n"),
                ("cmd/ctest/gated.infy", "//infy:build test\n\npackage ctest\n\n#[test]\nfn gated() { work(); }\n"),
            ],
        );
        let mut h = Handler::default();
        let normal = load(&dir, &mut h).expect("loads");
        assert_eq!(normal.files.len(), 1, "§2.2: `ignore` is out of every build; `test` is out of this one");
        assert!(!normal.handler.diags.iter().any(|d| d.code.starts_with('E')), "{:?}", normal.handler.diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>());
        let mut h2 = Handler::default();
        let scoped = load_scoped(&dir, &mut h2, &Scope { test: true, bench: false }, &host_triple()).expect("loads");
        assert_eq!(scoped.files.len(), 2, "§2.2: `//infy:build test` is in only for `infy test`");
        assert!(!scoped.handler.diags.iter().any(|d| d.code.starts_with('E')), "{:?}", scoped.handler.diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>());

        let dir2 = ctest_module(
            "cli-test-badconstraint",
            &[
                ("cmd/ctest/mod.infy", CTEST_MOD),
                ("cmd/ctest/odd.infy", "//infy:build riscv\n\npackage ctest\n\npub fn q() -> u32 { 1u32 }\n"),
            ],
        );
        let mut h3 = Handler::default();
        let l3 = load(&dir2, &mut h3).expect("loads");
        assert!(
            l3.handler.diags.iter().any(|d| d.code == "E-0012" && d.title.contains("riscv")),
            "§2.2: an unknown constraint name must be diagnosed, not silently excluded: {:?}",
            l3.handler.diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>()
        );
    }

    /// §5.9 + §13.4: every test passes ⇒ exit 0, and the harness's own report is forwarded
    /// byte-for-byte on stdout (the program's stream stays undiluted, D-35(a)). Empty program
    /// stderr is itself an assertion, as in the `run` tests: an artifact the runtime cannot read,
    /// or a profile it refuses, prints there.
    #[test]
    fn test_build_runs_the_harness_and_exits_zero() {
        let dir = ctest_module(
            "cli-test-green",
            &[
                ("cmd/ctest/mod.infy", CTEST_MOD),
                (
                    "cmd/ctest/mod_test.infy",
                    "package ctest\n\n#[test]\nfn calls_work() { work(); }\n\n#[test]\nfn result_ok() -> Result<u32, u32> { Ok(double(1u32)) }\n",
                ),
            ],
        );
        let out = run_module(&[dir.to_string_lossy().to_string()], &[], &Scope { test: true, bench: false });
        assert_eq!(out.code, exit::OK, "program stderr: {}", String::from_utf8_lossy(&out.program_stderr));
        assert_eq!(String::from_utf8_lossy(&out.program_stdout).replace("\r\n", "\n"), TEST_GREEN_REPORT);
        assert!(out.program_stderr.is_empty(), "§7.4/§9.2: no container or policy complaint, got {:?}", String::from_utf8_lossy(&out.program_stderr));
    }

    /// §13.4: a test that returns `Err` is a *general failure* (1), not a runtime panic (3) — and
    /// the suite still runs the remaining tests, so the report names both verdicts.
    #[test]
    fn a_failing_test_is_exit_1_not_a_panic() {
        let dir = ctest_module(
            "cli-test-red",
            &[
                ("cmd/ctest/mod.infy", CTEST_MOD),
                (
                    "cmd/ctest/mod_test.infy",
                    "package ctest\n\n#[test]\nfn a_ok() -> Result<u32, u32> { Ok(double(1u32)) }\n\n#[test]\nfn b_err() -> Result<u32, u32> { Err(7u32) }\n",
                ),
            ],
        );
        let out = run_module(&[dir.to_string_lossy().to_string()], &[], &Scope { test: true, bench: false });
        assert_eq!(out.code, exit::GENERIC, "§13.4: 1 = a test failed");
        assert_eq!(String::from_utf8_lossy(&out.program_stdout).replace("\r\n", "\n"), TEST_RED_REPORT);
    }

    /// §13.4's 3 = runtime panic, and the C4 lesson repeated: a panic is only visible by *running*
    /// the artifact. `overflow = "panic"` (§4.4) turns `u32::MAX + 1` inside a `#[test]` into
    /// `R-0001`; §4.16 has no catch point outside a focus, so the suite stops — the test that sorts
    /// after the panicking one must not appear in the report, and the §9.2 bytes are forwarded.
    #[test]
    fn a_panic_inside_a_test_is_exit_3_and_stops_the_suite() {
        let dir = ctest_module(
            "cli-test-panic",
            &[
                ("cmd/ctest/mod.infy", CTEST_MOD),
                (
                    "cmd/ctest/mod_test.infy",
                    "#![infy_version = \"2.0\"]\n\npackage ctest\n\n#[test]\nfn a_first() { work(); }\n\n#[test]\nfn m_boom() -> Result<u32, u32> {\n    let a: u32 = 4294967295u32;\n    Ok(a + 1u32)\n}\n\n#[test]\nfn z_after() { work(); }\n",
                ),
            ],
        );
        let out = run_module(&[dir.to_string_lossy().to_string()], &[], &Scope { test: true, bench: false });
        assert_eq!(out.code, exit::RUNTIME, "§13.4: 3 = 运行时 panic");
        let so = String::from_utf8_lossy(&out.program_stdout).replace("\r\n", "\n");
        assert!(so.contains("test example/ctest/cmd/ctest::a_first ... ok\n"), "the suite ran up to the panic: {so}");
        assert!(!so.contains("z_after"), "the tests after the panicking one must not run, and the summary must not be printed: {so}");
        let err = String::from_utf8_lossy(&out.program_stderr).replace("\r\n", "\n");
        assert!(err.starts_with(PANIC_EXPECTED_LINE), "§9.2 report first: {err}");
        assert!(err.contains("infy_add_u32 overflowed"), "the note names the operation: {err}");
    }

    /// §13.4: `--cov` has no consumer (no stage emits §12.4's `-O0` node-table instrumentation), so
    /// it is refused before any compilation happens. A module with no `#[test]` function is *not*
    /// an error: zero tests run and §13.4's 0 comes back — the harness never starts the §7.4 entry,
    /// which the byte-exact report proves (the tour's chain would otherwise print nothing and the
    /// summary would still be the only line).
    #[test]
    fn test_refuses_cov_and_runs_zero_tests_where_there_are_none() {
        assert_eq!(run(&argv(&["test", "--cov", "examples/tour"])), exit::GENERIC);
        let out = run_module(&["examples/tour".to_string()], &[], &Scope { test: true, bench: false });
        assert_eq!(out.code, exit::OK, "no `#[test]` function is success, program stderr: {}", String::from_utf8_lossy(&out.program_stderr));
        assert_eq!(String::from_utf8_lossy(&out.program_stdout).replace("\r\n", "\n"), TEST_NO_TESTS_REPORT);
        assert!(out.program_stderr.is_empty(), "the artifact loaded and its policy came from `.inf meta`: {:?}", String::from_utf8_lossy(&out.program_stderr));
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

    // ---------------------------------------------------------------- §7.1 [6a] B 轨摄取

    /// A scratch module under `target/scratch/` (§5.1's shape: manifest + one package directory).
    fn scratch_module(name: &str, manifest_tail: &str, src: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("scratch")
            .join(name);
        std::fs::create_dir_all(dir.join("core").join("embed")).expect("scratch module dir");
        std::fs::write(
            dir.join(pkg::MANIFEST_NAME),
            format!(
                "[module]\npath    = \"example/embed\"\nversion = \"0.1.0\"\ninfy    = \"2.0\"\n\n\
                 [dependencies]\n\"std\" = \"1.0\"\n\n[profile]\noverflow = \"panic\"\n{manifest_tail}"
            ),
        )
        .expect("manifest");
        std::fs::write(dir.join("core").join("embed").join("mod.infy"), src).expect("source");
        dir
    }

    /// §7.1's stage note: `infy check` runs [6a], so a module with a `@py` unit gets its IFBC image
    /// from the front end — and the image's own function record carries the **INFY-side declared**
    /// table (§7.7.1 rule 1: arity and the return's scalar code), which is what §7.8.3 item 5
    /// later compares a digest against. The declared minor is read off the interpreter this build
    /// finds, because §7.7.3 makes an agreement between the two a precondition, not a coincidence.
    #[test]
    fn check_ingests_a_py_unit_and_keeps_the_declared_table() {
        let py = crate::btrack::find_python().expect("阶段 [6a] 需要构建期 CPython（§7.7.3）");
        let observed = crate::btrack::probe_version(&py).expect("探针应报出解释器版本");
        let minor = observed.split('.').take(2).collect::<Vec<_>>().join(".");
        let dir = scratch_module(
            "cli-6a-py",
            &format!("[b_track]\npy = \"{minor}\"\n"),
            "#![infy_version = \"2.0\"]\n\npackage embed\n\n\
             event normalize(scale: f64, n: u32) -> f64 {\n    @py(scale=scale, n=n) {\n        return scale * 2.0\n    }\n}\n\nfn main() { }\n",
        );
        let (l, _prog, summary, code) =
            frontend(&[dir.to_string_lossy().to_string()], &[], &Scope::default())
                .expect("frontend 应返回结果");
        assert_eq!(code, exit::OK, "该模块在 [1]-[6a] 应是干净的：{summary}");
        assert_eq!(l.embed_images.len(), 1, "一个 @py 单元 = 一份镜像：{:?}", l.embed_images);
        let img = &l.embed_images[0];
        assert_eq!(img.unit, "normalize");
        assert!(!img.image.is_empty(), "阶段 [6a] 必须产出镜像字节");
        assert!(
            img.source_toolchain.starts_with("CPython "),
            "§7.7.3：源工具链串要进 `.inf meta` 与 build-id：{}",
            img.source_toolchain
        );
        let m = crate::ifbc::decode(&img.image).expect("[6a] 的镜像必须可解码");
        assert_eq!(m.funcs.len(), 1, "只有单元自己那一条函数记录（探针按写入的单元名取体）");
        assert_eq!(m.funcs[0].nargs, 2, "§7.7.1 规则 1：形参个数来自 INFY 侧声明");
        assert_eq!(m.funcs[0].ret_sig, 13, "f64 的标量码是 13（`runtime/IFBC.md` §2）");
        assert!(!m.lines.is_empty(), "§7.4：镜像必须带原始语言行映射");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A B-track prefix this build cannot lower is an **explicit** refusal naming that prefix
    /// (`E-0712`), not a unit that quietly vanishes: §7.7.1's ingest table names `luac` for `@lua`
    /// while the probe only drives CPython.
    #[test]
    fn a_lua_unit_is_refused_by_name_and_blocks_the_build() {
        let dir = scratch_module(
            "cli-6a-lua",
            "[b_track]\npy = \"3.14\"\nlua = \"5.4\"\n",
            "#![infy_version = \"2.0\"]\n\npackage embed\n\n\
             event shake(v: f64) -> f64 {\n    @lua(v=v) {\n        return v * 2.0\n    }\n}\n\nfn main() { }\n",
        );
        let (l, _prog, summary, code) =
            frontend(&[dir.to_string_lossy().to_string()], &[], &Scope::default())
                .expect("frontend 应返回结果");
        assert_eq!(code, exit::DIAGNOSED, "未实现的 B 轨前缀必须变红：{summary}");
        assert!(l.embed_images.is_empty(), "被拒绝的前缀不产出镜像：{:?}", l.embed_images);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §7.7.1 rule 1 with nothing to map: `u128` has no row in §2's closed signature set, so the
    /// table cannot be built and the build stops **naming the item and which part of the table**。
    /// Two shapes, because the parameter refusal and the return refusal are separate branches
    /// (folding either into `dyn`/`()` would be the 假成功 P3 forbids).
    #[test]
    fn a_type_with_no_ifabi_row_stops_the_ingest_by_name() {
        let py = crate::btrack::find_python().expect("阶段 [6a] 需要构建期 CPython（§7.7.3）");
        let observed = crate::btrack::probe_version(&py).expect("探针应报出解释器版本");
        let minor = observed.split('.').take(2).collect::<Vec<_>>().join(".");
        let tail = format!("[b_track]
py = \"{minor}\"
");
        for (name, src, want) in [
            (
                "cli-6a-u128-param",
                "#![infy_version = \"2.0\"]

package embed

                 event widen(v: u128) -> u32 {
    @py(v=v) {
        return 1u32
    }
}

fn main() { }
",
                "形参表",
            ),
            (
                "cli-6a-u128-ret",
                "#![infy_version = \"2.0\"]

package embed

                 event widen(v: u32) -> u128 {
    @py(v=v) {
        return v
    }
}

fn main() { }
",
                "返回类型",
            ),
        ] {
            let dir = scratch_module(name, &tail, src);
            let (l, _prog, summary, code) =
                frontend(&[dir.to_string_lossy().to_string()], &[], &Scope::default())
                    .expect("frontend 应返回结果");
            let text = l.handler.render(&l.sources);
            assert_eq!(code, exit::DIAGNOSED, "{name}：无可表达类型必须变红（{summary}）");
            assert!(text.contains("E-0712"), "{name} 应报 E-0712：{text}");
            assert!(
                text.contains(want) && text.contains("u128"),
                "{name} 的诊断必须点名{want}与 `u128`：{text}"
            );
            assert!(l.embed_images.is_empty(), "{name}：拒绝就没有镜像：{:?}", l.embed_images);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The other half of §7.1's stage note: a module with **no** B-track unit must not touch the
    /// source-language toolchain at all — `examples/tour` is exactly that, and its images list is
    /// empty even on a machine without Python. This is what keeps P6 (运行期零依赖) honest.
    #[test]
    fn a_module_without_embeds_produces_no_images() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples").join("tour");
        let (l, _prog, _summary, code) =
            frontend(&[root.to_string_lossy().to_string()], &[], &Scope::default())
                .expect("frontend 应返回结果");
        assert_eq!(code, exit::OK);
        assert!(l.embed_images.is_empty(), "纯 INFY 模块没有 [6a] 镜像：{:?}", l.embed_images);
    }

    // ------------------------------------------------------- §7.8.4 `infy ifbc dump|verify`

    /// A committed `runtime/fixtures` image (`.bin` = this side's writer output) or text
    /// (`.can.txt`/`.ver.txt` = the **C** side's rendering, written only by
    /// `INFY_REGENERATE_CANONICAL=1`).
    fn fixture_bytes(name: &str) -> Vec<u8> {
        std::fs::read(fixture_path(name)).unwrap_or_else(|e| panic!("runtime/fixtures/{name}: {e}"))
    }

    fn fixture_path(name: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtime")
            .join("fixtures")
            .join(name)
            .to_string_lossy()
            .to_string()
    }

    /// A §7.4 container whose `.inf ifbc` region carries `ifbc` bytes. No real build reaches that
    /// state today (F29: stage [8] refuses an embed body long before [9]), so the artifact side of
    /// `infy ifbc` is pinned by writing the container here.
    fn container_with_ifbc(ifbc: &[u8], n: u32) -> Vec<u8> {
        let content = artifact::Content {
            host_image: b"MZ\0\0\0\0",
            ifabi: crate::VERSION.ifabi,
            opt_level: 2,
            overflow: artifact::OV_PANIC,
            panic_radius: artifact::RADIUS_FOCUS,
            usize_bits: 64,
            fast_math: false,
            lto: 1,
            stack_bytes: 8 * 1024 * 1024,
            focus_stack_bytes: 256 * 1024,
            focus_quota: 1,
            iter_guard_steps: 1 << 32,
            target_triple: "x86_64-pc-windows-gnu",
            build_id: [0xAB; 32],
            platform: "windows",
            lang_version: "2.0",
            source_toolchain: "gcc 14.2.0",
            unreproducible: false,
            ifbc,
            n_ifbc_modules: n,
            chains: vec![],
            events: vec![],
            nodes: vec![],
            sums: vec![],
            resource_keys: vec![],
            entry: None,
        };
        artifact::write(&content).expect("容器写入应成功")
    }

    /// One-off inputs for the command go to `target/scratch/` (gitignored), never into the tree.
    fn scratch_file(name: &str, bytes: &[u8]) -> String {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("scratch");
        std::fs::create_dir_all(&dir).expect("target/scratch");
        let p = dir.join(name);
        std::fs::write(&p, bytes).expect("写 scratch 文件");
        p.to_string_lossy().to_string()
    }

    /// D2's acceptance at the **command** level: `infy ifbc dump --canonical` writes exactly the text
    /// the C loader rendered from the same bytes. D4(3) pinned that inside the C harness and in
    /// `ifbc::tests`; this is the first time the CLI itself is the producer of the diff. Canonical
    /// mode prints no header and no status line, which is what makes stdout byte-comparable.
    #[test]
    fn cli_canonical_dump_equals_the_text_the_c_loader_rendered() {
        for (bin, txt) in [
            ("ifbc_rust_writer.bin", "ifbc_rust_writer.can.txt"),
            ("ifbc_all_ops.bin", "ifbc_all_ops.can.txt"),
            ("ifbc_py_unit.bin", "ifbc_py_unit.can.txt"),
        ] {
            let image = fixture_bytes(bin);
            let want = String::from_utf8(fixture_bytes(txt)).expect("规范化文本是 UTF-8");
            let got = ifbc_dump_text(&image, true).unwrap_or_else(|e| panic!("{bin}: {e}"));
            assert_eq!(want, got, "`infy ifbc dump --canonical {bin}` 不再是 C 侧渲染的那一份文本");
            let path = fixture_path(bin);
            assert_eq!(run(&argv(&["ifbc", "dump", "--canonical", &path])), exit::OK, "{bin}");
        }
    }

    /// The default (human) form is §12.3's golden-file target. It has no cross-language byte lock —
    /// the C dumper is a deliberately different text (D-42) — so what is pinned is that the image
    /// renders and that the rendering depends on the bytes only, which is what a golden file needs.
    #[test]
    fn cli_human_dump_renders_the_module_and_is_deterministic() {
        let image = fixture_bytes("ifbc_py_unit.bin");
        let a = ifbc_dump_text(&image, false).expect("人类可读反汇编");
        let b = ifbc_dump_text(&image, false).expect("人类可读反汇编");
        assert_eq!(a, b, "反汇编文本必须只由镜像字节决定（§12.3 黄金文件）");
        assert!(a.contains("IFBC module"), "{}", a.lines().next().unwrap_or(""));
        assert!(a.contains("normalize"), "单元名必须在反汇编里（§7.7.1）");
        assert!(a.contains("func_table") || a.contains("funcs=1"), "{a}");
        let path = fixture_path("ifbc_py_unit.bin");
        assert_eq!(run(&argv(&["ifbc", "dump", &path])), exit::OK);
    }

    /// §7.8.3's verdicts as CLI output: the canonical form is the C side's `.ver.txt`, byte for byte,
    /// and the clean writer image is clean here too (the C harness asserts the same about its own
    /// render). This is **not** "the two sides' load-time checks are equivalent" — that claim is
    /// barred by F26 / deviation D-42; it is one committed text both sides reproduce.
    #[test]
    fn cli_canonical_verify_equals_the_verdict_text_the_c_loader_rendered() {
        let image = fixture_bytes("ifbc_verify_matrix.bin");
        let got = ifbc_verify_canonical_text(&[image.clone()]).expect("规范化结论");
        let want = String::from_utf8(fixture_bytes("ifbc_verify_matrix.ver.txt")).expect("UTF-8");
        assert_eq!(want, got, "`infy ifbc verify --canonical` 不再是 C 侧渲染的那一份结论文本");
        assert_eq!(got.lines().count(), 10, "{got}");
        let path = fixture_path("ifbc_verify_matrix.bin");
        assert_eq!(run(&argv(&["ifbc", "verify", "--canonical", &path])), exit::DIAGNOSED);
        let clean = fixture_bytes("ifbc_rust_writer.bin");
        assert_eq!(ifbc_verify_canonical_text(&[clean]).expect("干净镜像"), "");
    }

    /// D.10 + §13.4: the default reports only the first failing function (like the runtime),
    /// `--all-errors` lists every one, and a blocking finding is exit 2 (deviation D-45 — §13.4's
    /// list has no code for a load-time rejection).
    #[test]
    fn verify_reports_the_first_finding_by_default_and_all_of_them_with_the_flag() {
        let image = fixture_bytes("ifbc_verify_matrix.bin");
        let renders = |vs: &[ifbc::VerifyViolation]| -> Vec<String> {
            vs.iter().map(|v| v.render()).collect()
        };
        let one = ifbc_verify_image(&image, false).expect("可解码");
        assert_eq!(one.len(), 1, "默认只报首个失败函数：{:?}", renders(&one));
        assert_eq!(one[0].code, "R-0704");
        assert_eq!(one[0].func, 0);
        let all = ifbc_verify_image(&image, true).expect("可解码");
        assert_eq!(
            all.iter().filter(|v| !v.code.starts_with('W')).count(),
            10,
            "{:?}",
            renders(&all)
        );
        assert!(all.iter().all(|v| !v.code.starts_with('W')), "矩阵不该出 W-0705：{:?}", renders(&all));
        for c in ["R-0704", "R-0705", "R-0706", "R-0707", "R-0710", "R-0713"] {
            assert!(all.iter().any(|v| v.code == c), "校验矩阵没覆盖 {c}：{:?}", renders(&all));
        }
        let path = fixture_path("ifbc_verify_matrix.bin");
        assert_eq!(run(&argv(&["ifbc", "verify", &path])), exit::DIAGNOSED);
        assert_eq!(run(&argv(&["ifbc", "verify", "--all-errors", &path])), exit::DIAGNOSED);
    }

    /// F29's visible half, and the premise D2 had to accept: every artifact this build produces has
    /// an empty `.inf ifbc`. Exit 0 means "there is no bytecode here", and the text says exactly
    /// that plus why — so an empty region can never be read as "the artifact passed §7.8.3".
    #[test]
    fn an_artifact_with_an_empty_ifbc_region_is_reported_as_empty() {
        let bytes = container_with_ifbc(&[], 0);
        let input = classify_ifbc(&bytes).expect("空区仍是合法容器");
        assert!(input.images.is_empty(), "空区不该读出镜像：{input:?}");
        assert_eq!(input.container, Some((0, 0)));
        let text = ifbc_empty_region_text("verify", 0);
        assert!(text.contains("0 个 B 轨模块"), "{text}");
        assert!(text.contains("F29") && text.contains("不声称"), "{text}");
        let path = scratch_file("d2-empty.exe", &bytes);
        assert_eq!(run(&argv(&["ifbc", "dump", &path])), exit::OK);
        assert_eq!(run(&argv(&["ifbc", "verify", &path])), exit::OK);
    }

    /// The artifact path of the command is reachable even while real builds cannot fill the region:
    /// a container whose `.inf ifbc` holds exactly one module must inspect **those** bytes. This is
    /// the shape §13.4's `infy ifbc verify runner` example takes once F29 lands.
    #[test]
    fn a_one_module_artifact_is_read_from_the_ifbc_region() {
        let image = fixture_bytes("ifbc_rust_writer.bin");
        let want = String::from_utf8(fixture_bytes("ifbc_rust_writer.can.txt")).expect("UTF-8");
        let bytes = container_with_ifbc(&image, 1);
        let input = classify_ifbc(&bytes).expect("一个模块的容器");
        assert_eq!(input.images, vec![image.clone()], "区里读回的字节必须就是写进去的那一份");
        assert_eq!(input.container, Some((1, image.len() as u32)));
        assert_eq!(ifbc_dump_text(&input.images[0], true).expect("dump"), want);
        assert_eq!(
            ifbc_verify_image(&image, true)
                .expect("verify")
                .iter()
                .filter(|v| !v.code.starts_with('W'))
                .count(),
            0,
            "干净镜像在本侧也不该有阻断结论"
        );
        let path = scratch_file("d2-one-module.exe", &bytes);
        assert_eq!(run(&argv(&["ifbc", "dump", "--canonical", &path])), exit::OK);
        assert_eq!(run(&argv(&["ifbc", "verify", &path])), exit::OK);
    }

    /// §7.4 frames no delimiter for several modules in one region and neither reader implements one,
    /// so a container claiming more than one is refused **by name**: guessing a split would let the
    /// command hand back a clean bill of health for a prefix of the region (deviation D-45).
    #[test]
    fn several_modules_in_one_region_are_refused_by_name() {
        let image = fixture_bytes("ifbc_rust_writer.bin");
        let mut two = image.clone();
        two.extend_from_slice(&image);
        for n in [2u32, 0u32] {
            let bytes = container_with_ifbc(&two, n);
            match classify_ifbc(&bytes) {
                Err(IfbcOpenErr::Framing { n: got, len }) => {
                    assert_eq!(got, n);
                    assert_eq!(len as usize, two.len());
                }
                other => panic!("n_ifbc_modules={n} 必须走 Framing，实际 {other:?}"),
            }
            let path = scratch_file(&format!("d2-{n}modules.exe"), &bytes);
            assert_eq!(run(&argv(&["ifbc", "verify", &path])), exit::GENERIC, "n={n}");
        }
    }

    /// §13.4's codes for the failure modes the spec does not assign: an unrecognisable file keeps
    /// `artifact info`'s borrowing (E-0605 / exit 7, D-32), a bad subcommand or a missing path is 1.
    #[test]
    fn ifbc_refuses_a_file_that_is_neither_and_an_unusable_invocation() {
        let readme = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");
        let readme = readme.to_string_lossy().to_string();
        assert_eq!(run(&argv(&["ifbc", "dump", &readme])), exit::CHECKSUM);
        assert_eq!(run(&argv(&["ifbc", "frobnicate", &readme])), exit::GENERIC);
        assert_eq!(run(&argv(&["ifbc", "dump"])), exit::GENERIC);
        assert_eq!(
            run(&argv(&["ifbc", "verify", "target/scratch/definitely-not-here-ifbc.bin"])),
            exit::GENERIC
        );
    }
}
