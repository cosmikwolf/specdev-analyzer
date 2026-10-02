//! Content hashing for inlined units.
//!
//! The hash covers the exact bytes the analyzer copied from plugin to
//! project — no normalization, no re-emission. Canonicalization (LF,
//! single trailing newline, no BOM, valid UTF-8) is enforced upstream
//! by plugin CI; this crate only witnesses what was copied.

use sha2::{Digest, Sha256};

/// Returns `"sha256:" + lowercase-hex SHA-256 of `bytes`.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// `bytes` with every CRLF turned into LF.
pub fn to_lf(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !bytes.windows(2).any(|w| w == b"\r\n") {
        return std::borrow::Cow::Borrowed(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    std::borrow::Cow::Owned(out)
}

/// The hash a record holds from `spec_version` 2.0 on: over the LF form of
/// `bytes`, so a CRLF checkout of an unedited file still matches (#219).
pub fn recorded_hash(bytes: &[u8]) -> String {
    content_hash(&to_lf(bytes))
}

/// Whether `recorded` witnesses `bytes`: the LF form (2.0 records), the
/// bytes as they are, or their CRLF form (a 1.x record taken on a CRLF
/// checkout, read back on an LF one).
pub fn record_matches(recorded: &str, bytes: &[u8]) -> bool {
    let lf = to_lf(bytes);
    if recorded == content_hash(&lf) || recorded == content_hash(bytes) {
        return true;
    }
    let mut crlf = Vec::with_capacity(lf.len() + lf.len() / 16);
    for &b in lf.iter() {
        if b == b'\n' {
            crlf.push(b'\r');
        }
        crlf.push(b);
    }
    recorded == content_hash(&crlf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bytes_known_hash() {
        // SHA-256 of the empty string per FIPS 180-4 test vectors.
        assert_eq!(
            content_hash(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    #[test]
    fn hash_changes_with_trailing_newline() {
        // A trailing-newline difference is visible.
        let a = content_hash(b"hello");
        let b = content_hash(b"hello\n");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_deterministic_across_calls() {
        let bytes = b"a stable run produces the same digest twice";
        assert_eq!(content_hash(bytes), content_hash(bytes));
    }
}
