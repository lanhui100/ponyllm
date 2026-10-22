//! Wave-9 deepseek-web — L2-T functional contract tests: session API, client
//! request surface, completion body wire format, error mapping, WAF/429
//! compliance (`dw_s_*`, `dw_q_*`, `dw_e_*`, `dw_g_02`, `dw_g_03`; contract
//! `.dev-team/contracts/wave-9-deepseek-web.md` §3/§4/§6/§7).
//!
//! Red phase: every `dw_e_*`/`dw_s_*`/`dw_g_02/03` test exercises a frozen
//! `todo!()` stub and must FAIL. `dw_q_*` lock the already-frozen serde wire
//! surface (T1 types) — they are type-surface checks, green by design at red
//! time (the executable red for the Q-surface is `dw_s_04`, which drives the
//! client stub against the wire).

use ponyllm_core::deepseek_web::{
    classify_api_error, encode_pow_response_header, ChatSession, ClientError, CompletionRequest,
    DeepSeekWebClient, PowResponse,
};
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Mock upstream harness: records every request, answers per handler.
// ---------------------------------------------------------------------------

type Outcome = (StatusCode, Vec<(String, String)>, String);

fn make_app(
    handler: Arc<dyn Fn(Method, String, HeaderMap, Bytes) -> Outcome + Send + Sync>,
) -> axum::Router {
    axum::Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| async move {
            let (status, hdrs, out) = handler(method, uri.path().to_string(), headers, body);
            let mut builder = Response::builder().status(status);
            for (k, v) in hdrs {
                builder = builder.header(k, v);
            }
            builder.body(axum::body::Body::from(out)).unwrap()
        },
    )
}

async fn spawn_app(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// Records (method, path, headers, body) per request.
#[derive(Default, Clone)]
struct Capture {
    inner: Arc<Mutex<Vec<(Method, String, HeaderMap, String)>>>,
}
impl Capture {
    fn push(&self, method: Method, path: String, headers: HeaderMap, body: Bytes) {
        let text = String::from_utf8_lossy(&body).to_string();
        self.inner.lock().unwrap().push((method, path, headers, text));
    }
    fn snapshot(&self) -> Vec<(String, String, HeaderMap, String)> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(m, p, h, b)| (m.to_string(), p.clone(), h.clone(), b.clone()))
            .collect()
    }
    fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

fn client(base: &str, token: Option<String>) -> DeepSeekWebClient {
    DeepSeekWebClient {
        http: reqwest::Client::new(),
        base_url: base.to_string(),
        token,
        device_id: "11111111-2222-3333-4444-555555555555".to_string(),
    }
}

fn ok_json(body: serde_json::Value) -> Outcome {
    (
        StatusCode::OK,
        vec![("content-type".to_string(), "application/json".to_string())],
        body.to_string(),
    )
}

const TARGET_PATH: &str = "/api/v0/chat/completion";
const CHALLENGE_JSON: &str = r#"{"algorithm":"DeepSeekHashV1","challenge":"eb1cc3ae55acef598a8054febbf4d6f03a4f97556e0b958f9e7ddbbf47928f22","salt":"f9a1b2c3d4e5f60718293a4b5c6d7e8f","difficulty":100000,"signature":"a1b2c3d4e5f60718293a4b5c6d7e8f90"}"#;

fn challenge_envelope() -> Outcome {
    ok_json(json!({"data": {"biz_code": 0, "biz_data": {"challenge": serde_json::from_str::<serde_json::Value>(CHALLENGE_JSON).unwrap()}, "biz_msg": "ok"}}))
}

// ---------------------------------------------------------------------------
// DW-S-01 challenge request shape: method, path, body (REA §1.1; H-05)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_s_01_pow_challenge_request_shape() {
    let cap = Capture::default();
    let c2 = cap.clone();
    let app = make_app(Arc::new(move |method, path, _h, body| {
        c2.push(method, path, _h, body);
        challenge_envelope()
}));
    let base = spawn_app(app).await;
    let c = client(&base, Some("web-token-abc".into()));

    let _ = c.create_pow_challenge(TARGET_PATH).await.expect("stub panics -> red");

    let got = cap.snapshot();
    assert_eq!(got.len(), 1, "DW-S-01: exactly one challenge request");
    let (method, path, _h, body) = &got[0];
    assert_eq!(method, "POST", "DW-S-01: method must be POST");
    assert_eq!(path, "/api/v0/chat/create_pow_challenge", "DW-S-01: path must be /api/v0/...");
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        parsed,
        json!({"target_path": "/api/v0/chat/completion"}),
        "DW-S-01: request body must be exactly {{target_path}}"
    );
}

