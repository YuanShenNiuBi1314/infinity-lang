//! §7.4 single-file artifact container and the §7.3 `build-id`.
//!
//! The container is `host image | INFY segment blob | 64-byte trailer`. This module is the Rust
//! side of a contract whose C side already exists: `infy_art_parse_trailer`, `parse_meta_record`
//! and `meta_resolve_strings` in `runtime/infy_rt.c`, with the byte offsets published in
//! `runtime/infy_rt_internal.h`. The offsets below mirror that enum one-for-one — the
//! `meta_offsets_match_the_frozen_header` test pins every value, so a header edit that is not
//! matched here turns a test red rather than shipping an artifact the runtime refuses to load.
//!
//! Nothing here reads the clock, walks the filesystem for entropy, or iterates a
//! randomly-seeded map: §7.3 requires byte-identical output for byte-identical input, so every
//! collection that reaches the blob is a `BTreeMap`/`Vec` in a fixed field order.

use std::collections::BTreeMap;

use crate::btrack::crc32;
use crate::sha256::Sha256;

/// §7.4: the trailer is a fixed 64 bytes at the very end of the file.
pub const TRAILER_SIZE: usize = 64;
/// Trailer magic (`INFY_ART_MAGIC`).
pub const MAGIC: &[u8; 8] = b"INFYART2";
/// `INFY_ART_HDR_VER` — the trailer layout version this writer emits.
pub const HDR_VER: u16 = 2;
/// `.inf meta` record magic (`INFY_META_MAGIC`).
pub const META_MAGIC: &[u8; 8] = b"INFYMETA";
/// `INFY_META_FIXED_BYTES`.
pub const META_FIXED_BYTES: usize = 208;
/// The six §7.4 segments, counted into `seg_count`.
pub const SEG_COUNT: u16 = 6;
/// §11.2 `EnvOffsets`. The frozen 208-byte record has no slot for it — not even a row count — so
/// the region follows `ResourceKeys` (the D-32 derivation) and prefixes its own count (D-60).
pub const ENV_COUNT_BYTES: usize = 4;
/// One `EnvOffsets` row: `(u32 name pool index, u32 offset, u32 size, u32 type tag)`.
pub const ENV_REC_BYTES: usize = 16;
/// Type tags for the fourth column. §11.2 asks for a 「类型标签」 but gives no value table, so this
/// is the §4.6 field set's own order (u32, u64, f32, str) — recorded as D-60.
pub const ENV_TY_U32: u32 = 0;
pub const ENV_TY_U64: u32 = 1;
pub const ENV_TY_F32: u32 = 2;
pub const ENV_TY_STR: u32 = 3;

pub fn env_tag_name(tag: u32) -> &'static str {
    match tag {
        ENV_TY_U64 => "u64",
        ENV_TY_F32 => "f32",
        ENV_TY_STR => "str",
        _ => "u32",
    }
}

/// Byte offsets inside the 208-byte `.inf meta` fixed record; mirrors `INFY_META_OFF_*`.
pub mod meta_off {
    pub const IFABI: usize = 8;
    pub const OPT: usize = 12;
    pub const OVERFLOW: usize = 16;
    pub const RADIUS: usize = 17;
    pub const USIZE: usize = 18;
    pub const FASTMATH: usize = 19;
    pub const LTO: usize = 20;
    pub const STACK: usize = 24;
    pub const FOCUS_STACK: usize = 28;
    pub const QUOTA: usize = 32;
    pub const ITER_GUARD: usize = 40;
    pub const N_CHAINS: usize = 48;
    pub const N_EVENTS: usize = 52;
    pub const N_NODES: usize = 56;
    pub const N_IFBC: usize = 60;
    pub const N_RESKEYS: usize = 64;
    pub const CHAINS: usize = 72;
    pub const EVENTS: usize = 80;
    pub const NODES: usize = 88;
    pub const STRINGS: usize = 96;
    pub const IFBC: usize = 104;
    pub const SUM: usize = 112;
    pub const RODATA: usize = 120;
    pub const EXPORT: usize = 128;
    pub const S_TARGET: usize = 136;
    pub const S_BUILDID: usize = 144;
    pub const S_PLATFORM: usize = 152;
    pub const S_LANGVER: usize = 160;
    pub const S_TOOLCHAIN: usize = 168;
    pub const BUILDID32: usize = 176;
}

/// §7.4 policy byte names, matching `InfyOverflow` / `InfyPanicRadius` in `infy_rt.h`.
pub const OV_PANIC: u8 = 0;
pub const OV_SATURATE: u8 = 1;
pub const RADIUS_FOCUS: u8 = 0;
pub const RADIUS_CHAIN: u8 = 1;
pub const RADIUS_PROCESS: u8 = 2;

pub fn overflow_name(v: u8) -> &'static str {
    if v == OV_SATURATE {
        "saturate"
    } else {
        "panic"
    }
}

pub fn radius_name(v: u8) -> &'static str {
    match v {
        RADIUS_PROCESS => "process",
        RADIUS_CHAIN => "chain",
        _ => "focus",
    }
}

/// `opt_level` in the frozen record is a bare `uint32_t` and neither the header nor appendix D
/// gives it a value table, so the natural one is used: the §7.2 list order (D-32).
pub fn opt_level_index(o: crate::pkg::OptLevel) -> u32 {
    match o {
        crate::pkg::OptLevel::O0 => 0,
        crate::pkg::OptLevel::O1 => 1,
        crate::pkg::OptLevel::O2 => 2,
        crate::pkg::OptLevel::O3 => 3,
        crate::pkg::OptLevel::Oz => 4,
    }
}

/// Same for `lto` (§7.2 `off | thin | fat`) — 0/1/2 in the spec's written order (D-32).
pub fn lto_index(l: crate::pkg::Lto) -> u8 {
    match l {
        crate::pkg::Lto::Off => 0,
        crate::pkg::Lto::Thin => 1,
        crate::pkg::Lto::Fat => 2,
    }
}

/// §7.3 item 3: when two builds differ, name the first segment that does.
pub fn describe_offset(bytes: &[u8], off: usize) -> String {
    let t = match parse_trailer(bytes, crate::VERSION.ifabi) {
        Ok(t) => t,
        Err(e) => return format!("差异偏移 {off}，但容器本身不可解析：{e}"),
    };
    if off < t.blob_off as usize {
        return format!("差异偏移 {off} 落在宿主镜像（.inf text / .inf rt）—— A 轨工具链输出不确定，§7.3 第 4 项").to_string();
    }
    if off >= bytes.len() - TRAILER_SIZE {
        return format!("差异偏移 {off} 落在 64 字节段尾");
    }
    let rel = off - t.blob_off as usize;
    let art = match read(bytes, crate::VERSION.ifabi) {
        Ok(a) => a,
        Err(e) => return format!("差异偏移 {off} 在 blob 内，但 .inf meta 不可解析：{e}"),
    };
    let m = &art.meta;
    let named: [(&str, Region); 11] = [
        (".inf meta", Region { off: 0, len: META_FIXED_BYTES as u32 }),
        ("字符串池", m.strings),
        ("ChainTable", m.chains),
        ("EventTable", m.events),
        ("节点表", m.nodes),
        (".inf ifbc", m.ifbc),
        (".inf sum", m.sum),
        ("入口/导出表", m.export),
        ("ResourceKeys", Region { off: m.export.off + m.export.len, len: 4 * m.n_resource_keys }),
        ("EnvOffsets", art.env_region()),
        (".inf rodata", m.rodata),
    ];
    for (name, r) in named {
        if rel >= r.off as usize && rel < (r.off + r.len) as usize {
            return format!("差异偏移 {off} 落在 {name}（blob 内 {}..{}）", r.off, r.off + r.len);
        }
    }
    format!("差异偏移 {off} 落在 blob 的区段间隙（对齐填充？）")
}

/// A `(offset, length)` pair. Regions in `.inf meta` and in the trailer are blob-relative;
/// `blob_off` in the trailer is file-relative.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub off: u32,
    pub len: u32,
}

impl Region {
    pub const EMPTY: Region = Region { off: 0, len: 0 };
}

/// The parsed 64-byte trailer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trailer {
    pub hdr_ver: u16,
    pub ifabi: u16,
    pub seg_count: u16,
    pub flags: u16,
    pub blob_off: u32,
    pub blob_len: u32,
    pub meta: Region,
    pub ifbc: Region,
    pub sum: Region,
    pub rodata: Region,
    pub crc: u32,
    pub build_id_low: u32,
}

/// The parsed `.inf meta` fixed record, with the five string fields already resolved out of the
/// blob's string pool (what `meta_resolve_strings` does on the C side).
#[derive(Clone, Debug)]
pub struct MetaRecord {
    pub ifabi: u32,
    pub opt_level: u32,
    pub overflow: u8,
    pub panic_radius: u8,
    pub usize_bits: u8,
    pub fast_math: u8,
    pub lto: u8,
    pub stack_bytes: u32,
    pub focus_stack_bytes: u32,
    pub focus_quota: u32,
    pub iter_guard_steps: u64,
    pub n_chains: u32,
    pub n_events: u32,
    pub n_nodes: u32,
    pub n_ifbc_modules: u32,
    pub n_resource_keys: u32,
    pub chains: Region,
    pub events: Region,
    pub nodes: Region,
    pub strings: Region,
    pub ifbc: Region,
    pub sum: Region,
    pub rodata: Region,
    pub export: Region,
    pub build_id32: [u8; 32],
    pub target: String,
    pub build_id: String,
    pub platform: String,
    pub lang_version: String,
    pub source_toolchain: String,
}

fn put_u32(buf: &mut Vec<u8>, at: usize, v: u32) {
    let n = at + 4;
    assert!(n <= buf.len(), "meta record write escapes the fixed record");
    buf[at..n].copy_from_slice(&v.to_le_bytes());
}

