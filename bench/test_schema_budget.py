import argparse
import contextlib
import io
import json
import pathlib
import tempfile
import unittest

import schema_budget as budget


class SchemaBudgetTests(unittest.TestCase):
    def test_discovery_overrides_only_lifecycle_ownership_without_hiding_tools(self):
        original = {"KRITIC_KEV_EXTERNALLY_MANAGED": "false", "KRITIC_CAP_REVIEW": "true",
                    "KRITIC_BACKEND": "local", "UNRELATED": "preserved"}
        safe = budget.discovery_env(original)
        self.assertEqual(safe["KRITIC_KEV_EXTERNALLY_MANAGED"], "true")
        self.assertEqual(safe["KRITIC_VISION_EXTERNALLY_MANAGED"], "true")
        self.assertEqual(original["KRITIC_KEV_EXTERNALLY_MANAGED"], "false")
        for key in ("KRITIC_CAP_REVIEW", "KRITIC_BACKEND", "UNRELATED"):
            self.assertEqual(safe[key], original[key])

    def test_minimal_profile_clears_real_backend_disabled_tool_and_capability_overrides(self):
        original = {"KRITIC_BACKEND": "cloud", "KRITIC_KEV_BACKEND": "http",
                    "KRITIC_VISION_BACKEND": "cloud", "KRITIC_DISABLED_TOOLS": "artifact_validate",
                    "KRITIC_CAP_HISTORY": "true", "UNRELATED": "preserved"}
        sanitized = budget.minimal_env(original, pathlib.Path("minimal.toml"), pathlib.Path("output"))
        for name in original:
            if name != "UNRELATED":
                self.assertNotIn(name, sanitized)
                self.assertIn(name, original)
        self.assertEqual(sanitized["UNRELATED"], "preserved")
        self.assertEqual(sanitized["KRITIC_CONFIG"], str(pathlib.Path("minimal.toml").resolve()))

    def test_utf8_bytes_are_not_character_or_exact_token_counts(self):
        measured = budget.measure({"label": "é"}, None)
        self.assertEqual(budget.serialized({"label": "é"}), '{"label":"é"}')
        self.assertEqual(measured["utf8_bytes"], 14)
        self.assertEqual(measured["characters"], 13)
        self.assertIsNone(measured["reference_tokens"])

    def test_projection_preserves_property_names_literal_payloads_and_constraints(self):
        schema = {"type": "object", "title": "Presentation", "required": ["title"],
                  "properties": {"title": {"type": "string", "title": "Label", "description": "Safe title", "minLength": 1}},
                  "const": {"title": "literal", "examples": [1], "default": ""},
                  "enum": [{"title": "literal"}], "$defs": {"title": {"type": "integer", "minimum": 3}},
                  "additionalProperties": False}
        projected = budget.constraint_projection(schema)
        self.assertNotIn("title", projected)
        self.assertEqual(projected["properties"]["title"], {"type": "string", "description": "Safe title", "minLength": 1})
        for key in ("required", "const", "enum", "additionalProperties"):
            self.assertEqual(projected[key], schema[key])
        self.assertEqual(projected["$defs"]["title"]["minimum"], 3)
        self.assertEqual(schema["title"], "Presentation")

    def test_projection_preserves_boolean_numeric_and_nonempty_defaults(self):
        for value in (False, True, 0, 1, "x", [1], {"title": "literal"}):
            with self.subTest(value=value):
                self.assertEqual(budget.constraint_projection({"default": value})["default"], value)
        for value in (None, "", [], {}):
            self.assertNotIn("default", budget.constraint_projection({"default": value}))

    def test_malformed_empty_and_duplicate_captures_are_rejected(self):
        tool = {"name": "review", "inputSchema": {"type": "object"}}
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "capture.json"
            for value in ({}, {"tools": []}, {"tools": [tool, tool]}, {"tools": [{"name": "bad"}]}):
                with self.subTest(value=value):
                    path.write_text(json.dumps(value), encoding="utf-8")
                    with self.assertRaises(ValueError):
                        budget.read_capture(path)

    def test_actual_capture_comparison_rejects_changed_required_constraints(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for label in ("before", "after"):
                for profile in ("current", "minimal"):
                    path = root / label / profile / "tools-list.json"
                    path.parent.mkdir(parents=True)
                    schema = {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
                    if label == "after":
                        schema["required"] = []
                    path.write_text(json.dumps({"tools": [{"name": "review", "inputSchema": schema}]}), encoding="utf-8")
            with contextlib.redirect_stdout(io.StringIO()) as output:
                with self.assertRaises(SystemExit):
                    budget.compare(argparse.Namespace(baseline=root/"before", candidate=root/"after", output=None))
            self.assertFalse(json.loads(output.getvalue())["pass"])


if __name__ == "__main__":
    unittest.main()
