"""黑盒验收测试：ponyllm opencode-zen 免费模型 skill。

契约（只读冻结）：`.dev-team/contracts/zen-free-skill-contract.md`（v1.4，Lead 冻结）
被测对象（只读黑盒）：`skills/ponyllm-zen-free/SKILL.md`、`skills/ponyllm-zen-free/scripts/zen_free.py`
本文件不 import 任何业务代码，全部通过 `subprocess` 调 `python3 <repo>/skills/ponyllm-zen-free/scripts/zen_free.py ...`。

验收矩阵对应（契约 §6，A1–A9 逐条覆盖，每条至少一个断言函数）：
  A1 SKILL.md 六要素**并集**（触发式 description / 前置 / 真相源 / 步骤 / 样例 / 验证——§6 A1 与
     .agents/skills/README.md 取并集，v1.4 已裁决）+ §2 真相源路径引用
  A2 scripts/zen_free.py --help 退出码 0、列出 §4 全部子命令，且每个 `<cmd> --help` 退出码 0（§4.4-4）
  A3 `list --json` stdout 单一 JSON 对象、含 models 数组、每项含 id 且以 -free 结尾（离线路径）
  A4 `plan step-5-preview-free --json` 裸 CreateModelPayload（§4.4-2 无包装键），tier 缺省 == "L"，
     size 换算按 §4.3 锚点表（1048576→1M / 1000000→977K / 131072→128K / 65536→64K / 100000→98K / 5000→5K）
  A5 `plan --tier F` → "F"；非法 tier → 退出码 2（正反成对）
  A6 `check-gate`：`-free` 三项全 PASS 且退出码 0；非 `-free` 的 I2 必须 FAIL 且退出码 1
     （§4.4-1：无论是否 --json）
  A7 输出/日志不出现 api key 原文/前缀（env 与 `--key` 前后挂两种路径，含错误路径）
  A8 缺 key 时 info/plan/probe 退出码 2、stderr 有原因、无 traceback（负例）
  A9 源码无裸 `except:` / `except X: pass`（AST 静态检查）+ 只用标准库

附加（非 §6 矩阵，契约 §4.1/§4.5 明文要求，离线构造，已在交付报告中单独标注）：
  - §4 通用约定：key 三来源之一 `--key` 可用、配置文件 `provider.opencode.options.apiKey` 可用
  - §4.1：`probe --json` 成功路径必含 ok/protocol/http_status/latency_ms/error_type/gate_failure/
    transport/attempts/output_text/usage
  - §4.5：7 类 error_type 分类（geo_blocked/gate_failure/usage_limit/protocol_unsupported/
    not_found/transport_error）+ geo_blocked 时 `--via auto` 必须切代理重试

红相说明（RED phase）：
  skills/ponyllm-zen-free/** 当前**不存在**（Executor 尚未实现），因此 A1/A2/A3 及其余全部
  用例必然 RED。本文件在 Executor 交付后必须整体转 GREEN；实现者只读本文件，不得改测试。

物理限制（契约 §7，测试据此裁剪，**不写假断言**）：
  - 全部子命令走**离线路径**：子进程经 PYTHONPATH 注入 `sitecustomize.py`，把
    `urllib.request.OpenerDirector.open` / `urllib.request.urlopen` 换成固定夹具响应，
    并把 `socket.connect/create_connection` 全部封死（真发网络会直接炸 → 用例 FAIL，绝不静默联网）。
  - HOME/XDG_* 全部指向 pytest 临时目录，既不读真 key 也不读真缓存；无残留。
  - 每个 subprocess 显式 `timeout=30s`，超时 **FAIL**（不 skip、不静默跳过）。
  - 真实联网探活由 Executor 手工执行并贴 stdout 收据，不进 pytest。

checker 自检（防空断言自放行）：每个自定义判定器（要素缺失 / 子命令缺失 / payload 缺字段 /
tier 接受 / gate 判定抽取 / key 泄漏 / except 违规 / 非标准库 import）都配一组**反例输入**
证明它对坏样本确实报错、且对空输入不放行。
"""

from __future__ import annotations

import ast
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SKILL_DIR = REPO_ROOT / "skills" / "ponyllm-zen-free"
SKILL_MD = SKILL_DIR / "SKILL.md"
SCRIPT = SKILL_DIR / "scripts" / "zen_free.py"

# 契约 §4 声明的全部子命令（A2 逐个断言）
SUBCOMMANDS = ("list", "info", "probe", "plan", "apply", "check-gate")

TIMEOUT_S = 30

# 契约 §4 通用约定的唯一红线：api key 原文不得出现在任何输出/日志里。
# 预置虚拟凭据（test-expert §4.4：严禁硬编码真实密钥），绝不出现在真实系统里。
FAKE_KEY = "sk-zen-DUMMY-9f3a7c1e-0000-4fff-8888-aaaabbbbcccc"
FAKE_KEY_PREFIX = FAKE_KEY[:8]

# 契约 §4.3 error_type 全枚举
ERROR_TYPES = frozenset(
    {
        "geo_blocked",
        "gate_failure",
        "usage_limit",
        "protocol_unsupported",
        "not_found",
        "transport_error",
        "upstream_unavailable",
    }
)

# ---------------------------------------------------------------------------
# 离线夹具数据（真实 opencode models.json 形状，取自 ~/.cache/opencode/models.json）
# ---------------------------------------------------------------------------

STEP5 = {
    "id": "step-5-preview-free",
    "name": "Step 5 Preview Free",
    "reasoning": True,
    "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}],
    "tool_call": True,
    "attachment": True,
    "structured_output": True,
    "modalities": {"input": ["text", "image", "video"], "output": ["text"]},
    "limit": {"context": 1000000, "input": 1000000, "output": 65536},
    "cost": {"input": 0, "output": 0, "cache_read": 0},
}

MUSE_FREE = {
    "id": "muse-spark-1.3-contributor-free",
    "name": "Muse Spark 1.3 Free",
    "reasoning": True,
    "reasoning_options": [
        {"type": "effort", "values": ["minimal", "low", "medium", "high", "xhigh"]}
    ],
    "tool_call": True,
    "attachment": True,
    "modalities": {"input": ["text", "image", "video", "pdf", "audio"], "output": ["text"]},
    "limit": {"context": 1048576, "output": 131072},
    "cost": {"input": 0, "output": 0, "cache_read": 0},
}

# 非 -free 后缀的 zen 模型（契约 §3 I2 反例靶子）
MUSE_PAID = {
    "id": "muse-spark-1.3",
    "name": "Muse Spark 1.3",
    "reasoning": True,
    "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}],
    "tool_call": True,
    "modalities": {"input": ["text", "image"], "output": ["text"]},
    "limit": {"context": 262144, "output": 32768},
    "cost": {"input": 4, "output": 12, "cache_read": 0},
}

# 非推理模型（契约 §4.2：未声明 reasoning_options → 不得写 thinking_* 字段）
LITE_FREE = {
    "id": "muse-lite-1.0-free",
    "name": "Muse Lite 1.0 Free",
    "reasoning": False,
    "reasoning_options": [],
    "tool_call": True,
    "modalities": {"input": ["text"], "output": ["text"]},
    "limit": {"context": 131072, "output": 16384},
    "cost": {"input": 0, "output": 0, "cache_read": 0},
}

# 换算尾分支靶子：context/output 都小于 131072 ⇒ 走契约 §4.2 的「其余按 K 向上取整」，
# 1M/256K/128K 三个桶都盖不到这条分支（100000/1024=97.65 → 98K；5000/1024=4.88 → 5K）。
TAIL_FREE = {
    "id": "tail-lite-free",
    "name": "Tail Lite Free",
    "reasoning": False,
    "reasoning_options": [],
    "tool_call": True,
    "modalities": {"input": ["text"], "output": ["text"]},
    "limit": {"context": 100000, "output": 5000},
    "cost": {"input": 0, "output": 0, "cache_read": 0},
}


def _err_model(model_id: str) -> dict:
    """为 §4.3 错误分类构造的靶子模型（都在 zen 目录内、均为 -free 后缀）。"""
    return {
        "id": model_id,
        "name": model_id,
        "reasoning": False,
        "reasoning_options": [],
        "tool_call": True,
        "modalities": {"input": ["text"], "output": ["text"]},
        "limit": {"context": 131072, "output": 8192},
        "cost": {"input": 0, "output": 0, "cache_read": 0},
    }


ERR_GATE = _err_model("zen-gate-403-free")
ERR_GEO = _err_model("zen-geo-403-free")
ERR_USAGE = _err_model("zen-usage-429-free")
ERR_PROTO = _err_model("zen-unsupported-400-free")
ERR_404 = _err_model("zen-missing-404-free")
ERR_502 = _err_model("zen-tunnel-502-free")

CATALOG_MODELS = [
    STEP5,
    MUSE_FREE,
    MUSE_PAID,
    LITE_FREE,
    TAIL_FREE,
    ERR_GATE,
    ERR_GEO,
    ERR_USAGE,
    ERR_PROTO,
    ERR_404,
    ERR_502,
]

# 真实 opencode 缓存形状：{provider_id: {"api": ..., "models": {model_id: {...}}}}。
# 注意：契约 §2 写作 `providers.opencode.models[*]`，与本机真实文件结构不符（无 providers 包装层、
# models 是以 id 为键的对象而非数组）——已上报 Lead 裁决，此处按**本机真实结构**播种。
FAKE_CACHE = {
    "opencode": {
        "id": "opencode",
        "api": "https://opencode.ai/zen/v1",
        "name": "OpenCode Zen",
        "models": {m["id"]: m for m in CATALOG_MODELS},
    }
}


