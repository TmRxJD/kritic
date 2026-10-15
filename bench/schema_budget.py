#!/usr/bin/env python3
"""Measure MCP discovery metadata without model inference or review-tool calls.

Standard-library only; optional cached tiktoken counts are a reference, not billing.
Collect saves raw JSON-RPC responses; compare checks actual wire schemas and tool names.
Host-style numbers simulate server instructions repeated in every tool description.
"""
import argparse
import copy
import hashlib
import json
import os
import pathlib
import queue
# Explicitly user-selected executable; validated regular absolute path; no shell.
import subprocess  # nosec B404
import threading
import time


def serialized(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def tokenizer_for(name):
    if not name:
        return None, None
    try:
        import tiktoken
        import tiktoken.load

        original = tiktoken.load.read_file

        def cached_only(path):
            if path.startswith(("https://", "http://")):
                raise RuntimeError("Encoding cache missing; this benchmark never downloads tables")
            return original(path)

        tiktoken.load.read_file = cached_only
        try:
            return tiktoken.get_encoding(name), None
        finally:
            tiktoken.load.read_file = original
    except Exception as error:
        return None, str(error)


def measure(value, encoding):
    text = serialized(value)
    return {"utf8_bytes": len(text.encode()), "characters": len(text),
            "reference_tokens": len(encoding.encode(text, disallowed_special=())) if encoding else None}


def discovery_env(base):
    # Server warm-up may check health; ownership overrides prevent starting/evicting models.
    return dict(base, KRITIC_KEV_EXTERNALLY_MANAGED="true", KRITIC_VISION_EXTERNALLY_MANAGED="true")


def validated_binary(value, windows=None):
    path = pathlib.Path(value).expanduser().resolve(strict=True)
    if not path.is_file():
        raise ValueError("Binary must be a regular file: " + str(path))
    if (os.name == "nt" if windows is None else windows) and path.suffix.lower() != ".exe":
        raise ValueError("Windows discovery requires an .exe file, not a batch/shell script")
    return str(path)


def minimal_env(base, profile, folder):
    env = base.copy()
    for key in list(env):
        if key.startswith("KRITIC_CAP_") or key in (
                "KRITIC_BACKEND", "KRITIC_KEV_BACKEND", "KRITIC_VISION_BACKEND", "KRITIC_DISABLED_TOOLS"):
            env.pop(key)
    env.update(KRITIC_CONFIG=str(profile.resolve()), KRITIC_HOME=str((folder / "home").resolve()),
               KRITIC_RUNS_DIR=str((folder / "runs").resolve()))
    return env


def collect(args):
    binary = validated_binary(args.binary)
    if args.config and not args.config.is_file():
        raise ValueError("Config file does not exist: " + str(args.config))
    if not args.minimal_profile.is_file():
        raise ValueError("Minimal profile does not exist: " + str(args.minimal_profile))
    args.output.mkdir(parents=True, exist_ok=True)
    encoding, error = tokenizer_for(args.tokenizer)
    profiles = []
    for profile in ("current", "minimal"):
        folder = args.output / profile
        folder.mkdir(exist_ok=True)
        env = discovery_env(os.environ)
        if args.config:
            env["KRITIC_CONFIG"] = str(args.config.resolve())
        if profile == "minimal":
            env = minimal_env(env, args.minimal_profile, folder)
        stderr = (folder / "stderr.log").open("wb")
        # Explicitly user-selected executable; validated regular absolute path; no shell.
        process = subprocess.Popen(  # nosec B603
                                   [binary, *args.arg], env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=stderr, shell=False,
                                   creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        messages = queue.Queue()

        def read(stream=process.stdout, destination=messages):
            for line in stream:
                destination.put(line)
            destination.put(None)

        threading.Thread(target=read, daemon=True).start()

        def send(value):
            process.stdin.write((serialized(value) + "\n").encode())
            process.stdin.flush()

        def call(identifier, method, params):
            send({"jsonrpc": "2.0", "id": identifier, "method": method, "params": params})
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                line = messages.get(timeout=max(.01, deadline - time.monotonic()))
                if line is None:
                    raise RuntimeError("MCP server exited before responding")
                if len(line) > 10 * 1024 * 1024:
                    raise RuntimeError("MCP discovery response exceeds 10 MiB")
                value = json.loads(line)
                if value.get("id") == identifier:
                    if "error" in value:
                        raise RuntimeError(serialized(value["error"]))
                    return value["result"], line
            raise RuntimeError("MCP discovery timed out")

        try:
            initialized, raw = call(1, "initialize", {
                "protocolVersion": "2025-03-26", "capabilities": {},
                "clientInfo": {"name": "kritic-schema-budget", "version": "1"}})
            (folder / "initialize-response.json").write_bytes(raw)
            send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            tools, pages, cursor = [], [], None
            for page in range(20):
                result, raw = call(page + 2, "tools/list", {"cursor": cursor} if cursor else {})
                (folder / f"tools-list-page-{page}.json").write_bytes(raw)
                pages.append(raw)
                tools.extend(result["tools"])
                cursor = result.get("nextCursor")
                if not cursor:
                    break
            if cursor or not tools or len({tool["name"] for tool in tools}) != len(tools):
                raise RuntimeError("Incomplete, empty or duplicate tool discovery")
            (folder / "tools-list.json").write_text(serialized({"tools": tools}) + "\n", encoding="utf-8")
            instructions = initialized.get("instructions", "")
            host = copy.deepcopy(tools)
            if instructions:
                for tool in host:
                    tool["description"] = instructions + ("\n\n" + tool["description"] if tool.get("description") else "")
            fields = [{"name": tool["name"], "whole_tool": measure(tool, encoding), **{
                key: measure(tool[key], encoding) if key in tool else None
                for key in ("inputSchema", "description", "outputSchema")}} for tool in tools]
            summary = {"profile": profile, "tool_count": len(tools), "pages": len(pages),
                       "tools_list_result": measure({"tools": tools}, encoding),
                       "tools_list_wire_bytes": sum(len(page) for page in pages),
                       "initialize_result": measure(initialized, encoding),
                       "server_instructions": measure(instructions, encoding) if instructions else None,
                       "host_style_repeated_prefix": measure({"tools": host}, encoding),
                       "per_tool": sorted(fields, key=lambda tool: tool["whole_tool"]["utf8_bytes"], reverse=True)}
            (folder / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
            profiles.append(summary)
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
            stderr.close()
    report = {"binary": binary, "config": str(args.config.resolve()) if args.config else None,
              "minimal_profile": str(args.minimal_profile.resolve()), "tokenizer": args.tokenizer if encoding else None,
              "benchmark_lifecycle_overrides": {"KRITIC_KEV_EXTERNALLY_MANAGED": True, "KRITIC_VISION_EXTERNALLY_MANAGED": True},
              "tokenizer_error": error, "profiles": profiles,
              "limits": "Reference tokens are not Codex billing; fields are independently tokenized. Host-style simulation excludes unknown host wrappers."}
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(args.output), "tokenizer": report["tokenizer"], "profiles": [
        {key: profile[key] for key in ("profile", "tool_count", "tools_list_result", "host_style_repeated_prefix")}
        for profile in profiles]}))


