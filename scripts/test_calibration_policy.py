"""Regression: fitting-row evidence cannot authorize a production gate."""
import unittest
from calibration_policy import calibrated


class CalibrationPolicyTests(unittest.TestCase):
    def setUp(self):
        self.results = {
            "date": "2026-10-01", "minCatch": 0.7, "maxFalseReject": 0.05,
            "results": {"code_chunk.unfinished": {
                "good": 100, "bad": 100, "auc": 1.0, "promote": True,
                "operatingPoint": {"caught": 1.0}, "goodTail": [[0.05, 0.5]],
            }},
        }

    def test_perfect_legacy_fit_is_still_advisory(self):
        rule = calibrated("code_chunk.unfinished", "unfinished", "paired fixtures",
                          results=self.results, source="regression fixtures")
        self.assertEqual(rule["gate"], "advisory")
        self.assertNotIn("threshold", rule)
        self.assertIn("held-out", rule["why"])

    def test_missing_evidence_stays_advisory(self):
        rule = calibrated("absent", "unfinished", "fixtures",
                          results=self.results, source="regression fixtures")
        self.assertEqual(rule["gate"], "advisory")
        self.assertIn("missing", rule["why"])


if __name__ == "__main__":
    unittest.main()