def _sse(model: str, text: str, prompt_tokens: int = 11, completion_tokens: int = 3) -> str:
    chunks = [
        {
            "id": "chatcmpl-zenfree-1",
            "object": "chat.completion.chunk",
            "created": 1757000000,
            "model": model,
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-zenfree-1",
            "object": "chat.completion.chunk",
            "created": 1757000000,
            "model": model,
            "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-zenfree-1",
            "object": "chat.completion.chunk",
            "created": 1757000000,
            "model": model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens + completion_tokens,
            },
        },
    ]
    return "".join("data: " + json.dumps(c) + "\n\n" for c in chunks) + "data: [DONE]\n\n"


def _json_error(message: str, err_type: str, code: str | None = None) -> str:
    err: dict = {"message": message, "type": err_type}
    if code:
        err["code"] = code
    return json.dumps({"error": err})


def build_fake_http() -> dict:
    """离线 HTTP 夹具：URL 子串 + 请求体 model 双重路由；未命中一律 502（等价 §4.3 transport_error）。"""
    routes = [
        # §4.3 分类靶子（顺序在前，优先于 catch-all 成功路由）
        {
            "match": "/chat/completions",
            "model": ERR_GATE["id"],
            "status": 403,
            "body": _json_error("Free tier request rejected", "FreeTierError", "free_tier"),
        },
        {
            "match": "/chat/completions",
            "model": ERR_GEO["id"],
            "status": 403,
            "headers": {"cf-ray": "8a1b2c3d4e5f6789-ORD", "cf-placement": "remote-ORD"},
            "body": _json_error("This model is not available in your country", "Forbidden"),
        },
        {
            "match": "/chat/completions",
            "model": ERR_USAGE["id"],
            "status": 429,
            "body": _json_error("Free usage limit reached", "FreeUsageLimitError", "usage_limit"),
        },
        {
            "match": "/chat/completions",
            "model": ERR_PROTO["id"],
            "status": 400,
            "body": _json_error(
                "ModelProtocolUnsupported: model does not support this protocol",
                "invalid_request_error",
                "ModelProtocolUnsupported",
            ),
        },
        {
            "match": "/chat/completions",
            "model": ERR_404["id"],
            "status": 404,
            "body": _json_error("No endpoints found for " + ERR_404["id"], "invalid_request_error", "model_not_found"),
        },
        {
            "match": "/chat/completions",
            "model": ERR_502["id"],
            "status": 502,
            "body": _json_error("tunnel_failed", "transport_error"),
        },
        # 上游目录
        {
            "match": "/zen/v1/models",
            "status": 200,
            "body": json.dumps({"object": "list", "data": CATALOG_MODELS}),
        },
        # chat 成功（契约 §4.1：stream:true ⇒ SSE 事件流）
        {
            "match": "/chat/completions",
            "status": 200,
            "content_type": "text/event-stream",
            "body": _sse("step-5-preview-free", "bunny-ok"),
        },
        # responses 线（实测 step-5-preview-free → 400 ModelProtocolUnsupported）
        {
            "match": "/responses",
            "status": 400,
            "body": _json_error(
                "ModelProtocolUnsupported", "invalid_request_error", "ModelProtocolUnsupported"
            ),
        },
    ]
    return {
        "routes": routes,
        "default": {
            "status": 502,
            "body": _json_error("offline-fixture: no route matched (test guard)", "transport_error"),
        },
    }


# ---------------------------------------------------------------------------
# 子进程离线注入器（写入 pytest 临时目录，随 tmp_path 自动回收）
# ---------------------------------------------------------------------------

SITECUSTOMIZE_SRC = r'''"""zen_free 验收测试的离线注入器（写进 pytest tmp 目录，经 PYTHONPATH 自动导入）。

职责：
  1. 封死真网络：任何 socket 连接尝试直接抛 RuntimeError（测试必 FAIL，绝不静默联网）。
  2. 把 urllib 的出口换成固定夹具（同时覆盖 urlopen 与 build_opener().open 两种写法）。
"""
import email.message
import io
import json
import os
import socket
import sys
import urllib.error
import urllib.request

_FIXTURE = {}
try:
    with open(os.environ["ZENFREE_FAKE_HTTP"], "r", encoding="utf-8") as _fh:
        _FIXTURE = json.load(_fh)
except Exception as _exc:  # 夹具读不到就必须炸，绝不静默放行真网络
    print("zen_free-test: cannot load fake http fixture: %r" % (_exc,), file=sys.stderr)


class OfflineGuardError(RuntimeError):
    """真网络尝试（测试红线：禁止联网）。"""


def _blocked(*_args, **_kwargs):
    raise OfflineGuardError(
        "offline-guard: zen_free acceptance tests forbid real network access"
    )


socket.socket.connect = _blocked
socket.socket.connect_ex = _blocked
socket.create_connection = _blocked


def _headers(spec):
    msg = email.message.Message()
    msg["Content-Type"] = spec.get("content_type", "application/json")
    for k, v in (spec.get("headers") or {}).items():
        msg[k] = v
    return msg


class FakeResponse(object):
    """最小可用的 urllib 响应替身（支持 with / read / 迭代 / status / headers）。"""

    def __init__(self, url, status, headers, body):
        self._url = url
        self.status = status
        self.code = status
        self.reason = "OK" if status < 400 else "Error"
        self.headers = headers
        self.msg = headers
        self._body = body

    def read(self, amt=None):
        data = io.BytesIO(self._body)
        return data.read() if amt is None else data.read(amt)

    def readline(self):
        return io.BytesIO(self._body).readline()

    def readlines(self):
        return io.BytesIO(self._body).readlines()

    def __iter__(self):
        return iter(io.BytesIO(self._body).readlines())

    def getcode(self):
        return self.status

    def geturl(self):
        return self._url

    def info(self):
        return self.headers

    def close(self):
        return None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()
        return False


def _url_of(fullurl):
    return getattr(fullurl, "full_url", None) or getattr(fullurl, "url", None) or str(fullurl)


def _payload_text(payload):
    if payload is None:
        return ""
    if hasattr(payload, "read"):
        try:
            payload = payload.read()
        except Exception:
            return ""
    if isinstance(payload, (bytes, bytearray)):
        return payload.decode("utf-8", "replace")
    return str(payload)


def _model_of(text):
    try:
        return (json.loads(text) or {}).get("model")
    except Exception:
        return None


def _pick(url, text):
    for spec in _FIXTURE.get("routes", []):
        match = spec.get("match")
        if match and match not in url:
            continue
        want_model = spec.get("model")
        if want_model is not None and _model_of(text) != want_model:
            continue
        body_contains = spec.get("body_contains")
        if body_contains is not None and body_contains not in text:
            continue
        return spec
    return _FIXTURE.get("default", {"status": 502, "body": "{}"})


def _respond(fullurl, data=None):
    url = _url_of(fullurl)
    text = _payload_text(getattr(fullurl, "data", None) if data is None else data)
    spec = _pick(url, text)
    status = int(spec.get("status", 200))
    body = spec.get("body", "")
    raw = body.encode("utf-8") if isinstance(body, str) else bytes(body)
    hdrs = _headers(spec)
    hdrs["Content-Length"] = str(len(raw))
    if status >= 400:
        raise urllib.error.HTTPError(url, status, "Error", hdrs, io.BytesIO(raw))
    return FakeResponse(url, status, hdrs, raw)


def _opener_open(_self, fullurl, data=None, timeout=None, **_kwargs):
    return _respond(fullurl, data)


def _urlopen(fullurl, data=None, timeout=None, **_kwargs):
    return _respond(fullurl, data)


urllib.request.OpenerDirector.open = _opener_open
urllib.request.urlopen = _urlopen
'''


@pytest.fixture(scope="session")
def sandbox(tmp_path_factory):
    """会话级离线沙箱：注入目录 + 假 HTTP 夹具。"""
    root = tmp_path_factory.mktemp("zen_free_offline")
    inject = root / "inject"
    inject.mkdir()
    (inject / "sitecustomize.py").write_text(SITECUSTOMIZE_SRC, encoding="utf-8")
    fake_http = root / "fake_http.json"
    fake_http.write_text(json.dumps(build_fake_http(), ensure_ascii=False), encoding="utf-8")
    return {"root": root, "inject": inject, "fake_http": fake_http}


@pytest.fixture()
def home(tmp_path):
    """隔离 HOME：只播种离线元信息缓存，不放任何 key（谁创建谁清理——pytest tmp_path 自动回收）。"""
    h = tmp_path / "home"
    (h / ".cache" / "opencode").mkdir(parents=True)
    (h / ".cache" / "opencode" / "models.json").write_text(
        json.dumps(FAKE_CACHE, ensure_ascii=False), encoding="utf-8"
    )
    return h


# ---------------------------------------------------------------------------
# 运行器
# ---------------------------------------------------------------------------


def run_script(script, sandbox, home, args, *, env_key=None, env_extra=None, timeout=TIMEOUT_S):
    """跑任意脚本：隔离 env + 离线注入 + 显式超时（超时 FAIL，绝不静默跳过）。"""
    assert script.is_file(), f"脚本缺失：{script}（契约 §1 交付物，Executor 未实现 ⇒ 红相）"
    cmd = [sys.executable, str(script), *args]
    env = {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(Path(home) / ".config"),
        "XDG_CACHE_HOME": str(Path(home) / ".cache"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "PYTHONIOENCODING": "utf-8",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONPATH": str(sandbox["inject"]),
        "ZENFREE_FAKE_HTTP": str(sandbox["fake_http"]),
    }
    if env_key is not None:
        env["OPENCODE_ZEN_KEY"] = env_key
    if env_extra:
        env.update(env_extra)
    try:
        return subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout, env=env, cwd=str(REPO_ROOT)
        )
    except subprocess.TimeoutExpired as exc:
        pytest.fail(f"命令超过 {timeout}s 未返回（契约禁止挂死，必须 FAIL 暴露）：{cmd}\n{exc}")


