"""黑盒验收测试：DNS ndots 配置优化与 ADR 决策记录治理契约。

契约目标：
1. 治理：代码库中引用的 2026-10-06-egress-guard-proxied-dns-skip.md 必须物理存在于 .agents/notes/ 下，且全量 verify-note.sh 校验通过。
2. 配置：deploy/ponyllm-deployment.yaml 中 4 个 Deployment (dev/preprod/proserver/tencent) 的 dnsConfig options ndots 必须为 "1"。
"""

import os
import re
import subprocess
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

class DnsNdotsAndAdrAcceptanceTest(unittest.TestCase):
    def test_codebase_adr_links_point_to_valid_path(self):
        """验证源码注释中的 ADR 路径全部指向实际存在的文件（不指向 proposed 幽灵路径）"""
        target_file = REPO_ROOT / ".agents/notes/implemented/bug-fix/2026-10-06-egress-guard-proxied-dns-skip.md"
        self.assertTrue(target_file.exists(), "implemented 路径下的 ADR 必须存在")

        # 检查 crates 下源码中是否仍错误指向不存在的 proposed 目录
        grep_cmd = ["git", "grep", "-n", "proposed/bug-fix/2026-10-06-egress-guard-proxied-dns-skip.md", "crates/"]
        res = subprocess.run(grep_cmd, cwd=REPO_ROOT, capture_output=True, text=True)
        self.assertNotEqual(
            res.returncode,
            0,
            f"源码中仍存在指向 proposed 的失效链接，必须修正为 implemented:\n{res.stdout}"
        )

    def test_deployment_manifest_ndots_value_is_one(self):
        """验证 deploy/ponyllm-deployment.yaml 中所有 ndots 设置均为 1"""
        deploy_yaml = REPO_ROOT / "deploy/ponyllm-deployment.yaml"
        self.assertTrue(deploy_yaml.exists(), "deploy/ponyllm-deployment.yaml 不存在")
        content = deploy_yaml.read_text(encoding="utf-8")

        pattern = re.compile(
            r'-\s+name:\s+ndots\s*\n\s+value:\s*"([^"]+)"',
            re.MULTILINE
        )
        matches = pattern.findall(content)
        self.assertEqual(len(matches), 4, f"预期 4 个 Deployment 包含 ndots 配置，实际找到 {len(matches)} 处")

        for idx, val in enumerate(matches):
            self.assertEqual(val, "1", f"第 {idx+1} 处 ndots 配置值预期为 '1'，实际为 '{val}'")

if __name__ == "__main__":
    unittest.main()