// ---------------------------------------------------------------------------
// DW-S-02 session create: empty body, id extraction, error on biz_code != 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_s_02_session_create_shape_and_id() {
    let cap = Capture::default();
    let c2 = cap.clone();
    let app = make_app(Arc::new(move |method, path, _h, body| {
        c2.push(method, path, _h, body);
        ok_json(json!({"data": {"biz_code": 0, "biz_data": {"chat_session": {"id": "sess-123", "title": "New chat", "updated_at": "2026-10-22T00:00:00Z"}}, "biz_msg": "ok"}}))
}));
    let base = spawn_app(app).await;
    let c = client(&base, Some("web-token-abc".into()));

    let session: ChatSession = c.create_chat_session().await.expect("stub panics -> red");

    let got = cap.snapshot();
    let (method, path, _h, body) = &got[0];
    assert_eq!(method, "POST");
    assert_eq!(path, "/api/v0/chat_session/create", "DW-S-02: session path");
    assert!(
        body.trim().is_empty() || serde_json::from_str::<serde_json::Value>(body).unwrap() == json!({}),
        "DW-S-02: body must be empty ({{}}), got {body:?}"
    );
    assert_eq!(session.id, "sess-123", "DW-S-02: chat_session_id = biz_data.chat_session.id");

    // biz_code != 0 -> error, no session produced.
    let app_err = make_app(Arc::new(move |_m, _p, _h, _b| {
        ok_json(json!({"data": {"biz_code": 50006, "biz_msg": "MUTED", "biz_data": null}}))
}));
    let base_err = spawn_app(app_err).await;
    let err = client(&base_err, Some("web-token-abc".into()))
        .create_chat_session()
        .await
        .expect_err("DW-S-02: non-zero biz_code must error (no silent session)");
    assert!(matches!(err, ClientError::Other(_)), "got: {err:?}");
}

// ---------------------------------------------------------------------------
// DW-S-03 client headers: with and without a user-held token (REA §2; H-04)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_s_03_client_headers_present() {
    let cap = Capture::default();
    let c2 = cap.clone();
    let app = make_app(Arc::new(move |method, path, h, body| {
        c2.push(method, path, h, body);
        challenge_envelope()
}));
    let base = spawn_app(app).await;
    let c = client(&base, Some("web-token-abc".into()));
    let _ = c.create_pow_challenge(TARGET_PATH).await.expect("stub panics -> red");

    let got = cap.snapshot();
    assert_eq!(got.len(), 1);
    let headers = &got[0].2;
    assert_eq!(
        headers.get("authorization").map(|v| v.to_str().unwrap()),
        Some("Bearer web-token-abc"),
        "DW-S-03: Authorization Bearer <token> when a token is held"
    );
    assert_eq!(headers.get("x-client-bundle-id").map(|v| v.to_str().unwrap()), Some("com.deepseek.chat"));
    assert_eq!(headers.get("x-client-platform").map(|v| v.to_str().unwrap()), Some("web"));
    assert_eq!(headers.get("x-client-locale").map(|v| v.to_str().unwrap()), Some("en_US"));
    assert_eq!(headers.get("x-client-timezone-offset").map(|v| v.to_str().unwrap()), Some("0"));
    let ver = headers.get("x-client-version").map(|v| v.to_str().unwrap()).unwrap_or("");
    assert!(!ver.is_empty(), "DW-S-03: x-client-version must be non-empty");
    assert_eq!(
        headers.get("x-device-id").map(|v| v.to_str().unwrap()),
        Some("11111111-2222-3333-4444-555555555555"),
        "DW-S-03: x-device-id = locally generated uuid, not a fingerprint"
    );

    // No token: no Authorization header, and no guest-PoW header (DW-S-05).
    let cap2 = Capture::default();
    let c2b = cap2.clone();
    let app2 = make_app(Arc::new(move |method, path, h, body| {
        c2b.push(method, path, h, body);
        challenge_envelope()
}));
    let base2 = spawn_app(app2).await;
    let c2 = client(&base2, None);
    let _ = c2.create_pow_challenge(TARGET_PATH).await.expect("stub panics -> red");
    let got2 = cap2.snapshot();
    assert_eq!(got2.len(), 1);
    let h2 = &got2[0].2;
    assert!(
        h2.get("authorization").is_none(),
        "DW-S-03: no token -> no Authorization header (never fabricate credentials)"
    );
    assert!(
        h2.get("x-ds-guest-pow-response").is_none(),
        "DW-S-05: guest PoW header must never be sent"
    );
    assert!(
        h2.get("x-ds-pow-response").is_none(),
        "DW-S-03: challenge request carries no pow answer header"
    );
}

