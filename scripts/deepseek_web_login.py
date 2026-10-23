#!/usr/bin/env python3
"""获取 chat.deepseek.com web token（短信验证码登录，人类在环）。

机制（REA: `.dev-team/contracts/wave-13-deepseek-web-token-rea-evidence.md`）：
1. 用户先在浏览器/手机端完成「发送短信验证码」（人机验证 + 收短信，本工具不代发短信）；
2. 本工具取 guest PoW 挑战并诚实求解（DeepSeekHashV1，23 轮 Keccak）；
3. 用生产公钥加密登录请求体（AES-CBC + RSA-OAEP/SHA-256，`x-request-encryption-key`）；
4. `POST /api/v0/users/login_by_mobile_sms` 换取 token（`data.biz_data.user.token`）；
5. 可选 `--verify` 用 token 调 `create_chat_session` 做真实链路校验。

合规边界：不实现验证码/WAF/设备指纹绕过，不自动发送短信，不做批量登录；
token 仅打印/落盘到调用者指定位置，不写入日志。

依赖：Python 3.10+、`cryptography`（`pip install cryptography`）。
用法：
  python3 scripts/deepseek_web_login.py --mobile 15538643833 --sms-code 123456 --verify
  python3 scripts/deepseek_web_login.py --mobile 15538643833 --device-id <浏览器 device-id> --out token.txt
"""

from __future__ import annotations

import argparse
import base64
import getpass
import json
import os
import secrets
import struct
import sys
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

DEFAULT_BASE_URL = "https://chat.deepseek.com"
DEFAULT_CLIENT_VERSION = "2.5.0"
DEFAULT_DEVICE_STATE = Path("~/.local/state/ponyllm/deepseek-web-device-id").expanduser()

# 生产环境公钥（app="chat", environment="production"；REA §2.2）。
PUBLIC_KEY_B64 = (
    "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0RLAhx1TOJhAQBHS1LHXlnkf"
    "Og86O+uhsmKWNNkmugVrRqO6cl13SnjBs2otRYZGJd50xpBG4We2i6/oCJCwxA/49YZf"
    "IwaeDmTx+WBIN7qrwhwP7vGCi6Osesmn/Ddgdye6mifYkRpRMonQjGY6VPaUMTw1jjO"
    "AIKph5YZn0bDVmGLvs5eH6CoQYHM5/Z1Nfhj36VL28eZi3elJHoz/L6Tzi2wY1rCYqY"
    "Hfepq3Ysy8y5Y5BeFJXN60RYLlbs+MRC+2kyJqcfApUnn1O/W2om9dhxc6Rc5dqas1Y"
    "B4X4CWCttrKk/suhpIx9zpQY9kqtotbe5zweYaKv0rCVeqx8wIDAQAB"
)

LOGIN_PATH = "/api/v0/users/login_by_mobile_sms"
CHALLENGE_PATH = "/api/v0/users/create_guest_challenge"
SESSION_PATH = "/api/v0/chat_session/create"

# LOGIN_BY_MOBILE_SMS_ERROR_CODE（bundle enum，REA §1）
BIZ_OK = 0
BIZ_EXISTING_ACCOUNT = 1
BIZ_BANNED = 3
BIZ_MAINLAND_ONLY = 6
BIZ_SMS_EXPIRED = 7
BIZ_SMS_MISMATCH = 8
BIZ_TOO_MANY_ATTEMPTS = 9
BIZ_RISK_DEVICE = 11
BIZ_DISABLED = 98

EXIT_OK = 0
EXIT_GENERIC = 1
EXIT_SMS_INVALID = 2
EXIT_BANNED = 3
EXIT_MAINLAND_ONLY = 6
EXIT_TOO_MANY = 9
EXIT_RISK_DEVICE = 11
EXIT_DISABLED = 98
EXIT_AUTH_INVALID = 4
EXIT_IP_RESTRICTED = 5
EXIT_PROTOCOL = 10
EXIT_NETWORK = 12
EXIT_POW = 13
EXIT_MUTED = 14

