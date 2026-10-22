//! DeepSeekHashV1 PoW solver (wave-9 contract DW-P-*, H-01/02/03).
//!
//! ## Hash semantics (differentially validated, executable truth)
//!
//! The official PoW worker `/tmp/ds-pow76608.js` (module 88387) implements a
//! Keccak sponge with **SHA3-256 parameters** — rate 136 B (capacity 512 bit),
//! output 32 B, padding `0x06` + `0x80` — but runs **23 permutation rounds**
//! (iota indices `1..=23`, skipping round 0). Pinned by two independent
//! executable checks (see `.dev-team/exec-client-done-wave-9.json`):
//! * the 24-round variant of the same code equals standard SHA3-256
//!   (`sha3("") = a7ffc6f8…`, `sha3("abc") = 3a985da7…`);
//! * running the actual bundle in Node shows it solves only challenges built
//!   with the 23-round hash.
//!
//! ## Prefix semantics (deviation note, Lead-adjudicated)
//!
//! Contract text DW-P-02 says `prefix = salt + "_" + signature + "_"`, but
//! probing the *executed* worker proves the prefix is
//! `salt + "_" + expireAt + "_"` (the value under `expireAt`, the field the
//! frontend maps from `expire_at`): with `challenge = H23(salt_expireAt_i)`
//! the worker returns `i`, and with `challenge = H23(salt_signature_i)` it
//! raises "No solution found". REA §1.2 text derived the binding from
//! minified names and explicitly defers to worker execution
//! ("实现必须以 worker 为准做差分验证…不得仅凭此处推导"). Real challenges
//! always carry `expire_at`; a defensive `signature` fallback is kept only
//! for challenges without `expire_at` (never seen in production).

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Challenge object from `POST /api/v0/chat/create_pow_challenge`
/// (REA §1.1): wire `expire_at`/`expire_after` map to `expireAt`/`expireAfter`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowChallenge {
    /// Algorithm name, e.g. `"DeepSeekHashV1"` (asserted before solving).
    pub algorithm: String,
    /// Expected hex digest to match.
    pub challenge: String,
    /// Salt segment of the hash prefix.
    pub salt: String,
    /// Search upper bound: `i in [0, difficulty)`.
    pub difficulty: u64,
    /// Signature segment echoed in the `X-DS-PoW-Response` header.
    pub signature: String,
    /// Issued-at (mapped from `expire_at`); the worker hashes it into the
    /// prefix: `prefix = salt + "_" + expire_at + "_"` (see module docs).
    #[serde(rename = "expireAt", default, skip_serializing_if = "Option::is_none")]
    pub expire_at: Option<i64>,
    /// Validity window in seconds (mapped from `expire_after`).
    #[serde(rename = "expireAfter", default, skip_serializing_if = "Option::is_none")]
    pub expire_after: Option<u64>,
}

/// PoW answer carried in the `X-DS-PoW-Response` header (REA §1.3):
/// `base64(JSON{algorithm, challenge, salt, answer, signature, target_path})`
/// in this exact key order (H-02, DW-P-05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowResponse {
    pub algorithm: String,
    pub challenge: String,
    pub salt: String,
    pub answer: u64,
    pub signature: String,
    pub target_path: String,
}

/// Official algorithm name (REA §1.2).
pub const ALGORITHM_DEEPSEEK_HASH_V1: &str = "DeepSeekHashV1";

/// Keccak-f[1600] round constants (FIPS 202); the worker applies iota
/// constants `1..=23` (23 rounds, skipping round 0).
const KECCAK_RC: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

/// Rotation offsets r[x][y] of Keccak-f[1600] rho step (FIPS 202).
const RHO: [[u32; 5]; 5] = [
    [0, 36, 3, 41, 18],
    [1, 44, 10, 45, 2],
    [62, 6, 43, 15, 61],
    [28, 55, 25, 21, 56],
    [27, 20, 39, 8, 14],
];

/// One Keccak-f[1600] permutation of `state` (flat index `x + 5y`).
///
/// `start_round` + `rounds` selects the working round range (iota indices).
/// Standard SHA3-256 uses `start_round = 0, rounds = 24`; the official
/// DeepSeek PoW worker uses `start_round = 1, rounds = 23`.
fn keccak_f1600(state: &mut [u64; 25], start_round: usize, rounds: usize) {
    for round in start_round..start_round + rounds {
        // theta
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                state[x + 5 * y] ^= d;
            }
        }
        // rho + pi: B[y][2x+3y] = rot(A[x][y], rho[x][y])
        let mut b = [0u64; 25];
        for x in 0..5 {
            for y in 0..5 {
                b[y + 5 * ((2 * x + 3 * y) % 5)] = state[x + 5 * y].rotate_left(RHO[x][y]);
            }
        }
        // chi
        for x in 0..5 {
            for y in 0..5 {
                state[x + 5 * y] =
                    b[x + 5 * y] ^ ((!b[(x + 1) % 5 + 5 * y]) & b[(x + 2) % 5 + 5 * y]);
            }
        }
        // iota
        state[0] ^= KECCAK_RC[round];
    }
}