fn put_u64(buf: &mut Vec<u8>, at: usize, v: u64) {
    let n = at + 8;
    assert!(n <= buf.len());
    buf[at..n].copy_from_slice(&v.to_le_bytes());
}

fn put_region(buf: &mut Vec<u8>, at: usize, r: Region) {
    put_u32(buf, at, r.off);
    put_u32(buf, at + 4, r.len);
}

fn rd_u32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

fn rd_u64(p: &[u8]) -> u64 {
    u64::from_le_bytes([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]])
}

/// §8.4 rule 5's node-table row. The record encoding is not fixed by the spec (it names the
/// fields, not their bytes) — see `docs/spec-deviations.md` D-32: 8 × `u32`, pool-indexed names.
#[derive(Clone, Debug)]
pub struct NodeRec {
    pub chain_id: u32,
    pub node_idx: u32,
    pub name: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub linkage_count: u32,
    /// `None` is written as `0xFFFF_FFFF`: §8.4 makes `host_event_id` optional.
    pub host_event_id: Option<u32>,
}

pub const NODE_REC_BYTES: usize = 32;

impl NodeRec {
    fn bytes(&self, pool: &mut Pool) -> Vec<u8> {
        let mut v = Vec::with_capacity(NODE_REC_BYTES);
        push_u32(&mut v, self.chain_id);
        push_u32(&mut v, self.node_idx);
        push_u32(&mut v, pool.intern(&self.name));
        push_u32(&mut v, pool.intern(&self.file));
        push_u32(&mut v, self.line);
        push_u32(&mut v, self.col);
        push_u32(&mut v, self.linkage_count);
        push_u32(&mut v, self.host_event_id.unwrap_or(0xFFFF_FFFF));
        v
    }
}

/// `ChainTable` row (§8.4 / ABI §6). `chain_id` is the row index, as the runtime's FIFO ids are.
#[derive(Clone, Debug)]
pub struct ChainRec {
    pub name: String,
    pub package: String,
    pub node_count: u32,
    /// §8.2 focus-stack bytes of the chain frame (D.8 `frame_kind` is always stack in v2.0).
    pub frame_bytes: u32,
}

pub const CHAIN_REC_BYTES: usize = 20;

impl ChainRec {
    fn bytes(&self, pool: &mut Pool) -> Vec<u8> {
        let mut v = Vec::with_capacity(CHAIN_REC_BYTES);
        push_u32(&mut v, pool.intern(&self.name));
        push_u32(&mut v, pool.intern(&self.package));
        push_u32(&mut v, self.node_count);
        push_u32(&mut v, self.frame_bytes);
        push_u32(&mut v, 0); // reserved, so a future field cannot shift the row width
        v
    }
}

/// `EventTable` row (§11): the host event id **is** the row index (`HirCrate::events`).
#[derive(Clone, Debug)]
pub struct EventRec {
    pub name: String,
    pub arity: u32,
}

pub const EVENT_REC_BYTES: usize = 12;

impl EventRec {
    fn bytes(&self, pool: &mut Pool) -> Vec<u8> {
        let mut v = Vec::with_capacity(EVENT_REC_BYTES);
        push_u32(&mut v, pool.intern(&self.name));
        push_u32(&mut v, self.arity);
        push_u32(&mut v, 0);
        v
    }
}

/// `.inf sum` row (§5.8): `package@version` plus its dirhash, so the runtime self-check has the
/// manifest it needs without a network lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SumRec {
    pub package: String,
    pub version: String,
    pub dirhash: String,
}

pub const SUM_REC_BYTES: usize = 12;

impl SumRec {
    fn bytes(&self, pool: &mut Pool) -> Vec<u8> {
        let mut v = Vec::with_capacity(SUM_REC_BYTES);
        push_u32(&mut v, pool.intern(&self.package));
        push_u32(&mut v, pool.intern(&self.version));
        push_u32(&mut v, pool.intern(&self.dirhash));
        v
    }
}

/// §11.2 `EnvOffsets` row: one `env.*` name with the offset and size the host snapshot puts it at
/// plus the type tag codegen used. `name` is the §4.6 field name without the `env.` prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvRec {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub tag: u32,
}

impl EnvRec {
    fn bytes(&self, pool: &mut Pool) -> Vec<u8> {
        let mut v = Vec::with_capacity(ENV_REC_BYTES);
        push_u32(&mut v, pool.intern(&self.name));
        push_u32(&mut v, self.offset);
        push_u32(&mut v, self.size);
        push_u32(&mut v, self.tag);
        v
    }
}

fn push_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// The blob's string pool: a table of `(blob_off, len)` pairs plus the bytes they point at.
/// Interning order is first-use order over a fixed field sequence, and dedup goes through a
/// `BTreeMap` — a `HashMap` iteration order would be seeded and would break §7.3.
struct Pool {
    index: BTreeMap<String, u32>,
    order: Vec<String>,
}

impl Pool {
    fn new() -> Pool {
        Pool { index: BTreeMap::new(), order: Vec::new() }
    }
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.index.get(s) {
            return *i;
        }
        let i = self.order.len() as u32;
        self.order.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }
    /// `off` is where the payload area starts in the blob; payloads are laid out back to back.
    fn build(&self, off: u32) -> (Vec<u8>, Vec<u8>) {
        let mut pairs = Vec::with_capacity(self.order.len() * 8);
        let mut bytes = Vec::new();
        for s in &self.order {
            push_u32(&mut pairs, off + bytes.len() as u32);
            push_u32(&mut pairs, s.len() as u32);
            bytes.extend_from_slice(s.as_bytes());
        }
        (pairs, bytes)
    }
}

/// §7.4 entry rules, as recorded in the container. `kind` is 0 for an `entry_chain` and 1 for a
/// `[[bin]]` `fn main`; the name goes through the string pool so it stays stable across builds.
pub const ENTRY_CHAIN: u32 = 0;
pub const ENTRY_MAIN: u32 = 1;

/// Everything the container needs, already decided by the front end and the A-track toolchain.
#[derive(Clone, Debug)]
pub struct Content<'a> {
    /// The host image the segments get appended to (PE / ELF / Mach-O bytes).
    pub host_image: &'a [u8],
    pub ifabi: u32,
    pub opt_level: u32,
    pub overflow: u8,
    pub panic_radius: u8,
    pub usize_bits: u8,
    pub fast_math: bool,
    pub lto: u8,
    pub stack_bytes: u32,
    pub focus_stack_bytes: u32,
    pub focus_quota: u32,
    pub iter_guard_steps: u64,
    pub target_triple: &'a str,
    pub build_id: [u8; 32],
    pub platform: &'a str,
    pub lang_version: &'a str,
    pub source_toolchain: &'a str,
    /// §7.4 flags: bit 0 = `unreproducible` (§7.6 `--allow-toolchain-drift`).
    pub unreproducible: bool,
    /// `.inf ifbc`: the B-track module images, empty for a pure INFY + A-track artifact (§7.4).
    pub ifbc: &'a [u8],
    pub n_ifbc_modules: u32,
    pub chains: Vec<ChainRec>,
    pub events: Vec<EventRec>,
    pub nodes: Vec<NodeRec>,
    pub sums: Vec<SumRec>,
    /// §8.4/§12 `std.chain.lock` resource keys; the front end does not collect them yet, so the
    /// region stays declared-and-empty rather than invented (BACKLOG C3 "明确未做").
    pub resource_keys: Vec<String>,
    /// §11.2 `EnvOffsets`: the §4.6 field set as the compiler measured it for this target. Written
    /// whenever the front end has a table (production builds always do); an empty vec writes a
    /// zero-count region, which readers report as "no table" rather than as a mismatch.
    pub env_offsets: Vec<EnvRec>,
    /// §7.4 entry description: `(kind, name)` with kind 0 = `entry_chain`, 1 = `fn main`. The
    /// frozen 208-byte meta record has no entry slot, so it is written as the first record of the
    /// `export` region — `(u32 kind, u32 pool index)` (D-32).
    pub entry: Option<(u32, String)>,
}

/// Why a container was refused. Mirrors the seven `infy_art_parse_trailer` reasons plus the three
/// `.inf meta` refusals in `infy_art_read_file`, so both readers reject the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bad {
    TooShort,
    NoMagic,
    DuplicateMagic,
    Crc,
    HeaderVersion(u16),
    Ifabi(u16),
    RegionOutOfRange,
    MetaTruncated,
    MetaMagic,
    BlobCarriesMagic,
    IfabiMismatch(u32),
    SegmentRangeEscapesBlob,
    EnvTableEscapesBlob,
}

impl std::fmt::Display for Bad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Bad::TooShort => "file is shorter than the 64-byte INFY artifact trailer",
            Bad::NoMagic => "no INFYART2 trailer at end of file (not an INFY artifact)",
            Bad::DuplicateMagic => "duplicate INFYART2 trailer: the artifact was appended twice",
            Bad::Crc => "trailer_crc does not match the trailer bytes (§7.4 self-validation)",
            Bad::HeaderVersion(v) => return write!(f, "unsupported artifact header version {v}"),
            Bad::Ifabi(v) => return write!(f, "artifact IFABI {v} differs from the runtime's IFABI"),
            Bad::RegionOutOfRange => "trailer region offsets lie outside the file (§7.4)",
            Bad::MetaTruncated => ".inf meta fixed record is truncated",
            Bad::MetaMagic => ".inf meta magic mismatch",
            Bad::BlobCarriesMagic => {
                "the appended blob already contains INFYART2; the reader could not count one trailer"
            }
            Bad::IfabiMismatch(v) => {
                return write!(f, ".inf meta IFABI {v} != runtime IFABI (§11.3 step 1)")
            }
            Bad::SegmentRangeEscapesBlob => ".inf meta segment range escapes the blob",
            Bad::EnvTableEscapesBlob => {
                "the §11.2 EnvOffsets row count escapes the blob (D-60: the table is self-sized, so a count that does not fit is a broken container)"
            }
        };
        f.write_str(s)
    }
}

