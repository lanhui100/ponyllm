//! Executor-owned (task-6, wave-10) integration tests for the DeepSeek Web
//! image-upload client: upload_file / fetch_files / wait_file_ready against an
//! in-process axum mock upstream (random port). Separate from the wave-10
//! acceptance tests (task-8 owned).

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use base64::Engine;
use ponyllm_core::deepseek_web::{
    classify_api_error, DeepSeekWebClient, FileParseStatus,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct MockState {
    /// capture [method, uri, headers, body] per request
    seen: Mutex<Vec<(String, String, Vec<(String, String)>, String)>>,
    responses: Mutex<HashMap<String, Vec<MockReply>>>,
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
    let uri = req.uri().to_string();
    let method = req.method().as_str().to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024).await.unwrap();
    let body_str = String::from_utf8_lossy(&body).to_string();
    st.seen.lock().unwrap().push((method, uri.clone(), headers, body_str.clone()));

    let path = uri.split('?').next().unwrap_or(&uri).to_string();
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
        .unwrap_or(MockReply { status: 404, headers: vec![], body: "{}".to_string() });
    (StatusCode::from_u16(r.status).unwrap(), parse_headers(&r.headers), r.body).into_response()
}

async fn spawn_mock(responses: HashMap<String, Vec<MockReply>>) -> (String, Arc<MockState>) {
    let st = Arc::new(MockState {
        responses: Mutex::new(responses),
        ..Default::default()
    });
    let app = Router::new()
        .route("/api/v0/chat/create_pow_challenge", post(mock_handler))
        .route("/api/v0/file/upload_file", post(mock_handler))
        .route("/api/v0/file/fetch_files", get(mock_handler))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), st)
}

fn client_at(base: &str) -> DeepSeekWebClient {
    let mut c = DeepSeekWebClient::new(
        Some("tok-a".to_string()),
        "00000000-0000-4000-8000-000000000001".to_string(),
    );
    c.base_url = base.to_string();
    c
}

/// Solvable wave-9-validated challenge (golden vector: salt abc / sig1 /
/// expireAt 12345 / difficulty 1 / i0 0).
const CHALLENGE: &str = r#"{"biz_code":0,"biz_data":{"challenge":{"algorithm":"DeepSeekHashV1","challenge":"f8fb769c7e72e8c700bc813f7fea91949c91f96264d04cfe64063a76bd48fafb","salt":"abc","difficulty":1,"signature":"sig1","expireAt":12345,"expireAfter":60}}}"#;

const UPLOAD_OK: &str = r#"{"code":0,"data":{"biz_code":0,"biz_data":{"id":"file-4bb9f696","status":"PENDING","file_name":"tiny.png","file_size":97,"model_kind":"VISION","is_image":true,"audit_result":"unknown"}}}"#;

fn png_bytes() -> Vec<u8> {
    // arbitrary 97-byte binary payload standing in for a PNG
    (0..97u8).collect()
}

