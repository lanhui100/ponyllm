//! Phase-2 安全修复验收（隔离测试，Test Agent 于业务实施前编写）。
//!
//! 契约矩阵：`.dev-team/report/FIX-CONTRACT.md` F15（VULN-15 token fragment 传递）。
//!
//! 红相要求：`format_web_status_url` 当前生成 `{base_url}/?token={api_key}`（query 传
//! 凭据会落入访问日志/referrer/CDN 缓存）；修复后必须主用
//! `{base_url}/#token={api_key}`（fragment 永不上送服务器）。HEAD 上断言 #token=
//! 失败 = F15 CLI 侧未实现的直接证据。
//!
//! 注：web 侧 #token= 片段解析（router.ts extractFragmentToken / guard 清洗）经核查
//! 已在 HEAD 就绪并有既有测试覆盖（router.guard.test.ts），修复后保持绿即可，不再重复。

use ponyllm_cli::format_web_status_url;

#[test]
fn f15_cli_console_url_uses_fragment_token() {
    // 契约：带 token 时控制台链接必须为 fragment 形式（#token=），禁止 query（?token=）
    assert_eq!(
        format_web_status_url("http://127.0.0.1:8080", true, "sk-pony-test-123"),
        "http://127.0.0.1:8080/#token=sk-pony-test-123",
        "F15: CLI 控制台链接必须生成 #token= fragment（HEAD 上仍生成 ?token=，红相成立）"
    );
}

#[test]
fn f15_cli_console_url_trims_trailing_slash_and_no_token() {
    // 无 token / "none" 行为保持
    assert_eq!(format_web_status_url("http://127.0.0.1:8080/", true, ""), "http://127.0.0.1:8080/");
    assert_eq!(format_web_status_url("http://127.0.0.1:8080", true, "none"), "http://127.0.0.1:8080/");
}

// ---------------------------------------------------------------------------
// R9（Phase-2b）：fragment 内 token 需 percent-encode（encodeURIComponent 等价）
// ---------------------------------------------------------------------------

/// 含 `&`、`#`、`+` 的 key 必须生成完好链接：`&`/`#` 会截断 fragment 或注入参数，
/// `+` 会被解析为空格 —— 一律 percent-encode。HEAD 上原样拼接（红相成立）。
#[test]
fn r9_cli_console_url_encodes_special_chars_in_fragment_token() {
    let url = format_web_status_url("http://127.0.0.1:8080", true, "sk+b&c#d");
    assert_eq!(
        url,
        "http://127.0.0.1:8080/#token=sk%2Bb%26c%23d",
        "R9: 特殊字符必须 percent-encode（&→%26、#→%23、+→%2B），实际 {}",
        url
    );
    assert!(
        !url.contains("sk+b&c#d"),
        "R9: 裸特殊字符不得出现在 fragment 中（=&/&# 截断注入），实际 {}",
        url
    );
}