/// A parsed container: the trailer, the resolved `.inf meta` record, and the region slices.
pub struct Artifact<'a> {
    pub trailer: Trailer,
    pub meta: MetaRecord,
    pub blob: &'a [u8],
    /// `(off, len)` pairs of the blob string pool, blob-relative offsets.
    pub pool: Vec<(u32, u32)>,
}

impl<'a> Artifact<'a> {
    pub fn region_bytes(&self, r: Region) -> &'a [u8] {
        &self.blob[r.off as usize..(r.off + r.len) as usize]
    }
    /// §8.4 rule 5's node table, decoded back from the blob.
    pub fn nodes(&self) -> Vec<NodeRec> {
        let b = self.region_bytes(self.meta.nodes);
        b.chunks(NODE_REC_BYTES)
            .map(|c| NodeRec {
                chain_id: rd_u32(&c[0..4]),
                node_idx: rd_u32(&c[4..8]),
                name: self.pool_string(rd_u32(&c[8..12])),
                file: self.pool_string(rd_u32(&c[12..16])),
                line: rd_u32(&c[16..20]),
                col: rd_u32(&c[20..24]),
                linkage_count: rd_u32(&c[24..28]),
                host_event_id: match rd_u32(&c[28..32]) {
                    0xFFFF_FFFF => None,
                    v => Some(v),
                },
            })
            .collect()
    }
    /// `.inf sum` rows decoded back from the blob (§5.8's dependency dirhash list — the read side
    /// of what stage [9] wrote from `lock.dirh`). A trailing short chunk is dropped, not guessed
    /// at: `infy artifact verify` compares the row count against the region length and refuses the
    /// container when the two disagree.
    pub fn sums(&self) -> Vec<SumRec> {
        self.region_bytes(self.meta.sum)
            .chunks(SUM_REC_BYTES)
            .filter(|c| c.len() == SUM_REC_BYTES)
            .map(|c| SumRec {
                package: self.pool_string(rd_u32(&c[0..4])),
                version: self.pool_string(rd_u32(&c[4..8])),
                dirhash: self.pool_string(rd_u32(&c[8..12])),
            })
            .collect()
    }
    pub fn pool_string(&self, idx: u32) -> String {
        let (off, len) = match self.pool.get(idx as usize) {
            Some(p) => *p,
            // Same fallback as `meta_resolve_strings`: an out-of-range index reads as empty.
            None => return String::new(),
        };
        let end = (off as usize + len as usize).min(self.blob.len());
        String::from_utf8_lossy(&self.blob[off as usize..end]).to_string()
    }
    /// `ResourceKeys` rows: the `n_resource_keys` u32 pool indices right after the export region
    /// (no slot in the frozen meta record — D-32).
    pub fn resource_keys(&self) -> Vec<String> {
        let off = self.meta.export.off + self.meta.export.len;
        let len = (4 * self.meta.n_resource_keys) as usize;
        let end = (off as usize + len).min(self.blob.len());
        self.blob[off as usize..end]
            .chunks(4)
            .filter(|c| c.len() == 4)
            .map(|c| self.pool_string(rd_u32(c)))
            .collect()
    }
    /// `(off, len)` of the `EnvOffsets` region: it starts where `ResourceKeys` ends (D-60), and its
    /// length is its own count column, so a container without a table reads as a 4-byte region.
    pub fn env_region(&self) -> Region {
        let off = self.meta.export.off + self.meta.export.len + 4 * self.meta.n_resource_keys;
        let at = off as usize;
        if at + ENV_COUNT_BYTES > self.blob.len() {
            return Region { off, len: 0 };
        }
        let n = rd_u32(&self.blob[at..at + ENV_COUNT_BYTES]) as usize;
        Region { off, len: (ENV_COUNT_BYTES + n * ENV_REC_BYTES) as u32 }
    }
    /// §11.2 `EnvOffsets` rows. A count that promises more bytes than the blob holds yields the
    /// rows that do fit — a truncated tail is dropped, never guessed at (same rule as `sums()`).
    pub fn env_offsets(&self) -> Vec<EnvRec> {
        let r = self.env_region();
        if r.len == 0 || (r.off as usize) + ENV_COUNT_BYTES > self.blob.len() {
            return Vec::new();
        }
        let n = rd_u32(&self.blob[r.off as usize..r.off as usize + ENV_COUNT_BYTES]) as usize;
        let body_at = r.off as usize + ENV_COUNT_BYTES;
        self.blob[body_at..]
            .chunks(ENV_REC_BYTES)
            .take(n)
            .filter(|c| c.len() == ENV_REC_BYTES)
            .map(|c| EnvRec {
                name: self.pool_string(rd_u32(&c[0..4])),
                offset: rd_u32(&c[4..8]),
                size: rd_u32(&c[8..12]),
                tag: rd_u32(&c[12..16]),
            })
            .collect()
    }
    /// §7.4's entry description, read from the first record of the export region.
    pub fn entry(&self) -> Option<(u32, String)> {
        if self.meta.export.len < 8 {
            return None;
        }
        let b = self.region_bytes(self.meta.export);
        Some((rd_u32(&b[0..4]), self.pool_string(rd_u32(&b[4..8]))))
    }
}

/// Assemble the container: `host image | blob | trailer`.
///
/// The blob layout is `meta | pool pairs | chains | events | nodes | ifbc | sum | export |
/// resource keys | env offsets | rodata`, so every region offset falls out of the lengths before
/// it — no back-patching, which is also why two builds of the same content cannot differ by
/// layout luck.
pub fn write(c: &Content) -> Result<Vec<u8>, Bad> {
    let mut pool = Pool::new();
    // Intern in a fixed field order so indices are a function of content only.
    let idx_target = pool.intern(c.target_triple);
    let hex_id = hex_of(&c.build_id);
    let idx_build = pool.intern(&hex_id);
    let idx_platform = pool.intern(c.platform);
    let idx_lang = pool.intern(c.lang_version);
    let idx_tool = pool.intern(c.source_toolchain);
    let entry_name_idx = c.entry.as_ref().map(|(_, name)| pool.intern(name));
    let chain_bytes: Vec<u8> = c.chains.iter().flat_map(|ch| ch.bytes(&mut pool)).collect();
    let event_bytes: Vec<u8> = c.events.iter().flat_map(|e| e.bytes(&mut pool)).collect();
    let node_bytes: Vec<u8> = c.nodes.iter().flat_map(|n| n.bytes(&mut pool)).collect();
    let sum_bytes: Vec<u8> = c.sums.iter().flat_map(|s| s.bytes(&mut pool)).collect();
    let mut reskey_bytes: Vec<u8> = Vec::new();
    for k in &c.resource_keys {
        let idx = pool.intern(k);
        push_u32(&mut reskey_bytes, idx);
    }
    let mut env_bytes: Vec<u8> = Vec::with_capacity(ENV_COUNT_BYTES + c.env_offsets.len() * ENV_REC_BYTES);
    push_u32(&mut env_bytes, c.env_offsets.len() as u32);
    for e in &c.env_offsets {
        env_bytes.extend_from_slice(&e.bytes(&mut pool));
    }
    let export: Vec<u8> = match (c.entry.as_ref(), entry_name_idx) {
        (Some((kind, _)), Some(idx)) => {
            let mut v = Vec::with_capacity(8);
            push_u32(&mut v, *kind);
            push_u32(&mut v, idx);
            v
        }
        _ => Vec::new(),
    };

    let pairs_at = META_FIXED_BYTES as u32;
    let pairs_len = (pool.order.len() * 8) as u32;
    let mut cursor = (pairs_at + pairs_len) as usize;
    let mut region_of = |data: &[u8]| -> Region {
        let r = Region { off: cursor as u32, len: data.len() as u32 };
        cursor += data.len();
        r
    };
    let r_chains = region_of(&chain_bytes);
    let r_events = region_of(&event_bytes);
    let r_nodes = region_of(&node_bytes);
    let r_ifbc = region_of(c.ifbc);
    let r_sum = region_of(&sum_bytes);
    let r_export = region_of(&export);
    // The `ResourceKeys` table advances the cursor but has no slot in the frozen meta record;
    // readers derive it as `export.off + export.len` for `4 * n_resource_keys` bytes (D-32).
    let _ = region_of(&reskey_bytes);
    // `EnvOffsets` follows it and derives the same way, self-sized by its own count column (D-60).
    let _ = region_of(&env_bytes);
    let (pair_bytes, str_bytes) = pool.build(cursor as u32);
    let r_rodata = Region { off: cursor as u32, len: str_bytes.len() as u32 };
    let r_strings = Region { off: pairs_at, len: pairs_len };
    let blob_len = (cursor + str_bytes.len()) as u32;
    let meta_len = META_FIXED_BYTES as u32;

    let mut meta = vec![0u8; META_FIXED_BYTES];
    meta[0..8].copy_from_slice(META_MAGIC);
    put_u32(&mut meta, meta_off::IFABI, c.ifabi);
    put_u32(&mut meta, meta_off::OPT, c.opt_level);
    meta[meta_off::OVERFLOW] = c.overflow;
    meta[meta_off::RADIUS] = c.panic_radius;
    meta[meta_off::USIZE] = c.usize_bits;
    meta[meta_off::FASTMATH] = c.fast_math as u8;
    meta[meta_off::LTO] = c.lto;
    put_u32(&mut meta, meta_off::STACK, c.stack_bytes);
    put_u32(&mut meta, meta_off::FOCUS_STACK, c.focus_stack_bytes);
    put_u32(&mut meta, meta_off::QUOTA, c.focus_quota);
    put_u64(&mut meta, meta_off::ITER_GUARD, c.iter_guard_steps);
    put_u32(&mut meta, meta_off::N_CHAINS, c.chains.len() as u32);
    put_u32(&mut meta, meta_off::N_EVENTS, c.events.len() as u32);
    put_u32(&mut meta, meta_off::N_NODES, c.nodes.len() as u32);
    put_u32(&mut meta, meta_off::N_IFBC, c.n_ifbc_modules);
    put_u32(&mut meta, meta_off::N_RESKEYS, c.resource_keys.len() as u32);
    put_region(&mut meta, meta_off::CHAINS, r_chains);
    put_region(&mut meta, meta_off::EVENTS, r_events);
    put_region(&mut meta, meta_off::NODES, r_nodes);
    put_region(&mut meta, meta_off::STRINGS, r_strings);
    put_region(&mut meta, meta_off::IFBC, r_ifbc);
    put_region(&mut meta, meta_off::SUM, r_sum);
    put_region(&mut meta, meta_off::RODATA, r_rodata);
    put_region(&mut meta, meta_off::EXPORT, r_export);
    put_region(&mut meta, meta_off::S_TARGET, Region { off: idx_target, len: 0 });
    put_region(&mut meta, meta_off::S_BUILDID, Region { off: idx_build, len: 0 });
    put_region(&mut meta, meta_off::S_PLATFORM, Region { off: idx_platform, len: 0 });
    put_region(&mut meta, meta_off::S_LANGVER, Region { off: idx_lang, len: 0 });
    put_region(&mut meta, meta_off::S_TOOLCHAIN, Region { off: idx_tool, len: 0 });
    meta[meta_off::BUILDID32..meta_off::BUILDID32 + 32].copy_from_slice(&c.build_id);

    let mut blob = Vec::new();
    blob.extend_from_slice(&meta);
    blob.extend_from_slice(&pair_bytes);
    blob.extend_from_slice(&chain_bytes);
    blob.extend_from_slice(&event_bytes);
    blob.extend_from_slice(&node_bytes);
    blob.extend_from_slice(c.ifbc);
    blob.extend_from_slice(&sum_bytes);
    blob.extend_from_slice(&export);
    blob.extend_from_slice(&reskey_bytes);
    blob.extend_from_slice(&env_bytes);
    blob.extend_from_slice(&str_bytes);
    debug_assert_eq!(blob.len(), blob_len as usize);
    // The reader counts trailers over the appended area only (D-33), so the blob must not carry a
    // second copy of the magic — e.g. a source string literal that happens to be `INFYART2`.
    if blob.windows(MAGIC.len()).any(|w| w == MAGIC) {
        return Err(Bad::BlobCarriesMagic);
    }

    let mut tr = vec![0u8; TRAILER_SIZE];
    tr[0..8].copy_from_slice(MAGIC);
    tr[8..10].copy_from_slice(&HDR_VER.to_le_bytes());
    tr[10..12].copy_from_slice(&(c.ifabi as u16).to_le_bytes());
    tr[12..14].copy_from_slice(&SEG_COUNT.to_le_bytes());
    tr[14..16].copy_from_slice(&(c.unreproducible as u16).to_le_bytes());
    let blob_off = c.host_image.len() as u32;
    let blob_len = blob.len() as u32;
    put_u32(&mut tr, 16, blob_off);
    put_u32(&mut tr, 20, blob_len);
    put_region(&mut tr, 24, Region { off: 0, len: meta_len });
    put_region(&mut tr, 32, r_ifbc);
    put_region(&mut tr, 40, r_sum);
    put_region(&mut tr, 48, r_rodata);
    let crc = crc32(&tr[..56]);
    put_u32(&mut tr, 56, crc);
    put_u32(&mut tr, 60, rd_u32(&c.build_id[0..4]));

    let mut out = Vec::with_capacity(c.host_image.len() + blob.len() + TRAILER_SIZE);
    out.extend_from_slice(c.host_image);
    out.extend_from_slice(&blob);
    out.extend_from_slice(&tr);
    Ok(out)
}