def run_skill(sandbox, home, args, **kwargs):
    """跑被测脚本 zen_free.py（契约 §1 交付物；缺失即红相 FAIL）。"""
    return run_script(SCRIPT, sandbox, home, args, **kwargs)


def assert_exit(cp, expected: int, what: str):
    assert cp.returncode == expected, (
        f"{what}：期望退出码 {expected}，实际 {cp.returncode}"
        f"\n--- STDOUT ---\n{cp.stdout}\n--- STDERR ---\n{cp.stderr}"
    )


def assert_exit_zero(cp, what: str):
    assert_exit(cp, 0, what)


def assert_exit_nonzero(cp, what: str):
    assert cp.returncode != 0, f"{what}：期望非零退出码，实际 0\n--- STDOUT ---\n{cp.stdout}"


def parse_json_stdout(cp, what: str):
    """契约 §4：--json 时 stdout **只出** 一个 JSON 对象（多余文本 ⇒ json.loads 失败 ⇒ FAIL）。"""
    raw = cp.stdout.strip()
    assert raw, f"{what}：--json 模式 stdout 为空\n--- STDERR ---\n{cp.stderr}"
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        pytest.fail(
            f"{what}：--json 模式 stdout 必须是且只能是一个 JSON 对象（解析失败：{exc}）"
            f"\n--- STDOUT ---\n{cp.stdout}\n--- STDERR ---\n{cp.stderr}"
        )


# ---------------------------------------------------------------------------
# 通用判定器 + 各自反例自检
# ---------------------------------------------------------------------------

# A1 五要素：契约 §6 A1 明列 真相源/前置/步骤/样例/验证（与 .agents/skills/README.md 的
# 「触发式 description + 四要素」合并判定，见交付报告的歧义说明）。
ELEMENT_PATTERNS = {
    "truth_sources": r"^#{1,6}[^\n]*真相源",
    "prerequisites": r"^#{1,6}[^\n]*前置",
    "steps": r"^#{1,6}[^\n]*步骤",
    "examples": r"^#{1,6}[^\n]*样例",
    "verification": r"^#{1,6}[^\n]*验证",
}

REQUIRED_PAYLOAD_KEYS = frozenset(
    {
        "provider",
        "name",
        "context_window",
        "max_output",
        "input_types",
        "output_types",
        "protocol",
        "pricing_mode",
    }
)

SIZE_RE = re.compile(r"^(1M|(\d+)K)$")
THINKING_VALUES = frozenset({"low", "medium", "high", "max"})


def frontmatter(text: str) -> str:
    m = re.match(r"\s*---\s*\n(.*?)\n---\s*\n?", text, re.S)
    assert m, "SKILL.md 缺少 YAML frontmatter（--- 包裹的头部）"
    return m.group(1)


def missing_elements(text: str) -> list[str]:
    """返回缺失的 SKILL.md 要素名；空列表 = 齐全。"""
    fm = frontmatter(text)
    missing: list[str] = []
    dm = re.search(r"description\s*:\s*\|?-?(.*)", fm, re.S)
    if not (dm and ("何时用" in fm or "触发" in fm)):
        missing.append("description")
    body = text.split("---", 2)[-1] if text.count("---") >= 2 else text
    for name, pat in ELEMENT_PATTERNS.items():
        if not re.search(pat, body, re.M):
            missing.append(name)
    return missing


def missing_subcommands(help_text: str) -> list[str]:
    missing = []
    for cmd in SUBCOMMANDS:
        if not re.search(r"(?<![\w-])" + re.escape(cmd) + r"(?![\w-])", help_text):
            missing.append(cmd)
    return missing


def list_payload_problems(payload) -> list[str]:
    """A3 结构判定：必须是含 models 数组的对象，每项含非空字符串 id。"""
    problems: list[str] = []
    if not isinstance(payload, dict):
        return [f"顶层不是 JSON 对象：{type(payload).__name__}"]
    models = payload.get("models")
    if not isinstance(models, list):
        return [f"缺 models 数组（实得 {type(models).__name__}）"]
    if not models:
        problems.append("models 为空数组")
    for i, item in enumerate(models):
        if not isinstance(item, dict):
            problems.append(f"models[{i}] 不是对象")
            continue
        model_id = item.get("id")
        if not isinstance(model_id, str) or not model_id.strip():
            problems.append(f"models[{i}] 缺非空字符串 id（实得 {model_id!r}）")
    return problems


WRAPPER_KEYS = frozenset(
    {"payload", "source", "upstream", "meta", "result", "data", "notes", "note",
     "warnings", "debug", "logs", "raw", "result_json", "meta_info"}
)


def resolve_payload(obj) -> dict | None:
    """plan 输出解析（契约 v1.4 §4.4-2：`plan --json` **只出裸 payload 本体**，顶层平铺、无包装键）。

    不再接受 `{"payload": {...}}` 套壳形状——已被契约明文排除；顶层必须同时带 provider 与 name。
    「必需字段是否齐全」不在这里判（那是 A4 的结构断言），避免两个判定互相吞掉。
    """
    if isinstance(obj, dict) and {"provider", "name"} <= set(obj):
        return obj
    return None


def collect_keys(obj) -> set[str]:
    keys: set[str] = set()
    if isinstance(obj, dict):
        for k, v in obj.items():
            keys.add(str(k))
            keys |= collect_keys(v)
    elif isinstance(obj, list):
        for v in obj:
            keys |= collect_keys(v)
    return keys


def find_values(obj, key: str) -> list:
    found: list = []
    if isinstance(obj, dict):
        for k, v in obj.items():
            if str(k) == key:
                found.append(v)
            found.extend(find_values(v, key))
    elif isinstance(obj, list):
        for v in obj:
            found.extend(find_values(v, key))
    return found


def has_key(obj, key: str) -> bool:
    return key in collect_keys(obj)


# ---- gate 判定抽取（契约 §3 / A6） ----

GATE_MARKERS = (
    ("I1", ("i1", "作用域", "scope")),
    ("I2", ("i2", "后缀", "suffix")),
    ("I3", ("i3", "配置面", "base_url", "proxy")),
)


def _gate_of(haystack: str) -> str | None:
    low = haystack.lower()
    for gate, markers in GATE_MARKERS:
        if any(m in low for m in markers):
            return gate
    return None


def _verdict_of(value) -> str | None:
    if isinstance(value, bool):
        return "PASS" if value else "FAIL"
    if isinstance(value, str):
        s = value.strip().upper()
        if s in {"PASS", "PASSED", "OK", "TRUE"}:
            return "PASS"
        if s in {"FAIL", "FAILED", "ERROR", "FALSE"}:
            return "FAIL"
    return None


def _scan_gates(node, path, verdicts):
    if isinstance(node, dict):
        ctx = " ".join(
            [str(k) for k in node.keys()]
            + [str(v) for v in node.values() if isinstance(v, (str, int, float, bool))]
        )
        for k, v in node.items():
            verdict = None if isinstance(v, (dict, list)) else _verdict_of(v)
            if verdict is not None:
                gate = _gate_of(" ".join(path) + " " + str(k) + " " + ctx)
                if gate and gate not in verdicts:
                    verdicts[gate] = verdict
            elif isinstance(v, (dict, list)):
                _scan_gates(v, path + [str(k)], verdicts)
    elif isinstance(node, list):
        for i, v in enumerate(node):
            _scan_gates(v, path + [f"[{i}]"], verdicts)


def _verdict_token(text: str) -> str | None:
    """人读表格里的判定词（整行不一定等于 PASS/FAIL，如「[PASS] I1 作用域」「I2 后缀：FAIL」）。"""
    if re.search(r"(?<![A-Za-z])FAIL(?![A-Za-z])", text):
        return "FAIL"
    if re.search(r"(?<![A-Za-z])PASS(?![A-Za-z])", text):
        return "PASS"
    return None


def extract_gate_verdicts(stdout: str) -> dict[str, str]:
    """从 check-gate 输出抽取 {I1/I2/I3: PASS/FAIL}；抽不到就返回残缺 dict（由调用方 FAIL）。"""
    raw = stdout.strip()
    verdicts: dict[str, str] = {}
    try:
        obj = json.loads(raw)
    except json.JSONDecodeError:
        obj = None
    if obj is not None:
        _scan_gates(obj, [], verdicts)
    if len(verdicts) < 3:  # 非 JSON 人读表格兜底：逐行找「gate 标识 + PASS/FAIL 判定词」
        for line in raw.splitlines():
            gate = _gate_of(line)
            verdict = _verdict_of(line) or _verdict_token(line)
            if gate and verdict and gate not in verdicts:
                verdicts[gate] = verdict
    return verdicts


def require_gate_verdicts(cp, what: str) -> dict[str, str]:
    verdicts = extract_gate_verdicts(cp.stdout)
    missing = [g for g in ("I1", "I2", "I3") if g not in verdicts]
    assert not missing, (
        f"{what}：check-gate 未给出三条不变式（契约 §3）的 PASS/FAIL 判定，缺 {missing}"
        f"\n--- STDOUT ---\n{cp.stdout}\n--- STDERR ---\n{cp.stderr}"
    )
    return verdicts


# ---- key 泄漏判定（A7） ----


def key_leak_hits(text: str, key: str = FAKE_KEY) -> list[str]:
    hits = []
    if key and key in text:
        hits.append("key 原文")
    if len(key) >= 8 and key[:8] in text:
        hits.append("key 前 8 字符")
    return hits


# ---- A9 AST 判定 ----


