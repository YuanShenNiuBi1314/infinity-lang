//! SHA-256 (FIPS 180-4) and the §5.8 dirhash.
//!
//! Hand-written because the toolchain has **no dependencies** (crate header in [`crate`]): a
//! compiler is a trust boundary, and §5.8's supply-chain model is applied to user code, so the
//! digest the whole model rests on is audited here rather than pulled from a crate.
//!
//! Where the digest is used by the spec:
//! * §5.8 item 1 — module dirhash, `h1:<base64>` (see [`merkle_dirhash`]);
//! * §7.3 item 1 — `build-id = SHA-256(规范化的全部输入)`;
//! * §6.7 / appendix E.2 — `abi_hash = "sha256:…"`, `table_hash = "sha256:…"`.
//!
//! Note the division of labour with §6.7: `Hash` **derive** for user types is fixed to xxHash64,
//! which lives in [`crate::derive`]; this module is the *cryptographic* half (integrity, not hashing
//! for maps). Swapping either algorithm is a MAJOR change (§6.7, §7.3).
//!
//! Deterministic (§7.3): pure function of input bytes, no entropy, no timestamps, no locale.

use crate::base64;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Incremental SHA-256 context (FIPS 180-4 §6.2).
#[derive(Clone, Debug)]
pub struct Sha256 {
    h: [u32; 8],
    /// Unprocessed tail, always < 64 bytes.
    buf: [u8; 64],
    buf_len: usize,
    /// Total bytes absorbed, used for the length field of the padding block.
    len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 { h: H0, buf: [0u8; 64], buf_len: 0, len: 0 }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.len = self.len.wrapping_add(bytes.len() as u64);
        let mut input = bytes;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(input.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&input[..take]);
            self.buf_len += take;
            input = &input[take..];
            if self.buf_len < 64 {
                return;
            }
            let block: [u8; 64] = self.buf;
            compress(&mut self.h, &block);
            self.buf_len = 0;
        }
        let mut blocks = input.chunks_exact(64);
        for block in &mut blocks {
            let mut b = [0u8; 64];
            b.copy_from_slice(block);
            compress(&mut self.h, &b);
        }
        let rest = blocks.remainder();
        if !rest.is_empty() {
            self.buf[..rest.len()].copy_from_slice(rest);
            self.buf_len = rest.len();
        }
    }

    /// Absorbs the padding (FIPS 180-4 §5.1.1) and returns the 32-byte digest.
    pub fn finish(mut self) -> [u8; 32] {
        let bit_len = self.len.wrapping_mul(8);
        // `0x80` then zeros so that the block is 56 mod 64 bytes, then the 64-bit big-endian length.
        let mut tail = [0x00u8; 72];
        tail[0] = 0x80;
        let pad = if self.buf_len <= 55 { 56 - self.buf_len } else { 120 - self.buf_len };
        self.update(&tail[..pad]);
        let len_be = bit_len.to_be_bytes();
        self.update(&len_be);
        debug_assert_eq!(self.buf_len, 0, "padding must end on a block boundary");
        let mut out = [0u8; 32];
        for (i, w) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }
}

#[inline(always)]
fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let mut a = h[0];
    let mut b = h[1];
    let mut c = h[2];
    let mut d = h[3];
    let mut e = h[4];
    let mut f = h[5];
    let mut g = h[6];
    let mut hh = h[7];
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

/// One-shot digest.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut ctx = Sha256::new();
    ctx.update(bytes);
    ctx.finish()
}

