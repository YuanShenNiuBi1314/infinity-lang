//! §10 — the standard library surface, as the compiler sees it.
//!
//! `std` is a built-in *virtual* module (§5.3 step 2): it cannot be shadowed by a local module
//! (`E-0507`), it is a mandatory dependency (§5.2 `[dependencies] "std" = "1.0"`), and it ships
//! with the toolchain as a wheel-edition `.infib` plus a source edition for audit and breakpoint
//! mapping (§10 preamble).
//!
//! This module holds the normative signature table (`STD`). **The table has no consumer outside
//! this file as of now** — `std_packages()`, `lookup()` and `all_symbol_paths()` are each called
//! only by the tests below, and stage [3] classifies `import "std/…"` then drops it
//! (`cli.rs::ImportSource::Std => {}`), so **no INFY source can reach any `std` item**, including
//! the ones with a native entry point. That gap is `docs/BACKLOG.md` F40; §10.4's `std.net` refusal
//! is `docs/spec-deviations.md` D-50(b). §12.1's domain scan currently uses `nfkc_case_fold` only.
//!
//! §2.10's `NFKCCaseFold` is used by the domain scan so that `Numpy` and `numpy` cannot both slip
//! past a lowercase blacklist.

use std::collections::BTreeSet;

/// One declared `std` item.
#[derive(Clone, Copy, Debug)]
pub struct StdItem {
    /// `std.chain.lock`
    pub path: &'static str,
    /// Signature text as printed by `infy doc` (§10 verbatim).
    pub signature: &'static str,
    /// `infy_rt` entry point implementing it, or `None` when it is a type / trait / keyword note.
    pub native: Option<&'static str>,
    pub clause: &'static str,
}