pub fn hex_of(b: &[u8; 32]) -> String {    let mut s = String::with_capacity(64);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Read the 64-byte trailer only (same order of checks as `infy_art_parse_trailer`).
pub fn parse_trailer(file: &[u8], runtime_ifabi: u32) -> Result<Trailer, Bad> {
    if file.len() < TRAILER_SIZE {
        return Err(Bad::TooShort);
    }
    let tr = &file[file.len() - TRAILER_SIZE..];
    if &tr[0..8] != MAGIC {
        return Err(Bad::NoMagic);
    }
    let hdr_ver = u16::from_le_bytes([tr[8], tr[9]]);
    let ifabi = u16::from_le_bytes([tr[10], tr[11]]);
    let t = Trailer {
        hdr_ver,
        ifabi,
        seg_count: u16::from_le_bytes([tr[12], tr[13]]),
        flags: u16::from_le_bytes([tr[14], tr[15]]),
        blob_off: rd_u32(&tr[16..20]),
        blob_len: rd_u32(&tr[20..24]),
        meta: Region { off: rd_u32(&tr[24..28]), len: rd_u32(&tr[28..32]) },
        ifbc: Region { off: rd_u32(&tr[32..36]), len: rd_u32(&tr[36..40]) },
        sum: Region { off: rd_u32(&tr[40..44]), len: rd_u32(&tr[44..48]) },
        rodata: Region { off: rd_u32(&tr[48..52]), len: rd_u32(&tr[52..56]) },
        crc: rd_u32(&tr[56..60]),
        build_id_low: rd_u32(&tr[60..64]),
    };
    // The blob range has to be sane before the duplicate-trailer scan can bound itself (D-33).
    let blob_end = t.blob_off as u64 + t.blob_len as u64;
    if blob_end > file.len() as u64 - TRAILER_SIZE as u64 {
        return Err(Bad::RegionOutOfRange);
    }
    // §7.4: a duplicate trailer means the container was appended twice. The scan is bounded to the
    // appended area, *not* the whole file, because the embedded runtime keeps `"INFYART2"` as a
    // string constant — a whole-file count finds that copy too and rejects every real artifact.
    if file[t.blob_off as usize..].windows(8).filter(|w| w == MAGIC).count() > 1 {
        return Err(Bad::DuplicateMagic);
    }
    if crc32(&tr[..56]) != t.crc {
        return Err(Bad::Crc);
    }
    if t.hdr_ver != HDR_VER {
        return Err(Bad::HeaderVersion(t.hdr_ver));
    }
    if t.ifabi as u32 != runtime_ifabi {
        return Err(Bad::Ifabi(t.ifabi));
    }
    for r in [t.meta, t.ifbc, t.sum, t.rodata] {
        if r.off as u64 + r.len as u64 > t.blob_len as u64 {
            return Err(Bad::RegionOutOfRange);
        }
    }
    Ok(t)
}

/// Parse a whole container: trailer, `.inf meta`, and the resolved string pool.
pub fn read(file: &[u8], runtime_ifabi: u32) -> Result<Artifact<'_>, Bad> {
    let t = parse_trailer(file, runtime_ifabi)?;
    let blob = &file[t.blob_off as usize..(t.blob_off + t.blob_len) as usize];
    let m = &blob[t.meta.off as usize..(t.meta.off + t.meta.len) as usize];
    if m.len() < META_FIXED_BYTES {
        return Err(Bad::MetaTruncated);
    }
    if &m[0..8] != META_MAGIC {
        return Err(Bad::MetaMagic);
    }
    let region = |at: usize| Region { off: rd_u32(&m[at..at + 4]), len: rd_u32(&m[at + 4..at + 8]) };
    let mut meta = MetaRecord {
        ifabi: rd_u32(&m[meta_off::IFABI..]),
        opt_level: rd_u32(&m[meta_off::OPT..]),
        overflow: m[meta_off::OVERFLOW],
        panic_radius: m[meta_off::RADIUS],
        usize_bits: m[meta_off::USIZE],
        fast_math: m[meta_off::FASTMATH],
        lto: m[meta_off::LTO],
        stack_bytes: rd_u32(&m[meta_off::STACK..]),
        focus_stack_bytes: rd_u32(&m[meta_off::FOCUS_STACK..]),
        focus_quota: rd_u32(&m[meta_off::QUOTA..]),
        iter_guard_steps: rd_u64(&m[meta_off::ITER_GUARD..]),
        n_chains: rd_u32(&m[meta_off::N_CHAINS..]),
        n_events: rd_u32(&m[meta_off::N_EVENTS..]),
        n_nodes: rd_u32(&m[meta_off::N_NODES..]),
        n_ifbc_modules: rd_u32(&m[meta_off::N_IFBC..]),
        n_resource_keys: rd_u32(&m[meta_off::N_RESKEYS..]),
        chains: region(meta_off::CHAINS),
        events: region(meta_off::EVENTS),
        nodes: region(meta_off::NODES),
        strings: region(meta_off::STRINGS),
        ifbc: region(meta_off::IFBC),
        sum: region(meta_off::SUM),
        rodata: region(meta_off::RODATA),
        export: region(meta_off::EXPORT),
        build_id32: {
            let mut b = [0u8; 32];
            b.copy_from_slice(&m[meta_off::BUILDID32..meta_off::BUILDID32 + 32]);
            b
        },
        target: String::new(),
        build_id: String::new(),
        platform: String::new(),
        lang_version: String::new(),
        source_toolchain: String::new(),
    };
    // The range check `infy_art_read_file` performs before handing the blob out.
    for r in [
        meta.strings,
        meta.chains,
        meta.nodes,
        meta.ifbc,
        meta.sum,
        meta.export,
        meta.events,
        meta.rodata,
    ] {
        if r.off as u64 + r.len as u64 > t.blob_len as u64 {
            return Err(Bad::SegmentRangeEscapesBlob);
        }
    }
    // §11.2 `EnvOffsets` has no meta slot either, so its declared row count is the only thing that
    // bounds it (D-60): a count that does not fit the blob refuses the container instead of being
    // read as a shorter table — the same rule `.inf sum`'s verifier applies to its row count.
    {
        let at = (meta.export.off as usize + meta.export.len as usize) + 4 * meta.n_resource_keys as usize;
        if at + ENV_COUNT_BYTES <= blob.len() {
            let n = rd_u32(&blob[at..at + ENV_COUNT_BYTES]) as u64;
            if at as u64 + ENV_COUNT_BYTES as u64 + n * ENV_REC_BYTES as u64 > t.blob_len as u64 {
                return Err(Bad::EnvTableEscapesBlob);
            }
        }
    }
    let pairs = &blob[meta.strings.off as usize..(meta.strings.off + meta.strings.len) as usize];
    let pool: Vec<(u32, u32)> = pairs
        .chunks(8)
        .filter(|c| c.len() == 8)
        .map(|c| (rd_u32(&c[0..4]), rd_u32(&c[4..8])))
        .collect();
    let at = |idx: u32| -> String {
        match pool.get(idx as usize) {
            Some((off, len)) => {
                let end = (*off as usize + *len as usize).min(blob.len());
                String::from_utf8_lossy(&blob[*off as usize..end]).to_string()
            }
            None => String::new(),
        }
    };
    meta.target = at(rd_u32(&m[meta_off::S_TARGET..]));
    meta.build_id = at(rd_u32(&m[meta_off::S_BUILDID..]));
    meta.platform = at(rd_u32(&m[meta_off::S_PLATFORM..]));
    meta.lang_version = at(rd_u32(&m[meta_off::S_LANGVER..]));
    meta.source_toolchain = at(rd_u32(&m[meta_off::S_TOOLCHAIN..]));
    Ok(Artifact { trailer: t, meta, blob, pool })
}