// ---------------------------------------------------------------------------
// DW-S-04 completion body wire shape + SSE response (REA §3; H-07)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_s_04_completion_body_wire_shape() {
    let cap = Capture::default();
    let c2 = cap.clone();
    let app = make_app(Arc::new(move |method, path, h, body| {
        c2.push(method, path, h, body);
        (
            StatusCode::OK,
            vec![("content-type".to_string(), "text/event-stream".to_string())],
            "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"deepseek-chat\"}\n\nevent: close\ndata: {\"close_reason\":\"success\"}\n\n".to_string(),
        )
}));
    let base = spawn_app(app).await;
    let c = client(&base, Some("web-token-abc".into()));

    let req = CompletionRequest {
        chat_session_id: "sess-1".into(),
        parent_message_id: Some("parent-1".into()),
        model_type: "deepseek-chat".into(),
        prompt: "hello".into(),
        ref_file_ids: vec!["f1".into()],
        thinking_enabled: true,
        search_enabled: false,
        source: "default".into(),
        action: Some("continue".into()),
        preempt: true,
    };
    let pow = PowResponse {
        algorithm: "DeepSeekHashV1".into(),
        challenge: "eb1cc3ae55acef598a8054febbf4d6f03a4f97556e0b958f9e7ddbbf47928f22".into(),
        salt: "f9a1b2c3d4e5f60718293a4b5c6d7e8f".into(),
        answer: 42,
        signature: "a1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
        target_path: TARGET_PATH.into(),
    };

    let resp = c
        .stream_completion(&req, &pow)
        .await
        .expect("stub panics -> red");
    assert_eq!(
        resp.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("text/event-stream"),
        "DW-S-04: completion responds text/event-stream"
    );

    let got = cap.snapshot();
    let (method, path, headers, body) = &got[0];
    assert_eq!(method, "POST");
    assert_eq!(path, "/api/v0/chat/completion", "DW-S-04: completion path");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::to_value(&req).unwrap(),
        "DW-S-04: completion body == DW-Q-01 wire shape (checked in dw_q_01)"
    );
    let expected_header = encode_pow_response_header(&pow);
    assert_eq!(
        headers.get("x-ds-pow-response").and_then(|v| v.to_str().ok()),
        Some(expected_header.as_str()),
        "DW-S-04: X-DS-PoW-Response header (DW-P-05 encoding)"
    );
}

// ---------------------------------------------------------------------------
// DW-Q-01 exact JSON key set and types (REA §3; CompletionRequest frozen)
// ---------------------------------------------------------------------------

#[test]
fn dw_q_01_exact_key_set_and_types() {
    let full = CompletionRequest {
        chat_session_id: "sess-1".into(),
        parent_message_id: Some("parent-1".into()),
        model_type: "deepseek-chat".into(),
        prompt: "hello".into(),
        ref_file_ids: vec!["f1".into()],
        thinking_enabled: true,
        search_enabled: false,
        source: "default".into(),
        action: Some("continue".into()),
        preempt: true,
    };
    let j = serde_json::to_value(&full).unwrap();
    let obj = j.as_object().expect("DW-Q-01: request serializes as an object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    let expected = [
        "action",
        "chat_session_id",
        "model_type",
        "parent_message_id",
        "preempt",
        "prompt",
        "ref_file_ids",
        "search_enabled",
        "source",
        "thinking_enabled",
    ];
    assert_eq!(
        keys, expected,
        "DW-Q-01: exact 10-key set, no extras, no missing"
    );
    assert!(obj["chat_session_id"].is_string());
    assert!(obj["parent_message_id"].is_string());
    assert!(obj["model_type"].is_string());
    assert!(obj["prompt"].is_string());
    assert!(obj["ref_file_ids"].is_array());
    assert!(obj["ref_file_ids"][0].is_string());
    assert!(obj["thinking_enabled"].is_boolean());
    assert!(obj["search_enabled"].is_boolean());
    assert!(obj["source"].is_string());
    assert!(obj["action"].is_string());
    assert!(obj["preempt"].is_boolean());
}

