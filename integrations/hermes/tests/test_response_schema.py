"""Offline subprocess-seam regression: actual plugin/ABC, no provider calls."""
import importlib.util
import os
import subprocess  # noqa: S404; intended local tool invocation  # nosec B404
from pathlib import Path
import pytest
PLUGIN = Path(os.environ.get('STAGE3_PLUGIN', str(Path(__file__).resolve().parents[1] / 'kritic' / '__init__.py')))
def load():
    spec = importlib.util.spec_from_file_location('schema_owned_plugin', PLUGIN)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod

def test_original_list_regression_retains_context(monkeypatch):
    m = load()
    monkeypatch.setattr(m, '_kritic_exe', lambda: 'isolated-fake')
    monkeypatch.setattr(m.subprocess, 'run', lambda *a, **k: subprocess.CompletedProcess(a[0], 0, '[1]', ''))
    e = m.KriticContextEngine()
    messages = [{'role': 'user', 'content': 'essential diagnostic'}]
    e.last_prompt_tokens = 17
    assert e.compress(messages) is messages  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.compression_count == e.archived_total == 0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.last_prompt_tokens == 17  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.last_error and 'schema' in e.last_error  # noqa: S101; test assertion, not runtime authorization  # nosec B101


@pytest.mark.parametrize('wire', [
    '{}', '[]', 'null', '1', 'true', '"scalar"', '{bad',
    '{"messages":null}', '{"messages":{}}', '{"messages":[1]}',
    '{"messages":[{}]}', '{"messages":[{"role":1}]}',
    '{"messages":[{"role":"tool","content":5}]}',
    '{"messages":[{"role":"tool","content":[1]}]}',
    '{"messages":[{"role":"assistant","tool_calls":{}}]}',
    '{"messages":[],"archived":{}}', '{"messages":[],"archived":[1]}',
    '{"messages":[],"tokens_after":"8"}', '{"messages":[],"tokens_after":true}',
    '{"messages":[],"tokens_after":-1}', '{"messages":[],"tokens_after":1.5}',
    '{"messages":[],"tokens_after":NaN}',
    '{"messages":[],"error":"backend unavailable"}',
    '{"messages":[],"error":{"code":"unavailable","message":"retry later"}}',
    '{"messages":[],"error":false}', '{"messages":[],"error":{}}',
])
@pytest.mark.parametrize('method', ['compress', 'prune_tool_results_only'])
def test_invalid_preserves_context_and_all_counters(monkeypatch, wire, method):
    m = load()
    monkeypatch.setattr(m, '_kritic_exe', lambda: 'isolated-fake')
    monkeypatch.setattr(m.subprocess, 'run', lambda *a, **k: subprocess.CompletedProcess(a[0], 0, wire, ''))
    e = m.KriticContextEngine()
    e.compression_count, e.archived_total, e.last_prompt_tokens = 3, 5, 31
    original = [{'role': 'user', 'content': 'retained critical constraint'}]
    out = getattr(e, method)(original)
    assert (out[0] if method != 'compress' else out) is original  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    if method != 'compress': assert out[1] == 0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert (e.compression_count, e.archived_total, e.last_prompt_tokens) == (3, 5, 31)  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.get_status()['last_error']  # noqa: S101; test assertion, not runtime authorization  # nosec B101


@pytest.mark.parametrize('wire,archives,tokens', [
    ('{"messages":[]}', 0, 0),
    ('{"messages":[{"role":"user","content":"retained"}],"archived":[],"tokens_after":3}', 0, 3),
    ('{"messages":[{"role":"assistant","content":null,"tool_calls":[]}],"archived":[{"id":"kr-1"}],"tokens_after":4,"error":null}', 1, 4),
    ('{"messages":[{"role":"user","content":[{"type":"text","text":"fact"}]}]}', 0, 0),
])
def test_valid_schema_updates_only_on_success(monkeypatch, wire, archives, tokens):
    import json
    m = load()
    monkeypatch.setattr(m, '_kritic_exe', lambda: 'isolated-fake')
    monkeypatch.setattr(m.subprocess, 'run', lambda *a, **k: subprocess.CompletedProcess(a[0], 0, wire, ''))
    e = m.KriticContextEngine()
    e.last_error = 'previous error'
    assert e.compress([]) == json.loads(wire)['messages']  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.compression_count == 1 and e.archived_total == archives  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.last_prompt_tokens == tokens and e.last_error is None  # noqa: S101; test assertion, not runtime authorization  # nosec B101


