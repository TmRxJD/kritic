import argparse
import json
import os
from pathlib import Path
import shutil
# The benchmark launches the user-selected client with fixed argv and no shell.
import subprocess  # nosec B404
import sys
import time

parser = argparse.ArgumentParser(description="Compare fresh Claude Code sessions on synthetic Git/CI evidence.")
parser.add_argument("--scratch", type=Path, required=True, help="Existing task scratch directory")
parser.add_argument("--out", type=Path, required=True)
parser.add_argument("--claude", default="claude")
parser.add_argument("--reverse", action="store_true")
parser.add_argument("--mode", choices=("direct", "deferred", "batch", "portable"), help="Run only this variant")
args = parser.parse_args()
scratch = args.scratch.resolve(strict=True)
out = args.out.resolve()
out.mkdir(parents=True, exist_ok=True)
client = shutil.which(args.claude)
if client is None:
    parser.error("Claude CLI not found. Pass --claude with the trusted client executable.")
claude = Path(client).resolve(strict=True)
if not claude.is_file() or (os.name == "nt" and claude.suffix.lower() != ".exe"):
    parser.error("The selected Claude client must be an executable file, not a wrapper script.")
fixture = Path(__file__).with_name("tool_efficiency_fixture.py").resolve(strict=True)
config = json.dumps({"mcpServers": {"fixture": {"command": sys.executable, "args": [str(fixture)]}}})
prompt = 'Use the fixture MCP tools to inspect the synthetic pull request. Return the current PR head, failed check name, and exact error as JSON. Use the fixture as the only source.'
cases = [("direct", "false", "direct"), ("deferred", "true", "direct"), ("batch", "true", "batch"), ("portable", "false", "portable")]
if args.mode:
    cases = [case for case in cases if case[0] == args.mode]
failed = False
for label, deferral, mode in (reversed(cases) if args.reverse else cases):
    env = {**os.environ, "ENABLE_TOOL_SEARCH": deferral, "PROBE_MODE": mode, "TEMP": str(scratch), "TMP": str(scratch)}
    for key in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"]:
        env.pop(key, None)
    command = [str(claude), "-p", "--model", "sonnet", "--effort", "low", "--strict-mcp-config", "--mcp-config", config, "--setting-sources", "", "--settings", '{"disableAllHooks":true}', "--tools", "ToolSearch", "--allowedTools", "mcp__fixture__*", "ToolSearch", "--no-session-persistence", "--verbose", "--output-format", "stream-json"]
    start = time.monotonic()
    try:
        cp = subprocess.run(command, input=prompt, text=True, encoding="utf-8", errors="replace", capture_output=True, cwd=scratch, env=env, timeout=105, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))  # nosec B603
        (out / (label + ".jsonl")).write_text(cp.stdout, encoding="utf-8")
        (out / (label + ".stderr.log")).write_text(cp.stderr, encoding="utf-8")
        events = [json.loads(line) for line in cp.stdout.splitlines() if line.startswith("{")]
        terminal = next((x for x in reversed(events) if x.get("type") == "result"), {})
        calls = [b["name"] for row in events if row.get("type") == "assistant" for b in row.get("message", {}).get("content", []) if b.get("type") == "tool_use"]
        schemas = [b for row in events if row.get("type") == "user" for b in row.get("message", {}).get("content", []) if b.get("type") == "tool_result" and "tool_reference" in json.dumps(b)]
        result = {"mode": label, "exit": cp.returncode, "seconds": round(time.monotonic()-start, 2), "calls": calls, "tool_reference_results": len(schemas), "usage": terminal.get("usage"), "num_turns": terminal.get("num_turns"), "model_usage": terminal.get("modelUsage"), "answer": terminal.get("result"), "error": terminal.get("is_error"), "stderr": cp.stderr[-500:]}
        answer = (terminal.get("result") or "").strip()
        if answer.startswith("```json") and answer.endswith("```"):
            answer = answer[7:-3]
        try:
            evidence = json.loads(answer)
        except ValueError:
            evidence = {}
        usage = terminal.get("usage") or {}
        result["input_including_cache"] = sum(usage.get(key, 0) for key in (
            "input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
        result["verified"] = (
            cp.returncode == 0 and not terminal.get("is_error")
            and isinstance(evidence, dict)
            and all(evidence.get(key) == expected for key, expected in {
                "head": "a123", "failed_check": "Windows tests", "error": "fixture.rs:17: expected 3, got 2",
            }.items())
            and bool(schemas) == (deferral == "true")
            and sum(call.startswith("mcp__fixture__") for call in calls) == {"batch": 1, "portable": 2, "direct": 3}[mode]
        )
    except subprocess.TimeoutExpired:
        result = {"mode": label, "error": "timed out after 105 seconds"}
    (out / (label + ".json")).write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result), flush=True)
    failed |= not result.get("verified", False)
sys.exit(int(failed))