def except_violations(source: str) -> list[tuple[int, str]]:
    """返回 (行号, 说明)：裸 except: 以及任何「空处理」handler（pass / ... / continue）。

    契约 §6 A9 明列裸 except 与 except Exception: pass；契约 §4「禁止吞异常，必须打印原因到 stderr」
    同时否掉 `except KeyError: pass` 这类只 pass 的 handler，故按精神一并拦下。
    """
    tree = ast.parse(source)
    violations: list[tuple[int, str]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.ExceptHandler):
            continue
        if node.type is None:
            violations.append((node.lineno, "裸 except:"))
            continue
        swallow = all(
            isinstance(st, ast.Pass)
            or (isinstance(st, ast.Expr) and isinstance(st.value, ast.Constant) and st.value.value is Ellipsis)
            for st in node.body
        )
        if swallow:
            label = ast.unparse(node.type)
            violations.append((node.lineno, f"吞异常 handler：except {label}: pass/…"))
    return violations


def non_stdlib_imports(source: str) -> list[str]:
    stdlib = frozenset(getattr(sys, "stdlib_module_names", ())) | frozenset(sys.builtin_module_names)
    bad: list[str] = []
    tree = ast.parse(source)
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                root = alias.name.split(".")[0]
                if root not in stdlib and root != "__future__":
                    bad.append(alias.name)
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            root = node.module.split(".")[0]
            if root not in stdlib and root != "__future__":
                bad.append(node.module)
    return bad


def read_skill_text() -> str:
    assert SKILL_MD.is_file(), f"SKILL.md 缺失：{SKILL_MD}（契约 §1 交付物）"
    return SKILL_MD.read_text(encoding="utf-8")


def read_script_source() -> str:
    assert SCRIPT.is_file(), f"脚本缺失：{SCRIPT}（契约 §1 交付物）"
    return SCRIPT.read_text(encoding="utf-8")


# ===========================================================================
# 测试台架自证（不依赖被测实现，红相阶段就必须绿）
#   证明「RED 来自 skill 未实现」，而不是夹具/注入器本身坏了；
#   同时物理证明 §7「禁止联网」是强制的：真网络一尝试就炸。
# ===========================================================================

HARNESS_PROBE = r'''
import json, socket, sys, urllib.error, urllib.request

def emit(tag, fn):
    try:
        print("%s:%s" % (tag, fn()))
    except BaseException as exc:
        print("%s_ERR:%r" % (tag, exc))

# 1) urlopen 直写风格 ⇒ 夹具接管
def _catalog():
    with urllib.request.urlopen("https://opencode.ai/zen/v1/models", timeout=5) as resp:
        data = json.loads(resp.read())
        return "%s|%s" % (resp.status, ",".join(m["id"] for m in data["data"][:2]))
emit("CATALOG", _catalog)

# 2) build_opener(ProxyHandler({})) 风格 ⇒ 同样必须被接管（契约 §5 的直连写法）
def _opener():
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open("https://opencode.ai/zen/v1/models") as resp:
        return str(resp.status)
emit("OPENER", _opener)

# 3) 4xx 必须以 urllib.error.HTTPError 抛出，且 body 可读（§4.3 分类依赖它）
def _gate_error():
    req = urllib.request.Request(
        "https://opencode.ai/zen/v1/chat/completions",
        data=json.dumps({"model": "zen-gate-403-free", "stream": True}).encode("utf-8"),
        headers={"content-type": "application/json"},
    )
    try:
        urllib.request.urlopen(req, timeout=5)
    except urllib.error.HTTPError as exc:
        return "%s|%s" % (exc.code, exc.read().decode("utf-8"))
    return "NO_RAISE"
emit("GATE", _gate_error)

# 4) 200 + SSE 事件流必须能整段读出（§4.1 判定口径依赖它）
def _sse():
    req = urllib.request.Request(
        "https://opencode.ai/zen/v1/chat/completions",
        data=json.dumps({"model": "step-5-preview-free", "stream": True}).encode("utf-8"),
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        body = resp.read().decode("utf-8")
    return "%s|%s|%s" % (resp.status, "bunny-ok" in body, "[DONE]" in body)
emit("SSE", _sse)

# 5) 真网络必须被封死（契约 §7 禁止联网）
def _net():
    socket.create_connection(("opencode.ai", 443), timeout=5)
    return "ALLOWED"
emit("NET", _net)

# 6) 隔离 HOME 生效：~ 展开指向临时 home，真实缓存读不到
def _home():
    import os
    p = os.path.expanduser("~/.cache/opencode/models.json")
    exists = os.path.exists(p)
    return "%s|exists=%s" % (p, exists)
emit("LEAKHOME", _home)
'''


@pytest.fixture()
def harness_probe(tmp_path):
    probe = tmp_path / "harness_probe.py"
    probe.write_text(HARNESS_PROBE, encoding="utf-8")
    return probe


def test_harness_offline_fixture_is_wired(sandbox, home, harness_probe):
    """urlopen / build_opener 两种写法都被夹具接管；4xx 抛 HTTPError 且 body 可读；SSE 可整段读。"""
    cp = run_script(harness_probe, sandbox, home, [], timeout=TIMEOUT_S)
    assert_exit_zero(cp, "测试台架探针")
    out = cp.stdout
    assert "CATALOG:200|step-5-preview-free,muse-spark-1.3-contributor-free" in out, (
        f"夹具未接管 urlopen\n--- STDOUT ---\n{out}\n--- STDERR ---\n{cp.stderr}"
    )
    assert "OPENER:200" in out, f"夹具未接管 build_opener().open\n--- STDOUT ---\n{out}"
    assert re.search(r"GATE:403\|.*FreeTierError", out), f"403 未以 HTTPError 抛出或 body 不可读\n{out}"
    assert "SSE:200|True|True" in out, f"200/SSE 夹具不可读或内容不符\n{out}"
    for tag in ("CATALOG_ERR", "OPENER_ERR", "GATE_ERR", "SSE_ERR"):
        assert tag not in out, f"台架探针内部异常：{tag}\n--- STDOUT ---\n{out}\n--- STDERR ---\n{cp.stderr}"


def test_harness_real_network_is_killed(sandbox, home, harness_probe):
    """物理收据：真网络被封死（否则 §7「禁止联网」只是口头承诺）。"""
    cp = run_script(harness_probe, sandbox, home, [], timeout=TIMEOUT_S)
    assert "NET_ERR:" in cp.stdout, f"真网络未被封死（测试将可能联网）\n--- STDOUT ---\n{cp.stdout}"
    assert "NET:ALLOWED" not in cp.stdout, "真网络被放行：离线约束失效"


def test_harness_home_isolation_hides_real_cache(sandbox, home, harness_probe):
    """隔离 HOME 生效：子进程的 ~ 解析到临时 home，读不到真实 ~/.cache/opencode/models.json。"""
    cp = run_script(harness_probe, sandbox, home, [], timeout=TIMEOUT_S)
    m = re.search(r"LEAKHOME:(\S+)\|exists=(\w+)", cp.stdout)
    assert m, f"台架探针未输出 ~ 解析结果\n--- STDOUT ---\n{cp.stdout}"
    resolved, exists = m.group(1), m.group(2)
    assert str(Path(home).resolve()) in resolved, (
        f"~ 未解析到隔离 HOME（实得 {resolved}），真实 ~/.cache 可能被读取"
    )
    assert resolved != "/home/dm/.cache/opencode/models.json", "子进程指向了真实缓存路径"


# ===========================================================================
# A1 —— SKILL.md 存在 + 五要素 + 触发式 description + §2 真相源引用
# ===========================================================================


def test_a1_skill_md_exists():
    assert SKILL_MD.is_file(), f"SKILL.md 不存在：{SKILL_MD}"


def test_a1_skill_md_has_trigger_description():
    missing = missing_elements(read_skill_text())
    assert "description" not in missing, (
        "SKILL.md 头部缺触发式 description（契约 .agents/skills/README.md 要素 1："
        "必须内联『何时用』/触发条件，路由只发生在摘要可见时）"
    )


@pytest.mark.parametrize("element", sorted(ELEMENT_PATTERNS))
def test_a1_skill_md_has_element(element):
    missing = missing_elements(read_skill_text())
    assert element not in missing, f"SKILL.md 缺要素节标题：{element}（缺失清单：{missing}）"


def test_a1_skill_md_elements_complete():
    text = read_skill_text()
    missing = missing_elements(text)
    assert missing == [], f"SKILL.md 要素不齐全，缺：{missing}"


def test_a1_skill_md_cites_truth_sources():
    """契约 §2：真相源（upstream.rs / admin.rs）文档必须引用，不得凭记忆。"""
    text = read_skill_text()
    for src in (
        "crates/ponyllm-core/src/executor/upstream.rs",
        "crates/ponyllm-server/src/routes/admin.rs",
    ):
        assert src in text, f"SKILL.md 未引用真相源：{src}（契约 §2 要求逐个列出路径）"


VALID_DOC = """---
name: demo
description: |
  何时用：给 ponyllm 加个 zen free 模型时触发。
---

# demo

## 前置
- 网关已启动。

## 真相源
- crates/ponyllm-core/src/executor/upstream.rs

## 程序步骤
1. 干活

## 校准样例
- 正例：xxx → 接入

## 验证与报告
- 跑 pytest
"""


@pytest.mark.parametrize("drop", ["description", *sorted(ELEMENT_PATTERNS)])
def test_a1_negative_missing_element_is_detected(drop):
    doc = VALID_DOC
    if drop == "description":
        doc = doc.replace("何时用：给 ponyllm 加个 zen free 模型时触发。", "随便写写。")
    else:
        head = {
            "truth_sources": "## 真相源",
            "prerequisites": "## 前置",
            "steps": "## 程序步骤",
            "examples": "## 校准样例",
            "verification": "## 验证与报告",
        }[drop]
        doc = doc.replace(head, "## " + {"truth_sources": "背景", "prerequisites": "依赖", "steps": "做法",
                                           "examples": "例子", "verification": "收尾"}[drop])
    assert drop in missing_elements(doc), f"判定器自检失败：去掉 {drop} 的反例未被检出（checker 会空断言自放行）"


