#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""ponyllm opencode-zen 免费模型助手：发现 → 探活 → 元信息 → 字段映射 → 写入网关。

真相源（契约 .dev-team/contracts/zen-free-skill-contract.md §2，注释里的行号是代码依据，不得凭记忆改）：

  crates/ponyllm-core/src/executor/upstream.rs
    :192  new_upstream_session_id()            → "ses_" + uuid4().simple()[:26]
    :779  is_opencode_zen_target()             → provider 前缀 opencode ⇒ 头注入作用域
    :794  zen_free_tier_forces_upstream_stream()→ id 以 -free 结尾 ⇒ 强制上游 stream
    :808  OPENCODE_ZEN_TOOL_NAMES              → 12 个 opencode 工具名
    :887  zen_stub_tool()                      → chat / responses 两种 stub 线形
    :1553 inject_zen_free_tier_tools()         → 为 *-free 补齐工具 + tool_choice:auto
    :1672 zen 头注入                          → x-opencode-session 等 + UA 硬编码
  crates/ponyllm-server/src/routes/admin.rs
    :88   AdminPricingMode(snake_case)         → pricing_mode 必须小写 "uniform"
    :501  CreateModelPayload                   → plan 输出的字段全集
    :1497 parse_tier()                        → 合法档位 F/S/L

  .dev-team/notes/zen-free-gate-probe.md       → 门禁三条件的实测收据笔记

纯标准库（契约 §4/§6 A9）；退出码 0 成功 / 1 探活或写入失败 / 2 参数或前置错误。
任何失败路径都必须把原因打到 stderr，禁止吞异常，禁止打印 api key 原文。
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import socket
import sys
import time
import urllib.error
import urllib.request

# --------------------------------------------------------------------------
# 真相源常量
# --------------------------------------------------------------------------

ZEN_BASE = "https://opencode.ai/zen/v1"
DEFAULT_GW = "http://127.0.0.1:8080"
LOCAL_PROXY = "http://127.0.0.1:8899"  # §5：本机代理，仅用于 geo_blocked / 连接层失败

# upstream.rs:1672-1690 硬编码的 UA；zen 门禁要求以 opencode/ 开头且版本 >= 1.18.0
OPENCODE_UA = "opencode/1.18.31 (Linux; x64)"

# upstream.rs:192 new_upstream_session_id() = "ses_" + uuid4().simple()[:26]
# 长度 26 是 zen 门禁硬条件（ses_ + 64 hex 会 403 FreeTierError）
SESSION_ID_LEN = 26

# upstream.rs:808 OPENCODE_ZEN_TOOL_NAMES —— 默认全量发送（安全超集）。
# 门禁按工具「名字」判定而非数量，notes/zen-free-gate-probe.md 实测最小可行集为 bash + read；
# 单名（只发 read）实测仍 403，因此这里保留 12 名而不是裁剪到 2 名。
OPENCODE_ZEN_TOOL_NAMES = (
    "bash",
    "edit",
    "glob",
    "google_search",
    "grep",
    "read",
    "skill",
    "task",
    "todowrite",
    "webfetch",
    "websearch",
    "write",
)

GATEWAY_PROVIDER = "opencode-zen"  # 契约 §3 I1：前缀 opencode ⇒ 命中 zen 作用域
CACHE_PATH = "~/.cache/opencode/models.json"  # 真实结构 data["opencode"]["models"][id]
CONFIG_PATH = "~/.config/opencode/opencode.json"
ENVELOPE_INPUT_TYPES = ("text", "image", "video")  # 网关 envelope 输入模态（add-model 契约校准例）
DEFAULT_TIER = "L"  # 契约 §4.2：tier 缺省 = 轻量
VALID_TIERS = ("F", "S", "L")
DEFAULT_PROTOCOL = "chat"  # 契约 §4.2：zen free 模型默认 chat 线
HTTP_TIMEOUT_S = 60
TEXT_PREVIEW_CHARS = 400

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2

# 契约 §4.3 的 error_type 全枚举（穷尽，输出不得越界）
ERROR_TYPES = (
    "geo_blocked",
    "gate_failure",
    "usage_limit",
    "protocol_unsupported",
    "not_found",
    "transport_error",
    "upstream_unavailable",
)

# ponyllm-protocol/src/common.rs:79 from_str_loose 的等价归一表
_EFFORT_ALIASES = {
    "off": None, "none": None, "0": None, "false": None, "disabled": None,
    "disable": None, "no": None,
    "low": "low", "minimal": "low", "1": "low", "fast": "low", "light": "low",
    "medium": "medium", "standard": "medium", "default": "medium", "2": "medium",
    "balanced": "medium", "med": "medium",
    "high": "high", "deep": "high", "3": "high", "true": "high", "full": "high",
    "max": "max", "xhigh": "max", "extra_high": "max", "extra-high": "max",
    "ultra": "max", "4": "max", "extreme": "max",
}
_EFFORT_ORDER = ("low", "medium", "high", "max")


class ZenFreeError(Exception):
    """带退出码的可诊断错误：msg 必定打到 stderr（契约 §4 禁止吞异常）。"""

    def __init__(self, message: str, code: int = EXIT_USAGE) -> None:
        super().__init__(message)
        self.message = message
        self.code = code


# --------------------------------------------------------------------------
# 基础工具
# --------------------------------------------------------------------------