# Keccak-f[1600] constants; the official PoW worker applies iota rounds 1..=23.
RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
]
RHO = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61],
       [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
MASK = (1 << 64) - 1


class LoginError(Exception):
    """Login failure with an explicit exit code and upstream biz_code."""

    def __init__(self, message: str, exit_code: int = EXIT_GENERIC, biz_code: int | None = None):
        super().__init__(message)
        self.exit_code = exit_code
        self.biz_code = biz_code


@dataclass(frozen=True)
class LoginResult:
    token: str
    user_id: str
    biz_code: int
    biz_msg: str


def _rotl(value: int, shift: int) -> int:
    return ((value << shift) | (value >> (64 - shift))) & MASK if shift else value


def _keccak_f1600(state: list[int], start_round: int, rounds: int) -> None:
    for rnd in range(start_round, start_round + rounds):
        c = [state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20] for x in range(5)]
        for x in range(5):
            d = c[(x + 4) % 5] ^ _rotl(c[(x + 1) % 5], 1)
            for y in range(5):
                state[x + 5 * y] ^= d
        b = [0] * 25
        for x in range(5):
            for y in range(5):
                b[y + 5 * ((2 * x + 3 * y) % 5)] = _rotl(state[x + 5 * y], RHO[x][y])
        for x in range(5):
            for y in range(5):
                state[x + 5 * y] = b[x + 5 * y] ^ (
                    (~b[(x + 1) % 5 + 5 * y]) & b[(x + 2) % 5 + 5 * y] & MASK
                )
        state[0] ^= RC[rnd]


def pow_hash23(message: bytes) -> str:
    """DeepSeekHashV1: SHA3-256 sponge parameters with 23 Keccak rounds."""
    rate = 136
    pad_len = (rate - ((len(message) + 1) % rate)) % rate
    buf = bytearray(message)
    buf.append(0x06)
    buf.extend(b"\x00" * pad_len)
    buf[-1] |= 0x80
    state = [0] * 25
    for offset in range(0, len(buf), rate):
        chunk = buf[offset:offset + rate]
        for i in range(rate // 8):
            state[i] ^= struct.unpack_from("<Q", chunk, i * 8)[0]
        _keccak_f1600(state, 1, 23)
    return b"".join(struct.pack("<Q", state[i]) for i in range(4)).hex()


def solve_pow(challenge: dict) -> tuple[int, int]:
    """Return (answer, elapsed_ms); prefix is salt_expireAt_ (worker-verified)."""
    started = time.monotonic()
    prefix = f"{challenge['salt']}_{challenge['expire_at']}_"
    difficulty = int(challenge["difficulty"])
    target = challenge["challenge"]
    for answer in range(difficulty):
        if pow_hash23((prefix + str(answer)).encode()) == target:
            return answer, int((time.monotonic() - started) * 1000)
    raise LoginError(
        f"PoW 无解（difficulty={difficulty}，challenge 可能已过期或算法漂移）",
        exit_code=EXIT_POW,
    )


def encrypt_body(obj: dict, public_key_b64: str = PUBLIC_KEY_B64) -> tuple[bytes, str]:
    """AES-CBC(PKCS#7) body + RSA-OAEP(SHA-256) wrapped key/iv (REA §2.2)."""
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import padding
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

    key, iv = secrets.token_bytes(16), secrets.token_bytes(16)
    plain = json.dumps(obj, separators=(",", ":")).encode()
    pad = 16 - len(plain) % 16
    plain += bytes([pad]) * pad
    encryptor = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
    body = encryptor.update(plain) + encryptor.finalize()
    public_key = serialization.load_der_public_key(base64.b64decode(public_key_b64))
    wrapped = public_key.encrypt(
        key + iv,
        padding.OAEP(mgf=padding.MGF1(hashes.SHA256()), algorithm=hashes.SHA256(), label=None),
    )
    return body, base64.b64encode(wrapped).decode()


def parse_login_payload(payload: dict) -> LoginResult:
    """Parse the `{code,data:{biz_code,biz_msg,biz_data:{user}}}` envelope."""
    try:
        data = payload["data"]
        biz_code = int(data["biz_code"])
        biz_msg = str(data.get("biz_msg") or "")
    except (KeyError, TypeError, ValueError) as exc:
        raise LoginError(f"登录响应结构非法: {payload!r}"[:300], exit_code=EXIT_PROTOCOL) from exc

    if biz_code in (BIZ_OK, BIZ_EXISTING_ACCOUNT):
        user = (data.get("biz_data") or {}).get("user") or {}
        token = user.get("token")
        user_id = user.get("id")
        if not token or not user_id:
            raise LoginError("登录成功但响应缺少 user.token/user.id", exit_code=EXIT_PROTOCOL, biz_code=biz_code)
        return LoginResult(token=str(token), user_id=str(user_id), biz_code=biz_code, biz_msg=biz_msg)

    mapping = {
        BIZ_SMS_EXPIRED: ("短信验证码已过期，请重新获取", EXIT_SMS_INVALID),
        BIZ_SMS_MISMATCH: ("短信验证码错误", EXIT_SMS_INVALID),
        BIZ_TOO_MANY_ATTEMPTS: ("验证码尝试次数过多，稍后再试", EXIT_TOO_MANY),
        BIZ_RISK_DEVICE: ("风控拦截（RISK_DEVICE_DETECTED）；可尝试 --device-id 使用浏览器 device_id", EXIT_RISK_DEVICE),
        BIZ_BANNED: ("账号被停用", EXIT_BANNED),
        BIZ_MAINLAND_ONLY: ("仅支持中国大陆注册", EXIT_MAINLAND_ONLY),
        BIZ_DISABLED: ("该渠道暂时停用", EXIT_DISABLED),
    }
    message, exit_code = mapping.get(biz_code, (f"登录失败 biz_code={biz_code} biz_msg={biz_msg}", EXIT_GENERIC))
    raise LoginError(message, exit_code=exit_code, biz_code=biz_code)


def load_or_create_device_id(path: Path) -> str:
    try:
        if path.exists():
            value = path.read_text(encoding="utf-8").strip()
            if value:
                return value
        value = str(uuid.uuid4())
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value + "\n", encoding="utf-8")
        os.chmod(path, 0o600)
        return value
    except OSError:
        return str(uuid.uuid4())


