//! Executor-owned (task-2) integration tests for the DeepSeek Web client:
//! create_pow_challenge / create_chat_session / stream_completion against an
//! in-process axum mock upstream (random port), plus the frozen wire shapes.
//!
//! These are SEPARATE from the task-3 acceptance tests (T2-owned files); this
//! file is written by the implementing executor for its own verification gate
//! and is not part of the frozen acceptance suite.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::Engine;
use ponyllm_core::deepseek_web::{
    classify_api_error, encode_pow_response_header, solve_pow, ChatSession, ClientError,
    CompletionRequest, DeepSeekWebClient, PowChallenge, PowError, PowResponse,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct MockState {
    /// capture [method, path, headers, body] per request for shape assertions.
    seen: Mutex<Vec<(String, String, Vec<(String, String)>, String)>>,
    /// responses keyed by request URL path; cycle through the vec.
    responses: Mutex<HashMap<String, Vec<MockReply>>>,
    /// request counter per path (for bounded-retry assertions).
    counts: Mutex<HashMap<String, usize>>,
}

#[derive(Clone)]
struct MockReply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

fn parse_headers(vec: &[(String, String)]) -> HeaderMap {
    let mut hm = HeaderMap::new();
    for (k, v) in vec {
        let name = axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap();
        hm.insert(name, v.parse().unwrap());
    }
    hm
}

async fn mock_handler(
    State(st): State<Arc<MockState>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().as_str().to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body).to_string();
    st.seen.lock().unwrap().push((method, path.clone(), headers, body_str.clone()));

    let mut counts = st.counts.lock().unwrap();
    let n = counts.entry(path.clone()).or_insert(0);
    *n += 1;
    let idx = *n - 1;

    let r = st
        .responses
        .lock()
        .unwrap()
        .get(&path)
        .and_then(|v| v.get(idx.min(v.len() - 1)).cloned())
        .unwrap_or(MockReply {
            status: 404,
            headers: vec![],
            body: "{}".to_string(),
        });
    (StatusCode::from_u16(r.status).unwrap(), parse_headers(&r.headers), r.body).into_response()
}

/// Spin up an axum mock on a random port; returns the base URL.
async fn spawn_mock(responses: HashMap<String, Vec<MockReply>>) -> (String, Arc<MockState>) {
    let st = Arc::new(MockState {
        responses: Mutex::new(responses),
        ..Default::default()
    });
    let app = Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            post(mock_handler),
        )
        .route("/api/v0/chat_session/create", post(mock_handler))
        .route("/api/v0/chat/completion", post(mock_handler))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), st)
}

fn client_at(base: &str, token: Option<&str>) -> DeepSeekWebClient {
    let mut c = DeepSeekWebClient::new(token.map(|t| t.to_string()), "00000000-0000-4000-8000-000000000001".to_string());
    c.base_url = base.to_string();
    c
}

const CHALLENGE_JSON_CAMEL: &str = r#"{"biz_code":0,"biz_msg":"ok","biz_data":{"challenge":{"algorithm":"DeepSeekHashV1","challenge":"98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a","salt":"s000","difficulty":8,"signature":"sig2","expireAt":1729666800000,"expireAfter":60}}}"#;