// ---------------------------------------------------------------------------
// DW-Q-02 defaults: null/absent optionals, [] refs, false preempt
// ---------------------------------------------------------------------------

#[test]
fn dw_q_02_defaults_null_or_false() {
    let minimal = CompletionRequest {
        chat_session_id: "sess-1".into(),
        parent_message_id: None,
        model_type: "deepseek-chat".into(),
        prompt: "hi".into(),
        ref_file_ids: vec![],
        thinking_enabled: false,
        search_enabled: false,
        source: "default".into(),
        action: None,
        preempt: false,
    };
    let j = serde_json::to_value(&minimal).unwrap();
    let obj = j.as_object().unwrap();
    assert!(
        !obj.contains_key("parent_message_id"),
        "DW-Q-02: absent parent_message_id omitted (serde default+skip)"
    );
    assert!(!obj.contains_key("action"), "DW-Q-02: absent action omitted");
    assert_eq!(obj["ref_file_ids"], json!([]), "DW-Q-02: ref_file_ids defaults []");
    assert_eq!(obj["preempt"], json!(false), "DW-Q-02: preempt defaults false");

    // Explicit null must also be accepted (REA §3 `parent_message_id: null`).
    let with_null = serde_json::from_str::<CompletionRequest>(
        r#"{"chat_session_id":"s1","parent_message_id":null,"model_type":"m","prompt":"p","source":"default"}"#,
    )
    .unwrap();
    assert_eq!(with_null.parent_message_id, None, "DW-Q-02: explicit null -> None");
    assert_eq!(with_null.ref_file_ids, Vec::<String>::new());
    assert!(!with_null.preempt);
}

// ---------------------------------------------------------------------------
// DW-Q-03 (Amendment v4) source enum + thinking/search defaults (REA §3)
// ---------------------------------------------------------------------------

#[test]
fn dw_q_03_source_enum_and_flag_defaults() {
    // Source enum per Amendment v4 (real-chain smoke 2026-10-22): the server
    // accepts ONLY `default` | `landing` (anything else -> HTTP 422 "unknown
    // variant" — the old REA "web_chat"/"search" literals are DEBUNKED). The
    // GATEWAY default/passthrough value is FIXED to `default` (`landing` is
    // landing-page-specific, gateway never self-selects it).
    // (a) canonical gateway value "default" serializes verbatim
    let req_default = CompletionRequest {
        chat_session_id: "sess-1".into(),
        parent_message_id: None,
        model_type: "deepseek-chat".into(),
        prompt: "hello".into(),
        ref_file_ids: vec![],
        thinking_enabled: true,
        search_enabled: false,
        source: "default".into(),
        action: None,
        preempt: false,
    };
    let j = serde_json::to_value(&req_default).unwrap();
    assert_eq!(j["source"], json!("default"), "DW-Q-03: gateway source is 'default'");
    assert_eq!(j["thinking_enabled"], json!(true), "DW-Q-03: thinking default true");
    assert_eq!(j["search_enabled"], json!(false), "DW-Q-03: search default false");
    // (b) "landing" is the second (and only other) valid enum value
    let req_landing = CompletionRequest { source: "landing".into(), ..req_default };
    assert_eq!(
        serde_json::to_value(&req_landing).unwrap()["source"],
        json!("landing"),
        "DW-Q-03: landing is the server's only other valid source value"
    );
    // (c) any other value is server-invalid (422 unknown variant); the client
    // type is String so this is a gateway-side convention — the gateway MUST
    // emit "default" (asserted at the gateway acceptance layer via the
    // captured completion body). Values outside {default, landing} must
    // never be chosen by the gateway.
    assert!(
        !["web_chat", "search"].iter().any(|v| v == &req_default.source.as_str()),
        "DW-Q-03: debunked literals must not be the emitted source"
    );
}

