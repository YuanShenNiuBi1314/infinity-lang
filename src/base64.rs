//! Base64 (RFC 4648 standard alphabet) — the encoding of §5.8 item 1's `h1:<base64>` dirhash and
//! of the `dirh` / `dirh-m` / `sumdb` rows of appendix E.1.
//!
//! Why this module exists at all: the toolchain has **no dependencies** (crate header in [`crate`]),
//! so the 64-entry alphabet table is hand-written.
//!
//! Deliberate scope limits, all of them driven by §5.8:
//! * only the **standard** alphabet (`+` `/`), not URL-safe (`-` `_`);
//! * `=` padding is mandatory on decode and canonical on both sides.
//!
//! A dirhash is compared for *equality* against `MODULE.infy.lock` (§5.8 item 2, mismatch →
//! `E-0560`) and against the checksum database (§5.8 item 3, mismatch → `E-0562`). Accepting two
//! spellings of the same bytes (unpadded, URL-safe, or non-canonical trailing bits) would let two
//! byte-identical modules carry different digest strings, which is exactly the supply-chain hole
//! §5.8 exists to close. Everything rejected here maps onto `E-0560` at the caller.
//!
//! Deterministic (§7.3): no locale, no entropy, no time; `encode_standard` is a pure function of
//! its input bytes.

use std::fmt;

/// RFC 4648 Table 1 alphabet.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Reverse lookup: sextet value per byte, `0xFF` for "not in the alphabet".
const fn build_reverse() -> [u8; 256] {
    let mut t = [0xFFu8; 256];
    let mut i = 0usize;
    while i < 64 {
        t[ALPHABET[i] as usize] = i as u8;
        i += 1;
    }
    t
}

const REVERSE: [u8; 256] = build_reverse();

/// A rejected byte, reported with its 0-based index into the input so the caller can render
/// `E-0560`/`E-0506` style diagnostics with a position (§9.2 wants positions, not prose).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidBase64 {
    /// Byte offset (0-based) of the offending input byte. For structural failures — wrong total
    /// length, missing padding — this is the offset just past the last usable byte.
    pub index: usize,
    pub message: String,
}

impl InvalidBase64 {
    fn new(index: usize, message: impl Into<String>) -> Self {
        InvalidBase64 { index, message: message.into() }
    }
}

impl fmt::Display for InvalidBase64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid base64 at byte {}: {}", self.index, self.message)
    }
}

impl std::error::Error for InvalidBase64 {}

