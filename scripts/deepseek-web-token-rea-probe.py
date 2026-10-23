#!/usr/bin/env python3
"""deepseek-web token 获取机制 REA 探针（只读、无短信、无验证码绕过）。

证据文档：.dev-team/contracts/wave-13-deepseek-web-token-rea-evidence.md

三条断言（全部成立才 exit 0）：
1. 登录端点缺少 guest PoW 头时返回 `code=40300`（证明 PoW 是登录前置）。
2. 带 guest PoW + 加密请求体的登录请求返回 `biz_code in {7,8}`（dummy 短信码；
   证明公钥、AES-CBC+RSA-OAEP、字段名、PoW 求解均被服务端接受）。
3. 无 captcha token 的 `create_sms_verification_code` 返回 `biz_code=2`
   （RECAPTCHA_VERIFY_FAILED；证明短信发送强制人机验证）。

约束：
* 只使用非法手机号 `00000000000`，服务端在 captcha 校验处即拒绝，不会发短信。
* 不实现/不尝试任何验证码、WAF、设备指纹绕过。
* 依赖：Python 3 + `cryptography`（非仓库运行时依赖，仅供手工复现）。

用法：python3 scripts/deepseek-web-token-rea-probe.py [--json]
"""

from __future__ import annotations

import base64
import hashlib
import json
import secrets
import struct
import sys
import time
import urllib.error
import urllib.request
import uuid

BASE = "https://chat.deepseek.com"
DUMMY_MOBILE = "00000000000"
PUBLIC_KEY_B64 = (
    "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0RLAhx1TOJhAQBHS1LHXlnkf"
    "Og86O+uhsmKWNNkmugVrRqO6cl13SnjBs2otRYZGJd50xpBG4We2i6/oCJCwxA/49YZf"
    "IwaeDmTx+WBIN7qrwhwP7vGCi6Osesmn/Ddgdye6mifYkRpRMonQjGY6VPaUMTw1jjO"
    "AIKph5YZn0bDVmGLvs5eH6CoQYHM5/Z1Nfhj36VL28eZi3elJHoz/L6Tzi2wY1rCYqY"
    "Hfepq3Ysy8y5Y5BeFJXN60RYLlbs+MRC+2kyJqcfApUnn1O/W2om9dhxc6Rc5dqas1Y"
    "B4X4CWCttrKk/suhpIx9zpQY9kqtotbe5zweYaKv0rCVeqx8wIDAQAB"
)

# Keccak-f[1600] constants; PoW worker runs iota rounds 1..=23 (23 rounds).
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
                state[x + 5 * y] = b[x + 5 * y] ^ ((~b[(x + 1) % 5 + 5 * y]) & b[(x + 2) % 5 + 5 * y] & MASK)
        state[0] ^= RC[rnd]


def pow_hash23(message: bytes) -> str:
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


def solve(challenge: dict) -> int:
    prefix = f"{challenge['salt']}_{challenge['expire_at']}_"
    for i in range(int(challenge["difficulty"])):
        if pow_hash23((prefix + str(i)).encode()) == challenge["challenge"]:
            return i
    raise RuntimeError("no solution within difficulty")


def encrypt_body(obj: dict) -> tuple[bytes, str]:
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import padding
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

    key, iv = secrets.token_bytes(16), secrets.token_bytes(16)
    plain = json.dumps(obj, separators=(",", ":")).encode()
    pad = 16 - len(plain) % 16
    plain += bytes([pad]) * pad
    encryptor = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
    body = encryptor.update(plain) + encryptor.finalize()
    pub = serialization.load_der_public_key(base64.b64decode(PUBLIC_KEY_B64))
    wrapped = pub.encrypt(
        key + iv,
        padding.OAEP(mgf=padding.MGF1(hashes.SHA256()), algorithm=hashes.SHA256(), label=None),
    )
    return body, base64.b64encode(wrapped).decode()


