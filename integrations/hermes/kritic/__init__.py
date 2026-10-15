"""kritic context engine for Hermes Agent.

Replaces Hermes' summarising compressor with kritic's lossless, extractive one. When a
conversation has to shrink, whole middle messages are archived to disk and replaced by a short stub
naming an id; the agent reads any of them back with the ``kritic_recall`` tool. Nothing is
summarised, so nothing the model sees was written by another model.

This module is a thin shim: every decision is made by the ``kritic`` binary (``kritic context``),
which ranks messages with Kev when a Kev backend is available and oldest-first otherwise. The shim
fails open: if kritic is missing or errors, messages pass through unchanged and the error is logged.

Activate with ``context.engine: kritic`` in Hermes' ``config.yaml`` (``kritic register --hermes
--hermes-context`` does this). Optional settings, all with defaults, under a top-level
``kritic_context:`` section of the same file:

    kritic_context:
      threshold: 0.5        # compact at this share of the model's context window
      target: 0.35          # ...down to this share
      protect_first_n: 3    # non-system head messages kept verbatim
      protect_last_n: 6     # tail messages kept verbatim
      big_chars: 8000       # tool results above this are archived when pruning
      use_kev: true         # rank by relevance with Kev; false = oldest first
      timeout_s: 90         # one kritic call

The host's ``compression.threshold_tokens`` is also an absolute trigger ceiling
(an integer greater than one). Kritic scales its target proportionally when that
ceiling applies, while preserving the real model context length. Built-in tail
retention settings remain separate.

Concise replies: when ``output.style = "concise"`` in kritic.toml (the default), ``kritic register``
writes ``concise.md`` beside this file and the plugin adds it to each turn through Hermes'
``pre_llm_call`` hook (about 70 tokens per turn). Delete the file, or set ``output.style =
"default"`` and re-register, to turn it off.
"""

from __future__ import annotations

import copy
import json
import logging
import os
import shutil
import subprocess
from pathlib import Path
from typing import Any, Dict, List, Optional

from agent.context_engine import ContextEngine

logger = logging.getLogger(__name__)

_HERE = Path(__file__).resolve().parent

RECALL_TOOL = {
    "name": "kritic_recall",
    "description": (
        "Read back conversation content that kritic archived to keep the context small. "
        "Archive stubs look like `[kritic archived this tool message (...) as kr-...]`. "
        "Pass the stub's id for that message, or a query to search every archived message."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "id": {"type": "string", "description": "An archive id such as kr-1a2b3c4d5e6f"},
            "query": {"type": "string", "description": "Case-insensitive text to search archived content for"},
            "max_chars": {"type": "integer", "description": "Most characters to return (default 20000)"},
        },
    },
}


def _kritic_exe() -> Optional[str]:
    """KRITIC_EXE, else the path `kritic register` recorded beside this file, else PATH."""
    env = os.environ.get("KRITIC_EXE")
    if env and Path(env).is_file():
        return env
    recorded = _HERE / "kritic_exe.txt"
    if recorded.is_file():
        path = recorded.read_text(encoding="utf-8").strip()
        if path and Path(path).is_file():
            return path
    return shutil.which("kritic")


def _kritic_home() -> Path:
    return Path(os.environ.get("KRITIC_HOME") or Path.home() / ".kritic")


def _settings() -> Dict[str, Any]:
    defaults = {"threshold": 0.5, "target": 0.35, "protect_first_n": 3, "protect_last_n": 6,
                "big_chars": 8000, "use_kev": True, "timeout_s": 90}
    cfg = {}
    try:
        import yaml  # Hermes depends on PyYAML

        try:
            from hermes_constants import get_hermes_home  # knows the Windows location too

            home = Path(get_hermes_home())
        except Exception:
            home = Path(os.environ.get("HERMES_HOME") or Path.home() / ".hermes")
        cfg = yaml.safe_load((home / "config.yaml").read_text(encoding="utf-8")) or {}
        defaults.update({k: v for k, v in (cfg.get("kritic_context") or {}).items() if k in defaults})
    except Exception as exc:  # a missing or odd config keeps the defaults
        logger.debug("kritic: using default settings (%s)", exc)
    compression = cfg.get("compression", {})
    if compression is None:
        compression = {}
    if not isinstance(compression, dict):
        raise ValueError("compression must be an object")
    cap = compression.get("threshold_tokens")
    if cap is not None and (type(cap) is not int or cap < 2):
        raise ValueError("compression.threshold_tokens must be an integer greater than one")
    defaults["threshold_tokens_cap"] = cap
    return defaults