#[tokio::test]
async fn upload_file_multipart_shape_and_pow_header() {
    let (base, st) = spawn_mock(HashMap::from([
        (
            "/api/v0/chat/create_pow_challenge".to_string(),
            vec![MockReply { status: 200, headers: vec![], body: CHALLENGE.to_string() }],
        ),
        (
            "/api/v0/file/upload_file".to_string(),
            vec![MockReply { status: 200, headers: vec![], body: UPLOAD_OK.to_string() }],
        ),
    ]))
    .await;
    let c = client_at(&base);
    let file = c.upload_file("tiny.png", "image/png", png_bytes()).await.unwrap();

    assert_eq!(file.id, "file-4bb9f696");
    assert_eq!(file.status, FileParseStatus::Pending);
    assert_eq!(file.file_name, "tiny.png");
    assert_eq!(file.file_size, 97);
    assert_eq!(file.model_kind, "VISION");
    assert!(file.is_image);
    assert_eq!(file.audit_result, "unknown");

    let seen = st.seen.lock().unwrap();
    // request 0: fresh PoW challenge for the upload target
    let (m0, u0, _, b0) = &seen[0];
    assert_eq!(m0, "POST");
    assert_eq!(u0, "/api/v0/chat/create_pow_challenge");
    assert_eq!(serde_json::from_str::<Value>(b0).unwrap(), json!({"target_path": "/api/v0/file/upload_file"}));

    // request 1: multipart upload with X-DS-PoW-Response
    let (m1, u1, h1, b1) = &seen[1];
    assert_eq!(m1, "POST");
    assert_eq!(u1, "/api/v0/file/upload_file");
    // exactly ONE content-type header and it must be the multipart boundary
    // form (regression guard for the append-duplication bug, defect B)
    let hmap: HashMap<&str, &str> =
        h1.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let cts: Vec<&str> = h1
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(cts.len(), 1, "content-type headers: {cts:?}");
    let ct = cts[0];
    assert!(ct.starts_with("multipart/form-data; boundary="), "content-type: {ct}");
    let pow_header = hmap.get("x-ds-pow-response").expect("PoW header present");
    let decoded = base64::engine::general_purpose::STANDARD.decode(pow_header).unwrap();
    let pow_v: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(pow_v["algorithm"], "DeepSeekHashV1");
    assert_eq!(pow_v["answer"], 0);
    assert_eq!(pow_v["target_path"], "/api/v0/file/upload_file");
    assert_eq!(pow_v["salt"], "abc");
    assert_eq!(pow_v["signature"], "sig1");
    // multipart part shape: field name `file`, filename, content-type
    assert!(b1.contains("name=\"file\""), "field name: {b1}");
    assert!(b1.contains("filename=\"tiny.png\""), "filename: {b1}");
    assert!(b1.contains("Content-Type: image/png"), "part content-type: {b1}");
}

#[tokio::test]
async fn upload_file_biz_code_error_maps() {
    let banned = r#"{"code":0,"data":{"biz_code":40012,"biz_msg":"USER_IS_BANNED","biz_data":null}}"#;
    let (base, _) = spawn_mock(HashMap::from([
        (
            "/api/v0/chat/create_pow_challenge".to_string(),
            vec![MockReply { status: 200, headers: vec![], body: CHALLENGE.to_string() }],
        ),
        (
            "/api/v0/file/upload_file".to_string(),
            vec![MockReply { status: 200, headers: vec![], body: banned.to_string() }],
        ),
    ]))
    .await;
    let c = client_at(&base);
    match c.upload_file("x.png", "image/png", png_bytes()).await {
        Err(ponyllm_core::deepseek_web::ClientError::Banned) => {}
        other => panic!("expected Banned, got {other:?}"),
    }
}

#[tokio::test]
async fn upload_file_http_error_classifies() {
    let (base, _) = spawn_mock(HashMap::from([
        (
            "/api/v0/chat/create_pow_challenge".to_string(),
            vec![MockReply { status: 200, headers: vec![], body: CHALLENGE.to_string() }],
        ),
        (
            "/api/v0/file/upload_file".to_string(),
            vec![MockReply {
                status: 429,
                headers: vec![("retry-after".to_string(), "0".to_string())],
                body: "{}".to_string(),
            }],
        ),
    ]))
    .await;
    let c = client_at(&base);
    match c.upload_file("x.png", "image/png", png_bytes()).await {
        // Retry-After:0 -> one bounded retry -> second 429 -> explicit error
        Err(ponyllm_core::deepseek_web::ClientError::RateLimited(Some(_))) => {}
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn fetch_files_get_query_and_unknown_status_tolerance() {
    let resp = r#"{"code":0,"data":{"biz_code":0,"biz_data":{"file_infos":[
        {"id":"file-a","status":"SUCCESS","file_name":"a.png","file_size":1,"is_image":true},
        {"id":"file-b","status":"SOME_FUTURE_STATE","file_size":2}
    ]}}}"#;
    let (base, st) = spawn_mock(HashMap::from([(
        "/api/v0/file/fetch_files".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: resp.to_string() }],
    )]))
    .await;
    let c = client_at(&base);
    // safety net so a harness hiccup can never block the whole suite
    let run = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        async {
            let files = c
                .fetch_files(&["file-a".to_string(), "file-b".to_string()])
                .await
                .expect("fetch ok");
            assert_eq!(files.len(), 2);
            assert_eq!(files[0].id, "file-a");
            assert_eq!(files[0].status, FileParseStatus::Success);
            assert_eq!(files[0].is_image, true);
            // unknown status strings are tolerated, secondary fields default
            assert_eq!(
                files[1].status,
                FileParseStatus::Unknown("SOME_FUTURE_STATE".to_string())
            );
            assert_eq!(files[1].file_name, "");
            assert!(!files[1].is_image);

            // snapshot the captured request, drop the guard, then assert
            let captured = {
                let seen = st.seen.lock().unwrap();
                seen.first().cloned()
            };
            let (m, uri, _, _) = captured.expect("fetch request captured");
            assert_eq!(m, "GET");
            assert_eq!(uri, "/api/v0/file/fetch_files?file_ids=file-a,file-b");

            // empty ids: no request at all
            let empty = c.fetch_files(&[]).await.expect("empty ids ok");
            assert!(empty.is_empty());
            assert_eq!(st.seen.lock().unwrap().len(), 1);
        },
    )
    .await;
    run.expect("fetch test completed within 15s");
}