// ---------------------------------------------------------------------------
// DW-E-01 429 -> RateLimited(Retry-After) (REA §6; H-08)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_e_01_429_rate_limited_retry_after() {
    // With Retry-After header: the client surfaces Some(Duration).
    let app = make_app(Arc::new(move |_m, _p, _h, _b| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("retry-after".to_string(), "2".to_string()),
            ],
            r#"{"biz_code":0,"biz_msg":"requestFrequencyTooHigh","biz_data":null}"#.to_string(),
        )
}));
    let base = spawn_app(app).await;
    let err = client(&base, Some("t".into()))
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect_err("DW-E-01: 429 must error");
    assert!(
        matches!(&err, ClientError::RateLimited(Some(d)) if d.as_secs() == 2),
        "DW-E-01: 429 + Retry-After:2 -> RateLimited(Some(2s)); got {err:?}"
    );

    // Without a header: classify can only see (status, body) -> None duration.
    let err2 = classify_api_error(429, r#"{"biz_code":0,"biz_msg":"requestFrequencyTooHigh"}"#);
    assert!(
        matches!(err2, ClientError::RateLimited(None)),
        "DW-E-01: classify(429) without header -> RateLimited(None); got {err2:?}"
    );
}

// ---------------------------------------------------------------------------
// DW-E-02 40012 USER_IS_BANNED -> Banned (REA §6; H-08)
// ---------------------------------------------------------------------------

