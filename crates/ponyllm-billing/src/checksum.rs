//! Content checksums for migration files.
//!
//! The digest is a lowercase hexadecimal SHA-256 over the exact bytes of the
//! migration file. Any edit to an applied migration changes the checksum and
//! the runner refuses to continue (fail closed).

use sha2::{Digest, Sha256};

/// Lowercase hexadecimal SHA-256 of `bytes`.
pub fn checksum(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector_matches_sha256() {
        // FIPS 180-4 / RFC 6234 test vector for the empty message.
        assert_eq!(
            checksum(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            checksum(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn is_stable_lowercase_hex_and_sensitive_to_bytes() {
        let a = checksum(b"SELECT 1;");
        let b = checksum(b"SELECT 1;");
        let c = checksum(b"SELECT 1; ");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
        assert!(a
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch)));
    }
}