class Client:
    def __init__(self) -> None:
        self.device_id = str(uuid.uuid4())
        self.headers = {
            "x-client-bundle-id": "com.deepseek.chat",
            "x-client-platform": "web",
            "x-client-version": "2.5.0",
            "x-client-locale": "en_US",
            "x-client-timezone-offset": "0",
            "x-device-id": self.device_id,
            "Origin": BASE,
            "Referer": BASE + "/sign_in",
            "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/141.0.0.0 Safari/537.36",
            "Content-Type": "application/json",
        }

    def post(self, path: str, body: bytes, extra: dict[str, str] | None = None) -> tuple[int, str]:
        headers = dict(self.headers)
        headers.update(extra or {})
        request = urllib.request.Request(BASE + path, data=body, method="POST", headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return response.status, response.read().decode(errors="replace")
        except urllib.error.HTTPError as exc:
            return exc.code, exc.read().decode(errors="replace")

    def guest_pow(self, target_path: str) -> tuple[str, dict]:
        status, text = self.post(
            "/api/v0/users/create_guest_challenge",
            json.dumps({"target_path": target_path}).encode(),
        )
        if status != 200:
            raise RuntimeError(f"guest challenge HTTP {status}: {text[:200]}")
        challenge = json.loads(text)["data"]["biz_data"]["guest_challenge"]
        started = time.perf_counter()
        answer = solve(challenge)
        header = base64.b64encode(
            json.dumps({"salt": challenge["salt"], "answer": answer}, separators=(",", ":")).encode()
        ).decode()
        return header, {"difficulty": challenge["difficulty"], "answer": answer,
                        "solve_ms": round((time.perf_counter() - started) * 1000)}


def main() -> int:
    want_json = "--json" in sys.argv
    client = Client()
    receipts: dict[str, object] = {"started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "probes": {}}

    # Probe 1: encrypted login without guest PoW must be rejected as 40300.
    body, enc_key = encrypt_body({
        "region": "CN", "locale": "zh_CN", "mobile_number": DUMMY_MOBILE,
        "area_code": "+86", "sms_verification_code": "000000",
        "device_id": client.device_id, "os": "web",
    })
    status, text = client.post("/api/v0/users/login_by_mobile_sms", body,
                               {"x-request-encryption-key": enc_key})
    payload = json.loads(text)
    receipts["probes"]["login_without_pow"] = {"http": status, "code": payload.get("code"), "msg": payload.get("msg")}
    assert status == 200 and payload.get("code") == 40300, f"expected 40300, got {status} {text[:300]}"

    # Probe 2: encrypted login with guest PoW must be accepted (dummy code -> 7/8).
    pow_header, pow_meta = client.guest_pow("/api/v0/users/login_by_mobile_sms")
    body, enc_key = encrypt_body({
        "region": "CN", "locale": "zh_CN", "mobile_number": DUMMY_MOBILE,
        "area_code": "+86", "sms_verification_code": "000000",
        "device_id": client.device_id, "os": "web",
    })
    status, text = client.post("/api/v0/users/login_by_mobile_sms", body,
                               {"x-request-encryption-key": enc_key, "X-DS-Guest-PoW-Response": pow_header})
    payload = json.loads(text)
    biz_code = payload.get("data", {}).get("biz_code")
    receipts["probes"]["login_with_pow"] = {"http": status, "biz_code": biz_code,
                                            "biz_msg": payload.get("data", {}).get("biz_msg"), "pow": pow_meta}
    assert status == 200 and biz_code in (7, 8), f"expected biz_code 7/8, got {status} {text[:300]}"

    # Probe 3: SMS send without captcha must be rejected as 2 (RECAPTCHA_VERIFY_FAILED).
    pow_header, pow_meta = client.guest_pow("/api/v0/users/create_sms_verification_code")
    body, enc_key = encrypt_body({
        "locale": "zh_CN", "turnstile_token": "", "shumei_verification": None,
        "hcaptcha_token": None, "device_id": client.device_id,
        "scenario": "login", "mobile_number": DUMMY_MOBILE,
    })
    status, text = client.post("/api/v0/users/create_sms_verification_code", body,
                               {"x-request-encryption-key": enc_key, "X-DS-Guest-PoW-Response": pow_header})
    payload = json.loads(text)
    biz_code = payload.get("data", {}).get("biz_code")
    receipts["probes"]["sms_without_captcha"] = {"http": status, "biz_code": biz_code,
                                                 "biz_msg": payload.get("data", {}).get("biz_msg"), "pow": pow_meta}
    assert status == 200 and biz_code == 2, f"expected biz_code 2, got {status} {text[:300]}"

    receipts["verdict"] = "PASS"
    receipts["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
    if want_json:
        print(json.dumps(receipts, ensure_ascii=False, indent=2))
    else:
        print("PASS: guest PoW required; encrypted login accepted (dummy code 7/8); SMS send gated by captcha (2)")
        for name, probe in receipts["probes"].items():
            print(f"  {name}: {json.dumps(probe, ensure_ascii=False)}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as exc:  # noqa: BLE001 - probe must fail loudly with non-zero exit
        print(f"FAIL: {type(exc).__name__}: {exc}", file=sys.stderr)
        sys.exit(1)