def test_a1_negative_baseline_doc_is_complete():
    assert missing_elements(VALID_DOC) == [], "判定器自检失败：合法基准文档被判为缺要素"


# ===========================================================================
# A2 —— 脚本存在 + --help 退出码 0 + 列出全部子命令
# ===========================================================================


def test_a2_script_exists():
    assert SCRIPT.is_file(), f"脚本不存在：{SCRIPT}"


def test_a2_help_exit_code_zero(sandbox, home):
    cp = run_skill(sandbox, home, ["--help"])
    assert_exit_zero(cp, "zen_free.py --help")
    assert cp.stdout.strip(), "--help 输出为空"


@pytest.mark.parametrize("cmd", SUBCOMMANDS)
def test_a2_help_lists_subcommand(cmd):
    cp = subprocess.run(
        [sys.executable, str(SCRIPT), "--help"], capture_output=True, text=True, timeout=TIMEOUT_S,
        env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": os.environ.get("HOME", "/tmp"),
             "LANG": "C.UTF-8"},
        cwd=str(REPO_ROOT),
    )
    missing = missing_subcommands(cp.stdout)
    assert cmd not in missing, (
        f"--help 未列出契约 §4 子命令 {cmd}（缺失清单：{missing}）\n--- STDOUT ---\n{cp.stdout}"
    )


@pytest.mark.parametrize("cmd", SUBCOMMANDS)
def test_a2_declared_subcommand_is_real(cmd):
    """反例：--help 里的子命令名可能只是手写文案（metavar 硬编码），实际 parser 里却没有。

    故除了「--help 列得出来」，还要逐个验证子命令真能被 argparse 受理（`<cmd> --help` 退出码 0）。
    """
    cp = subprocess.run(
        [sys.executable, str(SCRIPT), cmd, "--help"], capture_output=True, text=True, timeout=TIMEOUT_S,
        env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": os.environ.get("HOME", "/tmp"),
             "LANG": "C.UTF-8"},
        cwd=str(REPO_ROOT),
    )
    assert cp.returncode == 0, (
        f"契约 §4 声明的子命令 {cmd} 未被 argparse 受理（`zen_free.py {cmd} --help` 退出码 "
        f"{cp.returncode}）：--help 文案与真实 parser 已漂移\n--- STDERR ---\n{cp.stderr}"
    )


def test_a2_negative_subcommand_checker_detects_missing():
    full = "usage: zen_free.py {list,info,probe,plan,apply,check-gate} [-h]"
    assert missing_subcommands(full) == [], "判定器自检失败：完整子命令列表被判缺项"
    partial = "usage: zen_free.py {list,info} [-h]"
    assert set(missing_subcommands(partial)) == {
        "probe", "plan", "apply", "check-gate"
    }, "判定器自检失败：残缺子命令列表未被检出"


def test_a2_negative_word_boundary_not_satisfied_by_substring():
    """防「在别处出现同名字符串就算命中」的假通过：listing/applyxyz 不算 list/apply。"""
    assert sorted(missing_subcommands("usage: listing applyxyz [-h]")) == sorted(SUBCOMMANDS), (
        "判定器自检失败：子串 listing/applyxyz 被误判为命中子命令"
    )


# ===========================================================================
# A3 —— list --json（离线路径）
# ===========================================================================


def test_a3_list_json_exit_zero(sandbox, home):
    cp = run_skill(sandbox, home, ["list", "--json"], env_key=FAKE_KEY)
    assert_exit_zero(cp, "list --json")
    parse_json_stdout(cp, "list --json")


def test_a3_list_json_structure(sandbox, home):
    cp = run_skill(sandbox, home, ["list", "--json"], env_key=FAKE_KEY)
    payload = parse_json_stdout(cp, "list --json")
    problems = list_payload_problems(payload)
    assert not problems, f"list --json 结构不合法：{problems}\n实际输出：{json.dumps(payload, ensure_ascii=False)[:800]}"


def test_a3_list_json_items_are_free_models(sandbox, home):
    """契约 §4：list 列的是 zen **免费**模型 ⇒ 每项 id 必须以 -free 结尾。"""
    payload = parse_json_stdout(run_skill(sandbox, home, ["list", "--json"], env_key=FAKE_KEY), "list --json")
    non_free = [m["id"] for m in payload["models"] if not m["id"].endswith("-free")]
    assert not non_free, f"list 混入了非 -free zen 模型（契约 §3 I2：非 -free 不受门禁保护）：{non_free}"


def test_a3_list_json_contains_contract_models(sandbox, home):
    payload = parse_json_stdout(run_skill(sandbox, home, ["list", "--json"], env_key=FAKE_KEY), "list --json")
    ids = {m["id"] for m in payload["models"]}
    for want in ("step-5-preview-free", "muse-spark-1.3-contributor-free"):
        assert want in ids, f"list --json 缺契约点名的免费模型 {want}；实得 {sorted(ids)}"


def test_a3_list_human_mode_lists_ids(sandbox, home):
    cp = run_skill(sandbox, home, ["list"], env_key=FAKE_KEY)
    assert_exit_zero(cp, "list（人读模式）")
    assert "step-5-preview-free" in cp.stdout, f"人读模式未列出模型 id\n--- STDOUT ---\n{cp.stdout}"


def test_a3_negative_list_payload_checker_detects_bad_shapes():
    assert list_payload_problems({"models": [{"id": "a-free"}]}) == [], "判定器自检失败：合法结构被判坏"
    for bad in (
        {"models": {}},
        {"items": []},
        {"models": [{"name": "a-free"}]},
        {"models": [{"id": ""}]},
        {"models": [{"id": 42}]},
        {"models": ["a-free"]},
        {"models": []},
    ):
        assert list_payload_problems(bad), f"判定器自检失败：坏结构 {bad} 未被检出"


# ===========================================================================
# A4 —— plan 产出合法 CreateModelPayload + tier 缺省 = "L"
# ===========================================================================


def _plan(sandbox, home, model, *extra, env_key=FAKE_KEY):
    cp = run_skill(sandbox, home, ["plan", model, "--json", *extra], env_key=env_key)
    assert_exit_zero(cp, f"plan {model} {' '.join(extra)} --json")
    payload = resolve_payload(parse_json_stdout(cp, f"plan {model} --json"))
    assert payload is not None, (
        f"plan {model} 输出不是 CreateModelPayload（契约 §4.2：输出必须可直接 json.load）"
        f"\n--- STDOUT ---\n{cp.stdout}"
    )
    return payload


def test_a4_payload_required_fields_present(sandbox, home):
    payload = _plan(sandbox, home, STEP5["id"])
    lacking = sorted(REQUIRED_PAYLOAD_KEYS - set(payload))
    assert not lacking, (
        f"plan payload 缺契约 §4.2 要求的字段：{lacking}；实得 {sorted(payload)}"
    )


def test_a4_default_tier_is_L(sandbox, home):
    """契约 §4.2：tier 用户缺省 ⇒ 默认 L（轻量）。"""
    payload = _plan(sandbox, home, STEP5["id"])
    assert payload.get("tier") == "L", f"tier 缺省时必须为 'L'，实得 {payload.get('tier')!r}"


def test_a4_identity_fields(sandbox, home):
    payload = _plan(sandbox, home, STEP5["id"])
    assert payload["provider"] == "opencode-zen", f"provider 必须固定 opencode-zen，实得 {payload['provider']!r}"
    assert payload["name"] == STEP5["id"], f"name 必须原样取上游 id，实得 {payload['name']!r}"
    assert payload["pricing_mode"] == "uniform", (
        f"pricing_mode 必须小写 uniform（'Uniform' 网关报 400），实得 {payload['pricing_mode']!r}"
    )
    assert payload["protocol"] in {"chat", "responses"}, (
        f"protocol 必须是命中协议 chat/responses，实得 {payload['protocol']!r}"
    )


def test_a4_max_output_conversion(sandbox, home):
    """契约 §4.2 明确样例：limit.output=65536 → '64K'（1024 进制）。"""
    payload = _plan(sandbox, home, STEP5["id"])
    assert payload["max_output"] == "64K", (
        f"max_output 应由 limit.output=65536 换算为 '64K'，实得 {payload['max_output']!r}"
    )


def test_a4_muse_context_and_output_conversion(sandbox, home):
    """契约 §4.2：context=1048576 → '1M'；output=131072 → '128K'（两个无歧义锚点）。"""
    payload = _plan(sandbox, home, MUSE_FREE["id"])
    assert payload["context_window"] == "1M", (
        f"context=1048576 应换算为 '1M'，实得 {payload['context_window']!r}"
    )
    assert payload["max_output"] == "128K", (
        f"max_output=131072 应换算为 '128K'，实得 {payload['max_output']!r}"
    )


def test_a4_context_window_format(sandbox, home):
    payload = _plan(sandbox, home, STEP5["id"])
    value = payload["context_window"]
    assert isinstance(value, str) and SIZE_RE.match(value), (
        f"context_window 必须是 1024 进制字符串（1M / nK），实得 {value!r}"
    )


def test_a4_step5_context_window_anchor(sandbox, home):
    """契约 v1.4.1 §4.3 锚点：1000000 → `977K`（1000000/1024 = 976.5625 → 就近取整 = 977）。

    上游用**十进制**声明上下文（1000000 ≈ 0.95 MiB）；v1.3 的「>=262144 → 256K」硬桶会把 1M 档旗舰
    模型向下低估 4 倍，已废止。新规则只在达到 1024-based 的 1M 整点时写 1M，其余一律就近取整
    （偏差 448 token ≤ 契约上界 512）。

    历史：v1.4 锚点表此格曾误写 976K（截断值），与同表 100000→98K / 5000→5K 的就近取整口径互斥，
    Lead 已于 v1.4.1 订正为 977K —— 整表现已与 §4.3 公式 `round(value/1024)` 自洽。
    """
    payload = _plan(sandbox, home, STEP5["id"])
    assert payload["context_window"] == "977K", (
        f"context=1000000 按契约 §4.3 公式/锚点表应为 '977K'（976.5625 就近取整，偏差 448 token），"
        f"实得 {payload['context_window']!r}"
    )