@pytest.mark.parametrize('failure', ['timeout', 'oserror', 'nonzero', 'json', 'schema', 'backend'])
def test_failure_then_next_call_recovers_without_hidden_retry(monkeypatch, failure):
    m = load()
    monkeypatch.setattr(m, '_kritic_exe', lambda: 'isolated-fake')
    calls = []
    def run(*a, **k):
        calls.append(a)
        if len(calls) == 1:
            if failure == 'timeout': raise subprocess.TimeoutExpired('isolated-fake', 0.01)
            if failure == 'oserror': raise OSError('isolated failure')
            if failure == 'nonzero': return subprocess.CompletedProcess(a[0], 7, '', 'isolated backend failure')
            return subprocess.CompletedProcess(a[0], 0, {'json':'{bad', 'schema':'[1]', 'backend':'{"error":{"code":"offline","message":"recover"}}'}[failure], '')
        return subprocess.CompletedProcess(a[0], 0, '{"messages":[{"role":"user","content":"ok"}],"tokens_after":1}', '')
    monkeypatch.setattr(m.subprocess, 'run', run)
    e = m.KriticContextEngine()
    original = [{'role':'user','content':'essential'}]
    assert e.compress(original) is original  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert len(calls) == 1 and e.last_error and e.compression_count == 0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.compress(original) == [{'role':'user','content':'ok'}]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert len(calls) == 2 and e.last_error is None and e.compression_count == 1  # noqa: S101; test assertion, not runtime authorization  # nosec B101


@pytest.mark.parametrize('wire', ['[]', '{}', '{"found":[1]}', '{"error":{"code":"offline","message":"recover"}}'])
def test_recall_malformed_is_explicit_error(monkeypatch, wire):
    import json
    m = load()
    monkeypatch.setattr(m, '_kritic_exe', lambda: 'isolated-fake')
    monkeypatch.setattr(m.subprocess, 'run', lambda *a, **k: subprocess.CompletedProcess(a[0], 0, wire, ''))
    e = m.KriticContextEngine()
    assert json.loads(e.handle_tool_call('kritic_recall', {'id':'kr-1'}))['error']  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.last_error and e.compression_count == e.archived_total == 0  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_actual_native_archive_namespace(tmp_path, monkeypatch):
    import json
    m = load()
    configured_exe = os.environ.get('KRITIC_EXE')
    assert configured_exe, 'Set KRITIC_EXE to the built native binary'  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    exe = Path(configured_exe)
    assert exe.is_absolute(), 'KRITIC_EXE must be an absolute path'  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert exe.is_file(), 'Required isolated native binary unavailable'  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    monkeypatch.setenv('HERMES_HOME', str(tmp_path / 'hermes'))
    monkeypatch.setenv('KRITIC_HOME', str(tmp_path / 'kritic'))
    monkeypatch.setenv('KRITIC_EXE', str(exe))
    e = m.KriticContextEngine()
    e.use_kev = False
    e.protect_first_n = e.protect_last_n = 0
    e.big_chars = 100
    e.on_session_start('stage3-owned-namespace')
    original = [{'role':'system','content':'retain'}, {'role':'assistant','content':None,'tool_calls':[{'id':'c1','type':'function','function':{'name':'read_file','arguments':'{}'}}]}, {'role':'tool','tool_call_id':'c1','content':'essential diagnostic\n' * 2000}, {'role':'user','content':'continue'}]
    out, n = e.prune_tool_results_only(original)
    assert n == 1 and e.last_error is None  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    rid = out[2]['content'].split(' as ', 1)[1].split('.', 1)[0]
    rec = json.loads(e.handle_tool_call('kritic_recall', {'id':rid,'format':'original','max_chars':100000,'max_output_bytes':200000}))
    assert rec['found'][0]['original_content'] == original[2]['content']  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    e.on_session_start('stage3-owned-other-namespace')
    wrong = json.loads(e.handle_tool_call('kritic_recall', {'id':rid,'format':'original'}))
    assert not wrong.get('found') and 'no archived message' in wrong['error']  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert e.last_error and e.archived_total == 1 and e.compression_count == 0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
