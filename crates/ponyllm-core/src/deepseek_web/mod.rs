//! chat.deepseek.com Web Provider client (wave-9 contract, implemented by
//! task-2 / T3 executor; stubs frozen by task-1 / L2-P).
//!
//! Evidence: `.dev-team/contracts/wave-9-deepseek-web-rea-evidence.md`.
//! Contract: `.dev-team/contracts/wave-9-deepseek-web.md` (frozen by L2-P).
//!
//! Modules:
//! * [`pow`] — DeepSeekHashV1 PoW solver (worker-validated 23-round
//!   Keccak/SHA3-256 semantics), `X-DS-PoW-Response` header encoding and the
//!   expireAfter expiry rule (H-01..H-03, DW-P-*).
//! * [`client`] — HTTP client: challenge fetch, chat session, completion SSE
//!   with the REA §2 header group, bounded 429 backoff and explicit WAF
//!   failure (H-04..H-08, DW-S-*/DW-E-*/DW-G-*).
//! * [`sse`] — typed event stream parsing (ready/delta/finish/close plus
//!   non-fatal events; delta JSON-patch ops with recursive BATCH) (H-09,
//!   DW-T-*).
//!
//! Compliance hard boundaries implemented per contract §7 (DW-G-*): no
//! captcha/slider/device-fingerprint bypass, no guest mode, honest
//! single-solve PoW with no replay/pre-batch, bounded Retry-After backoff,
//! explicit `CaptchaWall` failure when the upstream serves WAF markers.

pub mod client;
pub mod pow;
pub mod sse;

pub use client::{
    classify_api_error, ApiErrorEnvelope, ChatSession, ClientError, CompletionRequest,
    DeepSeekWebClient, FileInfo, FileParseStatus, UploadedFile, CLIENT_BUNDLE_ID, CLIENT_LOCALE,
    CLIENT_PLATFORM, CLIENT_TIMEZONE_OFFSET, CLIENT_VERSION, DEFAULT_BASE_URL,
};
pub use pow::{
    encode_pow_response_header, is_pow_challenge_expired, solve_pow, PowChallenge, PowError,
    PowResponse,
};
pub use sse::{
    parse_completion_event, parse_delta, parse_sse_frames, DeltaOp, SseError, SseEvent,
};