#[tokio::test]
async fn create_pow_challenge_parses_and_sends_shape() {
    let (base, st) = spawn_mock(HashMap::from([(
        "/api/v0/chat/create_pow_challenge".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: CHALLENGE_JSON_CAMEL.to_string() }],
    )]))
    .await;
    let c = client_at(&base, Some("tok-abc"));
    let ch = c.create_pow_challenge("/api/v0/chat/completion").await.unwrap();
    assert_eq!(ch.algorithm, "DeepSeekHashV1");
    assert_eq!(ch.challenge.len(), 64);
    assert_eq!(ch.salt, "s000");
    assert_eq!(ch.difficulty, 8);
    assert_eq!(ch.signature, "sig2");
    assert_eq!(ch.expire_at, Some(1729666800000));
    assert_eq!(ch.expire_after, Some(60));

    let seen = st.seen.lock().unwrap();
    let (method, path, headers, body) = &seen[0];
    assert_eq!(method, "POST");
    assert_eq!(path, "/api/v0/chat/create_pow_challenge");
    let body_v: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body_v, json!({"target_path": "/api/v0/chat/completion"}));
    let hmap: HashMap<&str, &str> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(hmap.get("x-client-bundle-id"), Some(&"com.deepseek.chat"));
    assert_eq!(hmap.get("x-client-platform"), Some(&"web"));
    assert_eq!(hmap.get("x-client-version"), Some(&"2.5.0"));
    assert_eq!(hmap.get("x-client-locale"), Some(&"en_US"));
    assert_eq!(hmap.get("x-client-timezone-offset"), Some(&"0"));
    assert_eq!(hmap.get("x-device-id"), Some(&"00000000-0000-4000-8000-000000000001"));
    assert_eq!(hmap.get("authorization"), Some(&"Bearer tok-abc"));
}

#[tokio::test]
async fn create_pow_challenge_accepts_snake_case_wire_keys() {
    let snake = r#"{"data":{"biz_code":0,"biz_msg":"ok","biz_data":{"challenge":{"algorithm":"DeepSeekHashV1","challenge":"aa","salt":"s","difficulty":1,"signature":"sig","expire_at":7,"expire_after":30}}}}"#;
    let (base, _) = spawn_mock(HashMap::from([(
        "/api/v0/chat/create_pow_challenge".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: snake.to_string() }],
    )]))
    .await;
    let c = client_at(&base, None);
    let ch = c.create_pow_challenge("/p").await.unwrap();
    assert_eq!(ch.expire_at, Some(7));
    assert_eq!(ch.expire_after, Some(30));
}

#[tokio::test]
async fn create_pow_challenge_maps_biz_error() {
    let banned = r#"{"biz_code":40012,"biz_msg":"USER_IS_BANNED","biz_data":null}"#;
    let (base, _) = spawn_mock(HashMap::from([(
        "/api/v0/chat/create_pow_challenge".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: banned.to_string() }],
    )]))
    .await;
    let c = client_at(&base, Some("tok"));
    match c.create_pow_challenge("/p").await {
        Err(ClientError::Banned) => {}
        other => panic!("expected Banned, got {other:?}"),
    }
}

#[tokio::test]
async fn create_chat_session_parses_id_and_empty_body() {
    let ok = r#"{"biz_code":0,"biz_msg":"ok","biz_data":{"chat_session":{"id":"sess-42","title":"New chat","updated_at":"2026-10-22T00:00:00Z"}}}"#;
    let (base, st) = spawn_mock(HashMap::from([(
        "/api/v0/chat_session/create".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: ok.to_string() }],
    )]))
    .await;
    let c = client_at(&base, Some("tok"));
    let s: ChatSession = c.create_chat_session().await.unwrap();
    assert_eq!(s.id, "sess-42");
    assert_eq!(s.title.as_deref(), Some("New chat"));
    let seen = st.seen.lock().unwrap();
    let (_, path, _, body) = &seen[0];
    assert_eq!(path, "/api/v0/chat_session/create");
    assert_eq!(serde_json::from_str::<Value>(body).unwrap(), json!({}));
}

#[tokio::test]
async fn create_chat_session_nonzero_biz_errors() {
    let bad = r#"{"biz_code":40003,"biz_msg":"Invalid Token","biz_data":null}"#;
    let (base, _) = spawn_mock(HashMap::from([(
        "/api/v0/chat_session/create".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: bad.to_string() }],
    )]))
    .await;
    let c = client_at(&base, Some("bad-token"));
    match c.create_chat_session().await {
        Err(ClientError::Token) => {}
        other => panic!("expected Token, got {other:?}"),
    }
}