def test_a4_input_types_subset_of_upstream_with_text(sandbox, home):
    """契约 §4.2：input_types = 上游 modalities.input ∩ 网关 envelope，text 恒含。"""
    payload = _plan(sandbox, home, STEP5["id"])
    upstream = set(STEP5["modalities"]["input"])
    got = payload["input_types"]
    assert isinstance(got, list) and got, f"input_types 必须是非空数组，实得 {got!r}"
    assert all(isinstance(t, str) and t.strip() for t in got), f"input_types 含非法元素：{got!r}"
    assert set(got) <= upstream, f"input_types 超出上游 modalities.input（必须是交集）：{sorted(set(got) - upstream)}"
    assert "text" in got, f"input_types 必须恒含 text，实得 {got!r}"


def test_a4_output_types_is_text(sandbox, home):
    payload = _plan(sandbox, home, STEP5["id"])
    assert payload["output_types"] == ["text"], (
        f"文本模型 output_types 必须恒为 ['text']，实得 {payload['output_types']!r}"
    )


def test_a4_omits_model_level_base_url_and_proxy(sandbox, home):
    """契约 §3 I3 / §4.2：模型级 base_url/proxy 一律不写（填了触发 400 egress_blocked）。"""
    payload = _plan(sandbox, home, STEP5["id"])
    for forbidden in ("base_url", "proxy"):
        assert not has_key(payload, forbidden), (
            f"payload 出现模型级 {forbidden}（契约 §3 I3：模型级一律不填，会触发 egress_blocked）"
        )


def test_a4_reasoning_model_has_normalized_thinking(sandbox, home):
    """契约 §4.2：reasoning_options 声明档位 ⇒ 写 thinking_* 并按 from_str_loose 归一。"""
    payload = _plan(sandbox, home, MUSE_FREE["id"])
    for key in ("thinking_default", "thinking_max"):
        assert key in payload, f"推理模型 payload 缺 {key}（上游声明了 reasoning_options 档位）"
        assert str(payload[key]).lower() in THINKING_VALUES, (
            f"{key}={payload[key]!r} 未归一到 low/medium/high/max"
        )


def test_a4_non_reasoning_model_omits_thinking(sandbox, home):
    """契约 §4.2：上游未声明 reasoning_options / 非推理模型 ⇒ 不写 thinking_* 字段。"""
    payload = _plan(sandbox, home, LITE_FREE["id"])
    for key in ("thinking_default", "thinking_max"):
        assert not has_key(payload, key), (
            f"非推理模型 payload 不应出现 {key}（契约 §4.2：上游未声明则不写该字段）"
        )


def test_a4_size_conversion_tail_branch(sandbox, home):
    """契约 §4.2 换算表的尾分支「其余按 K 向上取整」（1M/256K/128K 三桶盖不到，必须单独打靶）。"""
    payload = _plan(sandbox, home, TAIL_FREE["id"])
    assert payload["context_window"] == "98K", (
        f"context=100000 应按 1024 进制向上取整为 '98K'，实得 {payload['context_window']!r}"
    )
    assert payload["max_output"] == "5K", (
        f"output=5000 应向上取整为 '5K'，实得 {payload['max_output']!r}"
    )


def test_a4_negative_payload_resolver_and_keys():
    good = {k: None for k in REQUIRED_PAYLOAD_KEYS}
    assert resolve_payload(good) is not None, "判定器自检失败：裸 payload 未被识别"
    wrapped = {"payload": good, "source": "cache"}
    assert resolve_payload(wrapped) is None, (
        "判定器自检失败：套壳 payload 未被拒绝（契约 v1.4 §4.4-2：只出裸 payload 本体）"
    )
    for bad in ({}, {"provider": "opencode-zen"}, {"name": "x-free"}, [], "x"):
        assert resolve_payload(bad) is None, f"判定器自检失败：坏结构 {bad} 被误认成 payload"
    tree = {"payload": {"thinking_default": "Max"}}
    keys = collect_keys(tree)
    assert "thinking_default" in keys and "thinking_max" not in keys, (
        f"判定器自检失败：键收集不正确，实得 {keys}"
    )
    assert has_key(tree, "thinking_default") and not has_key(tree, "thinking_max"), (
        "判定器自检失败：has_key 嵌套查找不正确"
    )


def test_a4_plan_stdout_is_bare_payload(sandbox, home):
    """契约 v1.4 §4.4-2：plan --json 的 stdout **只出裸 payload 本体**，顶层平铺、无包装键。"""
    cp = run_skill(sandbox, home, ["plan", STEP5["id"], "--json"], env_key=FAKE_KEY)
    assert_exit_zero(cp, "plan --json")
    obj = parse_json_stdout(cp, "plan --json")
    assert isinstance(obj, dict), f"plan --json 顶层必须是对象（裸 payload），实得 {type(obj).__name__}"
    wrapped = sorted(set(obj) & WRAPPER_KEYS)
    assert not wrapped, (
        f"plan --json 顶层出现包装键 {wrapped}（契约 §4.4-2：顶层平铺 payload 本体，"
        f"无 source/upstream 等包装）；实得顶层键 {sorted(obj)}"
    )
    assert {"provider", "name"} <= set(obj), (
        f"plan --json 顶层缺身份字段（须顶层平铺 payload），实得 {sorted(obj)}"
    )


def test_a4_negative_size_and_tier_checkers_are_not_vacuous():
    assert SIZE_RE.match("1M") and SIZE_RE.match("128K"), "判定器自检失败：合法尺寸被判非法"
    for bad in ("", "1m", "128k", "1024", "1MB", None, "auto[1m]"):
        assert not SIZE_RE.match(str(bad)), f"判定器自检失败：非法尺寸 {bad!r} 被放行"


# ===========================================================================
# A5 —— tier 显式接受 / 非法拒绝（负例）
# ===========================================================================


@pytest.mark.parametrize("tier", ["F", "S"])
def test_a5_explicit_tier_accepted(sandbox, home, tier):
    cp = run_skill(sandbox, home, ["plan", STEP5["id"], "--json", "--tier", tier], env_key=FAKE_KEY)
    assert_exit_zero(cp, f"plan --tier {tier}")
    assert _tier_accepted(cp.returncode, cp.stdout, cp.stderr), (
        f"--tier {tier} 应被接受（契约 §6 A5）\n--- STDOUT ---\n{cp.stdout}"
    )
    payload = resolve_payload(parse_json_stdout(cp, f"plan --tier {tier} --json"))
    assert payload is not None and payload.get("tier") == tier, (
        f"--tier {tier} 应原样落到 payload.tier，实得 {(payload or {}).get('tier')!r}"
    )


@pytest.mark.parametrize("tier", ["xx", "leader", "9", "F/S"])
def test_a5_invalid_tier_rejected_with_exit_2(sandbox, home, tier):
    """契约 §6 A5：F/S/L 之外必须拒绝，退出码 2。"""
    cp = run_skill(sandbox, home, ["plan", STEP5["id"], "--json", "--tier", tier], env_key=FAKE_KEY)
    assert_exit(cp, 2, f"plan --tier {tier}（非法 tier 必须退出码 2）")
    assert not _tier_accepted(cp.returncode, cp.stdout, cp.stderr), (
        f"非法 tier {tier} 被判为已接受\n--- STDOUT ---\n{cp.stdout}"
    )
    assert cp.stderr.strip(), f"非法 tier 必须把原因打到 stderr（契约 §4：禁止吞异常）"


def test_a5_negative_tier_acceptance_checker():
    ok = json.dumps({"provider": "opencode-zen", "name": "x-free", "tier": "F"})
    bad_tier = json.dumps({"provider": "opencode-zen", "name": "x-free", "tier": "xx"})
    no_tier = json.dumps({"provider": "opencode-zen", "name": "x-free"})
    assert _tier_accepted(0, ok, ""), "判定器自检失败：合法 F 未被接受"
    assert not _tier_accepted(2, "", "bad tier"), "判定器自检失败：退出码 2 未被判拒绝"
    assert not _tier_accepted(0, bad_tier, ""), "判定器自检失败：tier=xx 被放行"
    assert not _tier_accepted(0, no_tier, ""), "判定器自检失败：缺 tier 字段被放行"
    assert not _tier_accepted(0, "not json", ""), "判定器自检失败：非 JSON 输出被放行"


def _tier_accepted(returncode: int, stdout: str, stderr: str) -> bool:
    if returncode != 0:
        return False
    try:
        payload = resolve_payload(json.loads(stdout.strip()))
    except json.JSONDecodeError:
        return False
    return bool(payload) and str(payload.get("tier", "")).strip().upper() in {"F", "S", "L"}


# ===========================================================================
# A6 —— check-gate 正反成对（-free 全 PASS / 非 -free 的 I2 必须 FAIL）
# ===========================================================================


def test_a6_free_model_all_three_pass(sandbox, home):
    cp = run_skill(sandbox, home, ["check-gate", STEP5["id"], "--json"], env_key=FAKE_KEY)
    assert_exit_zero(cp, f"check-gate {STEP5['id']}（三项全 PASS 属成功）")
    verdicts = require_gate_verdicts(cp, STEP5["id"])
    failed = sorted(g for g, v in verdicts.items() if v != "PASS")
    assert not failed, f"-free 结尾模型三项应全 PASS，实际 {verdicts}（契约 §3）"


