"""The kritic context-engine plugin against Hermes' real ContextEngine ABC and the real kritic binary.

Run (from the repo root, after `cargo build`):
    KRITIC_EXE=<target>/debug/kritic uv run --with hermes-agent --with pytest pytest integrations/hermes/tests -q
"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import sys
import types
from pathlib import Path

import pytest

from agent.context_engine import ContextEngine

PLUGIN = Path(__file__).resolve().parents[1] / "kritic" / "__init__.py"


def load_plugin():
    spec = importlib.util.spec_from_file_location("kritic_hermes_plugin", PLUGIN)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class Ctx:
    def __init__(self):
        self.engine = None

    def register_context_engine(self, engine):
        self.engine = engine


@pytest.fixture()
def env(tmp_path, monkeypatch):
    exe = os.environ.get("KRITIC_EXE")
    if not exe or not Path(exe).is_file():
        pytest.skip("set KRITIC_EXE to a built kritic binary")
    hermes = tmp_path / "hermes"
    hermes.mkdir()
    # Deterministic: rank oldest first, no model.
    (hermes / "config.yaml").write_text("kritic_context:\n  use_kev: false\n  protect_first_n: 1\n  protect_last_n: 2\n", encoding="utf-8")
    monkeypatch.setenv("HERMES_HOME", str(hermes))
    monkeypatch.setenv("KRITIC_HOME", str(tmp_path / "kritic"))
    monkeypatch.setenv("KRITIC_KEV_BACKEND", "off")
    return tmp_path


def transcript():
    msgs = [{"role": "system", "content": "You are a coding agent."}, {"role": "user", "content": "Fix the parser bug"}]
    for i in range(6):
        cid = f"call_{i}"
        msgs.append({"role": "assistant", "content": "", "tool_calls": [{"id": cid, "type": "function", "function": {"name": "read_file", "arguments": json.dumps({"path": f"f{i}.rs"})}}]})
        msgs.append({"role": "tool", "tool_call_id": cid, "content": f"f{i}.rs " + "line of code\n" * 300})
        msgs.append({"role": "assistant", "content": f"Read f{i}.rs."})
    msgs.append({"role": "user", "content": "Now handle empty input."})
    return msgs


def engine():
    ctx = Ctx()
    load_plugin().register(ctx)
    assert isinstance(ctx.engine, ContextEngine)
    return ctx.engine


def test_recall_forwards_options_and_bounds_python_serialization(monkeypatch):
    e = engine()
    calls = []

    def recall_result(op, payload):
        calls.append((op, payload))
        return {"found": [{"original_content": "🦀" * 10000}]}

    monkeypatch.setattr(e, "_run", recall_result)
    result = e.handle_tool_call("kritic_recall", {"id": "kr-test", "format": "original", "max_chars": 0, "max_output_bytes": 256})
    assert len(result.encode("utf-8")) <= 256
    assert "error" in json.loads(result)
    assert calls[0][1]["format"] == "original"
    assert calls[0][1]["max_chars"] == 0
    assert calls[0][1]["max_output_bytes"] == 256


def test_contract_and_thresholds(env):
    e = engine()
    assert e.name == "kritic"
    e.update_model("claude-sonnet-5", 20000)
    assert e.context_length == 20000 and e.threshold_tokens == 10000
    e.update_from_response({"prompt_tokens": 9000, "completion_tokens": 10, "total_tokens": 9010})
    assert not e.should_compress()
    assert e.should_compress(12000)
    assert e.should_compress_preflight(transcript()) is False
    assert [t["name"] for t in e.get_tool_schemas()] == ["kritic_recall"]
    clone = e.clone_for_agent()
    clone.update_model("other", 1000)
    assert e.context_length == 20000, "a clone's budget is its own"


def test_compress_is_lossless_and_recallable(env):
    e = engine()
    e.update_model("claude-sonnet-5", 12000)
    e.on_session_start("sess-1")
    msgs = transcript()
    out = e.compress(copy.deepcopy(msgs))
    assert len(out) == len(msgs)
    assert [m["role"] for m in out] == [m["role"] for m in msgs]
    assert [m.get("tool_calls") for m in out] == [m.get("tool_calls") for m in msgs]
    stubs = [m for m in out if isinstance(m.get("content"), str) and m["content"].startswith("[kritic archived")]
    assert stubs, "something was archived"
    assert e.compression_count == 1 and e.last_error is None
    stub = stubs[0]["content"]
    rid = stub.split(" as ", 1)[1].split(".", 1)[0]
    original = msgs[out.index(stubs[0])]["content"]
    got = json.loads(e.handle_tool_call("kritic_recall", {"id": rid}))
    assert got["found"][0]["content"] == original
    found = json.loads(e.handle_tool_call("kritic_recall", {"query": "f0.rs"}))
    assert found["found"]
    assert "error" in json.loads(e.handle_tool_call("nope", {}))


def test_prune_and_usage_log(env):
    e = engine()
    e.update_model("m", 1_000_000)
    e.on_session_start("sess-2")
    msgs = transcript()
    assert msgs[3]["role"] == "tool"
    msgs[3]["content"] = "x" * 9000  # an oversized tool result
    out, n = e.prune_tool_results_only(copy.deepcopy(msgs))
    assert n == 1 and out[3]["content"].startswith("[kritic archived")
    e.update_from_response({"prompt_tokens": 100, "completion_tokens": 5, "total_tokens": 105})
    log = env / "kritic" / "logs" / "hermes-usage.jsonl"
    row = json.loads(log.read_text(encoding="utf-8").splitlines()[-1])
    assert row["session_id"] == "sess-2" and row["prompt_tokens"] == 100


def test_explicit_invalid_kritic_selection_fails_closed(env, monkeypatch):
    monkeypatch.setenv("KRITIC_EXE", str(env / "missing.exe"))
    monkeypatch.setenv("PATH", str(env / "empty"))
    mod = load_plugin()
    monkeypatch.setattr(mod, "_HERE", env)  # no kritic_exe.txt either
    ctx = Ctx()
    mod.register(ctx)
    e = ctx.engine
    e.update_model("m", 12000)
    msgs = transcript()
    with pytest.raises(ValueError, match="fallback disabled"):
        e.compress(msgs)
    with pytest.raises(ValueError, match="fallback disabled"):
        e.handle_tool_call("kritic_recall", {"id": "kr-1"})


def test_settings_prefers_hermes_yaml_without_pyyaml(env, monkeypatch):
    parsed = []
    parser = types.ModuleType("hermes_yaml")

    def safe_load(text):
        parsed.append(text)
        return {"kritic_context": {"use_kev": False, "protect_first_n": 4}}

    parser.safe_load = safe_load
    monkeypatch.setitem(sys.modules, "hermes_yaml", parser)
    monkeypatch.setitem(sys.modules, "yaml", None)
    settings = load_plugin()._settings()
    assert settings["protect_first_n"] == 4
    assert settings["use_kev"] is False
    assert len(parsed) == 1 and "kritic_context:" in parsed[0]


def test_absolute_budget_survives_model_switches_and_clone(env, monkeypatch):
    import hermes_constants
    config = Path(hermes_constants.get_hermes_home()) / "config.yaml"
    config.write_text("compression:\n  threshold_tokens: 28000\nkritic_context:\n  use_kev: false\n", encoding="utf-8")
    e = engine()
    payloads = []
    monkeypatch.setattr(e, "_run", lambda op, payload: payloads.append(payload))
    original = transcript()
    snapshot = copy.deepcopy(original)
    for window, trigger, target in [(1_000_000, 28000, 19600), (65536, 28000, 19600), (32000, 16000, 11200)]:
        e.update_model("test-model", window)
        assert e.context_length == window
        assert not e.should_compress(trigger - 1)
        assert e.should_compress(trigger)
        assert e.compress(original) == snapshot  # failed backend never destroys history
        assert payloads[-1]["target_tokens"] == target < trigger
    clone = e.clone_for_agent()
    clone.update_model("large-model", 1_000_000)
    assert clone.threshold_tokens == 28000
    assert e.threshold_tokens == 16000
    assert original == snapshot


@pytest.mark.parametrize("cap", [True, False, 0, 1, -1, 28000.0, "28000"])
def test_invalid_absolute_budget_fails_closed(env, cap):
    import hermes_constants
    config = Path(hermes_constants.get_hermes_home()) / "config.yaml"
    config.write_text(json.dumps({"compression": {"threshold_tokens": cap}}), encoding="utf-8")
    with pytest.raises(ValueError):
        engine()


def test_whole_request_overhead_reduces_message_budget(env, monkeypatch):
    e = engine()
    e.update_model("fixture", 40000)
    messages = transcript()
    mod = load_plugin()
    message_tokens = mod._rough_tokens(messages)
    payloads = []
    monkeypatch.setattr(e, "_run", lambda op, payload: payloads.append(payload))
    e.compress(messages, current_tokens=message_tokens + 9000)
    assert payloads[-1]["target_tokens"] == 5000  # 14k total target minus 9k fixed overhead
    e.prune_tool_results_only(messages, current_tokens=message_tokens + 9000)
    assert payloads[-1]["target_tokens"] == 5000
    e.compress(messages, current_tokens=message_tokens + 20000)
    assert payloads[-1]["target_tokens"] == 1  # infeasible protected budgets remain bounded


def test_schema_heavy_request_actually_shrinks_with_real_backend(env):
    e = engine()
    e.update_model("fixture", 40000)
    messages = transcript()
    estimate = load_plugin()._rough_tokens(messages)
    assert estimate < 14000
    result = e.compress(messages, current_tokens=estimate + 12000)
    after = load_plugin()._rough_tokens(result)
    assert after < estimate
    assert e.last_prompt_tokens >= 12000  # fixed tool/system cost is not erased by archiving
    assert [m.get("tool_call_id") for m in result if m["role"] == "tool"] == [
        m["tool_call_id"] for m in messages if m["role"] == "tool"]


def test_null_absolute_budget_preserves_window_policy(env):
    import hermes_constants
    config = Path(hermes_constants.get_hermes_home()) / "config.yaml"
    config.write_text('compression: {threshold_tokens: null}\n', encoding="utf-8")
    e = engine()
    e.update_model("test-model", 1_000_000)
    assert e.context_length == 1_000_000
    assert e.threshold_tokens == 500000