/// Standard-alphabet encoding with `=` padding (§5.8 item 1 renders `h1:` + this string).
pub fn encode_standard(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        // A chunk of 1 or 2 bytes pads its sextets with zeros, which is what makes the output
        // canonical (the decoder below rejects anything else).
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let s0 = (b0 >> 2) as usize;
        let s1 = (((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize;
        let s2 = (((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize;
        let s3 = (b2 & 0b0011_1111) as usize;
        out.push(ALPHABET[s0] as char);
        out.push(ALPHABET[s1] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[s2] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[s3] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Decode standard-alphabet base64 with mandatory padding.
///
/// Rejects: characters outside the alphabet, a length that is not a multiple of 4, padding in the
/// wrong place or of the wrong width, and non-canonical tail bits (e.g. `Zm9n` vs `Zm9v` — the
/// last sextet must be zero when it is not backed by an input byte).
pub fn decode(s: &str) -> Result<Vec<u8>, InvalidBase64> {
    let bytes = s.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(InvalidBase64::new(
            bytes.len(),
            format!(
                "length {} is not a multiple of 4; `=` padding is mandatory for `h1:` digests (§5.8)",
                bytes.len()
            ),
        ));
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut i = 0usize;
    while i < bytes.len() {
        let mut sextets = [0u32; 4];
        let mut pad = 0usize;
        for k in 0..4 {
            let c = bytes[i + k];
            if c == b'=' {
                // Padding only ever appears in the final group and only at position 2 or 3, and
                // must be followed by nothing but `=`.
                if i + 4 != bytes.len() || k < 2 {
                    return Err(InvalidBase64::new(
                        i + k,
                        "`=` padding appears out of position".to_string(),
                    ));
                }
                pad = 4 - k;
                for j in k..4 {
                    if bytes[i + j] != b'=' {
                        return Err(InvalidBase64::new(
                            i + j,
                            "data character after `=` padding".to_string(),
                        ));
                    }
                }
                break;
            }
            let v = REVERSE[c as usize];
            if v == 0xFF {
                return Err(InvalidBase64::new(
                    i + k,
                    format!(
                        "'{}' is not in the RFC 4648 standard alphabet (URL-safe `-`/`_` are not accepted)",
                        c as char
                    ),
                ));
            }
            sextets[k] = v as u32;
        }
        let data = 4 - pad;
        // Canonicality: the bits that the padding threw away must have been zero. With three
        // sextets carrying two bytes, 2 low bits are slack; with two sextets carrying one byte, 4.
        if data == 3 && sextets[2] & 0b11 != 0 {
            return Err(InvalidBase64::new(
                i + 3,
                "non-canonical trailing bits before `=` padding".to_string(),
            ));
        }
        if data == 2 && sextets[1] & 0b1111 != 0 {
            return Err(InvalidBase64::new(
                i + 2,
                "non-canonical trailing bits before `=` padding".to_string(),
            ));
        }
        let packed = (sextets[0] << 18) | (sextets[1] << 12) | (sextets[2] << 6) | sextets[3];
        match data {
            4 => out.extend_from_slice(&[(packed >> 16) as u8, (packed >> 8) as u8, packed as u8]),
            3 => out.extend_from_slice(&[(packed >> 16) as u8, (packed >> 8) as u8]),
            2 => out.push((packed >> 16) as u8),
            // `====` and `A===` are rejected above, so this is unreachable; kept for totality.
            _ => unreachable!("base64 group with {} data characters", data),
        }
        i += 4;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §3 worked examples.
    #[test]
    fn rfc4648_vectors() {
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_standard(raw.as_bytes()), enc, "encode {:?}", raw);
            assert_eq!(decode(enc).unwrap().as_slice(), raw.as_bytes(), "decode {:?}", enc);
        }
    }

    /// Every byte value, in every possible alignment mod 3.
    #[test]
    fn round_trip_all_bytes() {
        let all: Vec<u8> = (0u16..=255).map(|v| v as u8).collect();
        for start in 0..all.len() {
            let slice = &all[start..];
            let enc = encode_standard(slice);
            let dec = decode(&enc).unwrap_or_else(|e| panic!("{} rejected: {}", enc, e));
            assert_eq!(dec, slice, "round trip from offset {}", start);
        }
    }

    /// A 32-byte SHA-256 root — the only thing `h1:` actually carries — encodes to 44 chars.
    #[test]
    fn sha256_root_shape() {
        let root = [0u8; 32];
        let enc = encode_standard(&root);
        assert_eq!(enc.len(), 44);
        assert_eq!(enc, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert_eq!(decode(&enc).unwrap(), root.to_vec());
    }

    #[test]
    fn rejects_non_alphabet() {
        for bad in ["****", "a-_=", "h1:a", "AA\u{00e9}"] {
            let e = decode(bad).unwrap_err();
            assert!(e.message.contains("alphabet"), "{}: {}", bad, e);
            assert!(e.index < bad.len(), "{}: index {}", bad, e.index);
        }
    }

    #[test]
    fn rejects_bad_length_and_padding() {
        assert_eq!(decode("a").unwrap_err().index, 1);
        assert_eq!(decode("abc").unwrap_err().index, 3);
        assert_eq!(decode("abcde").unwrap_err().index, 5);
        assert!(decode("=abc").unwrap_err().message.contains("out of position"));
        assert!(decode("a=bc").unwrap_err().message.contains("out of position"));
        assert!(decode("ab=c").unwrap_err().message.contains("data character after"));
        assert!(decode("ab==c===").unwrap_err().message.contains("out of position"));
        assert!(decode("====").unwrap_err().message.contains("out of position"));
    }

    /// Two spellings must not decode to the same bytes: that is what keeps `E-0560` decidable.
    #[test]
    fn rejects_non_canonical_tail_bits() {
        // "fo" is `Zm8=`; `9` keeps the same top 4 bits but a non-zero slack sextet.
        assert_eq!(decode("Zm8=").unwrap(), b"fo");
        assert!(decode("Zm9=").unwrap_err().message.contains("non-canonical"));
        // "f" is `Zg==`; `h` keeps the same top 2 bits but 2 non-zero slack bits.
        assert_eq!(decode("Zg==").unwrap(), b"f");
        assert!(decode("Zh==").unwrap_err().message.contains("non-canonical"));
        // A full four-sextet group has no slack bits at all, so `Zm9n` = "fog" is legitimate.
        assert_eq!(decode("Zm9n").unwrap(), b"fog");
    }

    #[test]
    fn alphabet_table_is_consistent() {
        for (i, c) in ALPHABET.iter().enumerate() {
            assert_eq!(REVERSE[*c as usize], i as u8);
        }
        assert_eq!(ALPHABET[0], b'A');
        assert_eq!(ALPHABET[62], b'+');
        assert_eq!(ALPHABET[63], b'/');
    }
}