def test_a6_non_free_model_i2_must_fail(sandbox, home):
    """契约 §3 I2 + v1.4 §4.4-1：非 -free 后缀的 zen 模型必须 FAIL，且退出码 1。"""
    cp = run_skill(sandbox, home, ["check-gate", MUSE_PAID["id"], "--json"], env_key=FAKE_KEY)
    assert_exit(cp, 1, f"check-gate {MUSE_PAID['id']}（任一 FAIL ⇒ 退出码 1）")
    verdicts = require_gate_verdicts(cp, MUSE_PAID["id"])
    assert verdicts["I2"] == "FAIL", (
        f"非 -free 模型 {MUSE_PAID['id']} 的 I2（后缀不变式）必须 FAIL，实得 {verdicts}"
        f"\n--- STDOUT ---\n{cp.stdout}"
    )


def test_a6_non_free_model_other_gates_still_pass(sandbox, home):
    """反例的对照组：I2 FAIL 不能是「一律判 FAIL」的假阳性 —— I1/I3 与后缀无关，应仍 PASS。"""
    cp = run_skill(sandbox, home, ["check-gate", MUSE_PAID["id"], "--json"], env_key=FAKE_KEY)
    verdicts = require_gate_verdicts(cp, MUSE_PAID["id"])
    assert verdicts["I1"] == "PASS" and verdicts["I3"] == "PASS", (
        f"I1（provider 前缀 opencode）/I3（不填模型级 base_url、proxy）与后缀无关，应 PASS，实得 {verdicts}"
        f"\n--- STDOUT ---\n{cp.stdout}"
    )


def test_a6_human_mode_gates_are_machine_readable(sandbox, home):
    """人读表格（无 --json）同样必须给出三条可机读判定，且退出码口径与 --json 一致（§4.4-1）。"""
    cp = run_skill(sandbox, home, ["check-gate", MUSE_PAID["id"]], env_key=FAKE_KEY)
    assert_exit(cp, 1, f"check-gate {MUSE_PAID['id']}（人读模式：任一 FAIL ⇒ 退出码 1）")
    verdicts = require_gate_verdicts(cp, f"{MUSE_PAID['id']}（人读模式）")
    assert verdicts["I2"] == "FAIL", f"人读模式下非 -free 模型 I2 必须 FAIL，实得 {verdicts}\n{cp.stdout}"


def test_a6_free_model_human_mode_exit_zero(sandbox, home):
    """正反成对的另一半：人读模式下三项全 PASS ⇒ 退出码 0。"""
    cp = run_skill(sandbox, home, ["check-gate", STEP5["id"]], env_key=FAKE_KEY)
    assert_exit_zero(cp, f"check-gate {STEP5['id']}（人读模式：三条全 PASS ⇒ 退出码 0）")
    verdicts = require_gate_verdicts(cp, f"{STEP5['id']}（人读模式）")
    failed = sorted(g for g, v in verdicts.items() if v != "PASS")
    assert not failed, f"人读模式下 -free 模型三项应全 PASS，实得 {verdicts}\n{cp.stdout}"


def test_a6_negative_gate_extractor_detects_failures():
    good = {"gates": [{"id": "I1", "status": "PASS"}, {"id": "I2", "status": "PASS"}, {"id": "I3", "status": "PASS"}]}
    bad = {"gates": [{"id": "I1", "status": "PASS"}, {"id": "I2", "status": "FAIL"}, {"id": "I3", "status": "PASS"}]}
    assert extract_gate_verdicts(json.dumps(good)) == {"I1": "PASS", "I2": "PASS", "I3": "PASS"}, (
        "判定器自检失败：全 PASS JSON 未被正确抽取"
    )
    assert extract_gate_verdicts(json.dumps(bad))["I2"] == "FAIL", "判定器自检失败：FAIL 未被抽出"
    text = "I1 作用域：PASS\nI2 后缀：FAIL\nI3 配置面：PASS\n"
    assert extract_gate_verdicts(text)["I2"] == "FAIL", "判定器自检失败：人读表格的 FAIL 未被抽出"
    nested = {"checks": {"suffix": {"passed": False}, "provider": {"passed": True}, "egress": {"passed": True}}}
    assert extract_gate_verdicts(json.dumps(nested))["I2"] == "FAIL", "判定器自检失败：布尔型判定未被抽出"
    assert extract_gate_verdicts("毫无判定信息") == {}, "判定器自检失败：空输出被误判成有判定"


# ===========================================================================
# A7 —— 输出/日志不出现 api key 原文
# ===========================================================================


@pytest.mark.parametrize(
    "args",
    [
        ["list", "--json"],
        ["info", STEP5["id"], "--json"],
        ["plan", STEP5["id"], "--json"],
        ["probe", STEP5["id"], "--json"],
        ["check-gate", STEP5["id"], "--json"],
    ],
    ids=["list", "info", "plan", "probe", "check-gate"],
)
def test_a7_fake_key_absent_from_json_run(sandbox, home, args):
    cp = run_skill(sandbox, home, args, env_key=FAKE_KEY)
    hits = key_leak_hits(cp.stdout + cp.stderr)
    assert not hits, f"{' '.join(args)} 泄漏了 {'/'.join(hits)}（契约 §4 红线）\nSTDOUT:\n{cp.stdout}\nSTDERR:\n{cp.stderr}"


def test_a7_fake_key_absent_from_human_mode(sandbox, home):
    cp = run_skill(sandbox, home, ["plan", STEP5["id"]], env_key=FAKE_KEY)
    hits = key_leak_hits(cp.stdout + cp.stderr)
    assert not hits, f"人读模式泄漏了 {'/'.join(hits)}\nSTDOUT:\n{cp.stdout}"


def test_a7_fake_key_absent_on_parameter_error_path(sandbox, home):
    """错误路径最容易顺手 echo key —— 非法 tier 触发退出码 2 时仍不得泄漏。"""
    cp = run_skill(sandbox, home, ["plan", STEP5["id"], "--json", "--tier", "xx"], env_key=FAKE_KEY)
    assert_exit(cp, 2, "plan --tier xx（错误路径）")
    hits = key_leak_hits(cp.stdout + cp.stderr)
    assert not hits, f"错误路径泄漏了 {'/'.join(hits)}\nSTDERR:\n{cp.stderr}"


def test_a7_fake_key_absent_on_unknown_model_path(sandbox, home):
    cp = run_skill(sandbox, home, ["plan", "no-such-model-xyz", "--json"], env_key=FAKE_KEY)
    assert_exit_nonzero(cp, "plan no-such-model-xyz")
    hits = key_leak_hits(cp.stdout + cp.stderr)
    assert not hits, f"未知模型错误路径泄漏了 {'/'.join(hits)}\nSTDOUT:\n{cp.stdout}\nSTDERR:\n{cp.stderr}"


def test_a7_negative_key_leak_detector():
    assert key_leak_hits("Authorization: Bearer " + FAKE_KEY), "判定器自检失败：key 原文未被检出"
    assert key_leak_hits("token=" + FAKE_KEY_PREFIX + "***"), "判定器自检失败：key 前缀未被检出"
    assert not key_leak_hits("masked: sk-ze****"), "判定器自检失败：正常脱敏输出被误判为泄漏"


def test_a7_flag_key_option_is_supported(sandbox, home):
    """契约 v1.4 §4.4-3：`--key` 挂在子命令**前或后等价**，两种写法都不得泄漏 key。"""
    before = run_skill(sandbox, home, ["--key", FAKE_KEY, "plan", STEP5["id"], "--json"])
    after = run_skill(sandbox, home, ["plan", STEP5["id"], "--key", FAKE_KEY, "--json"])
    assert_exit_zero(before, "zen_free.py --key <KEY> plan ... --json（--key 前挂）")
    assert_exit_zero(after, "zen_free.py plan ... --key <KEY> --json（--key 后挂）")
    # 等价性：两种写法的 payload 必须一致（仅靠 --key 提供凭据，结果不得因挂点而异）
    p_before = resolve_payload(parse_json_stdout(before, "--key 前挂"))
    p_after = resolve_payload(parse_json_stdout(after, "--key 后挂"))
    assert p_before == p_after, (
        f"--key 前挂/后挂不等价：{json.dumps(p_before, ensure_ascii=False, sort_keys=True)}\n"
        f"vs {json.dumps(p_after, ensure_ascii=False, sort_keys=True)}"
    )
    # 反例防回归：两条路径都不得把 key 原文/前缀打进 stdout 或 stderr
    for tag, cp in (("子命令前", before), ("子命令后", after)):
        hits = key_leak_hits(cp.stdout + cp.stderr)
        assert not hits, f"--key 挂点在{tag}时泄漏了 {'/'.join(hits)}\nSTDOUT:\n{cp.stdout}\nSTDERR:\n{cp.stderr}"


def test_a7_key_from_opencode_config_file(sandbox, home):
    """契约 §4：第三个 key 来源 ~/.config/opencode/opencode.json 的 provider.opencode.options.apiKey。"""
    cfg = Path(home) / ".config" / "opencode"
    cfg.mkdir(parents=True, exist_ok=True)
    (cfg / "opencode.json").write_text(
        json.dumps({"provider": {"opencode": {"options": {"apiKey": FAKE_KEY}}}}, ensure_ascii=False),
        encoding="utf-8",
    )
    cp = run_skill(sandbox, home, ["plan", STEP5["id"], "--json"])
    assert_exit_zero(cp, "plan（key 来自 opencode.json）")
    hits = key_leak_hits(cp.stdout + cp.stderr)
    assert not hits, f"配置文件 key 路径泄漏了 {'/'.join(hits)}\nSTDOUT:\n{cp.stdout}"


