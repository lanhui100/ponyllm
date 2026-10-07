"""黑盒验收测试：ponyllm 生产服务加模型 skill。

契约：.dev-team/contracts/add-model-contract.md
对象：skills/ponyllm-add-model/SKILL.md（只读黑盒，不导入任何业务代码）

验收矩阵（契约 §验收矩阵）：
  1. SKILL.md 存在且含五要素标题（触发式 description / 真相源 / 程序步骤 / 校准样例 / 验证与报告）
  2. 契约字段映射表示例可解析（context_window/max_output/input_types/thinking_default/thinking_max/protocol/tier 齐全）
  3. Admin API payload 示例 JSON 可解析且字段齐全
     （provider/name/context_window/max_output/input_types/output_types/protocol/thinking）
  4. 反例：缺任一要素即失败（checker 负例自检，防止空断言）

红相说明：本文件在空桩阶段必须整体 RED（SKILL.md 仅标题 + TODO，无四节正文、无 payload JSON）。
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SKILL_PATH = REPO_ROOT / "skills" / "ponyllm-add-model" / "SKILL.md"
CONTRACT_PATH = REPO_ROOT / ".dev-team" / "contracts" / "add-model-contract.md"

# 五要素标题判定（heading 关键字，大小写不敏感；覆盖"程序步骤/步骤"、"校准样例/样例"等写法）
ELEMENT_PATTERNS = {
    "truth_sources": r"^#{1,6}\s*.*真相源",
    "steps": r"^#{1,6}\s*.*步骤",
    "examples": r"^#{1,6}\s*.*样例",
    "verification": r"^#{1,6}\s*.*验证",
}

# 契约字段映射表要求齐全的字段（thinking_default/thinking_max 在契约表中是合并行，解析时拆分）
REQUIRED_MAPPING_FIELDS = frozenset(
    {
        "context_window",
        "max_output",
        "input_types",
        "thinking_default",
        "thinking_max",
        "protocol",
        "tier",
    }
)

# Admin payload 示例要求齐全的键（thinking 允许 thinking_default/thinking_max 任一）
REQUIRED_PAYLOAD_KEYS = frozenset(
    {
        "provider",
        "name",
        "context_window",
        "max_output",
        "input_types",
        "output_types",
        "protocol",
    }
)


def read_skill() -> str:
    assert SKILL_PATH.is_file(), f"SKILL.md 缺失：{SKILL_PATH}"
    return SKILL_PATH.read_text(encoding="utf-8")


def frontmatter(text: str) -> str:
    m = re.match(r"\s*---\s*\n(.*?)\n---\s*\n?", text, re.S)
    assert m, "SKILL.md 缺少 YAML frontmatter（--- 包裹的头部）"
    return m.group(1)


def missing_elements(text: str) -> list[str]:
    """返回缺失的五要素名；空列表 = 齐全。缺任一要素即失败的判定核心。"""
    fm = frontmatter(text)
    missing: list[str] = []
    # 要素1：触发式 description（头部内联"何时用"/触发条件，路由发生在只有摘要可见时）
    dm = re.search(r"description\s*:\s*\|?-?(.*)", fm, re.S)
    desc_ok = bool(dm) and ("何时用" in fm or "触发" in fm)
    if not desc_ok:
        missing.append("description")
    body = text.split("---", 2)[-1] if text.count("---") >= 2 else text
    for name, pat in ELEMENT_PATTERNS.items():
        if not re.search(pat, body, re.M):
            missing.append(name)
    return missing


def extract_json_blocks(text: str) -> list[str]:
    return re.findall(r"```json\s*\n(.*?)```", text, re.S | re.I)


def parse_mapping_table(contract: str) -> dict[str, str]:
    """解析契约中的字段映射表 → {网关字段: 例}。合并单元格（如 a/b）拆分为两个键。"""
    lines = [ln.strip() for ln in contract.splitlines() if ln.strip().startswith("|")]
    assert lines, "契约中未找到字段映射表（无 | 起始行）"
    assert "网关字段" in lines[0], f"映射表头异常：{lines[0]}"
    mapping: dict[str, str] = {}
    for ln in lines[2:]:  # 跳过表头与分隔行
        cells = [c.strip() for c in ln.strip().strip("|").split("|")]
        if len(cells) < 4 or not cells[0] or set(cells[0]) <= set("-: "):
            continue
        for field in cells[0].split("/"):
            field = field.strip().strip("`")
            if field:
                mapping[field] = cells[3]
    return mapping


# ---- 验收 1：SKILL.md 存在 + 五要素标题 ----


def test_skill_exists():
    assert SKILL_PATH.is_file(), f"SKILL.md 不存在：{SKILL_PATH}"


def test_skill_has_trigger_description():
    text = read_skill()
    assert "description" not in missing_elements(text), "缺少触发式 description（头部须内联'何时用'/触发条件）"


@pytest.mark.parametrize("element", sorted(ELEMENT_PATTERNS))
def test_skill_has_body_element(element):
    text = read_skill()
    assert element not in missing_elements(text), f"SKILL.md 缺少要素节标题：{element}"


def test_skill_five_elements_complete():
    assert missing_elements(read_skill()) == [], (
        f"SKILL.md 五要素不齐全，缺失：{missing_elements(read_skill())}"
    )


# ---- 验收 2：契约字段映射表示例可解析 ----


def test_contract_mapping_fields_complete():
    contract = CONTRACT_PATH.read_text(encoding="utf-8")
    mapping = parse_mapping_table(contract)
    lacking = sorted(REQUIRED_MAPPING_FIELDS - set(mapping))
    assert not lacking, f"字段映射表缺字段：{lacking}；实有：{sorted(mapping)}"


def test_contract_mapping_examples_nonempty():
    contract = CONTRACT_PATH.read_text(encoding="utf-8")
    mapping = parse_mapping_table(contract)
    empty = sorted(f for f in REQUIRED_MAPPING_FIELDS if not mapping.get(f, "").strip())
    assert not empty, f"字段映射表示例列为空：{empty}"


# ---- 验收 3：Admin payload 示例 JSON 可解析且字段齐全 ----


def test_admin_payload_example_parseable_and_complete():
    text = read_skill()
    blocks = extract_json_blocks(text)
    assert blocks, "SKILL.md 中未找到 ```json payload 示例块"
    parsed = []
    for b in blocks:
        try:
            parsed.append(json.loads(b))
        except json.JSONDecodeError as e:
            pytest.fail(f"payload 示例 JSON 解析失败：{e}\n块内容：{b[:300]}")
    ok = False
    for obj in parsed:
        if not isinstance(obj, dict):
            continue
        keys = set(obj)
        thinking_ok = bool(keys & {"thinking", "thinking_default", "thinking_max"})
        if REQUIRED_PAYLOAD_KEYS <= keys and thinking_ok:
            ok = True
            break
    assert ok, (
        f"payload 示例缺字段：需 {sorted(REQUIRED_PAYLOAD_KEYS)} + thinking*；"
        f"实得 {sorted(parsed[0].keys()) if parsed and isinstance(parsed[0], dict) else parsed}"
    )


# ---- 验收 4：反例 —— 缺任一要素即失败（checker 自检，防止空断言） ----

VALID_DOC = """---
name: demo
description: 何时用：给 ponyllm 加个模型时触发。
---

# demo

## 真相源
- a.md

## 程序步骤
1. 干活

## 校准样例
- 正例

## 验证与报告
- 跑测试
"""


@pytest.mark.parametrize(
    "drop",
    ["description", *sorted(ELEMENT_PATTERNS)],
)
def test_negative_missing_any_element_fails(drop):
    if drop == "description":
        doc = VALID_DOC.replace("何时用：给 ponyllm 加个模型时触发。", "随便写写。")
    elif drop == "truth_sources":
        doc = VALID_DOC.replace("## 真相源", "## 背景")
    elif drop == "steps":
        doc = VALID_DOC.replace("## 程序步骤", "## 做法")
    elif drop == "examples":
        doc = VALID_DOC.replace("## 校准样例", "## 例子")
    elif drop == "verification":
        doc = VALID_DOC.replace("## 验证与报告", "## 收尾")
    else:  # pragma: no cover
        raise AssertionError(drop)
    assert drop in missing_elements(doc), f"反例未被检出：去掉 {drop} 本应失败"


def test_negative_no_json_payload_fails():
    assert extract_json_blocks(VALID_DOC) == [], "反例基准文档本不应含 JSON 块"