def constraint_projection(schema):
    """Ignore permitted annotations only at schema nodes, never inside literal values/maps."""
    if not isinstance(schema, dict):
        return schema
    result = copy.deepcopy(schema)
    for key in ("title", "examples"):
        result.pop(key, None)
    if "default" in result:
        value = result["default"]
        if value is None or value == "" or isinstance(value, (dict, list)) and not value:
            result.pop("default")
    for key in ("properties", "patternProperties", "$defs", "definitions", "dependentSchemas"):
        if isinstance(result.get(key), dict):
            result[key] = {name: constraint_projection(value) for name, value in result[key].items()}
    for key in ("allOf", "anyOf", "oneOf", "prefixItems"):
        if isinstance(result.get(key), list):
            result[key] = [constraint_projection(value) for value in result[key]]
    for key in ("items", "additionalItems", "contains", "not", "if", "then", "else",
                "additionalProperties", "unevaluatedProperties", "unevaluatedItems", "propertyNames", "contentSchema"):
        if key in result:
            result[key] = constraint_projection(result[key])
    return result


def compare(args):
    results = []
    for profile in ("current", "minimal"):
        before = read_capture(args.baseline / profile / "tools-list.json")
        after = read_capture(args.candidate / profile / "tools-list.json")
        left = {tool["name"]: tool for tool in before}
        right = {tool["name"]: tool for tool in after}
        failures = []
        if set(left) != set(right):
            failures.append({"tool_names_changed": {"removed": sorted(set(left)-set(right)), "added": sorted(set(right)-set(left))}})
        for name in sorted(set(left) & set(right)):
            for field in ("inputSchema", "outputSchema"):
                if constraint_projection(left[name].get(field)) != constraint_projection(right[name].get(field)):
                    failures.append({"tool": name, "field": field, "error": "constraints, parameter descriptions, or nonempty/numeric/boolean defaults changed"})
            old = {key: value for key, value in left[name].items() if key not in ("description", "inputSchema", "outputSchema")}
            new = {key: value for key, value in right[name].items() if key not in ("description", "inputSchema", "outputSchema")}
            if old != new:
                failures.append({"tool": name, "error": "other tool metadata changed"})
        results.append({"profile": profile, "pass": not failures, "before_tools": len(left),
                        "after_tools": len(right), "failures": failures,
                        "baseline_bytes": len(serialized({"tools": before}).encode()),
                        "candidate_bytes": len(serialized({"tools": after}).encode()),
                        "constraint_projection_sha256": hashlib.sha256(json.dumps({name: {
                            field: constraint_projection(tool.get(field)) for field in ("inputSchema", "outputSchema")}
                            for name, tool in left.items()}, sort_keys=True).encode()).hexdigest()})
    report = {"pass": all(result["pass"] for result in results), "profiles": results,
              "limits": "Compares actual discovered names/constraints/parameter descriptions, not runtime behavior; only tool descriptions and schema title/examples/empty-default annotations may differ."}
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))
    if not report["pass"]:
        raise SystemExit(1)