#[tokio::test]
async fn completion_full_chain_pow_header_and_body() {
    // challenge -> solve -> completion. Mock upstream asserts the
    // X-DS-PoW-Response decodes to the exact six-key JSON.
    let (base, st) = spawn_mock(HashMap::from([
        (
            "/api/v0/chat/create_pow_challenge".to_string(),
            vec![MockReply { status: 200, headers: vec![], body: CHALLENGE_JSON_CAMEL.to_string() }],
        ),
        (
            "/api/v0/chat/completion".to_string(),
            vec![MockReply {
                status: 200,
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\n".to_string(),
            }],
        ),
    ]))
    .await;
    let c = client_at(&base, Some("tok"));
    let ch = c.create_pow_challenge("/api/v0/chat/completion").await.unwrap();
    let pow = solve_pow(&ch, "/api/v0/chat/completion").unwrap();
    assert_eq!(pow.answer, 3); // golden vector (s000/sig2, expireAt 1729666800000, difficulty 8, i0=3)

    let req = CompletionRequest {
        chat_session_id: "sess-42".to_string(),
        parent_message_id: Some("parent-1".to_string()),
        model_type: "deepseek-chat".to_string(),
        prompt: "hello".to_string(),
        ref_file_ids: vec![],
        thinking_enabled: true,
        search_enabled: false,
        source: "default".to_string(),
        action: Some("continue".to_string()),
        preempt: false,
    };
    let resp = c.stream_completion(&req, &pow).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let seen = st.seen.lock().unwrap();
    let (_, path, headers, body) = &seen[1];
    assert_eq!(path, "/api/v0/chat/completion");
    let hmap: HashMap<&str, &str> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let pow_header = hmap
        .get("x-ds-pow-response")
        .expect("X-DS-PoW-Response header present");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(pow_header)
        .expect("pow header is valid base64");
    let pow_v: Value = serde_json::from_slice(&decoded).unwrap();
    let obj = pow_v.as_object().unwrap();
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
    assert_eq!(obj["answer"], 3);
    assert_eq!(obj["target_path"], "/api/v0/chat/completion");
    assert_eq!(obj["signature"], "sig2");

    // request body key set is exact (DW-Q-01) with explicit Some fields
    let req_v: Value = serde_json::from_str(body).unwrap();
    let req_obj = req_v.as_object().unwrap();
    let expected_keys = [
        "chat_session_id",
        "parent_message_id",
        "model_type",
        "prompt",
        "ref_file_ids",
        "thinking_enabled",
        "search_enabled",
        "source",
        "action",
        "preempt",
    ];
    assert_eq!(req_obj.keys().map(|k| k.as_str()).collect::<Vec<_>>(), expected_keys.to_vec());
    assert_eq!(req_obj["chat_session_id"], "sess-42");
    assert_eq!(req_obj["parent_message_id"], "parent-1");
    assert_eq!(req_obj["model_type"], "deepseek-chat");
    assert_eq!(req_obj["prompt"], "hello");
    assert_eq!(req_obj["ref_file_ids"], json!([]));
    assert_eq!(req_obj["thinking_enabled"], true);
    assert_eq!(req_obj["search_enabled"], false);
    assert_eq!(req_obj["source"], "default");
    assert_eq!(req_obj["action"], "continue");
    assert_eq!(req_obj["preempt"], false);
}