#[test]
fn dw_e_02_40012_banned() {
    let err = classify_api_error(
        200,
        r#"{"biz_code":40012,"biz_msg":"USER_IS_BANNED","biz_data":null}"#,
    );
    assert!(
        matches!(err, ClientError::Banned),
        "DW-E-02: 40012 must map to Banned; got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// DW-E-03 40300/40301 -> InvalidPow (no infinite re-solve is a caller rule,
// pinned by DW-P-06 refetch window + DW-G-03 bounded retry)
// ---------------------------------------------------------------------------

#[test]
fn dw_e_03_40301_invalid_pow() {
    for code in ["40300", "40301"] {
        let err = classify_api_error(
            200,
            &format!(r#"{{"biz_code":{code},"biz_msg":"INVALID_POW_RESPONSE","biz_data":null}}"#),
        );
        assert!(
            matches!(err, ClientError::InvalidPow),
            "DW-E-03: {code} must map to InvalidPow; got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DW-E-04 40002/40003 -> Token (REA §6; H-08)
// ---------------------------------------------------------------------------

#[test]
fn dw_e_04_40003_token() {
    for (code, msg) in [("40002", "Missing Token"), ("40003", "Invalid Token")] {
        let err = classify_api_error(
            200,
            &format!(r#"{{"biz_code":{code},"biz_msg":"{msg}","biz_data":null}}"#),
        );
        assert!(
            matches!(err, ClientError::Token),
            "DW-E-04: {code} must map to Token; got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DW-E-05 transient: 5xx -> HttpStatus; connection failure -> Network
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_e_05_transient_errors_classified() {
    let err_503 = classify_api_error(503, "upstream boom");
    assert!(matches!(err_503, ClientError::HttpStatus(503)), "got {err_503:?}");
    let err_500 = classify_api_error(500, "boom");
    assert!(matches!(err_500, ClientError::HttpStatus(500)), "got {err_500:?}");

    // Connection refused -> Network (client hits a dead port).
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead); // port is now closed
    let err = client(&format!("http://{dead_addr}"), Some("t".into()))
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect_err("DW-E-05: refused connection must error");
    assert!(
        matches!(err, ClientError::Network(_)),
        "DW-E-05: connection failure -> Network; got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// DW-E-06 unknown non-zero biz_code -> Other with biz_msg surfaced
// ---------------------------------------------------------------------------

#[test]
fn dw_e_06_unknown_biz_code_surfaced() {
    let err = classify_api_error(
        200,
        r#"{"biz_code":99999,"biz_msg":"mystery business error","biz_data":null}"#,
    );
    match err {
        ClientError::Other(msg) => assert!(
            msg.contains("mystery business error"),
            "DW-E-06: biz_msg must be surfaced; got {msg:?}"
        ),
        other => panic!("DW-E-06: unknown biz_code -> Other; got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// DW-E-07 (SHOULD) 40029 / 50006 -> Other with clear biz_msg (no new consts)
// ---------------------------------------------------------------------------

#[test]
fn dw_e_07_restricted_and_muted() {
    for (code, msg) in [("40029", "IP_ACCESS_RESTRICTED"), ("50006", "MUTED")] {
        let err = classify_api_error(
            200,
            &format!(r#"{{"biz_code":{code},"biz_msg":"{msg}","biz_data":null}}"#),
        );
        match err {
            ClientError::Other(s) => assert!(
                s.contains(msg),
                "DW-E-07: {code} biz_msg must be surfaced; got {s:?}"
            ),
            other => panic!("DW-E-07: {code} -> Other (no new variant); got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// DW-G-02 WAF/captcha identifiers -> explicit CaptchaWall, zero retries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_g_02_waf_captcha_explicit_error() {
    let req = CompletionRequest {
        chat_session_id: "s1".into(),
        parent_message_id: None,
        model_type: "deepseek-chat".into(),
        prompt: "hi".into(),
        ref_file_ids: vec![],
        thinking_enabled: true,
        search_enabled: false,
        source: "default".into(),
        action: None,
        preempt: false,
    };
    let pow = PowResponse {
        algorithm: "DeepSeekHashV1".into(),
        challenge: "c".into(),
        salt: "s".into(),
        answer: 0,
        signature: "g".into(),
        target_path: TARGET_PATH.into(),
    };

    for (header, value, expect_in_message) in [
        ("x-amzn-waf-action", "captcha", "captcha"),
        ("cf-mitigated", "challenge", "challenge"),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let app = make_app(Arc::new(move |_m, _p, _h, _b| {
            c2.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                vec![
                    ("content-type".to_string(), "text/event-stream".to_string()),
                    (header.to_string(), value.to_string()),
                ],
                "event: close\ndata: {\"close_reason\":\"success\"}\n\n".to_string(),
            )
}));
        let base = spawn_app(app).await;
        let err = client(&base, Some("t".into()))
            .stream_completion(&req, &pow)
            .await
            .expect_err("DW-G-02: WAF identifier must error, never bypass");
        match &err {
            ClientError::CaptchaWall(ident) => assert!(
                ident.contains(expect_in_message),
                "DW-G-02: CaptchaWall carries the identifier原文; got {ident:?}"
            ),
            other => panic!("DW-G-02: {header}:{value} -> CaptchaWall; got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "DW-G-02: zero retries on WAF wall (no bypass attempts)"
        );
    }
}

// ---------------------------------------------------------------------------
// DW-G-03 429 bounded backoff: single Retry-After wait, then explicit error;
// no header -> immediate error, no retry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_g_03_429_bounded_backoff() {
    // With Retry-After:2 -> exactly 2 requests, >=2s wait, then explicit error.
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let app = make_app(Arc::new(move |_m, _p, _h, _b| {
        let n = c2.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            (
                StatusCode::TOO_MANY_REQUESTS,
                vec![("retry-after".to_string(), "2".to_string())],
                r#"{"biz_code":0,"biz_msg":"requestFrequencyTooHigh"}"#.to_string(),
            )
        } else {
            (
                StatusCode::TOO_MANY_REQUESTS,
                vec![],
                r#"{"biz_code":0,"biz_msg":"requestFrequencyTooHigh"}"#.to_string(),
            )
        }
}));
    let base = spawn_app(app).await;
    let started = std::time::Instant::now();
    let err = client(&base, Some("t".into()))
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect_err("DW-G-03: repeated 429 must error after bounded backoff");
    let elapsed = started.elapsed();
    assert!(
        matches!(err, ClientError::RateLimited(_)),
        "DW-G-03: final 429 -> explicit RateLimited error; got {err:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "DW-G-03: exactly 2 requests (one bounded Retry-After wait, no infinite retry)"
    );
    assert!(
        elapsed >= std::time::Duration::from_secs(2),
        "DW-G-03: waited for the server-indicated Retry-After; elapsed {:?}",
        elapsed
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "DW-G-03: backoff must be bounded, no unbounded spin; elapsed {:?}",
        elapsed
    );

    // No Retry-After header -> no wait, no retry: exactly 1 request, error.
    let calls2 = Arc::new(AtomicUsize::new(0));
    let c3 = calls2.clone();
    let app2 = make_app(Arc::new(move |_m, _p, _h, _b| {
        c3.fetch_add(1, Ordering::SeqCst);
        (
            StatusCode::TOO_MANY_REQUESTS,
            vec![],
            r#"{"biz_code":0,"biz_msg":"requestFrequencyTooHigh"}"#.to_string(),
        )
}));
    let base2 = spawn_app(app2).await;
    let _ = client(&base2, Some("t".into()))
        .create_pow_challenge(TARGET_PATH)
        .await
        .expect_err("DW-G-03: 429 without Retry-After must error immediately");
    assert_eq!(
        calls2.load(Ordering::SeqCst),
        1,
        "DW-G-03: no Retry-After -> no retry (exactly 1 request)"
    );
}