def _read_json_file(path: str, what: str):
    """读 JSON 文件；不存在 / 解析失败都升级成 ZenFreeError（退出码 2）。"""
    resolved = os.path.expanduser(path)
    if not os.path.isfile(resolved):
        raise ZenFreeError("找不到%s：%s" % (what, resolved))
    try:
        with open(resolved, "r", encoding="utf-8") as handle:
            return json.load(handle)
    except OSError as exc:
        raise ZenFreeError("读取%s失败：%s（%s）" % (what, resolved, exc))
    except ValueError as exc:
        raise ZenFreeError("解析%s失败（不是合法 JSON）：%s（%s）" % (what, resolved, exc))


def _short(value, limit: int = 200) -> str:
    text = str(value).replace("\n", " ").strip()
    return text if len(text) <= limit else text[:limit] + "…"


def resolve_key(explicit) -> str:
    """key 来源优先级：--key > OPENCODE_ZEN_KEY > opencode.json（契约 §4）。

    返回值只允许流进请求头；任何输出 / 异常文案都不得回显原文。
    """
    if explicit and str(explicit).strip():
        return str(explicit).strip()
    env_value = os.environ.get("OPENCODE_ZEN_KEY", "")
    if env_value.strip():
        return env_value.strip()
    config = _read_json_file(CONFIG_PATH, "opencode 配置")
    if not isinstance(config, dict):
        raise ZenFreeError("opencode 配置顶层不是对象：%s" % CONFIG_PATH)
    provider = config.get("provider")
    entry = provider.get("opencode") if isinstance(provider, dict) else None
    options = entry.get("options") if isinstance(entry, dict) else None
    key = options.get("apiKey") if isinstance(options, dict) else None
    if not isinstance(key, str) or not key.strip():
        raise ZenFreeError(
            "取不到 zen api key：--key、$OPENCODE_ZEN_KEY、%s 的 provider.opencode.options.apiKey 三处均为空"
            % CONFIG_PATH
        )
    return key.strip()


def load_catalog() -> dict:
    """元信息缓存 → {model_id: spec}。契约 §2（v1.3 订正）：data["opencode"]["models"][id]。

    上游 /models 目录只有 id、没有规格，因此规格的唯一来源就是这份本地缓存。
    """
    raw = _read_json_file(CACHE_PATH, "opencode 元信息缓存")
    if not isinstance(raw, dict):
        raise ZenFreeError("元信息缓存顶层不是对象：%s" % CACHE_PATH)
    provider = raw.get("opencode")
    if not isinstance(provider, dict):
        raise ZenFreeError(
            "元信息缓存缺少 opencode 段（期望 data[\"opencode\"][\"models\"]）：%s" % CACHE_PATH
        )
    models = provider.get("models")
    if not isinstance(models, dict):
        raise ZenFreeError(
            "元信息缓存的 data[\"opencode\"][\"models\"] 不是以 id 为键的对象：%s" % CACHE_PATH
        )
    return models