#[tokio::test]
async fn completion_defaults_omit_nullable_keys() {
    // DW-Q-02: parent_message_id/action omitted when None (frozen serde attrs)
    let (base, st) = spawn_mock(HashMap::from([(
        "/api/v0/chat/completion".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: "ok".to_string() }],
    )]))
    .await;
    let c = client_at(&base, None);
    let req = CompletionRequest {
        chat_session_id: "s".to_string(),
        parent_message_id: None,
        model_type: "m".to_string(),
        prompt: "p".to_string(),
        ref_file_ids: vec![],
        thinking_enabled: false,
        search_enabled: false,
        source: "default".to_string(),
        action: None,
        preempt: false,
    };
    let pow = PowChallenge {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: "aa".repeat(32),
        salt: "s".to_string(),
        difficulty: 0,
        signature: "sig".to_string(),
        expire_at: Some(1),
        expire_after: None,
    };
    // difficulty 0 → NoSolution; craft the PowResponse directly instead.
    let _ = pow;
    let resp = PowResponse {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: "aa".repeat(32),
        salt: "s".to_string(),
        answer: 0,
        signature: "sig".to_string(),
        target_path: "/api/v0/chat/completion".to_string(),
    };
    c.stream_completion(&req, &resp).await.unwrap();
    let seen = st.seen.lock().unwrap();
    let body = &seen[0].3;
    let v: Value = serde_json::from_str(body).unwrap();
    let obj = v.as_object().unwrap();
    assert!(!obj.contains_key("parent_message_id"));
    assert!(!obj.contains_key("action"));
    assert!(obj.contains_key("ref_file_ids"));
    assert!(obj.contains_key("preempt"));
}

#[tokio::test]
async fn waf_headers_fail_explicitly_without_retry() {
    for (header, value) in [
        ("x-amzn-waf-action", "captcha"),
        ("cf-mitigated", "challenge"),
    ] {
        let (base, st) = spawn_mock(HashMap::from([(
            "/api/v0/chat/completion".to_string(),
            vec![MockReply {
                status: 200,
                headers: vec![(header.to_string(), value.to_string())],
                body: "blocked".to_string(),
            }],
        )]))
        .await;
        let c = client_at(&base, Some("tok"));
        let resp = PowResponse {
            algorithm: "DeepSeekHashV1".to_string(),
            challenge: "aa".repeat(32),
            salt: "s".to_string(),
            answer: 0,
            signature: "sig".to_string(),
            target_path: "/api/v0/chat/completion".to_string(),
        };
        let req = CompletionRequest {
            chat_session_id: "s".to_string(),
            parent_message_id: None,
            model_type: "m".to_string(),
            prompt: "p".to_string(),
            ref_file_ids: vec![],
            thinking_enabled: false,
            search_enabled: false,
            source: "default".to_string(),
            action: None,
            preempt: false,
        };
        match c.stream_completion(&req, &resp).await {
            Err(ClientError::CaptchaWall(m)) if m == value => {}
            other => panic!("expected CaptchaWall({value}), got {other:?}"),
        }
        let counts = st.counts.lock().unwrap();
        assert_eq!(counts.get("/api/v0/chat/completion"), Some(&1), "zero retries on WAF wall");
    }
}

#[tokio::test]
async fn completion_429_bounded_backoff_exactly_twice_then_success() {
    let (base, st) = spawn_mock(HashMap::from([(
        "/api/v0/chat/completion".to_string(),
        vec![
            MockReply {
                status: 429,
                headers: vec![("retry-after".to_string(), "0".to_string())],
                body: r#"{"error":"requestFrequencyTooHigh"}"#.to_string(),
            },
            MockReply { status: 200, headers: vec![], body: "ok".to_string() },
        ],
    )]))
    .await;
    let c = client_at(&base, Some("tok"));
    let resp = PowResponse {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: "aa".repeat(32),
        salt: "s".to_string(),
        answer: 0,
        signature: "sig".to_string(),
        target_path: "/api/v0/chat/completion".to_string(),
    };
    let req = CompletionRequest {
        chat_session_id: "s".to_string(),
        parent_message_id: None,
        model_type: "m".to_string(),
        prompt: "p".to_string(),
        ref_file_ids: vec![],
        thinking_enabled: false,
        search_enabled: false,
        source: "default".to_string(),
        action: None,
        preempt: false,
    };
    let _ = c.stream_completion(&req, &resp).await.expect("second attempt succeeds");
    let counts = st.counts.lock().unwrap();
    assert_eq!(counts.get("/api/v0/chat/completion"), Some(&2), "exactly one bounded retry");
}