def _rough_tokens(messages: List[Dict[str, Any]]) -> int:
    total = 0
    for m in messages:
        c = m.get("content")
        if isinstance(c, list):
            c = "\n".join(p.get("text", "") for p in c if isinstance(p, dict))
        total += len(c or "") // 4 + len(json.dumps(m.get("tool_calls") or "")) // 4 + 4
    return total


class KriticContextEngine(ContextEngine):
    """Lossless extractive compaction with recall, backed by the kritic binary."""

    def __init__(self) -> None:
        s = _settings()
        self.threshold_percent = float(s["threshold"])
        self.target_percent = float(s["target"])
        self.threshold_tokens_cap = s.get("threshold_tokens_cap")
        self.protect_first_n = int(s["protect_first_n"])
        self.protect_last_n = int(s["protect_last_n"])
        self.big_chars = int(s["big_chars"])
        self.use_kev = bool(s["use_kev"])
        self.timeout_s = float(s["timeout_s"])
        self.session_id = "default"
        self.archived_total = 0
        self.last_error: Optional[str] = None
        # Successful compaction is routine maintenance: stay quiet unless something goes wrong.
        self.emit_automatic_compaction_status = False

    @property
    def name(self) -> str:
        return "kritic"

    # ---------------------------------------------------------------- kritic calls

    def _run(self, op: str, payload: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        exe = _kritic_exe()
        if not exe:
            self.last_error = "kritic binary not found (set KRITIC_EXE or run `kritic register --hermes`)"
            logger.warning("kritic: %s", self.last_error)
            return None
        try:
            proc = subprocess.run(
                [exe, "context", op], input=json.dumps(payload), capture_output=True, text=True,
                encoding="utf-8", timeout=self.timeout_s,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            self.last_error = f"kritic context {op}: {exc}"
            logger.warning("kritic: %s", self.last_error)
            return None
        if proc.returncode != 0:
            self.last_error = f"kritic context {op} exited {proc.returncode}: {proc.stderr.strip()[:500]}"
            logger.warning("kritic: %s", self.last_error)
            return None
        try:
            self.last_error = None
            return json.loads(proc.stdout)
        except json.JSONDecodeError as exc:
            self.last_error = f"kritic context {op}: bad JSON ({exc})"
            logger.warning("kritic: %s", self.last_error)
            return None

    def _shrink(self, messages: List[Dict[str, Any]], mode: str, focus: Optional[str] = None,
                current_tokens: Optional[int] = None) -> Optional[Dict[str, Any]]:
        target = int(self.context_length * self.target_percent) if self.context_length else 0
        if self.threshold_tokens_cap is not None and self.threshold_tokens:
            # Preserve retention without overstating actual model capacity.
            target = min(target, int(self.threshold_tokens * self.target_percent / self.threshold_percent))
        if self.threshold_tokens > 1:
            target = min(target, self.threshold_tokens - 1)
        # Hermes supplies the whole-request estimate; the backend receives only messages.
        # Reserve the system/tool overhead instead of spending the full budget twice.
        if type(current_tokens) is int and current_tokens >= 0:
            target -= max(0, current_tokens - _rough_tokens(messages))
        return self._run("compress", {
            "messages": messages, "target_tokens": max(target, 1), "session_id": self.session_id,
            "focus": focus, "protect_first_n": self.protect_first_n, "protect_last_n": self.protect_last_n,
            "big_chars": self.big_chars, "mode": mode, "use_kev": self.use_kev,
            "rank_deadline_s": max(5, int(self.timeout_s * 0.6)),
        })

    # ---------------------------------------------------------------- ContextEngine

    def update_model(self, *args: Any, **kwargs: Any) -> None:
        super().update_model(*args, **kwargs)
        if self.threshold_tokens_cap is not None:
            self.threshold_tokens = min(self.threshold_tokens, self.threshold_tokens_cap)

    def update_from_response(self, usage: Dict[str, Any]) -> None:
        self.last_prompt_tokens = int(usage.get("prompt_tokens") or 0)
        self.last_completion_tokens = int(usage.get("completion_tokens") or 0)
        self.last_total_tokens = int(usage.get("total_tokens") or 0)
        self._log_usage(usage)

    def _log_usage(self, usage: Dict[str, Any]) -> None:
        """One line per model call for kritic's token_metrics (best effort, never raises).

        Logged here rather than in on_turn_complete: every Hermes release calls this hook.
        """
        try:
            log = _kritic_home() / "logs" / "hermes-usage.jsonl"
            log.parent.mkdir(parents=True, exist_ok=True)
            row = {"session_id": self.session_id, "context_length": self.context_length,
                   "compression_count": self.compression_count, "archived_total": self.archived_total,
                   **{k: usage.get(k) for k in ("prompt_tokens", "completion_tokens", "total_tokens",
                                                 "cache_read_tokens", "cache_write_tokens", "reasoning_tokens")}}
            with log.open("a", encoding="utf-8") as f:
                f.write(json.dumps(row) + "\n")
        except Exception as exc:
            logger.debug("kritic: usage not logged (%s)", exc)

    def should_compress(self, prompt_tokens: int = None) -> bool:
        tokens = prompt_tokens if prompt_tokens is not None else self.last_prompt_tokens
        return bool(self.threshold_tokens) and tokens >= self.threshold_tokens

    def should_compress_preflight(self, messages: List[Dict[str, Any]]) -> bool:
        return bool(self.threshold_tokens) and _rough_tokens(messages) >= self.threshold_tokens

    def compress(self, messages: List[Dict[str, Any]], current_tokens: Optional[int] = None,
                 focus_topic: Optional[str] = None, force: bool = False, memory_context: str = "") -> List[Dict[str, Any]]:
        result = self._shrink(messages, "compress", focus_topic, current_tokens)
        if not result or not isinstance(result.get("messages"), list):
            return messages
        self.compression_count += 1
        self.archived_total += len(result.get("archived") or [])
        overhead = max(0, current_tokens - _rough_tokens(messages)) if type(current_tokens) is int and current_tokens >= 0 else 0
        self.last_prompt_tokens = int(result.get("tokens_after") or 0) + overhead
        return result["messages"]

    def prune_tool_results_only(self, messages: List[Dict[str, Any]], current_tokens: int = None):
        result = self._shrink(messages, "prune", current_tokens=current_tokens)
        if not result or not isinstance(result.get("messages"), list):
            return messages, 0
        n = len(result.get("archived") or [])
        self.archived_total += n
        return result["messages"], n

    def has_content_to_compress(self, messages: List[Dict[str, Any]]) -> bool:
        return len(messages) > self.protect_first_n + self.protect_last_n + 1

    def on_session_start(self, session_id: str, **kwargs) -> None:
        self.session_id = session_id or "default"

    def get_tool_schemas(self) -> List[Dict[str, Any]]:
        return [RECALL_TOOL]

    def handle_tool_call(self, name: str, args: Dict[str, Any], **kwargs) -> str:
        if name != "kritic_recall":
            return json.dumps({"error": f"Unknown context engine tool: {name}"})
        result = self._run("recall", {"session_id": self.session_id, "id": args.get("id"),
                                      "query": args.get("query"), "max_chars": int(args.get("max_chars") or 20000)})
        return json.dumps(result if result is not None else {"error": self.last_error or "kritic unavailable"})

    def clone_for_agent(self) -> "KriticContextEngine":
        """Each agent gets its own budget; the archive on disk is shared by session id.

        Defined here so older Hermes releases, whose base class lacks it, behave the same.
        """
        return copy.deepcopy(self)

    def get_status(self) -> Dict[str, Any]:
        status = super().get_status()
        status.update({"engine": "kritic", "archived_total": self.archived_total, "last_error": self.last_error})
        return status


def _concise_rule() -> str:
    try:
        return (_HERE / "concise.md").read_text(encoding="utf-8").strip()
    except OSError:
        return ""


def _on_pre_llm_call(**_kwargs: Any) -> Optional[Dict[str, str]]:
    rule = _concise_rule()
    return {"context": rule} if rule else None


def register(ctx) -> None:
    ctx.register_context_engine(KriticContextEngine())
    if _concise_rule() and hasattr(ctx, "register_hook"):
        ctx.register_hook("pre_llm_call", _on_pre_llm_call)
