//! Infinity Language (INFY) v2.0 — reference toolchain.
//!
//! The pipeline stages are numbered exactly as §7.1 numbers them, and each module below owns one
//! or more of those stages:
//!
//! | stage (§7.1) | module |
//! | --- | --- |
//! | [1] lex / parse | [`lexer`], [`parser`], [`ast`], [`token`], [`span`] |
//! | [2] macro expansion | [`macro_expand`], [`derive`] |
//! | [3] name resolution + dependency graph | [`pkg`] |
//! | [4] type check | [`sema`] |
//! | [5] borrow check | [`borrow`] |
//! | [6a] B-track bytecode ingest | [`btrack`] |
//! | [6b] A-track toolchain drive | [`atrack`] |
//! | [7] IFIR + optimise | [`hir`] |
//! | [8] codegen | [`codegen`] |
//! | [9] link `infyl` | [`artifact`] |
//! | [10] signature / checksum segments | [`artifact`], [`pkg`] |
//!
//! Cross-cutting: [`diag`] (§9), [`codes`] (§9.3), [`fmt`] (§9.5), [`ifbc`] (§7.8 + appendix D),
//! [`host`] (§11), [`stdlib`] (§10), [`conformance`] (§12), [`cli`] (§13.4).
//!
//! The crate has **no external dependencies**: a compiler is a trust boundary, and §5.8's
//! supply-chain model is applied to user code, so the toolchain itself must be auditable without a
//! lockfile.

pub mod artifact;
pub mod ast;
pub mod atrack;
pub mod base64;
pub mod borrow;
pub mod btrack;
pub mod cap_opmap;
pub mod cli;
pub mod codegen;
pub mod codes;
pub mod conformance;
pub mod derive;
pub mod diag;
pub mod doc;
pub mod fmt;
pub mod hir;
pub mod host;
pub mod ifbc;
pub mod lexer;
pub mod macro_expand;
pub mod parser;
pub mod pkg;
pub mod sema;
pub mod sha256;
pub mod span;
pub mod stdlib;
pub mod toml;
pub mod token;

use std::env;
use std::path::{Path, PathBuf};

/// Toolchain identity. §1.2 keeps the product ABI (`IFABI`) independent of the language version:
/// IFABI-1 belonged to the never-ratified v1.0 proposal, so v2.0 starts at IFABI-2.
#[derive(Clone, Debug)]
pub struct ToolVersion {
    pub name: &'static str,
    pub version: &'static str,
    pub lang: &'static str,
    pub ifabi: u32,
    pub ifbc: u32,
    /// §10 / §5.8: which std version ships with this compiler.
    pub std_version: &'static str,
    /// Unicode `Confusables.txt` / `EastAsianWidth.txt` declaration (§2.10, §9.5 rule 1).
    pub unicode_data: &'static str,
}

pub const VERSION: ToolVersion = ToolVersion {
    name: "infyc",
    version: "2.0.0",
    lang: "2.0",
    ifabi: 2,
    ifbc: 1,
    std_version: "1.0",
    unicode_data: "Unicode 16.0.0",
};

impl ToolVersion {
    /// `infy -V` output (§13.4): language version, tool version, IFABI and IFBC version.
    pub fn banner(&self) -> String {
        format!(
            "INFY-v{}.{} {} (IFABI-{}, IFBC v{}, std {}, unicode {})",
            self.lang,
            "0",
            self.version,
            self.ifabi,
            self.ifbc,
            self.std_version,
            self.unicode_data
        )
    }
}

/// §13.4 environment variables: `INFY_CACHE`, `INFY_PRIVATE`, `INFY_NOSUMDB`, `INFY_TOOLCHAIN`,
/// `INFY_TARGET_DIR`, `INFY_DIAG_COLOR`, `INFY_THREADS` (§8.5).
pub fn env_str(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

/// §5.10 cache layout: `%LOCALAPPDATA%/Infinity/` on Windows, `$XDG_CACHE_HOME/infinity/` else.
pub fn cache_root() -> PathBuf {
    if let Some(p) = env_str("INFY_CACHE") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        if let Some(local) = env_str("LOCALAPPDATA") {
            return Path::new(&local).join("Infinity");
        }
        dirs_fallback().join("Infinity")
    }
    #[cfg(not(windows))]
    {
        if let Some(x) = env_str("XDG_CACHE_HOME") {
            return Path::new(&x).join("infinity");
        }
        dirs_fallback().join(".cache").join("infinity")
    }
}

fn dirs_fallback() -> PathBuf {
    env_str("HOME")
        .or_else(|| env_str("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where the toolchain keeps its own resources (`runtime/infy_rt.c`, `std/*.infy`,
/// capability tables): next to the executable, with `INFY_TOOLCHAIN` as the override and the
/// source tree as a development fallback.
pub fn resource_dir() -> PathBuf {
    if let Some(p) = env_str("INFY_TOOLCHAIN") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = env::current_exe() {
        for up in [1u32, 2, 3, 4] {
            let mut cand = exe.clone();
            for _ in 0..up {
                if !cand.pop() {
                    break;
                }
            }
            if cand.join("runtime").join("infy_rt.c").is_file() {
                return cand;
            }
        }
    }
    PathBuf::from(".")
}