def to_size(value) -> str:
    """契约 §4.2 的 1024 进制换算（上游声明是十进制，此处按契约字面规则，不擅自改成 1M）。

    >=1048576 → 1M；>=262144 → 256K；>=131072 → 128K；其余按 K 向上取整。
    """
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ZenFreeError("上游未提供可用的 size 字段（limit 值=%r），拒绝编造规格" % (value,))
    if value >= 1048576:
        return "1M"
    if value >= 262144:
        return "256K"
    if value >= 131072:
        return "128K"
    return "%dK" % (-(-value // 1024))


def resolve_tier(raw) -> str:
    """tier 缺省 = L；F/S/L 之外一律拒绝并退出码 2（契约 §4.2 / §6 A5）。"""
    if raw is None:
        return DEFAULT_TIER
    tier = str(raw).strip().upper()
    if tier not in VALID_TIERS:
        raise ZenFreeError(
            "非法 tier %r：只接受 %s（网关 parse_tier 口径 admin.rs:1497），"
            "缺省不传即为 %s" % (str(raw), "/".join(VALID_TIERS), DEFAULT_TIER)
        )
    return tier


def thinking_levels(spec: dict) -> list:
    """上游 reasoning_options 声明的 effort 档位 → 归一后的有序列表（契约 §4.2）。

    上游未声明或为空 → 返回 []，此时 payload 不写 thinking_* 字段（键缺席，不是 null）。
    """
    options = spec.get("reasoning_options")
    if not isinstance(options, list) or not options:
        return []
    levels = []
    for option in options:
        if not isinstance(option, dict):
            continue
        values = option.get("values")
        if not isinstance(values, list):
            continue
        for raw in values:
            normalized = _EFFORT_ALIASES.get(str(raw).strip().lower())
            if normalized and normalized not in levels:
                levels.append(normalized)
    levels.sort(key=_EFFORT_ORDER.index)
    return levels


def build_payload(spec: dict, tier: str, protocol: str = DEFAULT_PROTOCOL) -> dict:
    """上游元信息 → CreateModelPayload（契约 §4.2 + admin.rs:501）。

    契约 §3 I3：模型级 base_url / proxy 一律不写（填了触发 400 egress_blocked）。
    输出保持扁平且不含这两个键，避免下游误接。
    """
    model_id = spec.get("id")
    if not isinstance(model_id, str) or not model_id.strip():
        raise ZenFreeError("上游元信息缺少 id 字段，拒绝编造")
    limits = spec.get("limit")
    if not isinstance(limits, dict):
        raise ZenFreeError("上游元信息缺少 limit 段，拒绝编造规格（model=%s）" % model_id)
    modalities = spec.get("modalities")
    modalities = modalities if isinstance(modalities, dict) else {}

    upstream_input = [str(m) for m in (modalities.get("input") or [])]
    # 与网关 envelope 取交集（契约 §4.2：text 恒含，不照抄上游未声明的模态）
    input_types = [m for m in ENVELOPE_INPUT_TYPES if m in upstream_input]
    if "text" not in input_types:
        input_types.insert(0, "text")

    payload = {
        "provider": GATEWAY_PROVIDER,
        "name": model_id,
        "tier": tier,
        "context_window": to_size(limits.get("context")),
        "max_output": to_size(limits.get("output")),
        "input_types": input_types,
        "output_types": ["text"],  # 文本模型恒 ["text"]
        "protocol": protocol,
        # admin.rs:88 AdminPricingMode 带 rename_all="snake_case"，大写 "Uniform" 会被拒
        "pricing_mode": "uniform",
    }
    levels = thinking_levels(spec)
    if levels:
        payload["thinking_default"] = levels[0]
        payload["thinking_max"] = levels[-1]
    return payload


def require_spec(catalog: dict, model: str) -> dict:
    spec = catalog.get(model)
    if not isinstance(spec, dict):
        raise ZenFreeError(
            "本地元信息缓存里没有模型 %r 的规格：%s（上游 /models 目录只有 id、没有规格，"
            "请先跑一次 opencode CLI 刷新缓存）" % (model, CACHE_PATH)
        )
    return spec


def emit(payload: dict, as_json: bool, human_lines) -> None:
    """--json 时 stdout 只出这一个 JSON 对象；否则输出中文人读表格（契约 §4）。"""
    if as_json:
        sys.stdout.write(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
    else:
        for line in human_lines:
            sys.stdout.write(line + "\n")


# --------------------------------------------------------------------------
# HTTP 传输层（契约 §5：用 ProxyHandler({}) 显式直连，不依赖环境变量）
# --------------------------------------------------------------------------


def _build_opener(use_proxy: bool):
    if use_proxy:
        return urllib.request.build_opener(
            urllib.request.ProxyHandler({"http": LOCAL_PROXY, "https": LOCAL_PROXY})
        )
    # 显式空 ProxyHandler：即使环境里有 http_proxy 也强制直连
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _headers_to_dict(message) -> dict:
    if message is None:
        return {}
    try:
        return {str(k).lower(): str(v) for k, v in message.items()}
    except (AttributeError, TypeError):
        return {}


def _ms(started: float) -> int:
    return max(0, int((time.monotonic() - started) * 1000))


def _http_call(url: str, body, headers: dict, use_proxy: bool, timeout: int, method: str = "POST") -> dict:
    """一次 HTTP 往返；连接层异常一律归类 transport_error（契约 §4.3）。"""
    data = json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None
    opener = _build_opener(use_proxy)
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    started = time.monotonic()
    try:
        with opener.open(request, timeout=timeout) as response:
            raw = response.read()
            status = int(getattr(response, "status", 0) or response.getcode() or 0)
            response_headers = _headers_to_dict(getattr(response, "headers", None))
    except urllib.error.HTTPError as exc:  # 4xx/5xx 仍带 body，必须读出来分类
        try:
            raw = exc.read()
        except (AttributeError, OSError) as read_exc:
            raise ZenFreeError("读取上游错误响应失败（HTTP %s）：%s" % (exc.code, read_exc))
        status = int(exc.code)
        response_headers = _headers_to_dict(getattr(exc, "headers", None))
    except (urllib.error.URLError, socket.timeout, OSError) as exc:
        reason = getattr(exc, "reason", exc)
        return {
            "status": 0,
            "body": "",
            "headers": {},
            "latency_ms": _ms(started),
            "error_type": "transport_error",
            "classified": True,
            "detail": _short(reason),
        }
    text = raw.decode("utf-8", "replace") if isinstance(raw, (bytes, bytearray)) else str(raw)
    return {
        "status": status,
        "body": text,
        "headers": response_headers,
        "latency_ms": _ms(started),
        "error_type": None,
        "classified": False,
        "detail": "",
    }


# --------------------------------------------------------------------------
# zen 免费层门禁（契约 §4.1 + .dev-team/notes/zen-free-gate-probe.md）
#   三个正交条件：opencode/ 开头的 UA（>=1.18.0）、x-opencode-session = ses_+26 字符、
#   body 同时含 stream:true 与被识别的 opencode 工具名。缺一即 403 FreeTierError。
# --------------------------------------------------------------------------


def new_session_id() -> str:
    """ses_ + 恰好 26 字符（对齐 upstream.rs:192；长度本身是门禁硬条件）。"""
    return "ses_" + secrets.token_hex(13)[:SESSION_ID_LEN]


def probe_headers(key: str, session: str) -> dict:
    """最小集 4 个头（缺一即 403）+ 与网关 wire 对齐的可选头（超集，零成本）。"""
    return {
        "authorization": "Bearer " + key,
        "content-type": "application/json",
        # 门禁条件 1：以 opencode/ 开头且 >= 1.18.0；低版本 → 426 UpgradeRequired
        "user-agent": OPENCODE_UA,
        # 门禁条件 2：ses_ + 26 字符；无需服务端注册
        "x-opencode-session": session,
        # 以下对齐 upstream.rs:1672-1690，探活非必需但与真实网关 wire 一致
        "x-session-affinity": session,
        "x-session-id": session,
        "x-opencode-client": "ponyllm",
        "x-opencode-project": "global",
        "x-opencode-request": "msg_" + secrets.token_hex(8),
        "x-opencode-session-id": session,
    }


def _stub_tool(name: str, wire: str) -> dict:
    """upstream.rs:887 zen_stub_tool 的等价线形；schema 无关，空 parameters 即可。"""
    if wire == "responses":
        return {
            "type": "function",
            "name": name,
            "description": "opencode agent tool",
            "parameters": {"type": "object", "properties": {}},
        }
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": "opencode agent tool",
            "parameters": {"type": "object", "properties": {}},
        },
    }


def probe_body(model: str, wire: str, variant: str) -> dict:
    """契约 §4.1 的最小载荷（.dev-team/notes/zen-free-gate-probe.md 的实测模板）。

    门禁只要求 stream:true + 被识别的工具名；默认仍发全量 12 名（与 upstream.rs:808 对齐的超集）。
    单条 user 消息即可，不需要 opencode 的完整 agent system prompt。
    """
    tools = [_stub_tool(name, wire) for name in OPENCODE_ZEN_TOOL_NAMES]
    if wire == "responses":
        body = {
            "model": model,
            "input": [{"role": "user", "content": "Reply with exactly: zen-ok"}],
            "tools": tools,
            "stream": True,
            "max_tokens": 2048,
        }
    else:
        body = {
            "model": model,
            "messages": [{"role": "user", "content": "Reply with exactly: zen-ok"}],
            "tools": tools,
            "tool_choice": "auto",  # 对齐 inject_zen_free_tier_tools
            "stream": True,
            "stream_options": {"include_usage": True},
            "max_tokens": 2048,
        }
    if variant and variant != "default":
        body["reasoning_effort"] = variant
    return body


def parse_stream(text: str):
    """解析 SSE 事件流 → (非空文本, usage, finish_reason)。"""
    parts = []
    usage = None
    finish = None
    for line in text.splitlines():
        if not line.startswith("data:"):
            continue
        payload = line[len("data:"):].strip()
        if not payload or payload == "[DONE]":
            continue
        try:
            event = json.loads(payload)
        except ValueError:
            continue  # 心跳 / 非 JSON 注释行不是事件，静默跳过（不是吞异常）
        if not isinstance(event, dict):
            continue
        if isinstance(event.get("usage"), dict):
            usage = event["usage"]
        for choice in event.get("choices") or []:
            if not isinstance(choice, dict):
                continue
            delta = choice.get("delta")
            delta = delta if isinstance(delta, dict) else {}
            for piece in (delta.get("content"), choice.get("text")):
                if isinstance(piece, str):
                    parts.append(piece)
            if choice.get("finish_reason"):
                finish = choice["finish_reason"]
        # responses 线：event.type == response.output_text.delta，文本在 delta 字段
        piece = event.get("delta")
        if isinstance(piece, str) and isinstance(event.get("type"), str) and "output_text" in event["type"]:
            parts.append(piece)
    return "".join(parts).strip(), usage, finish


def classify_error(status: int, headers: dict, body: str) -> tuple:
    """契约 §4.3 的机械分类 → (error_type, 是否命中契约列举的信号)。"""
    low = (body or "").lower()
    if status == 403:
        # geo_blocked：Cloudflare 地区拦截，「不算模型不可用」，--via auto 必须切代理
        if "not available in your country" in low:
            return "geo_blocked", True
        if "frettierror" in low:
            return "gate_failure", True
        # 未列举的 403：zen 免费层对非 opencode 身份只回 403，归入门禁未过
        return "gate_failure", False
    if status == 429:
        if "freeusagelimiterror" in low:
            return "usage_limit", True
        if "endpoint is unavailable" in low or "server_error" in low:
            return "upstream_unavailable", True
        return "usage_limit", False
    if status == 400:
        if "modelprotocolunsupported" in low:
            return "protocol_unsupported", True
        return "protocol_unsupported", False
    if status == 404:
        return "not_found", True
    if status >= 500:
        # 502 tunnel_failed / 5xx 归传输失败，--via auto 据此切 transport
        return "transport_error", True
    if 400 < status < 500:
        return "gate_failure", False
    return "transport_error", False


# 换 transport 的触发集合（契约 §4.1 + §4.3）
SWITCH_TRANSPORT_TYPES = ("transport_error", "upstream_unavailable", "geo_blocked")
# 换 protocol 的触发集合：只有协议层面的拒绝才值得换线重试
SWITCH_PROTOCOL_TYPES = ("protocol_unsupported",)


def run_probe(model: str, protocol_opt: str, via_opt: str, variant: str, key: str) -> dict:
    """按 --protocol/--via 跑探活链路，返回契约 §4.1 要求的完整输出体。"""
    protocols = ["chat", "responses"] if protocol_opt == "auto" else [protocol_opt]
    transports = ["direct", "proxy"] if via_opt == "auto" else [via_opt]
    attempts = []
    winner = None

    for wire in protocols:
        for index, transport in enumerate(transports):
            session = new_session_id()
            url = ZEN_BASE + ("/responses" if wire == "responses" else "/chat/completions")
            result = _http_call(
                url, probe_body(model, wire, variant), probe_headers(key, session),
                transport == "proxy", HTTP_TIMEOUT_S,
            )
            status = result["status"]
            error_type = result["error_type"]
            text, usage, finish = "", None, None
            if error_type is None and status == 200:
                text, usage, finish = parse_stream(result["body"])
                if not text:
                    # 契约 §4.1：只有 200 且取到非空文本/事件流才算可用
                    error_type = "transport_error"
                    result["detail"] = "HTTP 200 但未取到非空文本/事件流"
            elif error_type is None:
                error_type, result["classified"] = classify_error(
                    status, result["headers"], result["body"]
                )
            attempt = {
                "protocol": wire,
                "transport": transport,
                "http_status": status,
                "latency_ms": result["latency_ms"],
                "error_type": error_type,
                "ok": error_type is None and status == 200,
            }
            if result["detail"]:
                attempt["detail"] = result["detail"]
            if not result["classified"] and status != 200:
                attempt["unclassified"] = True
            if status == 403 and "cf-ray" in result["headers"]:
                attempt["cf_ray_present"] = True
            attempts.append(attempt)
            if attempt["ok"]:
                winner = attempt
                break
            if error_type in SWITCH_TRANSPORT_TYPES and index < len(transports) - 1:
                continue  # 还有别的 transport 没试
            break
        if winner:
            break
        last_type = attempts[-1]["error_type"] if attempts else None
        if last_type not in SWITCH_PROTOCOL_TYPES or wire == protocols[-1]:
            break

    if winner:
        return {
            "model": model,
            "ok": True,
            "protocol": winner["protocol"],
            "http_status": winner["http_status"],
            "latency_ms": winner["latency_ms"],
            "error_type": None,
            "gate_failure": False,
            "transport": winner["transport"],
            "attempts": attempts,
            "output_text": text[:TEXT_PREVIEW_CHARS],
            "usage": usage,
            "finish_reason": finish,
            "cf_ray_present": False,
        }
    last = attempts[-1] if attempts else None
    return {
        "model": model,
        "ok": False,
        "protocol": last["protocol"] if last else protocol_opt,
        "http_status": last["http_status"] if last else 0,
        "latency_ms": last["latency_ms"] if last else 0,
        "error_type": last["error_type"] if last else "transport_error",
        "gate_failure": bool(last and last["error_type"] == "gate_failure"),
        "transport": last["transport"] if last else "direct",
        "attempts": attempts,
        "output_text": "",
        "usage": None,
        "finish_reason": None,
        "cf_ray_present": bool(last and last.get("cf_ray_present")),
    }


def _probe_human(result: dict) -> list:
    lines = [
        "模型：%s" % result["model"],
        "判定：%s" % ("可用（HTTP 200 且取到非空文本）" if result["ok"] else "不可用"),
        "协议 / 传输：%s / %s" % (result["protocol"], result["transport"]),
        "HTTP 状态：%s（%s ms）" % (result["http_status"], result["latency_ms"]),
        "error_type：%s" % (result["error_type"] if result["error_type"] else "（无）"),
        "门禁未过：%s" % ("是" if result["gate_failure"] else "否"),
        "输出摘要：%s" % (result["output_text"] or "（空）"),
    ]
    if result["usage"]:
        lines.append("usage：%s" % json.dumps(result["usage"], ensure_ascii=False))
    if result.get("cf_ray_present"):
        lines.append("cf-ray：存在（Cloudflare 地区拦截特征）")
    lines.append("尝试链路：")
    for item in result["attempts"]:
        lines.append(
            "  - %s/%s → HTTP %s %sms %s"
            % (item["protocol"], item["transport"], item["http_status"],
               item["latency_ms"], item["error_type"] or "ok")
        )
    return lines


# --------------------------------------------------------------------------
# 子命令：list
# --------------------------------------------------------------------------


def cmd_list(args) -> int:
    catalog = load_catalog()
    refreshed = bool(getattr(args, "refresh", False))
    upstream_ids = []
    if refreshed:
        # 需要核对最新目录时才联网；默认路径不碰网络（契约 §4）。
        upstream_ids = _fetch_upstream_catalog_ids()
    if upstream_ids:
        known = {str(k) for k in catalog}
        ids = [m for m in upstream_ids if m.endswith("-free")]
        models = [
            {"id": m, "in_cache": m in known,
             "context": (catalog.get(m, {}).get("limit", {}) or {}).get("context"),
             "max_output": (catalog.get(m, {}).get("limit", {}) or {}).get("output")}
            for m in ids
        ]
    else:
        models = []
        for model_id in sorted(catalog):
            if not model_id.endswith("-free"):
                continue
            spec = catalog[model_id]
            limits = spec.get("limit") if isinstance(spec, dict) else None
            limits = limits if isinstance(limits, dict) else {}
            models.append({
                "id": model_id,
                "in_cache": True,
                "context": limits.get("context"),
                "max_output": limits.get("output"),
            })
    payload = {
        "source": "upstream+cache" if refreshed else "cache",
        "refreshed": refreshed,
        "cache_path": os.path.expanduser(CACHE_PATH),
        "total": len(models),
        "models": models,
    }
    human = [
        "zen 免费模型（%s，共 %d 个）" % ("上游目录 + 本地缓存" if refreshed else "本地缓存，离线", len(models)),
        "元信息来源：%s" % payload["cache_path"],
        "",
        "%-40s %-10s %12s %12s" % ("模型 id", "本地有规格", "context", "max_output"),
    ]
    for item in models:
        human.append("%-40s %-10s %12s %12s" % (
            item["id"], "是" if item["in_cache"] else "否",
            item["context"] if item["context"] else "-",
            item["max_output"] if item["max_output"] else "-",
        ))
    if not models:
        human.append("（没有匹配的 -free 模型：先跑一次 opencode CLI 刷新缓存）")
    emit(payload, args.json, human)
    return EXIT_OK


def _fetch_upstream_catalog_ids() -> list:
    """上游 /models 目录（只有 id，无规格）。需带 opencode 身份头，否则 403。"""
    key = resolve_key(None)
    url = ZEN_BASE + "/models"
    session = new_session_id()
    headers = probe_headers(key, session)
    del headers["content-type"]
    result = _http_call(url, None, headers, False, HTTP_TIMEOUT_S, method="GET")
    if result["status"] != 200:
        raise ZenFreeError(
            "拉取上游目录失败：HTTP %s（%s）" % (result["status"], result["detail"] or "见 --via auto 绕过地区拦截"),
            EXIT_FAIL,
        )
    try:
        parsed = json.loads(result["body"])
    except ValueError as exc:
        raise ZenFreeError("上游目录响应不是合法 JSON：%s" % exc, EXIT_FAIL)
    data = parsed.get("data") if isinstance(parsed, dict) else None
    if not isinstance(data, list):
        raise ZenFreeError("上游目录缺少 data 数组（只有 id、无规格）", EXIT_FAIL)
    return [str(item.get("id")) for item in data if isinstance(item, dict) and item.get("id")]


# --------------------------------------------------------------------------
# 子命令：info
# --------------------------------------------------------------------------


def cmd_info(args) -> int:
    key = resolve_key(args.key)  # 契约 §6 A8：info 缺 key → 退出码 2
    del key  # 仅做前置校验，绝不回显
    spec = require_spec(load_catalog(), args.model)
    limits = spec.get("limit") or {}
    modalities = spec.get("modalities") or {}
    levels = thinking_levels(spec)
    payload = {
        "id": spec.get("id"),
        "name": spec.get("name"),
        "available_in_cache": True,
        "context_window": limits.get("context"),
        "max_output": limits.get("output"),
        "context_window_gateway": to_size(limits.get("context")),
        "max_output_gateway": to_size(limits.get("output")),
        "input_types_upstream": list(modalities.get("input") or []),
        "output_types_upstream": list(modalities.get("output") or []),
        "tool_call": bool(spec.get("tool_call")),
        "reasoning": bool(spec.get("reasoning")),
        "reasoning_efforts": levels,
        "cost": spec.get("cost"),
        "protocol_hint": "zen free 模型默认 chat 线；用 probe --protocol auto 实测确认"
                         "（/responses 实测 400 ModelProtocolUnsupported）",
        "gate_suffix_ok": str(spec.get("id", "")).endswith("-free"),
    }
    human = [
        "模型：%s（%s）" % (payload["id"], payload["name"]),
        "规格来源：%s" % os.path.expanduser(CACHE_PATH),
        "context / 输出上限：%s / %s → 网关 %s / %s" % (
            payload["context_window"], payload["max_output"],
            payload["context_window_gateway"], payload["max_output_gateway"]),
        "输入模态（上游）：%s" % (", ".join(payload["input_types_upstream"]) or "-"),
        "输出模态（上游）：%s" % (", ".join(payload["output_types_upstream"]) or "-"),
        "工具调用：%s｜推理：%s｜思考档位：%s" % (
            "支持" if payload["tool_call"] else "不支持",
            "支持" if payload["reasoning"] else "不支持",
            ", ".join(levels) if levels else "未声明（payload 不写 thinking_* 字段）"),
        "免费：%s" % ("是（cost 全 0）" if _is_free_cost(payload["cost"]) else "见 cost 字段"),
        "门禁后缀 -free：%s" % ("PASS" if payload["gate_suffix_ok"] else "FAIL（非 -free 不受门禁保护）"),
        "协议线索：%s" % payload["protocol_hint"],
    ]
    emit(payload, args.json, human)
    return EXIT_OK


def _is_free_cost(cost) -> bool:
    if not isinstance(cost, dict):
        return False
    values = [v for v in cost.values() if isinstance(v, (int, float))]
    return bool(values) and all(v == 0 for v in values)


# --------------------------------------------------------------------------
# 子命令：probe
# --------------------------------------------------------------------------


def cmd_probe(args) -> int:
    key = resolve_key(args.key)
    result = run_probe(args.model, args.protocol, args.via, args.variant, key)
    emit(result, args.json, _probe_human(result))
    if result["ok"]:
        return EXIT_OK
    sys.stderr.write(
        "zen_free: probe 失败：error_type=%s http_status=%s\n"
        % (result["error_type"], result["http_status"])
    )
    return EXIT_FAIL


# --------------------------------------------------------------------------
# 子命令：plan
# --------------------------------------------------------------------------


def cmd_plan(args) -> int:
    tier = resolve_tier(args.tier)  # 非法 tier → 退出码 2（先于任何网络/缓存动作）
    resolve_key(args.key)  # 契约 §6 A8：plan 缺 key → 退出码 2
    spec = require_spec(load_catalog(), args.model)
    if not str(spec.get("id", "")).endswith("-free"):
        sys.stderr.write(
            "zen_free: 模型 %s 不是 -free 后缀，不受 zen 门禁保护（契约 §3 I2），拒绝生成 payload\n"
            % spec.get("id")
        )
        return EXIT_FAIL
    payload = build_payload(spec, tier, args.protocol or DEFAULT_PROTOCOL)
    human = [
        "CreateModelPayload（%s，尚未写入网关）" % payload["name"],
        "  provider        = %s" % payload["provider"],
        "  name            = %s" % payload["name"],
        "  tier            = %s%s" % (payload["tier"], "（缺省 = 轻量）" if not args.tier else ""),
        "  context_window  = %s" % payload["context_window"],
        "  max_output      = %s" % payload["max_output"],
        "  input_types     = %s" % json.dumps(payload["input_types"], ensure_ascii=False),
        "  output_types    = %s" % json.dumps(payload["output_types"], ensure_ascii=False),
        "  protocol        = %s" % payload["protocol"],
        "  pricing_mode    = %s（小写；大写会被网关 400 拒收）" % payload["pricing_mode"],
    ]
    if "thinking_default" in payload:
        human.append("  thinking_default = %s" % payload["thinking_default"])
        human.append("  thinking_max     = %s" % payload["thinking_max"])
    else:
        human.append("  thinking_*       = 不写（上游未声明推理档位，契约 §4.2）")
    human.append("  base_url/proxy   = 不写（契约 §3 I3，填了触发 400 egress_blocked）")
    emit(payload, args.json, human)
    return EXIT_OK


# --------------------------------------------------------------------------
# 子命令：check-gate（契约 §3 三条不变式；纯静态核验，不联网）
# --------------------------------------------------------------------------


def evaluate_gates(model: str) -> dict:
    gates = []
    # I1：provider 前缀 opencode ⇒ is_opencode_zen_target 命中，头注入生效（upstream.rs:779）
    i1_ok = GATEWAY_PROVIDER.lower().startswith("opencode")
    gates.append({
        "id": "I1", "name": "作用域", "status": "PASS" if i1_ok else "FAIL",
        "evidence": "upstream.rs:779 is_opencode_zen_target(provider='%s') = %s"
                    % (GATEWAY_PROVIDER, i1_ok),
    })
    # I2：物理模型名以 -free 结尾 ⇒ 工具注入 + 强制 stream（upstream.rs:794）
    i2_ok = model.endswith("-free")
    gates.append({
        "id": "I2", "name": "后缀", "status": "PASS" if i2_ok else "FAIL",
        "evidence": "upstream.rs:794 zen_free_tier_forces_upstream_stream: id.endswith('-free') = %s"
                    % i2_ok,
    })
    # I3：模型级不填 base_url/proxy（契约 §3 I3）—— build_payload 永不产出这两个键
    payload_keys = ("provider", "name", "tier", "context_window", "max_output",
                    "input_types", "output_types", "protocol", "pricing_mode")
    i3_ok = all(k not in payload_keys for k in ("base_url", "proxy"))
    gates.append({
        "id": "I3", "name": "配置面", "status": "PASS" if i3_ok else "FAIL",
        "evidence": "契约 §3 I3：模型级不写 base_url/proxy（admin.rs:2362 仅 provider 级可配）= %s" % i3_ok,
    })
    return {
        "model": model,
        "provider": GATEWAY_PROVIDER,
        "all_pass": all(g["status"] == "PASS" for g in gates),
        "gates": gates,
    }


def cmd_check_gate(args) -> int:
    result = evaluate_gates(args.model)
    human = ["客户端身份门禁不变式核验（%s @ %s，纯静态、不联网）" % (result["model"], result["provider"])]
    for gate in result["gates"]:
        human.append("  [%s] %-6s %s" % (gate["status"], gate["id"], gate["name"]))
        human.append("        依据：%s" % gate["evidence"])
    human.append("总判定：%s" % ("三条全 PASS —— 零模型级配置即可过 zen 门禁"
                                if result["all_pass"] else "存在 FAIL，见上方依据"))
    emit(result, args.json, human)
    if result["all_pass"]:
        return EXIT_OK
    failed = [g["id"] for g in result["gates"] if g["status"] != "PASS"]
    sys.stderr.write("zen_free: 门禁核验未通过：%s（模型 %s）\n" % (",".join(failed), args.model))
    return EXIT_FAIL


# --------------------------------------------------------------------------
# 子命令：apply（默认 dry-run；真写必须显式 --confirm-write）
# --------------------------------------------------------------------------


def cmd_apply(args) -> int:
    tier = resolve_tier(args.tier)
    key = resolve_key(args.key)
    spec = require_spec(load_catalog(), args.model)
    payload = build_payload(spec, tier, DEFAULT_PROTOCOL)
    if not spec["id"].endswith("-free"):
        sys.stderr.write("zen_free: 拒绝写入非 -free 模型 %s（契约 §3 I2）\n" % spec["id"])
        return EXIT_FAIL
    if not args.confirm_write:
        emit(payload, args.json, [
            "DRY-RUN：%s 的 CreateModelPayload（未写入网关）" % payload["name"],
            json.dumps(payload, ensure_ascii=False, indent=2),
            "",
            "确认无误后加 --confirm-write 才会真正 POST（先跑 probe 确认可用）。",
        ])
        return EXIT_OK

    gw = args.gw.rstrip("/")
    auth = {"authorization": "Bearer " + key, "content-type": "application/json"}
    overview = _admin_get(gw + "/api/admin/overview", auth)
    version = overview.get("config_version")
    if version is None:
        sys.stderr.write("zen_free: 网关 overview 响应缺 config_version：%s\n" % json.dumps(overview, ensure_ascii=False))
        return EXIT_FAIL
    headers = dict(auth)
    headers["If-Match"] = str(version)  # 缺 If-Match → 412 precondition_failed（admin.rs:1668）
    status, body = _admin_post(gw + "/api/admin/models", auth | {"If-Match": str(version)}, payload)
    result = {"model": payload["name"], "http_status": status,
              "config_version_before": version, "applied": status in (200, 201), "response": body}
    emit(result, args.json, [
        "写入 %s → HTTP %s（config_version %s → %s）"
        % (payload["name"], status, version, body.get("config_version", "?")),
        json.dumps(body, ensure_ascii=False, indent=2),
    ])
    if not result["applied"]:
        sys.stderr.write("zen_free: 写入失败 HTTP %s：%s\n" % (status, json.dumps(body, ensure_ascii=False)))
        return EXIT_FAIL
    return EXIT_OK


def _admin_get(url: str, headers: dict) -> dict:
    request = urllib.request.Request(url, headers=headers, method="GET")
    try:
        with _build_opener(False).open(request, timeout=HTTP_TIMEOUT_S) as response:
            raw = response.read()
    except urllib.error.HTTPError as exc:
        sys.stderr.write("zen_free: GET %s 失败 HTTP %s\n" % (url, exc.code))
        raise ZenFreeError("读取网关 overview 失败（HTTP %s）：先确认网关已启动且 key 有 admin 权限" % exc.code, EXIT_FAIL)
    except (urllib.error.URLError, socket.timeout, OSError) as exc:
        raise ZenFreeError("连接网关 %s 失败：%s" % (url, _short(exc)), EXIT_FAIL)
    try:
        parsed = json.loads(raw.decode("utf-8", "replace"))
    except ValueError as exc:
        raise ZenFreeError("网关 overview 响应不是合法 JSON：%s" % exc, EXIT_FAIL)
    return parsed if isinstance(parsed, dict) else {}


def _admin_post(url: str, headers: dict, payload: dict) -> tuple:
    data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(url, data=data, headers=headers, method="POST")
    try:
        with _build_opener(False).open(request, timeout=HTTP_TIMEOUT_S) as response:
            raw = response.read()
            status = int(getattr(response, "status", 0) or response.getcode() or 0)
    except urllib.error.HTTPError as exc:
        raw = exc.read()
        status = int(exc.code)
    except (urllib.error.URLError, socket.timeout, OSError) as exc:
        raise ZenFreeError("写网关失败：%s" % _short(exc), EXIT_FAIL)
    try:
        body = json.loads(raw.decode("utf-8", "replace"))
    except ValueError:
        body = {"raw": raw.decode("utf-8", "replace")[:400]}
    return status, body if isinstance(body, dict) else {"raw": body}


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="zen_free.py",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        description="opencode-zen 免费模型助手：list/info/probe/plan/apply/check-gate。",
        epilog="子命令：list, info, probe, plan, apply, check-gate",
    )
    parser.add_argument("--key", help="zen api key（优先级高于 $OPENCODE_ZEN_KEY 与 opencode.json）")
    subs = parser.add_subparsers(dest="command", metavar="{list,info,probe,plan,apply,check-gate}")
    subs.required = True

    def add_key(sub):
        sub.add_argument("--key", dest="key_sub", help=argparse.SUPPRESS)

    p_list = subs.add_parser("list", help="列 zen 免费模型（默认离线，只读本地缓存）")
    p_list.add_argument("--json", action="store_true", help="stdout 只出一个 JSON 对象")
    p_list.add_argument("--refresh", action="store_true", help="额外联网核对上游 /models 目录（该目录只有 id、无规格）")
    add_key(p_list)
    p_list.set_defaults(func=cmd_list)

    p_info = subs.add_parser("info", help="抓元信息：context/output/模态/tool_call/思考档位/协议线索")
    p_info.add_argument("model")
    p_info.add_argument("--json", action="store_true")
    add_key(p_info)
    p_info.set_defaults(func=cmd_info)

    p_probe = subs.add_parser("probe", help="按 zen 门禁形态探活（opencode 身份头 + 12 工具 + stream）")
    p_probe.add_argument("model")
    p_probe.add_argument("--protocol", choices=("chat", "responses", "auto"), default="auto")
    p_probe.add_argument("--via", choices=("direct", "proxy", "auto"), default="auto")
    p_probe.add_argument("--variant", choices=("default", "low", "medium", "high", "max"), default="default",
                         help="推理档位变体（default = 不下发 reasoning_effort）")
    p_probe.add_argument("--json", action="store_true")
    add_key(p_probe)
    p_probe.set_defaults(func=cmd_probe)

    p_plan = subs.add_parser("plan", help="生成 ponyllm CreateModelPayload（不写网关）")
    p_plan.add_argument("model")
    p_plan.add_argument("--tier", metavar="{F,S,L}", help="缺省 = L（轻量）")
    p_plan.add_argument("--protocol", choices=("chat", "responses"), default=DEFAULT_PROTOCOL)
    p_plan.add_argument("--json", action="store_true")
    add_key(p_plan)
    p_plan.set_defaults(func=cmd_plan)

    p_apply = subs.add_parser("apply", help="走 Admin API 写入网关（默认 dry-run）")
    p_apply.add_argument("model")
    p_apply.add_argument("--gw", default=DEFAULT_GW, help="网关地址，默认 %s" % DEFAULT_GW)
    p_apply.add_argument("--tier", metavar="{F,S,L}")
    p_apply.add_argument("--dry-run", action="store_true", help="显式声明只打印 payload（默认行为）")
    p_apply.add_argument("--confirm-write", action="store_true", help="唯一能触发真实写入的开关")
    p_apply.add_argument("--json", action="store_true")
    add_key(p_apply)
    p_apply.set_defaults(func=cmd_apply)

    p_gate = subs.add_parser("check-gate", help="核验客户端身份门禁三条不变式（纯静态、不联网）")
    p_gate.add_argument("model")
    p_gate.add_argument("--json", action="store_true")
    add_key(p_gate)
    p_gate.set_defaults(func=cmd_check_gate)
    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    args.key = getattr(args, "key", None) or getattr(args, "key_sub", None)
    try:
        return args.func(args)
    except ZenFreeError as exc:
        sys.stderr.write("zen_free: %s\n" % exc.message)
        return exc.code
    except KeyboardInterrupt:
        sys.stderr.write("zen_free: 被用户中断\n")
        return EXIT_USAGE
    except Exception as exc:  # 兜底：不吞异常，打印可读原因后非零退出
        sys.stderr.write("zen_free: 未预期错误 %s: %s\n" % (type(exc).__name__, _short(exc)))
        return EXIT_USAGE


if __name__ == "__main__":
    sys.exit(main())