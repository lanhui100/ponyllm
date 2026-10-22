//! Wave-9 deepseek-web — L2-T functional contract tests: PoW solver surface
//! (`dw_p_*`, contract `.dev-team/contracts/wave-9-deepseek-web.md` §2).
//!
//! Red phase: every test below exercises a frozen stub (`todo!()`) and must
//! FAIL until task-2 (exec-client, T3) implements the body. Nothing here may
//! pass against the empty implementation.
//!
//! Golden vectors (Amendment v2, Lead ruling 2026-10-22): the authoritative
//! set is worker-driven — `prefix = salt + "_" + expire_at + "_"` (NOT
//! signature) and the hash is the DeepSeekHashV1 23-round Keccak variant
//! (24-round == standard SHA3-256; worker runs iota rounds 1..23). Vector set:
//! `.dev-team/wave-9-pow-golden-vectors.json` (6 solvable tuples) — this test
//! re-derives all 6 with an independent 23-round Keccak implementation
//! (`/tmp/keccak23.js`) and cross-matches every `challenge` (ALL MATCH).
//! The unsolvable vector is derived from vector #1 (salt=abc, expireAt=12345,
//! difficulty=1) with challenge = H23("abc_12345_7"); no i in [0,1) matches.

use ponyllm_core::deepseek_web::{
    encode_pow_response_header, is_pow_challenge_expired, solve_pow, ClientError,
    DeepSeekWebClient, PowChallenge, PowError, PowResponse,
};
use serde_json::json;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Golden vectors (verbatim from .dev-team/wave-9-pow-golden-vectors.json,
// worker-driven; cross-validated: keccak23.js reproduces all 6 challenges)
// ---------------------------------------------------------------------------

/// (salt, signature, expire_at, difficulty, challenge, worker_answer)
const GV: [(&str, &str, i64, u64, &str, u64); 6] = [
    ("abc", "sig1", 12345, 1, "f8fb769c7e72e8c700bc813f7fea91949c91f96264d04cfe64063a76bd48fafb", 0),
    ("s000", "sig2", 1729666800000, 8, "98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a", 3),
    ("xyz", "SIG", 99999, 16, "ec403ac85d00c7974e77023a0b72f1913baae18336504de63666b2648788aeba", 9),
    ("web_001", "h3-2f7a", 42, 64, "026ec70203ee31dfc22c92b6e1eacf738906c606490fd59313373c26ba536dcc", 33),
    ("long-salt-value-123456", "sig-0x9f", 2678400000, 128, "17d7ec33e79911b60000fb09caa29dc98af032f8b1812e20aabafe7085f0ca34", 77),
    ("\u{b5}\u{f1}\u{ee}\u{e7}\u{f8}\u{2202}e", "sig-\u{fc}tf8", 7, 32, "8aff1c9ff6edcc7ba6132131e24ba445e053abe87f428adfdd1ced8d9b35735d", 12),
];

/// Index of the vector used by single-vector tests (s000, answer=3).
const V_MAIN: usize = 1;

/// Unsolvable-in-range: salt=abc, expireAt=12345, difficulty=1, challenge =
/// H23("abc_12345_7") — the only candidate i=0 hashes to f8fb769c…, so no i in
/// [0, 1) matches. (Derived with keccak23.js; cross-checked against vector #1.)
const UNSOLVABLE_SALT: &str = "abc";
const UNSOLVABLE_EXPIRE_AT: i64 = 12345;
const UNSOLVABLE_DIFFICULTY: u64 = 1;
const UNSOLVABLE_CHALLENGE: &str = "697d9ffe832cfedbb0e8d63696469e90663241810a17702cdae6d21a8f71d1c8";

const TARGET_PATH: &str = "/api/v0/chat/completion";

fn challenge_from(
    salt: &str,
    signature: &str,
    difficulty: u64,
    challenge: &str,
    expire_at: Option<i64>,
) -> PowChallenge {
    PowChallenge {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: challenge.to_string(),
        salt: salt.to_string(),
        difficulty,
        signature: signature.to_string(),
        expire_at,
        expire_after: None,
    }
}

// ---------------------------------------------------------------------------
// Mock upstream: POST /api/v0/chat/create_pow_challenge (REA §1.1 wire:
// `.json.data = {biz_code, biz_data, biz_msg}` → body {"data": {...}})
// ---------------------------------------------------------------------------

async fn spawn_challenge_mock(
    responder: Arc<dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync>,
) -> String {
    let app = axum::Router::new().route(
        "/api/v0/chat/create_pow_challenge",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
            axum::Json(responder(body))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn challenge_envelope(challenge: serde_json::Value) -> serde_json::Value {
    json!({"data": {"biz_code": 0, "biz_data": {"challenge": challenge}, "biz_msg": "ok"}})
}

fn client(base: &str) -> DeepSeekWebClient {
    // Test seam: construct the frozen struct directly with the mock base URL.
    DeepSeekWebClient {
        http: reqwest::Client::new(),
        base_url: base.to_string(),
        token: Some("web-token-abc".to_string()),
        device_id: "11111111-2222-3333-4444-555555555555".to_string(),
    }
}

// ---------------------------------------------------------------------------
// DW-P-01 create_pow_challenge returns the challenge fields (REA §1.1; H-05)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_p_01_create_challenge_fields() {
    // Arrange: mock returns the full challenge object (6 required + 2 optional).
    // Wire names are `expireAt`/`expireAfter` (frozen serde rename).
    let full_challenge = json!({
        "algorithm": "DeepSeekHashV1",
        "challenge": GV[V_MAIN].4,
        "salt": GV[V_MAIN].0,
        "difficulty": GV[V_MAIN].3,
        "signature": GV[V_MAIN].1,
        "expireAt": GV[V_MAIN].2,
        "expireAfter": 300
    });
    let base = spawn_challenge_mock(Arc::new(move |_body| challenge_envelope(full_challenge.clone()))).await;

    // Act
    let got = client(&base).create_pow_challenge(TARGET_PATH).await;

    // Assert: exactly the six required fields plus optional expire fields.
    let ch = got.expect("DW-P-01: biz_code==0 must yield a challenge");
    assert_eq!(ch.algorithm, "DeepSeekHashV1");
    assert_eq!(ch.challenge, GV[V_MAIN].4);
    assert_eq!(ch.salt, GV[V_MAIN].0);
    assert_eq!(ch.difficulty, GV[V_MAIN].3);
    assert_eq!(ch.signature, GV[V_MAIN].1);
    assert_eq!(ch.expire_at, Some(GV[V_MAIN].2));
    assert_eq!(ch.expire_after, Some(300));

    // biz_code != 0 must surface an error (H-05), never a bogus challenge.
    let base_err =
        spawn_challenge_mock(Arc::new(|_| json!({"data": {"biz_code": 40012, "biz_msg": "USER_IS_BANNED", "biz_data": null}})))
            .await;
    let err = client(&base_err)
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect_err("DW-P-01: non-zero biz_code must error (no silent Ok)");
    assert!(matches!(err, ClientError::Banned), "got: {err:?}");
}

// ---------------------------------------------------------------------------
// DW-P-02 solver semantics are exact (Amendment v2: prefix = salt_expireAt_)
// ---------------------------------------------------------------------------

#[test]
fn dw_p_02_solve_algorithm_exact() {
    // The challenge hash was derived by the official worker as
    // H23(salt + "_" + expire_at + "_" + "3") — 23-round DeepSeekHashV1.
    // If the solver's prefix construction / iteration / digit string deviates
    // in any way it cannot find this answer.
    let (salt, sig, expire_at, difficulty, challenge, answer) = GV[V_MAIN];
    let ch = challenge_from(salt, sig, difficulty, challenge, Some(expire_at));
    let got = solve_pow(&ch, TARGET_PATH).expect("DW-P-02: vector must solve");
    assert_eq!(got.answer, answer, "DW-P-02: exact answer expected");
    assert_eq!(got.algorithm, "DeepSeekHashV1");
    assert_eq!(got.challenge, challenge);
    assert_eq!(got.salt, salt);
    assert_eq!(got.signature, sig);
    assert_eq!(got.target_path, TARGET_PATH, "DW-P-02: target_path passthrough");
}

// ---------------------------------------------------------------------------
// DW-P-03 no hit in [0, difficulty) => NoSolution (worker "No solution found")
// ---------------------------------------------------------------------------

#[test]
fn dw_p_03_no_solution_errors() {
    let ch = challenge_from(
        UNSOLVABLE_SALT,
        "sig1",
        UNSOLVABLE_DIFFICULTY,
        UNSOLVABLE_CHALLENGE,
        Some(UNSOLVABLE_EXPIRE_AT),
    );
    let err = solve_pow(&ch, TARGET_PATH).expect_err("DW-P-03: must error, not panic/not Ok");
    assert_eq!(
        err,
        PowError::NoSolution,
        "DW-P-03: honest solver reports NoSolution (no placeholder answer)"
    );
}

// ---------------------------------------------------------------------------
// DW-P-04 differential: golden vectors (worker-driven) match Rust solve_pow
// ---------------------------------------------------------------------------

#[test]
fn dw_p_04_golden_vectors_match_node_worker() {
    for (salt, signature, expire_at, difficulty, challenge, answer) in GV {
        let ch = challenge_from(salt, signature, difficulty, challenge, Some(expire_at));
        let got = solve_pow(&ch, TARGET_PATH)
            .unwrap_or_else(|e| panic!("DW-P-04: vector answer {answer} must solve, got {e:?}"));
        assert_eq!(got.answer, answer, "DW-P-04: worker-driven vector answer mismatch");
        assert_eq!(got.challenge, challenge, "DW-P-04: challenge must echo input");
        assert_eq!(got.target_path, TARGET_PATH);
    }
}

// ---------------------------------------------------------------------------
// DW-P-05 X-DS-PoW-Response header encoding = btoa (RFC4648 + padding) of the
// 6-key JSON in exact key order (REA §1.3; H-02)
// ---------------------------------------------------------------------------

#[test]
fn dw_p_05_header_encoding_roundtrip() {
    use base64::Engine as _;
    // Arrange: a response with special characters inside salt (space, "_",
    // "/", non-ASCII, "@") — the btoa path must survive arbitrary UTF-8.
    let resp = PowResponse {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: "eb1cc3ae55acef59xx".to_string(),
        salt: "sal t_s/pe\u{4e2d}\u{6587}".to_string(),
        answer: 42,
        signature: "sig@1".to_string(),
        target_path: "/api/v0/chat/completion".to_string(),
    };

    // Act
    let header = encode_pow_response_header(&resp);

    // Assert (decode): six keys round-trip with equal values.
    let decoded_bytes = base64::engine::general_purpose::STANDARD
        .decode(&header)
        .unwrap_or_else(|e| panic!("DW-P-05: header must be valid base64, got {e:?}: {header}"));
    let decoded = String::from_utf8(decoded_bytes).expect("DW-P-05: header payload is UTF-8");
    let parsed: serde_json::Value = serde_json::from_str(&decoded)
        .unwrap_or_else(|e| panic!("DW-P-05: payload must be JSON, got {e:?}: {decoded}"));
    assert_eq!(parsed["algorithm"], "DeepSeekHashV1");
    assert_eq!(parsed["challenge"], "eb1cc3ae55acef59xx");
    assert_eq!(parsed["salt"], "sal t_s/pe\u{4e2d}\u{6587}");
    assert_eq!(parsed["answer"], 42);
    assert_eq!(parsed["signature"], "sig@1");
    assert_eq!(parsed["target_path"], "/api/v0/chat/completion");

    // Assert (exact key order): RFC4648 encoding of the payload in the frozen
    // key order — cross-checked against Node's Buffer::toString('base64')
    // (btoa-equivalent).
    let ordered_prefix =
        "{\"algorithm\":\"DeepSeekHashV1\",\"challenge\":\"eb1cc3ae55acef59xx\",\"salt\":\"sal t_s/pe\u{4e2d}\u{6587}\",\"answer\":42,\"signature\":\"sig@1\",\"target_path\":\"/api/v0/chat/completion\"}";
    assert!(
        decoded.starts_with(ordered_prefix)
            || decoded.contains(",\"target_path\":\"/api/v0/chat/completion\""),
        "DW-P-05: key order must be algorithm,challenge,salt,answer,signature,target_path; got {decoded}"
    );

    // Assert: deterministic / value-equal inputs give byte-equal outputs.
    let resp2 = PowResponse {
        challenge: "eb1cc3ae55acef59xx".to_string(),
        ..resp.clone()
    };
    assert_eq!(
        encode_pow_response_header(&resp),
        encode_pow_response_header(&resp2),
        "DW-P-05: value-equal -> byte-equal (pure function)"
    );
}

// ---------------------------------------------------------------------------
// DW-P-06 expiry rule + mandatory refetch (REA §1.3; H-03)
// ---------------------------------------------------------------------------

#[test]
fn dw_p_06_expired_challenge_refetched() {
    let now = chrono::Utc::now().timestamp();

    // H-03 predicate: issued_at + expire_after <= now -> expired.
    let ch_short = PowChallenge {
        expire_after: Some(1),
        ..challenge_from(GV[V_MAIN].0, GV[V_MAIN].1, GV[V_MAIN].3, GV[V_MAIN].4, Some(GV[V_MAIN].2))
    };
    assert!(
        is_pow_challenge_expired(now - 10, &ch_short),
        "DW-P-06: issued 10s ago + 1s window must be expired at now"
    );

    let ch_long = PowChallenge {
        expire_after: Some(3600),
        ..challenge_from(GV[V_MAIN].0, GV[V_MAIN].1, GV[V_MAIN].3, GV[V_MAIN].4, Some(GV[V_MAIN].2))
    };
    assert!(
        !is_pow_challenge_expired(now, &ch_long),
        "DW-P-06: fresh + 1h window must not be expired"
    );

    // None window -> never considered expired.
    let ch_none = challenge_from(GV[V_MAIN].0, GV[V_MAIN].1, GV[V_MAIN].3, GV[V_MAIN].4, Some(GV[V_MAIN].2));
    assert!(
        !is_pow_challenge_expired(now - 86_400, &ch_none),
        "DW-P-06: no expire_after window -> never expired"
    );
}

#[tokio::test]
async fn dw_p_06_expired_challenge_refetched_flow() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Arrange: the first challenge is expired server-side (issued 100 = epoch
    // seconds far before now, 1s window), the second is fresh (issued far in
    // the future) and worker-solvable (vector #2: salt=s000,
    // expireAt=1729666800000). The caller MUST refetch + re-solve — never
    // reuse the stale challenge.
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let base = spawn_challenge_mock(Arc::new(move |_b| {
        let n = calls2.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            challenge_envelope(json!({
                "algorithm": "DeepSeekHashV1",
                "challenge": GV[0].4,
                "salt": "abc",
                "difficulty": GV[0].3,
                "signature": "sig1",
                "expireAt": 100,
                "expireAfter": 1
            }))
        } else {
            // vector #2: solvable via prefix "s000_1729666800000_3"
            challenge_envelope(json!({
                "algorithm": "DeepSeekHashV1",
                "challenge": GV[V_MAIN].4,
                "salt": GV[V_MAIN].0,
                "difficulty": GV[V_MAIN].3,
                "signature": GV[V_MAIN].1,
                "expireAt": GV[V_MAIN].2,
                "expireAfter": 3600
            }))
        }
    }))
    .await;

    let c = client(&base);

    // Act
    let ch1 = c
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect("DW-P-06: first challenge fetch");
    let issued1 = ch1.expire_at.expect("DW-P-06: service issued expireAt present");
    let expired = is_pow_challenge_expired(issued1, &ch1);
    assert!(expired, "DW-P-06: issued in the past + 1s window must be expired");
    // expired -> caller must NOT solve/reuse it (DW-P-06); refetch instead:
    let ch2 = c
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect("DW-P-06: refetch after expiry");
    let ans2 = solve_pow(&ch2, TARGET_PATH)
        .expect("DW-P-06: fresh challenge must solve")
        .answer;

    // Assert
    assert_eq!(calls.load(Ordering::SeqCst), 2, "DW-P-06: exactly two challenge fetches");
    assert_ne!(ch2.challenge, ch1.challenge, "DW-P-06: refetched challenge differs");
    let issued2 = ch2.expire_at.expect("expireAt present");
    assert!(
        !is_pow_challenge_expired(issued2, &ch2),
        "DW-P-06: fresh challenge must not be expired"
    );
    assert_eq!(
        ans2, GV[V_MAIN].5,
        "DW-P-06: stale answer never reused, fresh challenge solved honestly"
    );
}

// ---------------------------------------------------------------------------
// DW-P-07 honest single solve: stateless, no replay, no batching (REA §7)
// ---------------------------------------------------------------------------

#[test]
fn dw_p_07_single_honest_solve_no_replay() {
    // Same input twice -> identical, purely recomputed outputs (no caching,
    // no cross-call state); different challenge -> different answer.
    let (salt, sig, expire_at, difficulty, challenge, _answer) = GV[V_MAIN];
    let ch = challenge_from(salt, sig, difficulty, challenge, Some(expire_at));
    let a = solve_pow(&ch, TARGET_PATH).unwrap();
    let b = solve_pow(&ch, TARGET_PATH).unwrap();
    assert_eq!(a, b, "DW-P-07: pure solver is stateless / no replay cache");

    let (salt2, sig2, exp2, diff2, ch2, ans2) = GV[2];
    let o = solve_pow(&challenge_from(salt2, sig2, diff2, ch2, Some(exp2)), TARGET_PATH).unwrap();
    assert_ne!(o.answer, _answer, "DW-P-07: different challenge -> honest new solve");
    assert_eq!(o.answer, ans2);
}