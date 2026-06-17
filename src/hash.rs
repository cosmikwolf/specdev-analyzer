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
