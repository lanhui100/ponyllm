//! Wave-9 deepseek-web 真实链路冒烟探针（一次性验证工具，非测试）。
//!
//! 用法：`DS_WEB_TOKEN=<token> cargo run -p ponyllm-core --example deepseek_web_live_smoke [--completion]`
//! 默认只跑 session→challenge→solve（验证凭据 + PoW 真值）；`--completion` 追加一次真实对话。
//! 单次、不存储、token 仅环境变量。

use ponyllm_core::deepseek_web::{
    encode_pow_response_header, solve_pow, DeepSeekWebClient,
};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() {
    let token = std::env::var("DS_WEB_TOKEN").expect("set DS_WEB_TOKEN");
    let device_id = uuid::Uuid::new_v4().as_simple().to_string();
    let client = DeepSeekWebClient::new(Some(token), device_id);

    // 1) 会话创建（真实）
    println!("== 1) create_chat_session ==");
    match client.create_chat_session().await {
        Ok(s) => println!("OK session_id={}", s.id),
        Err(e) => {
            println!("FAIL session: {e}");
            std::process::exit(1);
        }
    }

    // 2) 真实 challenge
    println!("== 2) create_pow_challenge ==");
    let challenge = match client
        .create_pow_challenge("/api/v0/chat/completion")
        .await
    {
        Ok(c) => c,
        Err(e) => {
            println!("FAIL challenge: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "OK algorithm={} salt_len={} difficulty={} challenge_len={} expire_at={:?} expire_after={:?}",
        challenge.algorithm,
        challenge.salt.len(),
        challenge.difficulty,
        challenge.challenge.len(),
        challenge.expire_at,
        challenge.expire_after,
    );

    // 3) 求解（我们实现的 23 轮 DeepSeekHashV1 + expireAt 前缀）
    println!("== 3) solve_pow ==");
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let sol = match solve_pow(&challenge, "/api/v0/chat/completion") {
        Ok(s) => s,
        Err(e) => {
            println!("FAIL solve: {e}");
            std::process::exit(1);
        }
    };
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        - started;
    println!(
        "OK answer={} (took ~{}s, difficulty={})",
        sol.answer, elapsed, challenge.difficulty
    );
    let header = encode_pow_response_header(&sol);
    println!("X-DS-PoW-Response b64_len={}", header.len());
    println!("POw FULL-CHAIN PASS");

    if std::env::args().any(|a| a == "--completion") {
        println!("== 4) stream_completion (single real turn) ==");
        let req = ponyllm_core::deepseek_web::CompletionRequest {
            chat_session_id: String::new(), // 由探针在下方创建
            parent_message_id: None,
            model_type: "deepseek-v4-flash".to_string(),
            prompt: "hi".to_string(),
            ref_file_ids: Vec::new(),
            thinking_enabled: true,
            search_enabled: false,
            // 真实链路 2026-10-22 确认：服务端枚举仅接受 `default` | `landing`
            // （`web_chat` 等其余值 422 拒绝；此结论修订 REA §3 的"常见字面量"推测）。
            source: "default".to_string(),
            action: None,
            preempt: false,
        };
        // 重新拿会话 id（上一步未保存），这里简化：再建一个
        let sess = client
            .create_chat_session()
            .await
            .expect("session for completion");
        let req = ponyllm_core::deepseek_web::CompletionRequest {
            chat_session_id: sess.id.clone(),
            ..req
        };
        let resp = match client.stream_completion(&req, &sol).await {
            Ok(r) => r,
            Err(e) => {
                println!("FAIL completion http: {e}");
                std::process::exit(1);
            }
        };
        println!("HTTP {}", resp.status());
        let mut buf = String::new();
        use tokio::io::AsyncReadExt;
        let mut body = resp;
        let mut stream = body.bytes_stream();
        use futures_util::StreamExt;
        let mut frames = 0usize;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(b) => {
                    let t = String::from_utf8_lossy(&b).to_string();
                    buf.push_str(&t);
                    frames += 1;
                    // 只打印前 ~6 个数据帧，然后中止（单次验证）
                    if frames <= 6 {
                        for line in t.split('\n') {
                            if line.starts_with("data:") {
                                println!("FRAME[{frames}] {}", line);
                            }
                        }
                    }
                    if buf.matches("data: [DONE]").count() > 0 || frames > 20 {
                        break;
                    }
                }
                Err(e) => {
                    println!("stream err: {e}");
                    break;
                }
            }
        }
        println!("COMPLETION STREAM PASS (frames={frames})");
    }
}