/// Absorb the final padded block of a rate-136 sponge and squeeze 32 bytes.
///
/// Mirrors the worker's `squeeze`: the pad block is `input || 0x06 || 0…0`
/// with the last rate byte `|= 0x80`; after the permutation the first 32
/// state bytes (lanes 0..=3, little-endian) are the digest.
fn pow_hash_hex_23_rounds(input: &str) -> String {
    const RATE: usize = 136;
    let bytes = input.as_bytes();
    // pad10*1 with domain byte 0x06 (SHA3): 0x06 immediately after the
    // message, 0x80 at the last byte of the final rate block. When 0x06
    // already completes the block (len == rate-1 mod rate), it shares its
    // byte with 0x80 (0x86) — mirroring the worker's `queue[off]|=6;
    // queue[rate-1]|=0x80` squeeze.
    // (boundary len%rate==rate-1 verified against the worker, see tests)
    let pad_len = (RATE - ((bytes.len() + 1) % RATE)) % RATE;
    let mut buf = Vec::with_capacity(bytes.len() + 1 + pad_len);
    buf.extend_from_slice(bytes);
    buf.push(0x06);
    buf.resize(bytes.len() + 1 + pad_len, 0);
    *buf.last_mut().expect("pad block non-empty") |= 0x80;

    let mut state = [0u64; 25];
    for chunk in buf.chunks_exact(RATE) {
        for (i, lane_bytes) in chunk.chunks_exact(8).enumerate() {
            let mut lane = [0u8; 8];
            lane.copy_from_slice(lane_bytes);
            state[i] ^= u64::from_le_bytes(lane);
        }
        // 23 rounds, iota 1..=23 — the DeepSeekDigest worker semantics.
        keccak_f1600(&mut state, 1, 23);
    }

    let mut out = String::with_capacity(64);
    for lane in &state[..4] {
        for byte in lane.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

/// Solve `DeepSeekHashV1` honestly for the given challenge (DW-P-02/03).
///
/// `prefix = salt + "_" + expire_at + "_"` (worker-verified; see module
/// docs) — or `salt + "_" + signature + "_"` when `expire_at` is absent —
/// then the first `i in [0, difficulty)` with
/// `hex(H23(prefix + String(i))) == challenge` is the answer.
///
/// Returns `PowError::NoSolution` when no `i` matches (DW-P-03). Pure CPU,
/// zero I/O, zero clocks (expiry is H-03's job).
pub fn solve_pow(challenge: &PowChallenge, target_path: &str) -> Result<PowResponse, PowError> {
    if challenge.algorithm != ALGORITHM_DEEPSEEK_HASH_V1 {
        return Err(PowError::Other(format!(
            "unsupported algorithm: {}",
            challenge.algorithm
        )));
    }
    // See module-level deviation note. `expire_at` participates in the
    // prefix per the executed worker; `signature` only as a defensive
    // fallback for challenges that omit `expire_at`.
    let prefix = match challenge.expire_at {
        Some(expire_at) => format!("{}_{}_", challenge.salt, expire_at),
        None => format!("{}_{}_", challenge.salt, challenge.signature),
    };
    for i in 0..challenge.difficulty {
        let candidate = format!("{prefix}{i}");
        if pow_hash_hex_23_rounds(&candidate) == challenge.challenge {
            return Ok(PowResponse {
                algorithm: challenge.algorithm.clone(),
                challenge: challenge.challenge.clone(),
                salt: challenge.salt.clone(),
                answer: i,
                signature: challenge.signature.clone(),
                target_path: target_path.to_string(),
            });
        }
    }
    Err(PowError::NoSolution)
}

/// Encode a PoW answer into the `X-DS-PoW-Response` header value (DW-P-05):
/// standard base64 with padding (`btoa` semantics) of the 6-key JSON object
/// in `PowResponse` field order (algorithm, challenge, salt, answer,
/// signature, target_path; serde_json `preserve_order` keeps it).
pub fn encode_pow_response_header(resp: &PowResponse) -> String {
    let json = serde_json::to_string(resp).expect("PowResponse serialization is infallible");
    STANDARD.encode(json)
}

/// REA §1.3 expiry rule: expired when `issued_at_unix + expire_after <= now`;
/// `None` window is never considered expired. Expired challenges MUST be
/// re-fetched (DW-P-06) — answers are never reused across challenges.
pub fn is_pow_challenge_expired(issued_at_unix: i64, challenge: &PowChallenge) -> bool {
    match challenge.expire_after {
        Some(secs) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            issued_at_unix.saturating_add(secs as i64) <= now
        }
        None => false,
    }
}

/// PoW solver errors (frozen; contract DW-P-03 / DW-P-06).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PowError {
    /// No `i` in `[0, difficulty)` matched (worker raises "No solution found").
    #[error("no solution found within difficulty")]
    NoSolution,
    /// Challenge expired before solving; caller must refetch (DW-P-06).
    #[error("pow challenge expired")]
    Expired,
    /// Any other solver failure.
    #[error("pow error: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Low-level sponge test oracle: standard 24-round SHA3-256 (iota 0..24).
    /// Implemented here without the `sha3` crate so the crate itself stays an
    /// independent cross-check (see below).
    fn sha3_256_24_rounds_hex(input: &str) -> String {
        const RATE: usize = 136;
        let bytes = input.as_bytes();
        let pad_len = (RATE - ((bytes.len() + 1) % RATE)) % RATE;
        let mut buf = Vec::with_capacity(bytes.len() + 1 + pad_len);
        buf.extend_from_slice(bytes);
        buf.push(0x06);
        buf.resize(bytes.len() + 1 + pad_len, 0);
        *buf.last_mut().expect("pad block non-empty") |= 0x80;
        let mut state = [0u64; 25];
        for chunk in buf.chunks_exact(RATE) {
            for (i, lane_bytes) in chunk.chunks_exact(8).enumerate() {
                let mut lane = [0u8; 8];
                lane.copy_from_slice(lane_bytes);
                state[i] ^= u64::from_le_bytes(lane);
            }
            keccak_f1600(&mut state, 0, 24);
        }
        let mut out = String::new();
        for lane in &state[..4] {
            for byte in lane.to_le_bytes() {
                out.push_str(&format!("{byte:02x}"));
            }
        }
        out
    }

    /// Cross-check the sponge core against the audited `sha3` crate:
    /// with 24 rounds and identical padding the digest must equal
    /// `sha3::Digest::Sha3_256` on several inputs (incl. block-boundary
    /// crossing and non-ASCII).
    #[test]
    fn keccak_core_matches_sha3_crate_at_24_rounds() {
        use sha3::Digest;
        let filler = "x";
        let long135 = filler.repeat(135);
        let long136 = filler.repeat(136);
        let long271 = filler.repeat(271);
        let cases: Vec<&str> = vec![
            "",
            "a",
            "abc",
            "prefix_sig_42",
            &long135,
            &long136,
            &long271,
            "µñîçø∂e_with_utf8",
        ];
        for input in cases {
            let mine = sha3_256_24_rounds_hex(input);
            let theirs: String = sha3::Sha3_256::digest(input.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(mine, theirs, "24-round keccak diverges from sha3 crate for {input:?}");
        }
    }

    /// The worker hash (23 rounds) differs from standard SHA3-256 — pinned
    /// digests, produced by running the official bundle's module 88387 in
    /// Node (see .dev-team/exec-client-done-wave-9.json). This is the
    /// `DeepSeekHashV1` hash H23.
    #[test]
    fn pow_hash_is_23_round_worker_semantics() {
        let cases = [
            ("", "e594808bc5b7151ac160c6d39a02e0a8e261ed588578403099e3561dc40c26b3"),
            ("abc", "f841106c601ce9be9bc38525e90d4178d47f21dd8eb9f238fc55ffaa4ca94506"),
            // computed by the worker-derived reference for a PoW-length input
            ("abc_12345_0", "f8fb769c7e72e8c700bc813f7fea91949c91f96264d04cfe64063a76bd48fafb"),
        ];
        for (input, expected) in cases {
            let got = pow_hash_hex_23_rounds(input);
            assert_eq!(got, expected, "23-round worker hash mismatch for {input:?}");
        }
    }

    #[test]
    fn solve_pow_matches_worker_golden_vectors() {
        // Golden vectors: generated by driving the official worker bundle in
        // Node — challenge = H23(salt + "_" + expireAt + "_" + i0), then the
        // worker's own solver returns i0 (see golden generator output).
        let vectors: &[(&str, &str, i64, u64, u64, &str)] = &[
            ("abc", "sig1", 12345, 1, 0, "f8fb769c7e72e8c700bc813f7fea91949c91f96264d04cfe64063a76bd48fafb"),
            ("s000", "sig2", 1729666800000, 8, 3, "98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a"),
            ("xyz", "SIG", 99999, 16, 9, "ec403ac85d00c7974e77023a0b72f1913baae18336504de63666b2648788aeba"),
            ("web_001", "h3-2f7a", 42, 64, 33, "026ec70203ee31dfc22c92b6e1eacf738906c606490fd59313373c26ba536dcc"),
            ("long-salt-value-123456", "sig-0x9f", 2678400000, 128, 77, "17d7ec33e79911b60000fb09caa29dc98af032f8b1812e20aabafe7085f0ca34"),
            ("µñîçø∂e", "sig-ütf8", 7, 32, 12, "8aff1c9ff6edcc7ba6132131e24ba445e053abe87f428adfdd1ced8d9b35735d"),
        ];
        for (salt, signature, expire_at, difficulty, want_answer, challenge) in vectors {
            let c = PowChallenge {
                algorithm: ALGORITHM_DEEPSEEK_HASH_V1.to_string(),
                challenge: (*challenge).to_string(),
                salt: (*salt).to_string(),
                difficulty: *difficulty,
                signature: (*signature).to_string(),
                expire_at: Some(*expire_at),
                expire_after: None,
            };
            let got = solve_pow(&c, "/api/v0/chat/completion").expect("worker-validated vector must solve");
            assert_eq!(got.answer, *want_answer, "answer mismatch for salt={salt}");
            assert_eq!(got.algorithm, ALGORITHM_DEEPSEEK_HASH_V1);
            assert_eq!(got.challenge, *challenge);
            assert_eq!(got.salt, *salt);
            assert_eq!(got.signature, *signature);
            assert_eq!(got.target_path, "/api/v0/chat/completion");
        }
    }

    #[test]
    fn solve_pow_no_solution_errors() {
        // No i in [0, difficulty) matches a hard-coded 64-hex digest.
        let c = PowChallenge {
            algorithm: ALGORITHM_DEEPSEEK_HASH_V1.to_string(),
            challenge: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string(),
            salt: "s".to_string(),
            difficulty: 8,
            signature: "sig".to_string(),
            expire_at: Some(1),
            expire_after: None,
        };
        assert_eq!(solve_pow(&c, "/api/v0/chat/completion"), Err(PowError::NoSolution));
    }

    #[test]
    fn solve_pow_rejects_unknown_algorithm() {
        let c = PowChallenge {
            algorithm: "SHA2-whatever".to_string(),
            challenge: String::new(),
            salt: "s".to_string(),
            difficulty: 1,
            signature: "sig".to_string(),
            expire_at: Some(1),
            expire_after: None,
        };
        assert!(matches!(solve_pow(&c, "/p"), Err(PowError::Other(_))));
    }

    #[test]
    fn encode_header_roundtrips_six_keys_in_order() {
        let resp = PowResponse {
            algorithm: ALGORITHM_DEEPSEEK_HASH_V1.to_string(),
            challenge: "aa".repeat(32),
            salt: "s<>\"&µ".to_string(), // special chars survive JSON+base64
            answer: 42,
            signature: "sig".to_string(),
            target_path: "/api/v0/chat/completion".to_string(),
        };
        let encoded = encode_pow_response_header(&resp);
        let decoded = STANDARD.decode(&encoded).expect("valid base64 with padding");
        let value: serde_json::Value = serde_json::from_slice(&decoded).expect("json roundtrip");
        let obj = value.as_object().expect("header payload is an object");
        // exact key set and order
        let keys: Vec<&String> = obj.keys().collect();
        assert_eq!(
            keys,
            vec![
                &"algorithm".to_string(),
                &"challenge".to_string(),
                &"salt".to_string(),
                &"answer".to_string(),
                &"signature".to_string(),
                &"target_path".to_string(),
            ]
        );
        assert_eq!(obj["algorithm"], "DeepSeekHashV1");
        assert_eq!(obj["answer"], 42);
        assert_eq!(obj["target_path"], "/api/v0/chat/completion");
    }

    #[test]
    fn expiry_rule_matches_contract() {
        let mk = |expire_after: Option<u64>| PowChallenge {
            algorithm: ALGORITHM_DEEPSEEK_HASH_V1.to_string(),
            challenge: "0".repeat(64),
            salt: "s".to_string(),
            difficulty: 1,
            signature: "sig".to_string(),
            expire_at: Some(0),
            expire_after,
        };
        // None window → never expired.
        assert!(!is_pow_challenge_expired(0, &mk(None)));
        // far future issued_at + long window → not expired.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        assert!(!is_pow_challenge_expired(now - 10, &mk(Some(3600))));
        // issued_at + window <= now → expired. Construct issued_at so the
        // boundary is deterministic: issued_at = now - 3600 → +3600 == now.
        assert!(is_pow_challenge_expired(now - 3600, &mk(Some(3600))));
        // window already elapsed → expired.
        assert!(is_pow_challenge_expired(now - 7200, &mk(Some(3600))));
    }
}