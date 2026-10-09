//! 红相对抗测试（B001）：HS256 JWT 模块（`ponyllm_core::jwt`）。
//!
//! 冻结契约（protocol-agent 锁定，Lead 契约为准）：
//! ```text
//! pub struct Claims { pub sub: String, pub username: String, pub role: String,
//!                     pub tv: u64, pub iat: i64, pub exp: i64, pub iss: String }
//! pub fn sign(claims: &Claims, secret: &[u8]) -> Result<String, JwtError>;
//! pub fn verify(token: &str, secret: &[u8], issuer: &str) -> Result<Claims, JwtError>;
//! pub enum JwtError { Expired, InvalidSignature, InvalidToken, Other(String) }
//! ```
//! 契约语义：固定 `alg=HS256` 白名单（alg:none / RS256 一律拒）；iss 不匹配 → InvalidToken；
//! role 为 wire lowercase 字符串（"admin"/"user"）。
//!
//! 红相状态：契约尚未实现，断言预期 FAIL。对抗注入（L3）：过期、签名改一字符、payload
//! 篡改、非 base64 畸形、空/垃圾输入、错误 secret、alg:none / RS256 伪造、iss 不匹配 ——
//! 全部确定性构造，无网络无 sleep。

use ponyllm_core::jwt::{sign, verify, Claims, JwtError};

/// 测试虚拟 secret（非生产凭证，凭据脱敏纪律）。
const TEST_SECRET: &[u8] = b"red-phase-test-secret-0123456789abcdef";
/// 与签发时不同的 secret，用于错误密钥验签。
const OTHER_SECRET: &[u8] = b"another-completely-different-secret-value-0000";
const TEST_ISS: &str = "ponyllm";

/// 构造指定 exp 的 claims（其余字段固定，结构合理，role 为 wire 字符串）。
fn claims_with_exp(exp: i64) -> Claims {
    Claims {
        sub: "usr-001".to_string(),
        username: "alice".to_string(),
        role: "user".to_string(),
        tv: 3,
        iat: 1_700_000_000, // 早已过去，无未来 iat 问题
        exp,
        iss: TEST_ISS.to_string(),
    }
}

/// 未过期 claims（exp 落在未来）。
fn valid_claims() -> Claims {
    claims_with_exp(1_900_000_000)
}

/// RFC 4648 base64url 编码（无填充），纯 std 实现，用于对抗构造伪造 token。
fn b64url(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        }
    }
    out
}

/// 把第一个字符替换为另一个合法 base64url 字符（保证字节改变）。
fn flip_first_char(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    chars[0] = match chars[0] {
        'A'..='Z' => char::from_u32(u32::from(chars[0]) + 32).unwrap(),
        'a'..='z' => 'A',
        '0'..='9' => 'a',
        '-' => 'A',
        '_' => 'a',
        _ => 'A',
    };
    chars.into_iter().collect()
}

/// 正常签发 + 验签成功，且 claims 完整还原（sub/username/role/tv/iat/exp/iss）。
#[test]
fn sign_then_verify_roundtrip_restores_all_claims() {
    // Arrange
    let expected = valid_claims();

    // Act
    let token = sign(&expected, TEST_SECRET).expect("sign must succeed");
    let restored = verify(&token, TEST_SECRET, TEST_ISS).expect("verify must succeed");

    // Assert —— 逐字段还原，避免整结构比较掩盖字段丢失
    assert_eq!(restored.sub, expected.sub);
    assert_eq!(restored.username, expected.username);
    assert_eq!(restored.role, expected.role);
    assert_eq!(restored.tv, expected.tv);
    assert_eq!(restored.iat, expected.iat);
    assert_eq!(restored.exp, expected.exp);
    assert_eq!(restored.iss, expected.iss);
}

/// 过期 token（exp 在过去）→ Err(JwtError::Expired)。
#[test]
fn expired_token_is_rejected_with_expired() {
    // Arrange
    let expired = claims_with_exp(1_700_000_001); // exp < 当前时间
    let token = sign(&expired, TEST_SECRET).expect("sign must succeed");

    // Act
    let result = verify(&token, TEST_SECRET, TEST_ISS);

    // Assert
    assert!(
        matches!(result, Err(JwtError::Expired)),
        "expected JwtError::Expired, got {result:?}"
    );
}

/// 改签名区一个字符 → Err(JwtError::InvalidSignature)。
#[test]
fn single_character_signature_mutation_is_rejected() {
    // Arrange
    let token = sign(&valid_claims(), TEST_SECRET).expect("sign must succeed");
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "malformed token produced by sign: {token}"
    );
    let tampered_sig = flip_first_char(segments[2]);
    let forged = format!("{}.{}.{}", segments[0], segments[1], tampered_sig);

    // Act
    let result = verify(&forged, TEST_SECRET, TEST_ISS);

    // Assert
    assert!(
        matches!(result, Err(JwtError::InvalidSignature)),
        "one mutated signature character must fail signature check, got {result:?}"
    );
}