Transport = Callable[[str, str, bytes, dict[str, str]], tuple[int, str]]


@dataclass(frozen=True)
class AccountStatus:
    """Account-level chat state from `GET /api/v0/users/current` (REA §mute)."""

    is_muted: bool
    mute_until: float
    status: int


class DeepSeekWebLogin:
    def __init__(
        self,
        base_url: str = DEFAULT_BASE_URL,
        device_id: str | None = None,
        client_version: str = DEFAULT_CLIENT_VERSION,
        timeout: float = 15.0,
        transport: Transport | None = None,
    ):
        self.base_url = base_url.rstrip("/")
        self.device_id = device_id or str(uuid.uuid4())
        self.timeout = timeout
        self._transport = transport or self._urllib_request
        self._headers = {
            "x-client-bundle-id": "com.deepseek.chat",
            "x-client-platform": "web",
            "x-client-version": client_version,
            "x-client-locale": "en_US",
            "x-client-timezone-offset": "0",
            "x-device-id": self.device_id,
            "Origin": self.base_url,
            "Referer": self.base_url + "/sign_in",
            "User-Agent": (
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
                "(KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36"
            ),
            "Content-Type": "application/json",
        }

    def _urllib_request(
        self, method: str, path: str, body: bytes, extra: dict[str, str]
    ) -> tuple[int, str]:
        headers = dict(self._headers)
        headers.update(extra)
        request = urllib.request.Request(self.base_url + path, data=body or None, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                return response.status, response.read().decode(errors="replace")
        except urllib.error.HTTPError as exc:
            return exc.code, exc.read().decode(errors="replace")
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise LoginError(f"网络错误: {exc}", exit_code=EXIT_NETWORK) from exc

    def _call(
        self, method: str, path: str, body: bytes, extra: dict[str, str] | None = None
    ) -> tuple[int, str]:
        """Normalize transport failures (timeout/IO) into explicit LoginError."""
        try:
            return self._transport(method, path, body, extra or {})
        except LoginError:
            raise
        except (TimeoutError, OSError) as exc:
            raise LoginError(f"网络错误: {exc}", exit_code=EXIT_NETWORK) from exc

    def _post_json(self, path: str, payload: dict, extra: dict[str, str] | None = None) -> dict:
        status, text = self._call("POST", path, json.dumps(payload).encode(), extra)
        if status != 200:
            raise LoginError(f"HTTP {status}: {text[:200]}", exit_code=EXIT_NETWORK)
        try:
            return json.loads(text)
        except json.JSONDecodeError as exc:
            raise LoginError(f"响应不是合法 JSON: {text[:200]}", exit_code=EXIT_PROTOCOL) from exc

    def create_guest_challenge(self, target_path: str = LOGIN_PATH) -> dict:
        payload = self._post_json(CHALLENGE_PATH, {"target_path": target_path})
        try:
            challenge = payload["data"]["biz_data"]["guest_challenge"]
        except (KeyError, TypeError) as exc:
            raise LoginError(f"guest challenge 结构非法: {payload!r}"[:300], exit_code=EXIT_PROTOCOL) from exc
        required = ("algorithm", "challenge", "salt", "signature", "difficulty", "expire_at")
        if any(key not in challenge for key in required):
            raise LoginError(f"guest challenge 缺字段: {challenge!r}"[:300], exit_code=EXIT_PROTOCOL)
        if challenge["algorithm"] != "DeepSeekHashV1":
            raise LoginError(f"不支持的 PoW 算法: {challenge['algorithm']}", exit_code=EXIT_PROTOCOL)
        return challenge

    def login(
        self,
        mobile: str,
        sms_code: str,
        area_code: str = "+86",
        region: str = "CN",
        locale: str = "zh_CN",
        progress: Callable[[str], None] | None = None,
    ) -> LoginResult:
        notify = progress or (lambda _msg: None)
        notify("获取 guest PoW challenge ...")
        challenge = self.create_guest_challenge(LOGIN_PATH)
        notify(f"求解 PoW（difficulty={challenge['difficulty']}）...")
        answer, elapsed_ms = solve_pow(challenge)
        notify(f"PoW 完成：answer={answer}（{elapsed_ms}ms）")

        proof = base64.b64encode(
            json.dumps({"salt": challenge["salt"], "answer": answer}, separators=(",", ":")).encode()
        ).decode()
        body, encryption_key = encrypt_body({
            "region": region,
            "locale": locale,
            "mobile_number": mobile,
            "area_code": area_code,
            "sms_verification_code": sms_code,
            "device_id": self.device_id,
            "os": "web",
        })
        notify("提交加密登录请求 ...")
        status, text = self._call(
            "POST",
            LOGIN_PATH,
            body,
            {"x-request-encryption-key": encryption_key, "X-DS-Guest-PoW-Response": proof},
        )
        if status != 200:
            raise LoginError(f"HTTP {status}: {text[:200]}", exit_code=EXIT_NETWORK)
        try:
            payload = json.loads(text)
        except json.JSONDecodeError as exc:
            raise LoginError(f"登录响应不是合法 JSON: {text[:200]}", exit_code=EXIT_PROTOCOL) from exc
        return parse_login_payload(payload)

    def get_account_status(self, token: str) -> AccountStatus:
        """Read account chat state: `is_muted` / `mute_until` (account-level)."""
        status, text = self._call(
            "GET",
            "/api/v0/users/current",
            b"",
            {"Authorization": f"Bearer {token}"},
        )
        if status != 200:
            raise LoginError(f"账号状态查询 HTTP {status}: {text[:200]}", exit_code=EXIT_NETWORK)
        try:
            payload = json.loads(text)
            top_code = int(payload.get("code") or 0)
            if top_code in (40002, 40003):
                raise LoginError(
                    f"账号状态查询失败（{payload.get('msg') or top_code}）：token 缺失或已失效",
                    exit_code=EXIT_AUTH_INVALID,
                    biz_code=top_code,
                )
            if top_code == 40029:
                raise LoginError("IP 访问受限（40029）", exit_code=EXIT_IP_RESTRICTED, biz_code=top_code)
            data = payload["data"]["biz_data"]
            chat = data.get("chat") or {}
            return AccountStatus(
                is_muted=bool(chat.get("is_muted")),
                mute_until=float(chat.get("mute_until") or 0),
                status=int(data.get("status") or 0),
            )
        except LoginError:
            raise
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
            raise LoginError(f"账号状态响应结构非法: {text[:200]}", exit_code=EXIT_PROTOCOL) from exc

    def verify_token(self, token: str) -> str:
        """Real-link check: create a chat session with the token; return its id."""
        status, text = self._call(
            "POST",
            SESSION_PATH,
            b"{}",
            {"Authorization": f"Bearer {token}"},
        )
        if status != 200:
            raise LoginError(f"token 校验 HTTP {status}: {text[:200]}", exit_code=EXIT_NETWORK)
        try:
            payload = json.loads(text)
            top_code = int(payload.get("code") or 0)
            if top_code in (40002, 40003):
                raise LoginError(
                    f"token 校验失败（{payload.get('msg') or top_code}）：token 缺失或已失效",
                    exit_code=EXIT_AUTH_INVALID,
                    biz_code=top_code,
                )
            if top_code == 40029:
                raise LoginError("IP 访问受限（40029）", exit_code=EXIT_IP_RESTRICTED, biz_code=top_code)
            if top_code != 0:
                raise LoginError(
                    f"token 校验失败 code={top_code} msg={payload.get('msg')}",
                    exit_code=EXIT_GENERIC,
                    biz_code=top_code,
                )
            data = payload["data"]
            if int(data["biz_code"]) != 0:
                raise LoginError(
                    f"token 校验失败 biz_code={data['biz_code']} biz_msg={data.get('biz_msg')}",
                    exit_code=EXIT_GENERIC,
                    biz_code=int(data["biz_code"]),
                )
            return str(data["biz_data"]["chat_session"]["id"])
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
            raise LoginError(f"会话响应结构非法: {text[:200]}", exit_code=EXIT_PROTOCOL) from exc


def _mask(value: str) -> str:
    return value[:6] + "…" + value[-4:] if len(value) > 12 else "***"


def _format_epoch(value: float) -> str:
    if not value:
        return "unknown"
    import datetime

    return datetime.datetime.fromtimestamp(value).astimezone().strftime("%Y-%m-%d %H:%M:%S %Z")


def _require_account_not_muted(client: "DeepSeekWebLogin", token: str, progress: Callable[[str], None]) -> "AccountStatus":
    progress("检查账号禁言状态（users/current）...")
    account = client.get_account_status(token)
    if account.is_muted:
        raise LoginError(
            f"账号已被禁言至 {_format_epoch(account.mute_until)}（token 有效但不可聊天）",
            exit_code=EXIT_MUTED,
            biz_code=5,
        )
    return account


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="获取 chat.deepseek.com web token（短信验证码登录；不代发短信、不绕验证码）",
    )
    parser.add_argument("--mobile", help="手机号（不含国家码，如 15538643833）")
    parser.add_argument("--area-code", default="+86", help="国家码（默认 +86）")
    parser.add_argument("--region", default="CN", help="region（默认 CN）")
    parser.add_argument("--locale", default="zh_CN", help="locale（默认 zh_CN）")
    parser.add_argument("--sms-code", help="短信验证码；省略则交互输入")
    parser.add_argument("--device-id", help="设备 UUID；默认复用/生成持久 device id")
    parser.add_argument("--device-id-file", type=Path, default=DEFAULT_DEVICE_STATE,
                        help="device id 持久化文件（默认 ~/.local/state/ponyllm/deepseek-web-device-id）")
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL)
    parser.add_argument("--timeout", type=float, default=15.0, help="单次 HTTP 超时秒数（默认 15）")
    parser.add_argument("--out", type=Path, help="把 token 写入该文件（0600）")
    parser.add_argument("--verify", action="store_true",
                        help="真实链路校验：账号未禁言 + create_chat_session 成功")
    parser.add_argument("--status", action="store_true",
                        help="只查询账号状态（需 --token-file），不登录")
    parser.add_argument("--token-file", type=Path,
                        help="已有 token 文件（--status 必填；--verify 可与登录二选一）")
    parser.add_argument("--json", action="store_true", help="输出 JSON（含 token）")
    parser.add_argument("--quiet", action="store_true", help="不打印进度")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    progress = (lambda _msg: None) if args.quiet else (lambda msg: print(msg, file=sys.stderr))

    if args.status:
        if not args.token_file:
            print("错误：--status 需要 --token-file", file=sys.stderr)
            return EXIT_GENERIC
        token = args.token_file.read_text(encoding="utf-8").strip()
        client = DeepSeekWebLogin(base_url=args.base_url, timeout=args.timeout)
        try:
            account = client.get_account_status(token)
        except LoginError as exc:
            print(f"失败：{exc}", file=sys.stderr)
            return exc.exit_code
        if args.json:
            print(json.dumps({
                "ok": not account.is_muted,
                "is_muted": account.is_muted,
                "mute_until": account.mute_until,
                "mute_until_local": _format_epoch(account.mute_until),
                "status": account.status,
            }, ensure_ascii=False))
        else:
            print(
                f"is_muted={account.is_muted} mute_until={_format_epoch(account.mute_until)} "
                f"status={account.status}"
            )
        return EXIT_MUTED if account.is_muted else EXIT_OK

    if not args.mobile:
        print("错误：登录需要 --mobile（或使用 --status）", file=sys.stderr)
        return EXIT_GENERIC

    sms_code = args.sms_code
    if not sms_code:
        sms_code = getpass.getpass("短信验证码: ").strip()
    if not (sms_code.isdigit() and len(sms_code) == 6):
        print("错误：短信验证码必须是 6 位数字", file=sys.stderr)
        return EXIT_GENERIC

    device_id = args.device_id or load_or_create_device_id(args.device_id_file)
    client = DeepSeekWebLogin(
        base_url=args.base_url,
        device_id=device_id,
        timeout=args.timeout,
    )
    try:
        result = client.login(
            mobile=args.mobile,
            sms_code=sms_code,
            area_code=args.area_code,
            region=args.region,
            locale=args.locale,
            progress=progress,
        )
        session_id = None
        account = None
        if args.verify:
            account = _require_account_not_muted(client, result.token, progress)
            progress("校验 token（create_chat_session）...")
            session_id = client.verify_token(result.token)
            progress(f"token 有效且账号未禁言：session_id={session_id}")
    except LoginError as exc:
        if args.json:
            print(json.dumps({"ok": False, "error": str(exc), "biz_code": exc.biz_code}, ensure_ascii=False))
        print(f"失败：{exc}", file=sys.stderr)
        return exc.exit_code

    if args.out:
        args.out.write_text(result.token + "\n", encoding="utf-8")
        os.chmod(args.out, 0o600)
        progress(f"token 已写入 {args.out}（0600）")

    if args.json:
        print(json.dumps({
            "ok": True,
            "token": result.token,
            "user_id": result.user_id,
            "biz_code": result.biz_code,
            "session_id": session_id,
            "device_id": device_id,
            "is_muted": account.is_muted if account else None,
            "mute_until": account.mute_until if account else None,
        }, ensure_ascii=False))
    else:
        if args.out:
            print(f"token: {_mask(result.token)}（完整 token 见 {args.out}）")
        else:
            print(result.token)
        print(f"user_id: {result.user_id}，device_id: {device_id}", file=sys.stderr)
        if session_id:
            print(f"真实链路校验通过（账号未禁言）：session_id={session_id}", file=sys.stderr)
    return EXIT_OK


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("已取消", file=sys.stderr)
        sys.exit(130)