#[tokio::test]
async fn completion_429_without_retry_after_fails_immediately() {
    let (base, st) = spawn_mock(HashMap::from([(
        "/api/v0/chat/completion".to_string(),
        vec![MockReply {
            status: 429,
            headers: vec![],
            body: "{}".to_string(),
        }],
    )]))
    .await;
    let c = client_at(&base, Some("tok"));
    let resp = PowResponse {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: "aa".repeat(32),
        salt: "s".to_string(),
        answer: 0,
        signature: "sig".to_string(),
        target_path: "/api/v0/chat/completion".to_string(),
    };
    let req = CompletionRequest {
        chat_session_id: "s".to_string(),
        parent_message_id: None,
        model_type: "m".to_string(),
        prompt: "p".to_string(),
        ref_file_ids: vec![],
        thinking_enabled: false,
        search_enabled: false,
        source: "default".to_string(),
        action: None,
        preempt: false,
    };
    match c.stream_completion(&req, &resp).await {
        Err(ClientError::RateLimited(None)) => {}
        other => panic!("expected RateLimited(None), got {other:?}"),
    }
    let counts = st.counts.lock().unwrap();
    assert_eq!(counts.get("/api/v0/chat/completion"), Some(&1), "no retry without Retry-After");
}

#[test]
fn classify_api_error_maps_envelope_codes() {
    // 429 (DW-E-01)
    assert!(matches!(classify_api_error(429, "{}"), ClientError::RateLimited(None)));
    // USER_IS_BANNED (DW-E-02)
    assert!(matches!(
        classify_api_error(200, r#"{"biz_code":40012,"biz_msg":"USER_IS_BANNED"}"#),
        ClientError::Banned
    ));
    // INVALID_POW_RESPONSE (DW-E-03)
    assert!(matches!(
        classify_api_error(200, r#"{"biz_code":40301,"biz_msg":"INVALID_POW_RESPONSE"}"#),
        ClientError::InvalidPow
    ));
    // Invalid Token (DW-E-04)
    assert!(matches!(
        classify_api_error(200, r#"{"biz_code":40003,"biz_msg":"Invalid Token"}"#),
        ClientError::Token
    ));
    // wrapped in data (server actually nests under .data)
    assert!(matches!(
        classify_api_error(200, r#"{"data":{"biz_code":40029,"biz_msg":"IP_ACCESS_RESTRICTED"}}"#),
        ClientError::Other(ref m) if m == "IP_ACCESS_RESTRICTED"
    ));
    // unknown biz_code surfaces msg (DW-E-06/07)
    assert!(matches!(
        classify_api_error(200, r#"{"biz_code":50006,"biz_msg":"MUTED"}"#),
        ClientError::Other(ref m) if m == "MUTED"
    ));
    // HTTP status fallback
    assert!(matches!(classify_api_error(502, "<html>"), ClientError::HttpStatus(502)));
}

#[test]
fn encode_header_smoke_matches_roundtrip() {
    let resp = PowResponse {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: "ab".repeat(32),
        salt: "s".to_string(),
        answer: 1,
        signature: "sig".to_string(),
        target_path: "/api/v0/chat/completion".to_string(),
    };
    let header = encode_pow_response_header(&resp);
    let decoded = base64::engine::general_purpose::STANDARD.decode(&header).unwrap();
    let v: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(v["answer"], 1);
}

#[test]
fn solve_pow_no_solution_smoke() {
    let ch = PowChallenge {
        algorithm: "DeepSeekHashV1".to_string(),
        challenge: "ff".repeat(32),
        salt: "s".to_string(),
        difficulty: 4,
        signature: "sig".to_string(),
        expire_at: Some(1),
        expire_after: None,
    };
    assert_eq!(solve_pow(&ch, "/p"), Err(PowError::NoSolution));
}