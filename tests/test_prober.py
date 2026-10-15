import json
import unittest
from unittest.mock import patch, MagicMock
import urllib.error

import deploy.prober as prober


class ProberLogicTest(unittest.TestCase):
    def setUp(self):
        with prober.lock:
            prober.state["probe_success"] = 1
            prober.state["probe_throttled"] = 0
            prober.state["total_probes"] = 0
            prober.state["total_failures"] = 0
            prober.state["total_throttled"] = 0
            prober.state["failures_by_class"] = {"4xx": 0, "5xx": 0, "other": 0}

    @patch("urllib.request.urlopen")
    def test_run_probe_success_200(self, mock_urlopen):
        mock_resp = MagicMock()
        mock_resp.status = 200
        mock_resp.read.return_value = json.dumps({"choices": [{"message": {"content": "pong"}}]}).encode("utf-8")
        mock_urlopen.return_value.__enter__.return_value = mock_resp

        prober.run_probe()

        with prober.lock:
            self.assertEqual(prober.state["probe_success"], 1)
            self.assertEqual(prober.state["probe_throttled"], 0)
            self.assertEqual(prober.state["probe_http_status"], 200)
            self.assertEqual(prober.state["total_probes"], 1)
            self.assertEqual(prober.state["total_failures"], 0)
            self.assertEqual(prober.state["total_throttled"], 0)

    @patch("urllib.request.urlopen")
    def test_run_probe_throttled_429(self, mock_urlopen):
        # 模拟上游返回 429 Too Many Requests
        err = urllib.error.HTTPError("http://example.com", 429, "Too Many Requests", {}, None)
        mock_urlopen.side_effect = err

        prober.run_probe()

        with prober.lock:
            # 契约断言：429 判定为 throttled，有效探活成功（服务未宕机，网关正常响应），不作为 failure 导致报警
            self.assertEqual(prober.state["probe_success"], 1)
            self.assertEqual(prober.state["probe_throttled"], 1)
            self.assertEqual(prober.state["probe_http_status"], 429)
            self.assertEqual(prober.state["total_probes"], 1)
            self.assertEqual(prober.state["total_failures"], 0)
            self.assertEqual(prober.state["total_throttled"], 1)
            self.assertEqual(prober.state["failures_by_class"]["4xx"], 0)

    @patch("urllib.request.urlopen")
    def test_run_probe_failure_500(self, mock_urlopen):
        # 模拟服务 500 异常
        err = urllib.error.HTTPError("http://example.com", 500, "Internal Server Error", {}, None)
        mock_urlopen.side_effect = err

        prober.run_probe()

        with prober.lock:
            self.assertEqual(prober.state["probe_success"], 0)
            self.assertEqual(prober.state["probe_throttled"], 0)
            self.assertEqual(prober.state["probe_http_status"], 500)
            self.assertEqual(prober.state["total_probes"], 1)
            self.assertEqual(prober.state["total_failures"], 1)
            self.assertEqual(prober.state["total_throttled"], 0)
            self.assertEqual(prober.state["failures_by_class"]["5xx"], 1)


if __name__ == "__main__":
    unittest.main()