/// `infy artifact info` text — the same key order `infy_art_dump_meta` prints on the C side, so a
/// human can diff the two renderings.
pub fn dump_info(a: &Artifact) -> String {
    let m = &a.meta;
    let mut o = String::new();
    o.push_str("INFY artifact (.inf meta)\n");
    let mut line = |k: &str, v: String| o.push_str(&format!("{k:<18} {v}\n"));
    line("ifabi", m.ifabi.to_string());
    line("opt_level", m.opt_level.to_string());
    line("overflow", overflow_name(m.overflow).to_string());
    line("panic_radius", radius_name(m.panic_radius).to_string());
    line("usize_bits", m.usize_bits.to_string());
    line("fast_math", if m.fast_math != 0 { "true" } else { "false" }.to_string());
    line("lto", m.lto.to_string());
    line("stack_bytes", m.stack_bytes.to_string());
    line("focus_stack_bytes", m.focus_stack_bytes.to_string());
    line("focus_quota", m.focus_quota.to_string());
    line("iter_guard_steps", m.iter_guard_steps.to_string());
    line("target", m.target.clone());
    line("platform", m.platform.clone());
    line("build_id", m.build_id.clone());
    line("lang_version", m.lang_version.clone());
    line("source_toolchain", m.source_toolchain.clone());
    line("n_chains", m.n_chains.to_string());
    line("n_events", m.n_events.to_string());
    line("n_nodes", m.n_nodes.to_string());
    line("n_ifbc_modules", m.n_ifbc_modules.to_string());
    line("n_resource_keys", m.n_resource_keys.to_string());
    // §11.2: `infy artifact info` shows the whole `EnvOffsets` table — the §4.6 offsets are the
    // reason the runtime can refuse a host snapshot (§11.3 step 3), so they are not a debug detail.
    let env = a.env_offsets();
    line("n_env_offsets", env.len().to_string());
    for e in &env {
        line(
            &format!("env.{}", e.name),
            format!("off={} size={} {}", e.offset, e.size, env_tag_name(e.tag)),
        );
    }
    line(
        "entry",
        match a.entry() {
            Some((ENTRY_MAIN, name)) => format!("fn main `{name}`"),
            Some((kind, name)) => format!("chain `{name}` (kind {kind})"),
            None => "-".to_string(),
        },
    );
    line(
        "unreproducible",
        if a.trailer.flags != 0 { "true (§7.6 --allow-toolchain-drift)".to_string() } else { "false".to_string() },
    );
    let mut bid = String::new();
    for b in &m.build_id32 {
        bid.push_str(&format!("{b:02x}"));
    }
    line("build_id[32]", bid);
    o
}

/// `infy artifact sections` text: the §7.4 segment map.
pub fn dump_sections(a: &Artifact) -> String {
    fn seg(name: &str, r: Region) -> String {
        format!("{name:<18} off={:<10} len={}\n", r.off, r.len)
    }
    let m = &a.meta;
    let t = &a.trailer;
    let mut o = String::new();
    o.push_str(&format!(
        "host image {} byte(s) | blob {} byte(s) @ {} | trailer {TRAILER_SIZE} byte(s)\n",
        t.blob_off, t.blob_len, t.blob_off
    ));
    o.push_str(&format!("seg_count {} hdr_ver {} ifabi {}\n", t.seg_count, t.hdr_ver, t.ifabi));
    o.push_str("segments (blob-relative)\n");
    o.push_str(&seg("meta", Region { off: 0, len: META_FIXED_BYTES as u32 }));
    o.push_str(&seg("chains", m.chains));
    o.push_str(&seg("events", m.events));
    o.push_str(&seg("nodes", m.nodes));
    o.push_str(&seg("strings", m.strings));
    o.push_str(&seg("ifbc", m.ifbc));
    o.push_str(&seg("sum", m.sum));
    o.push_str(&seg("export", m.export));
    // `ResourceKeys` has no offset slot in the frozen meta record, so its place follows the blob
    // layout rule: the `n_resource_keys` u32 pool indices right after `export` (D-32).
    o.push_str(&seg("resource_keys", Region { off: m.export.off + m.export.len, len: 4 * m.n_resource_keys }));
    o.push_str(&seg("env_offsets", a.env_region()));
    o.push_str(&seg("rodata", m.rodata));
    o.push_str("trailer regions\n");
    o.push_str(&seg(".inf ifbc", t.ifbc));
    o.push_str(&seg(".inf sum", t.sum));
    o.push_str(&seg(".inf rodata", t.rodata));
    o
}

/// §8.4 rule 5's node table, as `infy artifact info --nodes` prints it.
pub fn dump_nodes(a: &Artifact) -> String {
    let mut o = String::new();
    o.push_str("chain_id\tnode_idx\tfile\tline\tcol\tlinkage\thost_event_id\tname\n");
    for n in a.nodes() {
        o.push_str(&format!(
            "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            n.chain_id,
            n.node_idx,
            n.file,
            n.line,
            n.col,
            n.linkage_count,
            match n.host_event_id {
                Some(id) => id.to_string(),
                None => "-".to_string(),
            },
            n.name,
        ));
    }
    o
}

// ---------------------------------------------------------------- §7.3 build identity

/// The normalised input set of §7.3 item 1 / ABI §7. Ordered; no timestamps, no absolute paths,
/// no usernames.
#[derive(Clone, Debug)]
pub struct BuildInputs<'a> {
    /// Source dirhash per package (`h1:<base64>`, §5.8 item 1) in §5.4 order.
    pub dirhashes: &'a [String],
    /// The rendered `MODULE.infy.lock` (§5.7).
    pub lock: &'a str,
    /// The rendered `MODULE.infy.toml` (§5.2).
    pub manifest: &'a str,
    pub target: &'a str,
    pub opt_level: &'a str,
    pub profile: &'a str,
    /// A-track compiler and B-track interpreter version strings (§7.6.2 / §7.7.3).
    pub toolchains: &'a [String],
    pub ifabi: u32,
    pub compiler: &'a str,
}

fn feed(h: &mut Sha256, s: &str) {
    h.update(&(s.len() as u32).to_le_bytes());
    h.update(s.as_bytes());
}

/// `build-id = SHA-256(normalised inputs)`, each input length-prefixed so a boundary between two
/// fields can never be moved without changing the digest.
pub fn build_id(i: &BuildInputs) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"INFY-BUILDID-1");
    for d in i.dirhashes {
        feed(&mut h, d);
    }
    feed(&mut h, i.lock);
    feed(&mut h, i.manifest);
    feed(&mut h, i.target);
    feed(&mut h, i.opt_level);
    feed(&mut h, i.profile);
    h.update(&(i.toolchains.len() as u32).to_le_bytes());
    for t in i.toolchains {
        feed(&mut h, t);
    }
    h.update(&i.ifabi.to_le_bytes());
    feed(&mut h, i.compiler);
    h.finish()
}

