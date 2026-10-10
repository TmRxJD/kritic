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
    assert isinstance(ctx.engine, ContextEngine)  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    return ctx.engine


def test_recall_forwards_options_and_bounds_python_serialization(monkeypatch):
    e = engine()
    calls = []

    def recall_result(op, payload):
        calls.append((op, payload))
        return {"found": [{"original_content": "🦀" * 10000}]}

    monkeypatch.setattr(e, "_run", recall_result)
    result = e.handle_tool_call("kritic_recall", {"id": "kr-test", "format": "original", "max_chars": 0, "max_output_bytes": 256})
    assert len(result.encode("utf-8")) <= 256  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "error" in json.loads(result)  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert calls[0][1]["format"] == "original"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert calls[0][1]["max_chars"] == 0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert calls[0][1]["max_output_bytes"] == 256  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_contract_and_thresholds(env):
    e = engine()
    assert e.name == "kritic"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    e.update_model("claude-sonnet-5", 20000)
    assert e.context_length == 20000 and e.threshold_tokens == 10000  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    e.update_from_response({"prompt_tokens": 9000, "completion_tokens": 10, "total_tokens": 9010})
    assert not e.should_compress()  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.should_compress(12000)  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.should_compress_preflight(transcript()) is False  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert [t["name"] for t in e.get_tool_schemas()] == ["kritic_recall"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    clone = e.clone_for_agent()
    clone.update_model("other", 1000)
    assert e.context_length == 20000, "a clone's budget is its own"  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_compress_is_lossless_and_recallable(env):
    e = engine()
    e.update_model("claude-sonnet-5", 12000)
    e.on_session_start("sess-1")
    msgs = transcript()
    out = e.compress(copy.deepcopy(msgs))
    assert len(out) == len(msgs)  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert [m["role"] for m in out] == [m["role"] for m in msgs]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert [m.get("tool_calls") for m in out] == [m.get("tool_calls") for m in msgs]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    stubs = [m for m in out if isinstance(m.get("content"), str) and m["content"].startswith("[kritic archived")]
    assert stubs, "something was archived"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.compression_count == 1 and e.last_error is None  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    stub = stubs[0]["content"]
    rid = stub.split(" as ", 1)[1].split(".", 1)[0]
    original = msgs[out.index(stubs[0])]["content"]
    got = json.loads(e.handle_tool_call("kritic_recall", {"id": rid}))
    assert got["found"][0]["content"] == original  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    found = json.loads(e.handle_tool_call("kritic_recall", {"query": "f0.rs"}))
    assert found["found"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "error" in json.loads(e.handle_tool_call("nope", {}))  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_prune_and_usage_log(env):
    e = engine()
    e.update_model("m", 1_000_000)
    e.on_session_start("sess-2")
    msgs = transcript()
    assert msgs[3]["role"] == "tool"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    msgs[3]["content"] = "x" * 9000  # an oversized tool result
    out, n = e.prune_tool_results_only(copy.deepcopy(msgs))
    assert n == 1 and out[3]["content"].startswith("[kritic archived")  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    e.update_from_response({"prompt_tokens": 100, "completion_tokens": 5, "total_tokens": 105})
    log = env / "kritic" / "logs" / "hermes-usage.jsonl"
    row = json.loads(log.read_text(encoding="utf-8").splitlines()[-1])
    assert row["session_id"] == "sess-2" and row["prompt_tokens"] == 100  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_invalid_explicit_kritic_exe_is_configuration_error(env, monkeypatch):
    monkeypatch.setenv("KRITIC_EXE", str(env / "missing.exe"))
    monkeypatch.setenv("PATH", str(env / "empty"))
    mod = load_plugin()
    monkeypatch.setattr(mod, "_HERE", env)  # no kritic_exe.txt either
    ctx = Ctx()
    mod.register(ctx)
    e = ctx.engine
    e.update_model("m", 12000)
    msgs = transcript()
    with pytest.raises(ValueError, match="KRITIC_EXE must be an existing absolute file"):
        e.compress(msgs)


def test_fails_open_without_implicit_kritic(env, monkeypatch):
    monkeypatch.delenv("KRITIC_EXE", raising=False)
    monkeypatch.setenv("PATH", str(env / "empty"))
    mod = load_plugin()
    monkeypatch.setattr(mod, "_HERE", env)  # no kritic_exe.txt either
    ctx = Ctx()
    mod.register(ctx)
    e = ctx.engine
    e.update_model("m", 12000)
    msgs = transcript()
    assert e.compress(msgs) == msgs  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "not found" in e.last_error  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert json.loads(e.handle_tool_call("kritic_recall", {"id": "kr-1"}))["error"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