/// 篡改 payload（改 role 为 admin 后重新编码但不重签）→ Err(JwtError::InvalidSignature)。
#[test]
fn tampered_payload_role_change_is_rejected() {
    // Arrange
    let token = sign(&valid_claims(), TEST_SECRET).expect("sign must succeed");
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(segments.len(), 3);

    // 攻击者：解码 payload，把 role 改为 admin，重新编码，保留原签名
    let mut tampered = valid_claims();
    tampered.role = "admin".to_string();
    let new_payload = b64url(&serde_json::to_vec(&tampered).expect("claims serialization"));
    let forged = format!("{}.{}.{}", segments[0], new_payload, segments[2]);

    // Act
    let result = verify(&forged, TEST_SECRET, TEST_ISS);

    // Assert —— 签名绑定全部 header.payload 字节，改 role 后签名必然失配
    assert!(
        matches!(result, Err(JwtError::InvalidSignature)),
        "role-tampered payload must fail signature check, got {result:?}"
    );
}

/// 非 base64 畸形 token → Err(JwtError::InvalidToken)（或 Other？不允许，走 InvalidToken）。
#[test]
fn malformed_non_base64_token_is_rejected() {
    // Arrange
    let token = sign(&valid_claims(), TEST_SECRET).expect("sign must succeed");
    let segments: Vec<&str> = token.split('.').collect();
    let garbage = format!("{}.!!!.{}", segments[0], segments[2]); // '!' 非 base64url

    // Act
    let result = verify(&garbage, TEST_SECRET, TEST_ISS);

    // Assert
    assert!(
        matches!(result, Err(JwtError::InvalidToken)),
        "non-base64 payload must be rejected as InvalidToken, got {result:?}"
    );
}

/// 空 token / 垃圾 token → Err(JwtError::InvalidToken)。
#[test]
fn empty_and_garbage_tokens_are_rejected() {
    // Act & Assert
    let empty = verify("", TEST_SECRET, TEST_ISS);
    assert!(
        matches!(empty, Err(JwtError::InvalidToken)),
        "empty token must be InvalidToken, got {empty:?}"
    );
    let garbage = verify("garbage-not-a-jwt", TEST_SECRET, TEST_ISS);
    assert!(
        matches!(garbage, Err(JwtError::InvalidToken)),
        "garbage token must be InvalidToken, got {garbage:?}"
    );
    let dots = verify("...", TEST_SECRET, TEST_ISS);
    assert!(
        matches!(dots, Err(JwtError::InvalidToken)),
        "empty three segments must be InvalidToken, got {dots:?}"
    );
}

/// 错误 secret 验签 → Err(JwtError::InvalidSignature)。
#[test]
fn wrong_secret_verify_is_rejected() {
    // Arrange
    let token = sign(&valid_claims(), TEST_SECRET).expect("sign must succeed");

    // Act
    let result = verify(&token, OTHER_SECRET, TEST_ISS);

    // Assert
    assert!(
        matches!(result, Err(JwtError::InvalidSignature)),
        "verification with a different secret must fail, got {result:?}"
    );
}

/// 伪造 header `alg:none`（空签名）→ 拒绝。
#[test]
fn alg_none_forged_token_is_rejected() {
    // Arrange —— 攻击者构造 alg:none + 合法 claims + 空签名
    let header = b64url(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = b64url(&serde_json::to_vec(&valid_claims()).expect("claims serialization"));
    let forged = format!("{header}.{payload}."); // 空签名

    // Act
    let result = verify(&forged, TEST_SECRET, TEST_ISS);

    // Assert
    assert!(
        matches!(result, Err(JwtError::InvalidToken)),
        "alg:none token must be rejected (fixed HS256 whitelist), got {result:?}"
    );
}

/// 伪造 header `alg=RS256`（任意签名垃圾）→ 拒绝。
#[test]
fn alg_rs256_forged_token_is_rejected() {
    // Arrange —— 攻击者声明 RS256（算法混淆攻击）
    let header = b64url(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = b64url(&serde_json::to_vec(&valid_claims()).expect("claims serialization"));
    let sig = b64url(b"fake-rsa-signature-bytes");
    let forged = format!("{header}.{payload}.{sig}");

    // Act
    let result = verify(&forged, TEST_SECRET, TEST_ISS);

    // Assert
    assert!(
        matches!(result, Err(JwtError::InvalidToken)),
        "RS256-declared token must be rejected (fixed HS256 whitelist), got {result:?}"
    );
}

/// iss 不匹配 → Err(JwtError::InvalidToken)。
#[test]
fn iss_mismatch_is_rejected() {
    // Arrange —— 用合法 secret 签发，但校验方把 iss 网关名指向别处
    let token = sign(&valid_claims(), TEST_SECRET).expect("sign must succeed");

    // Act
    let result = verify(&token, TEST_SECRET, "evil-issuer.example.com");

    // Assert
    assert!(
        matches!(result, Err(JwtError::InvalidToken)),
        "iss claim mismatch must be rejected as InvalidToken, got {result:?}"
    );
}