pub fn build_id_hex(i: &BuildInputs) -> String {
    hex_of(&build_id(i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content<'a>(image: &'a [u8]) -> Content<'a> {
        Content {
            host_image: image,
            ifabi: crate::VERSION.ifabi,
            opt_level: 2,
            overflow: OV_PANIC,
            panic_radius: RADIUS_FOCUS,
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
            ifbc: &[],
            n_ifbc_modules: 0,
            chains: vec![ChainRec {
                name: "Pipeline".into(),
                package: "tour/m".into(),
                node_count: 6,
                frame_bytes: 4,
            }],
            events: vec![],
            nodes: vec![
                NodeRec {
                    chain_id: 0,
                    node_idx: 0,
                    name: "gate".into(),
                    file: "tour/m/mod.infy".into(),
                    line: 8,
                    col: 1,
                    linkage_count: 0,
                    host_event_id: None,
                },
                NodeRec {
                    chain_id: 0,
                    node_idx: 3,
                    name: "run".into(),
                    file: "tour/m/mod.infy".into(),
                    line: 11,
                    col: 1,
                    linkage_count: 1,
                    host_event_id: Some(7),
                },
            ],
            sums: vec![SumRec {
                package: "dep/core".into(),
                version: "1.2.3".into(),
                dirhash: "h1:AAAA".into(),
            }],
            resource_keys: vec!["std.chain.lock:db".into()],
            env_offsets: env_rows(),
            entry: Some((ENTRY_CHAIN, "Pipeline".into())),
        }
    }

    /// §4.6's closed field set as `hir::env_table(Target::REFERENCE)` measures it — the same eleven
    /// offsets `runtime/infy_rt.h` fixes and `hir::tests` golden-tests. The checked-in C fixture
    /// carries them, so the cross-language lock point covers `EnvOffsets` too (F17).
    fn env_rows() -> Vec<EnvRec> {
        let rows: [(&str, u32, u32, u32); 11] = [
            ("iter", 0, 4, ENV_TY_U32),
            ("runtime_s", 4, 4, ENV_TY_U32),
            ("realtime", 8, 8, ENV_TY_U64),
            ("ram_mb", 16, 4, ENV_TY_U32),
            ("cpu_use", 20, 4, ENV_TY_F32),
            ("gpu_use", 24, 4, ENV_TY_F32),
            ("chain_id", 28, 4, ENV_TY_U32),
            ("node_idx", 32, 4, ENV_TY_U32),
            ("focus_id", 36, 4, ENV_TY_U32),
            ("platform", 40, 16, ENV_TY_STR),
            ("build_id", 56, 16, ENV_TY_STR),
        ];
        rows.iter()
            .map(|(name, offset, size, tag)| EnvRec { name: (*name).into(), offset: *offset, size: *size, tag: *tag })
            .collect()
    }

    /// §7.4 / `runtime/infy_rt_internal.h`: the 208-byte meta record is a frozen layout shared with
    /// the C reader. Read the header's own `INFY_META_OFF_*` enum back and compare, so a header edit
    /// that is not mirrored here fails a test instead of shipping an artifact the runtime rejects.
    #[test]
    fn meta_offsets_and_sizes_match_the_frozen_header() {
        let hdr = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/infy_rt_internal.h"))
            .expect("runtime/infy_rt_internal.h");
        let start = hdr.find("INFY_META_OFF_IFABI = 8").expect("the meta offset enum");
        let end = hdr[start..].find('}').expect("enum close") + start;
        let body = &hdr[start..end];
        let mut table: BTreeMap<String, u32> = BTreeMap::new();
        for item in body.split(',') {
            let name = item.split('=').next().unwrap_or("").trim().to_string();
            if let Some(v) = item.split('=').nth(1) {
                if let Ok(n) = v.trim().parse::<u32>() {
                    table.insert(name, n);
                }
            }
        }
        let want: [(&str, usize); 30] = [
            ("INFY_META_OFF_IFABI", meta_off::IFABI),
            ("INFY_META_OFF_OPT", meta_off::OPT),
            ("INFY_META_OFF_OVERFLOW", meta_off::OVERFLOW),
            ("INFY_META_OFF_RADIUS", meta_off::RADIUS),
            ("INFY_META_OFF_USIZE", meta_off::USIZE),
            ("INFY_META_OFF_FASTMATH", meta_off::FASTMATH),
            ("INFY_META_OFF_LTO", meta_off::LTO),
            ("INFY_META_OFF_STACK", meta_off::STACK),
            ("INFY_META_OFF_FOCUS_STACK", meta_off::FOCUS_STACK),
            ("INFY_META_OFF_QUOTA", meta_off::QUOTA),
            ("INFY_META_OFF_ITER_GUARD", meta_off::ITER_GUARD),
            ("INFY_META_OFF_N_CHAINS", meta_off::N_CHAINS),
            ("INFY_META_OFF_N_EVENTS", meta_off::N_EVENTS),
            ("INFY_META_OFF_N_NODES", meta_off::N_NODES),
            ("INFY_META_OFF_N_IFBC", meta_off::N_IFBC),
            ("INFY_META_OFF_N_RESKEYS", meta_off::N_RESKEYS),
            ("INFY_META_OFF_CHAINS", meta_off::CHAINS),
            ("INFY_META_OFF_EVENTS", meta_off::EVENTS),
            ("INFY_META_OFF_NODES", meta_off::NODES),
            ("INFY_META_OFF_STRINGS", meta_off::STRINGS),
            ("INFY_META_OFF_IFBC", meta_off::IFBC),
            ("INFY_META_OFF_SUM", meta_off::SUM),
            ("INFY_META_OFF_RODATA", meta_off::RODATA),
            ("INFY_META_OFF_EXPORT", meta_off::EXPORT),
            ("INFY_META_OFF_S_TARGET", meta_off::S_TARGET),
            ("INFY_META_OFF_S_BUILDID", meta_off::S_BUILDID),
            ("INFY_META_OFF_S_PLATFORM", meta_off::S_PLATFORM),
            ("INFY_META_OFF_S_LANGVER", meta_off::S_LANGVER),
            ("INFY_META_OFF_S_TOOLCHAIN", meta_off::S_TOOLCHAIN),
            ("INFY_META_OFF_BUILDID32", meta_off::BUILDID32),
        ];
        for (name, mine) in want {
            let theirs = *table.get(name).unwrap_or_else(|| panic!("{name} missing from the header enum"));
            assert_eq!(theirs as usize, mine, "{name}");
        }
        assert_eq!(META_FIXED_BYTES as u32, 208, "INFY_META_FIXED_BYTES");
        assert!(hdr.contains("#define INFY_META_FIXED_BYTES 208u"));
        assert!(hdr.contains("#define INFY_ART_TRAILER_SIZE 64u"));
        assert!(hdr.contains("#define INFY_ART_HDR_VER      2u"));
        assert_eq!(TRAILER_SIZE, 64);
        assert_eq!(HDR_VER, 2);
        assert!(hdr.contains("#define INFY_META_MAGIC \"INFYMETA\""));
        let h = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/infy_rt.h"))
            .expect("runtime/infy_rt.h");
        assert!(h.contains("#define INFY_ART_MAGIC \"INFYART2\""));
        assert!(h.contains("#define INFY_IFABI 2u"));
        assert_eq!(MAGIC, b"INFYART2");
        assert_eq!(META_MAGIC, b"INFYMETA");
    }

    /// §7.4: every field the writer puts in must come back out of the reader.
    #[test]
    fn written_container_reads_back_every_field() {
        let image = b"MZ fake host image".to_vec();
        let bytes = write(&content(&image)).expect("write");
        assert_eq!(&bytes[..image.len()], &image[..], "the host image stays byte-identical");
        let a = read(&bytes, crate::VERSION.ifabi).expect("read");
        assert_eq!(a.trailer.hdr_ver, HDR_VER);
        assert_eq!(a.trailer.ifabi as u32, crate::VERSION.ifabi);
        assert_eq!(a.trailer.seg_count, SEG_COUNT);
        assert_eq!(a.trailer.blob_off as usize, image.len());
        assert_eq!(a.meta.ifabi, crate::VERSION.ifabi);
        assert_eq!(a.meta.opt_level, 2);
        assert_eq!(a.meta.overflow, OV_PANIC);
        assert_eq!(a.meta.panic_radius, RADIUS_FOCUS);
        assert_eq!(a.meta.usize_bits, 64);
        assert_eq!(a.meta.fast_math, 0);
        assert_eq!(a.meta.lto, 1);
        assert_eq!(a.meta.stack_bytes, 8 * 1024 * 1024);
        assert_eq!(a.meta.focus_stack_bytes, 256 * 1024);
        assert_eq!(a.meta.iter_guard_steps, 1 << 32);
        assert_eq!(a.meta.target, "x86_64-pc-windows-gnu");
        assert_eq!(a.meta.build_id, hex_of(&[0xAB; 32]));
        assert_eq!(a.meta.platform, "windows");
        assert_eq!(a.meta.lang_version, "2.0");
        assert_eq!(a.meta.source_toolchain, "gcc 14.2.0");
        assert_eq!(a.meta.build_id32, [0xAB; 32]);
        assert_eq!(a.trailer.build_id_low, u32::from_le_bytes([0xAB; 4]));
        assert_eq!(a.meta.n_chains, 1);
        assert_eq!(a.meta.n_nodes, 2);
        assert_eq!(a.meta.n_resource_keys, 1);
        assert_eq!(a.entry(), Some((ENTRY_CHAIN, "Pipeline".to_string())));
        assert_eq!(a.resource_keys(), vec!["std.chain.lock:db".to_string()]);
        assert_eq!(a.region_bytes(a.meta.sum).len(), SUM_REC_BYTES);
    }

    /// §7.4 入口规则：入口描述是 `export` 区的首记录；没有入口时该区为空、读回 `None`（E-0600 的
    /// 容器形态）。资源键表紧随其后，所以入口占掉的 8 字节不能让它串位。
    #[test]
    fn entry_description_round_trips() {
        let image = b"x".to_vec();
        let mut c = content(&image);
        c.entry = Some((ENTRY_MAIN, "main".into()));
        let bytes = write(&c).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        assert_eq!(a.entry(), Some((ENTRY_MAIN, "main".to_string())));
        assert!(dump_info(&a).contains("fn main `main`"));

        let mut c = content(&image);
        c.entry = None;
        let bytes = write(&c).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        assert_eq!(a.entry(), None);
        assert_eq!(a.meta.export.len, 0);
        assert_eq!(a.resource_keys(), vec!["std.chain.lock:db".to_string()]);
    }

    /// §8.4 rule 5: the node table row carries `chain_id, node_idx, file:line:col, linkage_count,
    /// host_event_id?`; the optional id is the all-ones sentinel when absent.
    #[test]
    fn node_table_rows_survive_the_round_trip() {
        let image = b"x".to_vec();
        let bytes = write(&content(&image)).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        let rows = a.nodes();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].chain_id, rows[0].node_idx, rows[0].line, rows[0].col), (0, 0, 8, 1));
        assert_eq!(rows[0].host_event_id, None);
        assert_eq!(rows[0].name, "gate");
        assert_eq!(rows[0].file, "tour/m/mod.infy");
        assert_eq!((rows[1].node_idx, rows[1].line, rows[1].linkage_count), (3, 11, 1));
        assert_eq!(rows[1].host_event_id, Some(7));
        // §7.4 requires the table width to be exactly 8 u32 per row.
        assert_eq!(a.meta.nodes.len as usize, 2 * NODE_REC_BYTES);
    }

    /// §7.4 self-validation: `trailer_crc` covers the first 56 bytes only, so a bit flip inside the
    /// covered window is refused and the mirrored build-id (offset 60) is not crc-protected.
    #[test]
    fn crc_covers_the_first_56_bytes_and_no_further() {
        let image = b"x".to_vec();
        let mut bytes = write(&content(&image)).unwrap();
        let tr = bytes.len() - TRAILER_SIZE;
        bytes[tr + 30] ^= 0x01;
        assert_eq!(parse_trailer(&bytes, crate::VERSION.ifabi), Err(Bad::Crc));
        let mut bytes = write(&content(&image)).unwrap();
        let tr = bytes.len() - TRAILER_SIZE;
        bytes[tr + 60] ^= 0x01;
        assert!(parse_trailer(&bytes, crate::VERSION.ifabi).is_ok());
    }

    /// §7.4/D-33：重复段尾的计数只覆盖**追加区**（blob + 段尾）。宿主镜像里出现 `INFYART2`
    /// 是常态 —— 内嵌运行时本身就带这个字面量 —— 所以整文件扫描会否掉每一个真产物。
    /// 追加区里出现第二份则必须拒绝：段尾被再附一次（双附产物）、或 blob 内容含该字面量。
    #[test]
    fn duplicate_trailer_scan_is_scoped_to_the_appended_area() {
        let image = b"PE header ... INFYART2 ... (infy_rt.c's own string literal)".to_vec();
        let bytes = write(&content(&image)).expect("a magic inside the host image is legal");
        let a = read(&bytes, crate::VERSION.ifabi).expect("read back");
        assert_eq!(a.meta.target, "x86_64-pc-windows-gnu");

        let good = write(&content(&b"image".to_vec())).unwrap();
        let mut twice = good.clone();
        let tail = good[good.len() - TRAILER_SIZE..].to_vec();
        twice.extend_from_slice(&tail);
        assert_eq!(parse_trailer(&twice, crate::VERSION.ifabi), Err(Bad::DuplicateMagic));

        let img = b"image".to_vec();
        let mut c = content(&img);
        c.nodes[0].name = "INFYART2".into();
        assert_eq!(write(&c), Err(Bad::BlobCarriesMagic));
    }

    /// Re-seal a hand-edited trailer the way the writer would (`crc32` over the first 56 bytes), so
    /// a test can prove a *specific* field is what got refused, not the checksum.
    fn retag_crc(bytes: &mut Vec<u8>) {
        let tr = bytes.len() - TRAILER_SIZE;
        let c = crc32(&bytes[tr..tr + 56]);
        bytes[tr + 56..tr + 60].copy_from_slice(&c.to_le_bytes());
    }

    /// §11.3 step 1 / §7.4: an artifact for another IFABI or another trailer version is refused.
    #[test]
    fn foreign_ifabi_or_header_version_is_refused() {
        let bytes = write(&content(&b"image".to_vec())).unwrap();
        assert_eq!(parse_trailer(&bytes, 99), Err(Bad::Ifabi(2)));
        let mut bytes = write(&content(&b"image".to_vec())).unwrap();
        let tr = bytes.len() - TRAILER_SIZE;
        bytes[tr + 8] = 3;
        retag_crc(&mut bytes);
        assert_eq!(parse_trailer(&bytes, crate::VERSION.ifabi), Err(Bad::HeaderVersion(3)));
    }

    /// §7.4: a trailer whose regions escape the file, or a meta record whose segments escape the
    /// blob, is refused rather than trusted.
    #[test]
    fn out_of_range_regions_are_refused() {
        let mut bytes = write(&content(&b"image".to_vec())).unwrap();
        let tr = bytes.len() - TRAILER_SIZE;
        bytes[tr + 20..tr + 24].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        retag_crc(&mut bytes);
        assert_eq!(parse_trailer(&bytes, crate::VERSION.ifabi), Err(Bad::RegionOutOfRange));

        let mut bytes = write(&content(&b"image".to_vec())).unwrap();
        let t = parse_trailer(&bytes, crate::VERSION.ifabi).unwrap();
        let meta_at = t.blob_off as usize;
        bytes[meta_at + meta_off::NODES..meta_at + meta_off::NODES + 4]
            .copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
        retag_crc(&mut bytes);
        assert_eq!(read(&bytes, crate::VERSION.ifabi).err(), Some(Bad::SegmentRangeEscapesBlob));
    }

    /// §7.4: "not an INFY artifact" has two distinct causes on the way in.
    #[test]
    fn short_and_magicless_files_are_refused() {
        assert_eq!(parse_trailer(&[0u8; 63], crate::VERSION.ifabi), Err(Bad::TooShort));
        let mut buf = vec![0u8; TRAILER_SIZE];
        buf[..8].copy_from_slice(b"INFYART1");
        assert_eq!(parse_trailer(&buf, crate::VERSION.ifabi), Err(Bad::NoMagic));
        buf[..8].copy_from_slice(MAGIC);
        assert_eq!(parse_trailer(&buf, crate::VERSION.ifabi), Err(Bad::Crc));
    }

    /// §8.2 / ABI §6: a pool index that is not in the table reads as the empty string, exactly as
    /// `meta_resolve_strings` degrades on the C side.
    #[test]
    fn pool_index_out_of_range_reads_empty() {
        let bytes = write(&content(&b"image".to_vec())).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        assert_eq!(a.pool_string(0), "x86_64-pc-windows-gnu");
        assert_eq!(a.pool_string(a.pool.len() as u32 + 5), "");
    }

    /// §7.3 item 1: the same normalised inputs give the same 32 bytes, twice over.
    #[test]
    fn build_id_is_deterministic() {
        let dirh = vec!["h1:AAA".to_string(), "h1:BBB".to_string()];
        let tools = vec!["gcc 14.2.0".to_string()];
        let i = BuildInputs {
            dirhashes: &dirh,
            lock: "lock-text",
            manifest: "manifest-text",
            target: "x86_64-pc-windows-gnu",
            opt_level: "-O2",
            profile: "release",
            toolchains: &tools,
            ifabi: 2,
            compiler: "infyc 2.0.0",
        };
        assert_eq!(build_id(&i), build_id(&i));
        assert_eq!(build_id_hex(&i).len(), 64);
    }

    /// §7.3 item 1 lists nine input classes; each one alone must move the digest, or that class is
    /// not actually bound (the UB-022 failure mode).
    #[test]
    fn every_build_id_input_class_moves_the_digest() {
        let base_dir = vec!["h1:AAA".to_string()];
        let base_tools = vec!["gcc 14.2.0".to_string()];
        let base = BuildInputs {
            dirhashes: &base_dir,
            lock: "l",
            manifest: "m",
            target: "t",
            opt_level: "-O2",
            profile: "release",
            toolchains: &base_tools,
            ifabi: 2,
            compiler: "infyc 2.0.0",
        };
        let root = build_id(&base);

        let d2 = vec!["h1:AAB".to_string()];
        let t2 = vec!["gcc 14.3.0".to_string()];
        let cases: Vec<BuildInputs<'_>> = vec![
            BuildInputs { dirhashes: &d2, ..base.clone() },
            BuildInputs { lock: "l2", ..base.clone() },
            BuildInputs { manifest: "m2", ..base.clone() },
            BuildInputs { target: "t2", ..base.clone() },
            BuildInputs { opt_level: "-O3", ..base.clone() },
            BuildInputs { profile: "debug", ..base.clone() },
            BuildInputs { toolchains: &t2, ..base.clone() },
            BuildInputs { ifabi: 3, ..base.clone() },
            BuildInputs { compiler: "infyc 2.0.1", ..base },
        ];
        for (n, c) in cases.iter().enumerate() {
            assert_ne!(build_id(c), root, "input class {n} does not move the build-id");
        }
    }

    fn empty_tools() -> Vec<String> {
        Vec::new()
    }

    /// §5.4 / ABI §7: dirhashes enter in package order, so swapping the order is a different set.
    /// Length prefixes also stop two adjacent fields from being re-segmented into the same bytes.
    #[test]
    fn build_id_binds_order_and_field_boundaries() {
        fn mk<'a>(dirhashes: &'a [String], tools: &'a [String]) -> BuildInputs<'a> {
            BuildInputs {
                dirhashes,
                lock: "",
                manifest: "",
                target: "",
                opt_level: "",
                profile: "",
                toolchains: tools,
                ifabi: 2,
                compiler: "",
            }
        }
        let ab = vec!["h1:A".to_string(), "h1:B".to_string()];
        let ba = vec!["h1:B".to_string(), "h1:A".to_string()];
        let tools = empty_tools();
        assert_ne!(build_id(&mk(&ab, &tools)), build_id(&mk(&ba, &tools)), "§5.4 order is part of the identity");

        // ("ab","c") vs ("a","bc"): an unprefixed concat would hash identically.
        let s1 = BuildInputs { lock: "ab", manifest: "c", ..mk(&ab, &tools) };
        let s2 = BuildInputs { lock: "a", manifest: "bc", ..mk(&ab, &tools) };
        assert_ne!(build_id(&s1), build_id(&s2), "field boundary must be bound");
    }

    /// §7.3 item 2: the container itself must be a pure function of its content.
    #[test]
    fn writer_output_is_byte_identical_for_the_same_content() {
        let image = b"MZ image".to_vec();
        let a = write(&content(&image)).unwrap();
        let b = write(&content(&image)).unwrap();
        assert_eq!(a, b);
        let t = parse_trailer(&a, crate::VERSION.ifabi).unwrap();
        assert_eq!(t.blob_off as usize, image.len());
        assert_eq!(t.blob_len as usize + image.len() + TRAILER_SIZE, a.len());
    }

    /// §7.4 / CRC-32/IEEE: the shared table convention with `infy_art_crc32` in the runtime.
    #[test]
    fn crc32_uses_the_runtime_table_convention() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(&[]), 0);
    }

    /// §7.6: `--allow-toolchain-drift` marks the artifact unreproducible instead of hiding it.
    #[test]
    fn unreproducible_flag_round_trips() {
        let image = b"x".to_vec();
        let mut c = content(&image);
        c.unreproducible = true;
        let bytes = write(&c).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        assert_eq!(a.trailer.flags, 1);
        let info = dump_info(&a);
        assert!(info.contains("unreproducible"), "{info}");
        assert!(info.contains("true (§7.6 --allow-toolchain-drift)"), "{info}");
    }

    /// §8.4 rule 5's export form: the node table prints one row per node with its source position.
    #[test]
    fn node_dump_lists_positions() {
        let image = b"x".to_vec();
        let bytes = write(&content(&image)).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        let t = dump_nodes(&a);
        assert!(t.contains("tour/m/mod.infy\t8\t1"), "{t}");
        assert!(t.contains("gate"), "{t}");
        assert!(!t.contains("INFYART2"), "the dump must not echo the trailer magic");
    }

    fn fixture_bytes() -> Vec<u8> {
        // Not "MZ..." on purpose: MSYS flips the executable bit on PE-prefixed files, and this
        // fixture is data, not a runnable image.
        let image = b"\x7fELF checked-in container fixture, not a runnable host image".to_vec();
        write(&content(&image)).expect("fixture content is well-formed")
    }

    /// F17 ①: the table is written, derived and read back — `EnvOffsets` has no meta slot, so its
    /// place comes out of the regions before it and its size out of its own count column (D-60).
    #[test]
    fn env_offsets_region_is_derived_and_self_sized() {
        let bytes = write(&content(&b"img".to_vec())).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        let r = a.env_region();
        assert_eq!(r.off, a.meta.export.off + a.meta.export.len + 4 * a.meta.n_resource_keys);
        assert_eq!(r.len, (ENV_COUNT_BYTES + 11 * ENV_REC_BYTES) as u32);
        assert_eq!(a.meta.rodata.off, r.off + r.len, "rodata must start right after the table");
        assert_eq!(a.env_offsets().len(), 11, "§4.6's closed field set, 11 rows");
        assert!(dump_sections(&a).contains("env_offsets"), "the section map names the region");
    }

    /// Names, offsets, sizes and tags all survive the round trip in §4.6's order — codegen loads at
    /// exactly these offsets, so a reordered or re-tagged row is a different program.
    #[test]
    fn env_offsets_round_trip_in_spec_order() {
        let bytes = write(&content(&b"img".to_vec())).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        let got = a.env_offsets();
        assert_eq!(got, env_rows());
        assert_eq!(got.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().first(), Some(&"iter"));
        assert_eq!(env_tag_name(ENV_TY_F32), "f32");
    }

    /// The acceptance half of F17: `infy artifact info` prints the 11 offsets, not just a count.
    #[test]
    fn dump_info_prints_all_eleven_env_offsets() {
        let bytes = write(&content(&b"img".to_vec())).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        let t = dump_info(&a);
        assert!(t.contains(&format!("{:<18} {}\n", "n_env_offsets", 11)), "{t}");
        for (name, off) in
            [("iter", 0), ("runtime_s", 4), ("realtime", 8), ("ram_mb", 16), ("cpu_use", 20),
             ("gpu_use", 24), ("chain_id", 28), ("node_idx", 32), ("focus_id", 36),
             ("platform", 40), ("build_id", 56)]
        {
            assert!(t.contains(&format!("env.{name}")), "missing `env.{name}` in:\n{t}");
            assert!(t.contains(&format!("off={off} ")), "missing offset {off} for `env.{name}`:\n{t}");
        }
        assert!(t.contains("env.build_id"), "str rows print too");
    }

    /// No table is a legal container (a hand-built one may omit it) and must read as zero rows —
    /// never as eleven invented ones.
    #[test]
    fn an_empty_env_table_reads_as_zero_rows() {
        let image = b"img".to_vec();
        let mut c = content(&image);
        c.env_offsets = vec![];
        let bytes = write(&c).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        assert_eq!(a.env_region().len, ENV_COUNT_BYTES as u32);
        assert!(a.env_offsets().is_empty(), "a zero-count table must not synthesize rows");
        assert!(dump_info(&a).contains(&format!("{:<18} {}\n", "n_env_offsets", 0)));
    }

    /// A row count that does not fit the blob is a broken container: refuse it, do not read it as a
    /// shorter table (`.inf sum`'s verifier applies the same shape of rule).
    #[test]
    fn an_env_count_that_escapes_the_blob_refuses_the_container() {
        let mut bytes = write(&content(&b"img".to_vec())).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        let at = a.trailer.blob_off as usize + a.env_region().off as usize;
        bytes[at..at + ENV_COUNT_BYTES].copy_from_slice(&9999u32.to_le_bytes());
        assert_eq!(
            read(&bytes, crate::VERSION.ifabi).err(),
            Some(Bad::EnvTableEscapesBlob),
            "装不下的行数必须拒绝容器，而不是读成一张短表"
        );
    }

    /// §7.3 item 3: a byte inside the table is named, not reported as padding.
    #[test]
    fn describe_offset_names_the_env_region() {
        let bytes = write(&content(&b"img".to_vec())).unwrap();
        let a = read(&bytes, crate::VERSION.ifabi).unwrap();
        let off = a.trailer.blob_off as usize + a.env_region().off as usize + ENV_COUNT_BYTES;
        assert!(describe_offset(&bytes, off).contains("EnvOffsets"), "{}", describe_offset(&bytes, off));
    }

    /// C3 的交叉验证锁点：`runtime/fixtures/artifact_rust_writer.bin` 是**本模块写入器**的逐字节固定
    /// 输出，`runtime/test_rt.c` 用生产读取器 `infy_art_read_file` 读同一份字节。这条测试锁住
    /// "写入器再也不会改变夹具"，C 侧锁住"C 读者认得 Rust 作者写的每一个字段"。
    /// 只有 `INFY_REGENERATE_FIXTURE=1` 才重写文件（改布局必须是一次显式动作）。
    #[test]
    fn c_reader_fixture_is_exactly_what_the_writer_produces() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/fixtures/artifact_rust_writer.bin");
        let bytes = fixture_bytes();
        if std::env::var("INFY_REGENERATE_FIXTURE").is_ok() {
            std::fs::write(path, &bytes).expect("explicit fixture regeneration");
        }
        let on_disk = std::fs::read(path)
            .unwrap_or_else(|e| panic!("{} 不在仓库里（{e}）—— 用 INFY_REGENERATE_FIXTURE=1 生成一次", path));
        assert_eq!(on_disk, bytes, "写入器输出与已提交夹具不再逐字节相同（§7.4 布局漂移）");

        // 字段级断言（C 侧 `test_rt.c` 的 artifact 段对同一份字节断言同一组数字）。
        let a = read(&bytes, 2).expect("the fixture parses with IFABI 2");
        assert_eq!(a.meta.ifabi, 2);
        assert_eq!(a.meta.opt_level, 2);
        assert_eq!(a.meta.overflow, OV_PANIC);
        assert_eq!(a.meta.panic_radius, RADIUS_FOCUS);
        assert_eq!(a.meta.usize_bits, 64);
        assert_eq!(a.meta.lto, 1);
        assert_eq!(a.meta.iter_guard_steps, 1 << 32);
        assert_eq!(a.meta.target, "x86_64-pc-windows-gnu");
        assert_eq!(a.meta.build_id, hex_of(&[0xAB; 32]));
        assert_eq!(a.meta.platform, "windows");
        assert_eq!(a.meta.lang_version, "2.0");
        assert_eq!(a.meta.source_toolchain, "gcc 14.2.0");
        assert_eq!(a.meta.n_chains, 1);
        assert_eq!(a.meta.n_events, 0);
        assert_eq!(a.meta.n_nodes, 2);
        assert_eq!(a.meta.n_ifbc_modules, 0);
        assert_eq!(a.meta.n_resource_keys, 1);
        assert_eq!(a.env_offsets(), env_rows(), "夹具带着 11 行 EnvOffsets，C 侧读者按同一张表对账");
        assert_eq!(a.trailer.seg_count, SEG_COUNT);
        assert_eq!(a.entry(), Some((ENTRY_CHAIN, "Pipeline".to_string())));
        assert_eq!(a.pool_string(0), "x86_64-pc-windows-gnu", "池索引 0 = target，C 侧同序");
        assert_eq!(a.resource_keys(), vec!["std.chain.lock:db".to_string()]);
    }
}