/// Lowercase hex rendering, i.e. the `sha256:…` form used by appendix E.2 `abi_hash` and E.3
/// `table_hash`, and by §7.3's `build-id` printout.
pub fn hex(bytes: &[u8]) -> String {
    let d = sha256(bytes);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push(HEX_DIGITS[(b >> 4) as usize] as char);
        s.push(HEX_DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// §5.8 item 1, step 2: `SHA-256(path + "\0" + content + "\0")`.
///
/// The path is the module-relative path in its stored byte form; both terminators are part of the
/// pre-image so `(path, content)` pairs cannot be re-bracketed into an equal digest.
pub fn leaf_hash(path: &str, content: &[u8]) -> [u8; 32] {
    let mut ctx = Sha256::new();
    ctx.update(path.as_bytes());
    ctx.update(&[0u8]);
    ctx.update(content);
    ctx.update(&[0u8]);
    ctx.finish()
}

/// §5.8 item 1, step 3: pairwise Merkle reduction of the leaf hashes.
///
/// * the two NUL terminators of [`leaf_hash`] make leaves unambiguous;
/// * an interior node is `SHA-256(left ‖ right)` over the raw 32-byte children (fixed-width
///   children, so no separator is needed);
/// * **odd-leaf rule (spec hole, decided here)**: when a level has an odd number of nodes, the
///   last node is **duplicated** and paired with itself, i.e. `SHA-256(x ‖ x)`. §5.8 says only
///   "归约为 Merkle 树" and never fixes this case. Duplicating is what Git's fanout does, it keeps
///   the tree balanced, and it is injective in the sense that matters here: a single-leaf tree has
///   root = leaf, so `merkle_root(&[x]) == x`.
/// * **empty tree (spec hole, decided here)**: zero files hash to `SHA-256("")`, matching Go's
///   `dirhash.Hash1` on an empty file list, so an empty module and Go's tooling agree on `h1:`.
/// * **ordering**: leaves are sorted by path bytes (§5.8 item 1 "按路径字节序排序") — byte order,
///   never a locale collation, so the digest is identical on every host (§7.3). Equal paths are
///   tie-broken on the leaf digest, which makes the result a pure function of the *multiset* of
///   entries rather than of the caller's directory walk. Duplicate paths cannot exist in a real
///   module tree (§5.2 R3/R2: one package per directory), and §5.8 never says what to do with them;
///   this function therefore neither merges nor errors, it just defines them (erroring inside a
///   hash would push `E-0560` policy into a primitive).
pub fn merkle_root(files: &[(String, Vec<u8>)]) -> [u8; 32] {
    if files.is_empty() {
        return sha256(&[]);
    }
    let mut leaves: Vec<([u8; 32], &[u8])> =
        files.iter().map(|(p, c)| (leaf_hash(p, c), p.as_bytes())).collect();
    leaves.sort_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(&b.0)));
    let mut level: Vec<[u8; 32]> = leaves.into_iter().map(|(h, _)| h).collect();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut i = 0;
        while i < level.len() {
            let left = level[i];
            let right = if i + 1 < level.len() { level[i + 1] } else { level[i] };
            let mut ctx = Sha256::new();
            ctx.update(&left);
            ctx.update(&right);
            next.push(ctx.finish());
            i += 2;
        }
        level = next;
    }
    level[0]
}

