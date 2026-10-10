"""Regression: fitting-row evidence cannot authorize a production gate."""
import ast
from pathlib import Path
import unittest


class CalibrationPolicyTests(unittest.TestCase):
    def setUp(self):
        # Load only the policy functions: no calibration logs or generated specs are touched.
        source = Path(__file__).with_name("write-specs.py").read_text(encoding="utf-8")
        tree = ast.parse(source)
        functions = [node for node in tree.body if isinstance(node, ast.FunctionDef)
                     and node.name in ("advisory", "calibrated")]
        self.env = {"CODE": "regression fixtures", "RESULTS": {
            "date": "2026-10-01", "minCatch": 0.7, "maxFalseReject": 0.05,
            "results": {"code_chunk.unfinished": {
                "good": 100, "bad": 100, "auc": 1.0, "promote": True,
                "operatingPoint": {"caught": 1.0}, "goodTail": [[0.05, 0.5]],
            }},
        }}
        exec(compile(ast.Module(body=functions, type_ignores=[]), str(source), "exec"), self.env)

    def test_perfect_legacy_fit_is_still_advisory(self):
        rule = self.env["calibrated"]("code_chunk.unfinished", "unfinished", "paired fixtures")
        self.assertEqual(rule["gate"], "advisory")
        self.assertNotIn("threshold", rule)
        self.assertIn("held-out", rule["why"])

    def test_missing_evidence_stays_advisory(self):
        rule = self.env["calibrated"]("absent", "unfinished", "fixtures")
        self.assertEqual(rule["gate"], "advisory")
        self.assertIn("missing", rule["why"])


if __name__ == "__main__":
    unittest.main()