#[tokio::test]
async fn wait_file_ready_polls_to_success() {
    let fetch = |status: &str| {
        format!(
            r#"{{"code":0,"data":{{"biz_code":0,"biz_data":{{"file_infos":[{{"id":"file-4bb9f696","status":"{status}","file_name":"tiny.png","file_size":97,"is_image":true}}]}}}}}}"#
        )
    };
    let (base, _) = spawn_mock(HashMap::from([(
        "/api/v0/file/fetch_files".to_string(),
        vec![
            MockReply { status: 200, headers: vec![], body: fetch("PENDING") },
            MockReply { status: 200, headers: vec![], body: fetch("SUCCESS") },
        ],
    )]))
    .await;
    let c = client_at(&base);
    let ready = c
        .wait_file_ready("file-4bb9f696", std::time::Duration::from_secs(5))
        .await
        .expect("polls to SUCCESS");
    assert_eq!(ready.status, FileParseStatus::Success);
}

#[tokio::test]
async fn wait_file_ready_timeout_is_explicit_error() {
    let pending = r#"{"code":0,"data":{"biz_code":0,"biz_data":{"file_infos":[{"id":"file-x","status":"PENDING"}]}}}"#;
    let (base, _) = spawn_mock(HashMap::from([(
        "/api/v0/file/fetch_files".to_string(),
        vec![MockReply { status: 200, headers: vec![], body: pending.to_string() }],
    )]))
    .await;
    let c = client_at(&base);
    let err = c
        .wait_file_ready("file-x", std::time::Duration::from_millis(900))
        .await
        .expect_err("timeout must be an explicit error, never silent");
    match &err {
        ponyllm_core::deepseek_web::ClientError::Other(msg) => {
            assert!(msg.contains("file-x"), "message names the file: {msg}");
            assert!(msg.contains("not ready"), "message states not-ready: {msg}");
        }
        other => panic!("expected Other timeout error, got {other:?}"),
    }
}

#[test]
fn file_parse_status_deserializes_wire_strings() {
    for (wire, want) in [
        ("PENDING", FileParseStatus::Pending),
        ("pending", FileParseStatus::Pending),
        ("SUCCESS", FileParseStatus::Success),
        ("COMPLETED", FileParseStatus::Success),
        ("FAILED", FileParseStatus::Failed("FAILED".to_string())),
        ("weird-future-state", FileParseStatus::Unknown("weird-future-state".to_string())),
    ] {
        let v: FileParseStatus = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(v, want, "wire {wire}");
    }
}

#[test]
fn classify_upload_biz_code_via_h08() {
    assert!(matches!(
        classify_api_error(200, r#"{"code":0,"data":{"biz_code":40301,"biz_msg":"INVALID_POW_RESPONSE"}}"#),
        ponyllm_core::deepseek_web::ClientError::InvalidPow
    ));
}