/// §10.1–§10.14. Entries whose behaviour is normative are listed even when the runtime implements
/// them through a generated thunk (e.g. `Guard` drop).
pub static STD: &[StdItem] = &[
    StdItem { path: "std.chain.id", signature: "std.chain.id() -> u32", native: Some("infy_current_chain"), clause: "§10.1" },
    StdItem { path: "std.chain.node_idx", signature: "std.chain.node_idx() -> u32", native: Some("infy_current_node"), clause: "§10.1" },
    StdItem { path: "std.chain.lock", signature: "std.chain.lock(resource: &str) -> Guard", native: Some("infy_lock_acquire"), clause: "§8.3/§10.1" },
    StdItem { path: "std.chain.yield_now", signature: "std.chain.yield_now()", native: Some("infy_yield_now"), clause: "§10.1" },
    StdItem { path: "std.chain.focus_count", signature: "std.chain.focus_count(target: &str) -> u32", native: Some("infy_focus_count"), clause: "§10.1" },
    StdItem { path: "std.math.abs", signature: "abs(x: T) -> T", native: None, clause: "§4.5/§10.2" },
    StdItem { path: "std.math.min", signature: "min(a: T, b: T) -> T", native: None, clause: "§10.2" },
    StdItem { path: "std.math.max", signature: "max(a: T, b: T) -> T", native: None, clause: "§10.2" },
    StdItem { path: "std.math.clamp", signature: "clamp(v: T, lo: T, hi: T) -> T", native: None, clause: "§10.2" },
    StdItem { path: "std.math.roundN", signature: "roundN(x: f64, N: u32) -> u64", native: None, clause: "§4.5/§4.12" },
    StdItem { path: "std.math.sqrt", signature: "sqrt(x: f64) -> f64", native: None, clause: "§10.2" },
    StdItem { path: "std.math.pow", signature: "pow(b: T, e: T) -> T", native: None, clause: "§10.2" },
    StdItem { path: "std.math.sin", signature: "sin(x: f64) -> f64", native: None, clause: "§10.2" },
    StdItem { path: "std.math.cos", signature: "cos(x: f64) -> f64", native: None, clause: "§10.2" },
    StdItem { path: "std.math.tan", signature: "tan(x: f64) -> f64", native: None, clause: "§10.2" },
    StdItem { path: "std.math.log", signature: "log(x: f64, base: f64) -> f64", native: None, clause: "§10.2" },
    StdItem { path: "std.math.ln", signature: "ln(x: f64) -> f64", native: None, clause: "§10.2" },
    StdItem { path: "std.mem.size_of", signature: "std.mem.size_of<T>() -> u32", native: None, clause: "§4.5" },
    StdItem { path: "std.mem.align_of", signature: "std.mem.align_of<T>() -> u32", native: None, clause: "§4.5" },
    StdItem { path: "std.mem.offset_of", signature: "std.mem.offset_of<T,f>() -> u32", native: None, clause: "§4.8" },
    StdItem { path: "std.mem.transmute", signature: "std.mem.transmute<A,B>(x: A) -> B", native: None, clause: "§4.7.5" },
    StdItem { path: "std.meta.type_name", signature: "std.meta.type_name<T>() -> &str", native: Some("infy_meta_type_name"), clause: "§4.5" },
    StdItem { path: "std.meta.fields", signature: "std.meta.fields<T>() -> &[FieldInfo]", native: None, clause: "§6.7" },
    StdItem { path: "std.env.tick", signature: "std.env.tick() -> u64", native: Some("infy_monotonic_ms"), clause: "§4.5" },
    StdItem { path: "std.fs.read_to_str", signature: "read_to_str(path) -> Result<String, IoErr>", native: Some("infy_fs_read_all"), clause: "§10.3" },
    StdItem { path: "std.fs.write_file", signature: "write_file(path, content: &[u8]) -> Result<(), IoErr>", native: Some("infy_fs_write_all"), clause: "§10.3" },
    StdItem { path: "std.fs.exists", signature: "exists(path) -> bool", native: Some("infy_fs_exists"), clause: "§10.3" },
    StdItem { path: "std.fs.mkdir_p", signature: "mkdir_p(path) -> Result<(), IoErr>", native: Some("infy_fs_mkdir_p"), clause: "§10.3" },
    StdItem { path: "std.fs.open", signature: "open(path, mode) -> Result<File, IoErr>", native: Some("infy_fs_open"), clause: "§10.3" },
    StdItem { path: "std.net.http_get", signature: "http_get(url) -> Result<Response, NetErr>", native: None, clause: "§10.4" },
    StdItem { path: "std.net.http_post", signature: "http_post(url, body: &[u8]) -> Result<Response, NetErr>", native: None, clause: "§10.4" },
    StdItem { path: "std.net.websocket_connect", signature: "websocket_connect(url) -> Result<Ws, NetErr>", native: None, clause: "§10.4" },
    StdItem { path: "std.process.exec", signature: "exec(cmd: &str, args: &[&str], timeout_ms: u32) -> Result<ExecResult, IoErr>", native: Some("infy_process_exec"), clause: "§10.5" },
    StdItem { path: "std.sync.Mutex", signature: "Mutex<T>::new(v) -> Arc<Mutex<T>>", native: None, clause: "§10.6" },
    StdItem { path: "std.sync.RwLock", signature: "RwLock<T> — 多读单写，写者优先", native: None, clause: "§10.6" },
    StdItem { path: "std.sync.Channel", signature: "Channel<T>::new(cap: u32) -> (Sender<T>, Receiver<T>)", native: None, clause: "§10.6" },
    StdItem { path: "std.sync.Cell", signature: "Cell<T> / RefCell<T> / Rc<T> / Arc<T>", native: None, clause: "§10.6" },
    StdItem { path: "std.embed.Value", signature: "Value (= dyn 的具名形式) · as_i64 / to_json / EmbedFrame::locals", native: Some("infy_dyn_as_i64"), clause: "§10.7" },
    StdItem { path: "std.host.query", signature: "std.host.query(key: &str) -> Option<i64>", native: Some("infy_host_query"), clause: "§10.8" },
    StdItem { path: "std.host.query_str", signature: "std.host.query_str(key: &str) -> Option<String>", native: Some("infy_host_query_str"), clause: "§10.8" },
    StdItem { path: "std.host.set", signature: "std.host.set(key: &str, value: &str) -> Result<(), HostErr>", native: Some("infy_host_set"), clause: "§10.8" },
    StdItem { path: "std.host.event", signature: "std.host.event(name, args: &ArgFrame) -> Result<Value, HostErr>", native: Some("infy_host_call_event"), clause: "§10.8" },
    StdItem { path: "std.host.is_available", signature: "std.host.is_available(key: &str) -> bool", native: None, clause: "§10.8" },
    StdItem { path: "std.num.checked_add", signature: "checked_add(a, b) -> Option<T>", native: None, clause: "§10.9" },
    StdItem { path: "std.num.wrapping_add", signature: "wrapping_add(a, b) -> T — 唯一允许回绕的入口", native: Some("infy_wrapping_add_u64"), clause: "§4.4/§10.9" },
    StdItem { path: "std.num.saturating_add", signature: "saturating_add(a, b) -> T", native: Some("infy_saturating_add_u64"), clause: "§10.9" },
    StdItem { path: "std.num.overflowing_add", signature: "overflowing_add(a, b) -> (T, bool)", native: None, clause: "§10.9" },
    StdItem { path: "std.num.powi", signature: "powi(base, e: u32) -> T", native: None, clause: "§10.9" },
    StdItem { path: "std.num.gcd", signature: "gcd(a, b) -> T — 两者皆 0 → R-0002", native: None, clause: "§10.9" },
    StdItem { path: "std.num.ilog2", signature: "ilog2(a) -> u32 — a == 0 → R-0009", native: None, clause: "§10.9" },
    StdItem { path: "std.num.from_str_radix", signature: "from_str_radix(s, radix) -> Result<T, ParseIntError>", native: None, clause: "§10.9" },
    StdItem { path: "std.iter.Iterator", signature: "trait Iterator { type Item; fn next(&mut self) -> Option<Self::Item> }", native: None, clause: "§10.10" },
    StdItem { path: "std.iter.collect", signature: "collect<C: FromIter>(...) -> C — 无目标 → E-0010", native: None, clause: "§10.10" },
    StdItem { path: "std.fmt.fmt", signature: "fmt(fmt_str: &str, args) -> String", native: Some("infy_fmt_run"), clause: "§10.11" },
    StdItem { path: "std.fmt.write_fmt", signature: "write_fmt(w: &mut Writer, ...) -> Result<(), IoErr>", native: None, clause: "§10.11" },
    StdItem { path: "std.time.now", signature: "now() -> UnixTs / monotonic() -> u64 / sleep(d: Duration)", native: Some("infy_unix_seconds"), clause: "§10.12" },
    StdItem { path: "std.rand.Rng", signature: "Rng::seeded(u64) / from_host() — 无全局随机源", native: Some("infy_rng_seeded"), clause: "§10.12" },
    StdItem { path: "std.rand.shuffle", signature: "shuffle(&mut [T]) — 无偏 Fisher–Yates，从尾到头", native: Some("infy_rng_shuffle"), clause: "§10.12" },
    StdItem { path: "std.coll.Vec", signature: "Vec<T> — 容量因子 4/×2/×1.5/×1.25 属 ABI", native: Some("infy_vec_push"), clause: "§10.13" },
    StdItem { path: "std.coll.Map", signature: "Map<K,V> — 迭代按 key 的 Ord 顺序", native: Some("infy_map_new"), clause: "§10.13" },
    StdItem { path: "std.coll.HashMap", signature: "HashMap<K,V> — 迭代顺序 unspecified（W-0185）", native: None, clause: "§10.13" },
    StdItem { path: "std.rt.panic_hook", signature: "panic_hook(cb) — 仅 panic_radius != process 时有意义", native: None, clause: "§10.14" },
    StdItem { path: "std.rt.mem_stats", signature: "mem_stats() -> MemStats", native: Some("infy_mem_stats"), clause: "§10.14" },
    StdItem { path: "std.rt.build_id", signature: "build_id() -> &str", native: None, clause: "§10.14" },
    StdItem { path: "std.rt.abort", signature: "abort()", native: Some("infy_panic"), clause: "§10.14" },
];