# ===========================================================================
# A8 —— 缺 key 退出码 2 + stderr 有原因 + 无 traceback
# ===========================================================================


@pytest.mark.parametrize("args", [
    ["info", STEP5["id"], "--json"],
    ["plan", STEP5["id"], "--json"],
    ["probe", STEP5["id"], "--json"],
], ids=["info", "plan", "probe"])
def test_a8_missing_key_exit_code_2(sandbox, home, args):
    cp = run_skill(sandbox, home, args)  # 无 --key、无 OPENCODE_ZEN_KEY、隔离 HOME 里也没有配置文件
    assert_exit(cp, 2, f"{' '.join(args)} 缺 key（契约 §4：取不到 → 退出码 2）")


@pytest.mark.parametrize("args", [
    ["info", STEP5["id"], "--json"],
    ["plan", STEP5["id"], "--json"],
    ["probe", STEP5["id"], "--json"],
], ids=["info", "plan", "probe"])
def test_a8_missing_key_stderr_explains(sandbox, home, args):
    cp = run_skill(sandbox, home, args)
    assert_exit(cp, 2, f"{' '.join(args)} 缺 key")
    assert cp.stderr.strip(), f"{' '.join(args)} 缺 key 时 stderr 必须有原因（契约 §4：禁止吞异常）"
    assert "Traceback (most recent call last)" not in cp.stderr, (
        f"{' '.join(args)} 缺 key 时不应抛裸 traceback（契约 §4：必须打印可读原因）\n{cp.stderr}"
    )


# ===========================================================================
# A9 —— 源码静态检查
# ===========================================================================


def test_a9_no_bare_except():
    src = read_script_source()
    bare = [(ln, why) for ln, why in except_violations(src) if "裸 except" in why]
    assert not bare, f"源码含裸 except（契约 §6 A9）：{bare}"


def test_a9_no_swallowing_except_handler():
    src = read_script_source()
    swallow = [(ln, why) for ln, why in except_violations(src) if "裸 except" not in why]
    assert not swallow, f"源码含吞异常 handler（契约 §4 禁止吞异常 / §6 A9）：{swallow}"


@pytest.mark.parametrize(
    "bad_src",
    [
        "try:\n    pass\nexcept:\n    pass\n",
        "try:\n    pass\nexcept Exception:\n    pass\n",
        "try:\n    pass\nexcept (KeyError, ValueError):\n    ...\n",
        "try:\n    pass\nexcept BaseException:\n    pass\n",
    ],
    ids=["bare", "exception-pass", "tuple-ellipsis", "baseexception-pass"],
)
def test_a9_negative_except_checker_detects_violations(bad_src):
    assert except_violations(bad_src), f"判定器自检失败：违规源码未被检出 —— {bad_src!r}"


def test_a9_negative_except_checker_accepts_legit_handlers():
    ok = "try:\n    pass\nexcept ValueError as e:\n    print(e)\n    raise SystemExit(2)\n"
    assert except_violations(ok) == [], "判定器自检失败：合法 handler 被误判为吞异常"


def test_a9_source_uses_stdlib_only():
    """契约 §4/§1：脚本纯标准库（仅 urllib/json/argparse 一类）。"""
    src = read_script_source()
    bad = non_stdlib_imports(src)
    assert not bad, f"脚本引入了非标准库依赖（契约 §4：只用标准库）：{sorted(set(bad))}"


def test_a9_negative_stdlib_checker_detects_third_party():
    assert non_stdlib_imports("import urllib.request\nimport json\n") == [], "判定器自检失败：标准库 import 被误判"
    assert non_stdlib_imports("import requests\n") == ["requests"], "判定器自检失败：第三方 import 未被检出"
    assert non_stdlib_imports("from httpx import Client\n") == ["httpx"], "判定器自检失败：第三方 from-import 未被检出"


# ===========================================================================
# 附加：契约 §4.1 / §4.3 probe 输出形态（离线构造，非 §6 矩阵）
# ===========================================================================


def _probe(sandbox, home, model, *extra):
    return run_skill(sandbox, home, ["probe", model, "--json", *extra], env_key=FAKE_KEY)


def test_x41_probe_success_reports_required_fields(sandbox, home):
    cp = _probe(sandbox, home, STEP5["id"])
    assert_exit_zero(cp, f"probe {STEP5['id']}（夹具给 200 + 非空事件流 ⇒ 判定为可用）")
    payload = parse_json_stdout(cp, "probe --json")
    for key in ("ok", "protocol", "http_status", "latency_ms", "error_type", "gate_failure",
                "transport", "attempts", "output_text", "usage"):
        assert has_key(payload, key), f"probe --json 缺契约 §4.1 要求的字段 {key}；实得 {sorted(collect_keys(payload))}"


def test_x41_probe_success_field_values(sandbox, home):
    payload = parse_json_stdout(_probe(sandbox, home, STEP5["id"]), "probe --json")
    assert find_values(payload, "ok"), "契约 §4.1：HTTP 200 且取到非空文本 ⇒ ok 必须为真"
    assert 200 in [v for v in find_values(payload, "http_status")], (
        f"http_status 应为 200，实得 {find_values(payload, 'http_status')}"
    )
    assert find_values(payload, "gate_failure"), "契约 §4.1：200 成功路径 gate_failure 必须为假（布尔 false 仍是「该字段存在」）"
    assert "chat" in [v for v in find_values(payload, "protocol")], (
        f"夹具中 /chat/completions 命中，protocol 应为 chat，实得 {find_values(payload, 'protocol')}"
    )
    assert set(find_values(payload, "transport")) & {"direct", "proxy"}, (
        f"transport 必须标出 direct/proxy，实得 {find_values(payload, 'transport')}"
    )
    texts = [t for t in find_values(payload, "output_text") if isinstance(t, str)]
    assert any(t.strip() for t in texts), f"output_text 摘要应含夹具文本 bunny-ok，实得 {texts}"
    lat = find_values(payload, "latency_ms")
    assert any(isinstance(v, (int, float)) and v >= 0 for v in lat), f"latency_ms 应为非负数，实得 {lat}"
    assert find_values(payload, "attempts"), "契约 §4.1：attempts 逐次 transport+status 摘要不能为空"


@pytest.mark.parametrize(
    "model,expected",
    [
        (ERR_GATE["id"], "gate_failure"),
        (ERR_GEO["id"], "geo_blocked"),
        (ERR_USAGE["id"], "usage_limit"),
        (ERR_PROTO["id"], "protocol_unsupported"),
        (ERR_404["id"], "not_found"),
        (ERR_502["id"], "transport_error"),
    ],
)
def test_x43_probe_error_type_classification(sandbox, home, model, expected):
    """契约 §4.3：7 类 error_type 由 HTTP 状态 + body 信号机械判定（离线构造，不联网）。"""
    cp = _probe(sandbox, home, model, "--protocol", "chat", "--via", "direct")
    assert_exit(cp, 1, f"probe {model}（探活失败 ⇒ 退出码 1）")
    payload = parse_json_stdout(cp, "probe --json")
    got = [str(v).strip().lower() for v in find_values(payload, "error_type") if v]
    assert expected in got, (
        f"{model} 的 error_type 应为 {expected}（契约 §4.3），实得 {got}"
        f"\n--- STDOUT ---\n{cp.stdout}"
    )
    bad = [v for v in got if v not in ERROR_TYPES]
    assert not bad, f"error_type 超出契约 §4.3 枚举：{bad}"


def test_x43_geo_blocked_must_switch_transport(sandbox, home):
    """契约 §4.3：geo_blocked 不算模型不可用，--via auto 必须切代理重试一次。"""
    payload = parse_json_stdout(_probe(sandbox, home, ERR_GEO["id"], "--protocol", "chat"), "probe --json")
    assert "geo_blocked" in [str(v).lower() for v in find_values(payload, "error_type") if v], (
        f"geo_blocked 分类缺失：{find_values(payload, 'error_type')}"
    )
    attempts = [a for a in find_values(payload, "attempts") if isinstance(a, list)]
    assert attempts and attempts[0], f"attempts 应记录逐次尝试，实得 {find_values(payload, 'attempts')}"
    transports = set()
    for item in attempts[0]:
        if isinstance(item, dict):
            transports |= {str(v).lower() for v in item.values() if isinstance(v, str)}
    assert {"direct", "proxy"} <= transports, (
        f"--via auto 遇 geo_blocked 必须切本机代理重试（契约 §4.3），attempts 实得 {attempts[0]}"
    )


def test_x43_usage_limit_distinguishable_from_gate_failure(sandbox, home):
    """契约 §4.3：额度耗尽（429）必须与门禁未过（403 FreeTierError）区分开。"""
    usage = parse_json_stdout(_probe(sandbox, home, ERR_USAGE["id"], "--protocol", "chat", "--via", "direct"), "probe --json")
    gate = parse_json_stdout(_probe(sandbox, home, ERR_GATE["id"], "--protocol", "chat", "--via", "direct"), "probe --json")
    u = {str(v).lower() for v in find_values(usage, "error_type") if v}
    g = {str(v).lower() for v in find_values(gate, "error_type") if v}
    assert "usage_limit" in u and "gate_failure" in g, f"两类错误未区分：usage={u} gate={g}"
    assert "gate_failure" not in u, "429 额度耗尽被误判为门禁未过（契约 §4.3）"


def test_x43_negative_error_type_detector():
    assert ERROR_TYPES >= {"geo_blocked", "gate_failure", "usage_limit", "protocol_unsupported",
                           "not_found", "transport_error", "upstream_unavailable"}, (
        "判定器自检失败：枚举常量缺项"
    )
    for junk in ("FreeTierError", "", "error", "403"):
        assert junk not in ERROR_TYPES, f"判定器自检失败：非法 error_type {junk!r} 被收进枚举"