/// §5.8 item 1: the `h1:<base64 of the raw 32-byte Merkle root>` string that goes into
/// `MODULE.infy.lock`'s `dirh` rows (appendix E.1) and into `[[deps]] dirhash` (appendix E.2).
///
/// `files` is the **unpacked logical file tree** of the module@version — for a `.infib` container
/// that means the tree after unpacking, which is why §5.8 item 1 says container compression never
/// affects the hash. `.infib` metadata injected by the packer must therefore not appear here.
pub fn merkle_dirhash(files: &[(String, Vec<u8>)]) -> String {
    format!("h1:{}", base64::encode_standard(&merkle_root(files)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only hex → bytes, so the NIST vectors below can be compared as raw digests.
    fn unhex(s: &str) -> [u8; 32] {
        let b = s.as_bytes();
        assert_eq!(b.len(), 64);
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            let hi = (b[i * 2] as char).to_digit(16).unwrap() as u8;
            let lo = (b[i * 2 + 1] as char).to_digit(16).unwrap() as u8;
            *o = (hi << 4) | lo;
        }
        out
    }

    /// FIPS 180-4 / NIST sample 1: the empty message.
    #[test]
    fn nist_empty() {
        assert_eq!(
            hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b""),
            unhex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    /// NIST sample 2: "abc".
    #[test]
    fn nist_abc() {
        assert_eq!(
            hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// NIST sample 3: the 56-byte two-block message, which exercises the padding split.
    #[test]
    fn nist_56_bytes() {
        let msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(msg.len(), 56);
        assert_eq!(
            hex(msg),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    /// One million × 'a', fed in misaligned 999-byte streaming chunks to prove `update` is
    /// block-boundary independent. Digest cross-checked against two independent implementations
    /// (GNU coreutils `sha256sum` and OpenSSL `dgst -sha256`) on the same 1e6-byte input — worth
    /// stating, because this vector is widely mis-transcribed on the internet.
    #[test]
    fn streaming_million_a() {
        const MILLION_A: &str = "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0";
        let mut ctx = Sha256::new();
        let chunk = vec![b'a'; 999];
        let mut remaining = 1_000_000usize;
        while remaining > 0 {
            let n = remaining.min(chunk.len());
            ctx.update(&chunk[..n]);
            remaining -= n;
        }
        assert_eq!(ctx.finish(), unhex(MILLION_A));
        // Same answer in one call, and byte-at-a-time.
        let million = vec![b'a'; 1_000_000];
        assert_eq!(sha256(&million), unhex(MILLION_A));
        assert_eq!(hex(&million), MILLION_A);
        let mut one = Sha256::new();
        for _ in 0..1_000_000 {
            one.update(&[b'a']);
        }
        assert_eq!(one.finish(), sha256(&million));
    }

    /// The `a`×N lengths that straddle FIPS 180-4's padding rule (block, block+55, block+56,
    /// block+64) plus two longer runs. Digests taken from GNU coreutils `sha256sum`.
    #[test]
    fn padding_boundary_vectors() {
        for (n, want) in [
            (55usize, "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"),
            (56, "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"),
            (63, "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34"),
            (64, "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"),
            (65, "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0"),
            (1000, "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"),
            (100_000, "6d1cf22d7cc09b085dfc25ee1a1f3ae0265804c607bc2074ad253bcc82fd81ee"),
        ] {
            let data = vec![b'a'; n];
            assert_eq!(hex(&data), want, "one-shot, {} bytes", n);
            // And fed in 7-byte pieces, so the buffer tail is exercised too.
            let mut ctx = Sha256::new();
            for chunk in data.chunks(7) {
                ctx.update(chunk);
            }
            assert_eq!(ctx.finish(), unhex(want), "streamed, {} bytes", n);
        }
    }

    #[test]
    fn block_boundary_splits() {
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7 % 251) as u8).collect();
        let whole = sha256(&data);
        for split in [1usize, 55, 56, 63, 64, 65, 119, 120, 128] {
            let mut ctx = Sha256::new();
            ctx.update(&data[..split]);
            ctx.update(&data[split..]);
            assert_eq!(ctx.finish(), whole, "split at {}", split);
        }
    }

    #[test]
    fn leaf_hash_has_two_terminators() {
        let l = leaf_hash("a/b.infy", b"let x = 1;");
        let mut ctx = Sha256::new();
        ctx.update(b"a/b.infy");
        ctx.update(b"\0");
        ctx.update(b"let x = 1;");
        ctx.update(b"\0");
        assert_eq!(l, ctx.finish());
        assert_ne!(l, sha256(b"a/b.infy\0let x = 1;"));
    }

    /// §5.8 item 1: the digest is a function of the *set* of files, not of the order the caller
    /// happened to walk the directory in (§7.3 / UB-022).
    #[test]
    fn dirhash_is_order_independent() {
        let files: Vec<(String, Vec<u8>)> = [
            ("core/schedule/dispatch.infy", "module core;".as_bytes()),
            ("MODULE.infy.toml", "[module]\npath = \"x\"".as_bytes()),
            ("README.md", "# 竹空\n".as_bytes()),
            ("cmd/host/main.infy", "fn main() {}".as_bytes()),
            ("core/schedule/BUILD", "".as_bytes()),
            ("LICENSE", "MIT\n".as_bytes()),
            ("a.infy", "a".as_bytes()),
            ("z/z/z.infy", "zz".as_bytes()),
        ]
        .iter()
        .map(|(p, c)| (p.to_string(), c.to_vec()))
        .collect();
        let reference = merkle_dirhash(&files);
        assert!(reference.starts_with("h1:"));
        // Deterministic pseudo-permutation (§10.12 SplitMix64, never system entropy).
        let mut seeded = files.clone();
        let mut state = 0x9E3779B97F4A7C15u64;
        for i in (1..seeded.len()).rev() {
            state ^= state >> 30;
            state = state.wrapping_mul(0xBF58476D1CE4E5B9);
            state ^= state >> 27;
            state = state.wrapping_mul(0x94D049BB133111EB);
            state ^= state >> 31;
            let j = (state % (i as u64 + 1)) as usize;
            seeded.swap(i, j);
        }
        assert_ne!(seeded, files, "the shuffle must actually move something");
        assert_eq!(merkle_dirhash(&seeded), reference);
        assert_eq!(merkle_dirhash(&seeded), merkle_dirhash(&files));
    }

    /// The three odd-level cases of the reduction rule, spelled out.
    #[test]
    fn odd_leaf_rule_duplicates_last_node() {
        let a = leaf_hash("a", b"1");
        let b = leaf_hash("b", b"2");
        let c = leaf_hash("c", b"3");
        // 3 leaves: (a,b) fold, then (ab, ab') where the survivor c is paired with itself.
        let ab = {
            let mut ctx = Sha256::new();
            ctx.update(&a);
            ctx.update(&b);
            ctx.finish()
        };
        let cc = {
            let mut ctx = Sha256::new();
            ctx.update(&c);
            ctx.update(&c);
            ctx.finish()
        };
        let mut ctx = Sha256::new();
        ctx.update(&ab);
        ctx.update(&cc);
        let expected = ctx.finish();
        let files = vec![
            ("c".to_string(), b"3".to_vec()),
            ("a".to_string(), b"1".to_vec()),
            ("b".to_string(), b"2".to_vec()),
        ];
        assert_eq!(merkle_root(&files), expected);
        // A single leaf is its own root, so `h1:` of one file is that file's leaf hash.
        assert_eq!(
            merkle_dirhash(&[("a".to_string(), b"1".to_vec())]),
            format!("h1:{}", base64::encode_standard(&a))
        );
        // Empty tree = SHA-256("") (documented choice, §5.8 does not cover it).
        assert_eq!(merkle_dirhash(&[]), format!("h1:{}", base64::encode_standard(&sha256(&[]))));
    }

    /// §5.8 item 6: binary members are ordinary files, and wheel-vs-source containers agree only
    /// because paths are what is sorted. Confirm byte-order (not char-order) sorting is stable and
    /// that path bytes participate in the digest.
    #[test]
    fn paths_and_content_both_change_the_digest() {
        let one = merkle_dirhash(&[("mod.infy".to_string(), b"x".to_vec())]);
        assert_ne!(one, merkle_dirhash(&[("mod.inf".to_string(), b"x".to_vec())]));
        assert_ne!(one, merkle_dirhash(&[("mod.infy".to_string(), b"y".to_vec())]));
        // Byte order puts uppercase before lowercase, like §5.2 R3 paths expect.
        assert_ne!(
            merkle_dirhash(&[("A.infy".to_string(), b"x".to_vec())]),
            merkle_dirhash(&[("a.infy".to_string(), b"x".to_vec())])
        );
    }

    /// Equal paths (impossible in a real tree, §5.2 R2/R3) still hash order-independently, because
    /// the sort tie-breaks on the leaf digest.
    #[test]
    fn duplicate_paths_are_order_independent() {
        let files = vec![
            ("a".to_string(), b"1".to_vec()),
            ("a".to_string(), b"2".to_vec()),
            ("a".to_string(), b"3".to_vec()),
        ];
        let mut reversed = files.clone();
        reversed.reverse();
        assert_eq!(merkle_dirhash(&files), merkle_dirhash(&reversed));
        // …and a colliding tree differs from a distinct-path one.
        assert_ne!(
            merkle_dirhash(&files),
            merkle_dirhash(&[
                ("a".to_string(), b"1".to_vec()),
                ("b".to_string(), b"2".to_vec()),
                ("c".to_string(), b"3".to_vec()),
            ])
        );
    }

    #[test]
    fn digest_decodes_back_to_the_root() {
        let files = vec![("a".to_string(), b"1".to_vec()), ("b".to_string(), b"2".to_vec())];
        let rendered = merkle_dirhash(&files);
        let raw = &rendered["h1:".len()..];
        assert_eq!(base64::decode(raw).unwrap(), merkle_root(&files).to_vec());
    }
}