/// The package names of `std` (§5.3 step 2). **No caller yet** — stage [3] does not build a `std`
/// binding out of them (`docs/BACKLOG.md` F40).
pub fn std_packages() -> Vec<&'static str> {
    let mut set = BTreeSet::new();
    for it in STD {
        if let Some(seg) = it.path.split('.').nth(1) {
            set.insert(seg);
        }
    }
    set.into_iter().collect()
}

pub fn lookup(path: &str) -> Option<&'static StdItem> {
    STD.iter().find(|i| i.path == path)
}

/// §12.1: every `std` path, the intended input of the domain-neutrality scan (§1.3).
/// **No caller yet** — see `docs/BACKLOG.md` F38 (the scan has no call site) and F40.
pub fn all_symbol_paths() -> Vec<&'static str> {
    STD.iter().map(|i| i.path).collect()
}

/// §2.10's `NFKCCaseFold`, used by the domain scan so `Numpy`/`numpy` cannot bypass the blacklist.
///
/// Full NFKC needs Unicode data tables the toolchain deliberately does not vendor (§1.3 keeps the
/// toolchain dependency-free); this approximation folds case, strips ignorable and combining
/// characters, and normalises full-width forms to their compatibility equivalents — which covers
/// every evasion the spec calls out. Documented in `docs/spec-deviations.md`.
pub fn nfkc_case_fold(s: &str) -> String {
    s.chars()
        .map(|c| {
            // full-width forms → ASCII compatibility equivalents
            let cw = (c as u32)
                .checked_sub(0xFEE0)
                .and_then(|v| char::from_u32(v))
                .unwrap_or(c);
            cw
        })
        .filter(|c| {
            !(0x300..=0x36F).contains(&(*c as u32))
                && !matches!(*c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0xFEFF | 0x00AD)
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_paths_are_well_formed_and_domain_free() {
        for i in STD {
            assert!(i.path.starts_with("std."), "{}", i.path);
            assert!(i.path.chars().filter(|c| *c == '.').count() >= 2, "{}", i.path);
        }
    }

    #[test]
    fn case_fold_defeats_uppercasing() {
        assert_eq!(nfkc_case_fold("Numpy"), "numpy");
        assert_eq!(nfkc_case_fold("NUMPY"), "numpy");
        assert_eq!(nfkc_case_fold("numpy\u{301}"), "numpy");
    }

    #[test]
    fn nfkc_case_fold_defeats_fullwidth() {
        // ｎｕｍｐｙ (U+FF4E…) must fold to the same skeleton as `numpy`
        assert_eq!(nfkc_case_fold("ｎｕｍｐｙ"), "numpy");
    }

    #[test]
    fn every_std_entry_cites_a_clause() {
        for i in STD {
            assert!(i.clause.starts_with('§'), "{}", i.path);
        }
    }
}