def read_capture(path):
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or not isinstance(value.get("tools"), list) or not value["tools"]:
        raise ValueError("Capture must contain a nonempty tools list: " + str(path))
    names = set()
    for tool in value["tools"]:
        if (not isinstance(tool, dict) or not isinstance(tool.get("name"), str) or not tool["name"]
                or tool["name"] in names or not isinstance(tool.get("inputSchema"), dict)
                or tool["inputSchema"].get("type") != "object"):
            raise ValueError("Malformed or duplicate tool in capture: " + str(path))
        if "outputSchema" in tool and not isinstance(tool["outputSchema"], dict):
            raise ValueError("Malformed output schema in capture: " + str(path))
        if "description" in tool and not isinstance(tool["description"], str):
            raise ValueError("Malformed tool description in capture: " + str(path))
        names.add(tool["name"])
    return value["tools"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="action", required=True)
    live = subcommands.add_parser("collect", help="Capture initialize/tools-list; health checks possible, no owned model startup/inference")
    live.add_argument("--binary", required=True)
    live.add_argument("--arg", action="append", default=[])
    live.add_argument("--config", type=pathlib.Path)
    live.add_argument("--minimal-profile", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1]/"examples/profiles/minimal.toml")
    live.add_argument("--output", type=pathlib.Path, required=True)
    live.add_argument("--tokenizer", default=None, help="Optional cached tiktoken encoding, e.g. o200k_base; never downloads")
    live.add_argument("--timeout", type=float, default=30)
    live.set_defaults(run=collect)
    offline = subcommands.add_parser("compare", help="Offline comparison of two collected directories")
    offline.add_argument("--baseline", type=pathlib.Path, required=True)
    offline.add_argument("--candidate", type=pathlib.Path, required=True)
    offline.add_argument("--output", type=pathlib.Path)
    offline.set_defaults(run=compare)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
