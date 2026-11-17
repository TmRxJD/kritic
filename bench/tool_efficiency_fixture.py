import json
import os
from pathlib import Path
import sys

mode = os.environ.get("PROBE_MODE", "direct")
check = {"name": "Windows tests", "status": "failure", "id": 27}
detail = "fixture.rs:17: expected 3, got 2"
tools = [
    {"name": "pr_head", "description": "Get current pull request head", "inputSchema": {"type": "object"}},
    {"name": "ci_checks", "description": "List CI checks for a verified head; use this first after pr_head", "inputSchema": {"type": "object", "properties": {"head": {"type": "string"}}, "required": ["head"]}},
    {"name": "ci_log", "description": "Read failed check evidence using its ID", "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}},
]
if mode == "batch":
    tools = [{"name": "ci_inspect", "description": "Get pull request head, failure name and exact error in one call", "inputSchema": {"type": "object"}}]
elif mode == "portable":
    tools = [
        {"name": "workflow_discover", "description": "Find the read-only workflow for Git/CI inspection and its arguments", "inputSchema": {"type": "object"}},
        {"name": "workflow_run", "description": "Run a named read-only workflow after discovery", "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "arguments": {"type": "object"}}, "required": ["name", "arguments"]}},
    ]
for i in range(24):
    tools.append({"name": f"unrelated_{i}", "description": f"Unrelated inventory operation {i} for synthetic bench", "inputSchema": {"type": "object", "properties": {f"field_{j}": {"type": "string", "description": f"Inventory audit metadata for unrelated operation {i}, field {j}. Provide the full record identifier when available."} for j in range(9)}}})

for line in sys.stdin:
    query = json.loads(line)
    if "id" not in query:
        continue
    method = query["method"]
    if method == "initialize":
        out = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "tool-efficiency-fixture", "version": "1"}}
    elif method == "tools/list":
        out = {"tools": tools}
    elif method == "tools/call":
        name = query["params"]["name"]
        args = query["params"].get("arguments", {})
        if name == "pr_head":
            value = {"head": "a123"}
        elif name == "ci_checks" and args.get("head") == "a123":
            value = {"checks": [check] + [{"name": f"Passing check {i}", "status": "success", "id": i + 100} for i in range(40)]}
        elif name == "ci_log" and args.get("id") == 27:
            value = {"excerpt": detail, "log": "\n".join(f"INFO compile component {i}: complete" for i in range(100))}
        elif name == "ci_inspect":
            value = {"head": "a123", "failure": check, "excerpt": detail}
        elif name == "workflow_discover":
            value = {"name": "git_ci_snapshot", "input_schema": {"type": "object", "properties": {"pr": {"type": "integer", "const": 36}}, "required": ["pr"]}}
        elif name == "workflow_run" and args == {"name": "git_ci_snapshot", "arguments": {"pr": 36}}:
            value = {"head": "a123", "failure": check, "excerpt": detail}
        else:
            value = {"error": "unexpected call"}
        out = {"content": [{"type": "text", "text": json.dumps(value)}], "isError": "error" in value}
    else:
        out = {}
    print(json.dumps({"jsonrpc": "2.0", "id": query["id"], "result": out}), flush=True)
