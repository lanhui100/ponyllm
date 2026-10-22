//! Wave-9 deepseek-web — L2-T contract tests: compliance boundary static
//! scans (`dw_s_05`, `dw_g_01`, `dw_g_04`, `dw_g_05`; contract §3/§7).
//!
//! These rows assert NEGATIVE properties (absent mechanisms). They read the
//! frozen module sources at runtime and fail if task-2 ever introduces the
//! forbidden surface. During the stub phase the absence trivially holds
//! (design intent: the frozen stub contains none of them) — the mechanical
//! red/green enforcement for these rows is the contract's own STATIC receipts
//! (STATIC-5/STATIC-6, Amendment v1) which the executor must keep NONE.
//! `dw_g_04` additionally pins solver purity at runtime (stub `todo!()` →
//! red at this phase).

use ponyllm_core::deepseek_web::{solve_pow, PowChallenge};
use std::path::PathBuf;

fn module_sources() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/deepseek_web");
    let mut out = Vec::new();
    for name in ["mod.rs", "pow.rs", "client.rs", "sse.rs"] {
        out.push(dir.join(name));
    }
    out
}

fn source_lines() -> Vec<(String, usize, String)> {
    let mut lines = Vec::new();
    for p in module_sources() {
        let text = std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("static test: cannot read {}: {e}", p.display()));
        for (i, l) in text.lines().enumerate() {
            lines.push((p.file_name().unwrap().to_string_lossy().to_string(), i + 1, l.to_string()));
        }
    }
    lines
}

fn assert_absent(needles: &[&str], exempt_pred: impl Fn(&str) -> bool, what: &str) {
    for (file, line_no, text) in source_lines() {
        for n in needles {
            if text.contains(n) && !exempt_pred(&text) {
                panic!(
                    "{what}: {file}:{line_no} contains {n:?}: {text}\n\
                     (frozen contract DW-* forbids this surface in implementation code)"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DW-S-05 no guest mode: create_guest_challenge / X-DS-Guest-PoW (REA §1.3)
// ---------------------------------------------------------------------------

#[test]
fn dw_s_05_no_guest_endpoints() {
    assert_absent(&["create_guest_challenge", "X-DS-Guest-PoW"], |_| false, "DW-S-05");
}

// ---------------------------------------------------------------------------
// DW-G-01 no captcha/slider/fingerprint bypass; x-request-encryption-key out
// of scope (REA §2/§7; Amendment v1: static check = x-request-encryption-key
// true-negative + review; the words captcha/fingerprint are only allowed in
// the frozen explicit-error narration)
// ---------------------------------------------------------------------------

#[test]
fn dw_g_01_no_captcha_or_fingerprint_bypass() {
    // Hard true-negative per amended STATIC-6.
    assert_absent(&["x-request-encryption-key"], |_| false, "DW-G-01");

    // The module's only captcha/fingerprint mentions must be frozen narration
    // of the compliance boundary (Amendment v1: doc-comment boundary narration
    // and the CaptchaWall explicit-error surface are contract-frozen text, not
    // bypass implementations) — i.e. there is no bypass implementation hiding
    // under these words.
    for (file, line_no, text) in source_lines() {
        let mentions = text.contains("captcha") || text.contains("fingerprint") || text.contains("滑块");
        if !mentions {
            continue;
        }
        let stripped = text.trim_start();
        let is_doc_narration = (stripped.starts_with("///") || stripped.starts_with("//!") || stripped.starts_with("// "))
            && (text.contains("DW-G") || text.contains("DW-S") || text.contains("bypass")
                || text.contains("合规") || text.contains("guest") || text.contains("边界"));
        let is_frozen_narration = is_doc_narration
            || text.contains("CaptchaWall")
            || text.contains("x-amzn-waf-action")
            || text.contains("cf-mitigated")
            || text.contains("#[error(")
            || text.contains("fabricated fingerprint")
            // DW-G-02 explicit-error DETECTION logic (never bypass): matching
            // the WAF identifier value "captcha"/"challenge" on the response
            // path is the contract-mandated error surface.
            || (text.contains("captcha") && text.contains("eq_ignore_ascii_case"))
            || (text.contains("challenge") && text.contains("eq_ignore_ascii_case"));
        assert!(
            is_frozen_narration,
            "DW-G-01: {file}:{line_no} captcha/fingerprint mention is not the frozen \
             explicit-error surface: {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// DW-G-04 PoW: no replay/batch cache, no persistent storage (REA §7)
// ---------------------------------------------------------------------------

#[test]
fn dw_g_04_pow_no_replay_no_batch() {
    // Source-level: the solver surface must hold no persistent answer cache /
    // queue / disk state that could enable replay or pre-batching.
    assert_absent(
        &[
            "OnceLock", "LazyLock", "Mutex<", "RwLock<", "HashMap<", "File::",
            "fs::write", "OpenOptions", "mpsc", "channel(",
        ],
        |_| false,
        "DW-G-04",
    );

    // Runtime purity: same challenge twice -> identical fresh answers
    // (stateless recompute; a replay/queue would exhibit different behaviour
    // or skip work). solve_pow is a stub -> red at this phase.
    // Vector #2 (Amendment v2): prefix "s000_1729666800000_3", worker answer 3.
    let ch = PowChallenge {
        algorithm: "DeepSeekHashV1".into(),
        challenge: "98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a".into(),
        salt: "s000".into(),
        difficulty: 8,
        signature: "sig2".into(),
        expire_at: Some(1729666800000),
        expire_after: None,
    };
    let a = solve_pow(&ch, "/api/v0/chat/completion").expect("stub panics -> red");
    let b = solve_pow(&ch, "/api/v0/chat/completion").expect("stub panics -> red");
    assert_eq!(a.answer, b.answer, "DW-G-04: pure solver, no replay/caching");
    assert_eq!(a.answer, 3, "DW-G-04: worker-verified answer (cross-check anchor)");
}

// ---------------------------------------------------------------------------
// DW-G-05 user-held token only: no credential fabrication / evasion (REA §7)
// ---------------------------------------------------------------------------

#[test]
fn dw_g_05_token_only_no_evasion() {
    // No credential-generation surface, no hardcoded credential literals, and
    // any "Bearer " string literal exists only in doc narration (never in
    // code that fabricates an Authorization value).
    assert_absent(
        &["password", "client_secret", "refresh_token", "credential"],
        |l| {
            // Boundary narration in doc comments (Amendment v1 principle) is
            // contract-frozen text; only CODE occurrences are forbidden.
            let s = l.trim_start();
            (s.starts_with("///") || s.starts_with("//!") || s.starts_with("// "))
                && (l.contains("DW-G") || l.contains("DW-S") || l.contains("credential")
                    || l.contains("凭据"))
        },
        "DW-G-05",
    );
    for (file, line_no, text) in source_lines() {
        if !text.contains("Bearer ") {
            continue;
        }
        let s = text.trim_start();
        let is_doc = s.starts_with("///") || s.starts_with("//!") || s.starts_with("// ");
        // DW-S-03: constructing `Bearer {token}` from the caller-provided token
        // is the contract-mandated path (no fabrication). Only a hardcoded
        // credential literal (Bearer followed by a fixed secret) is forbidden.
        let is_callers_token = text.contains("{token}") || text.contains("{token");
        assert!(
            is_doc || is_callers_token,
            "DW-G-05: {file}:{line_no} hardcoded Bearer credential literal in code: {text}"
        );
    }
}