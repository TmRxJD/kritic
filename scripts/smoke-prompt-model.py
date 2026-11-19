"""Windows-only, owned CPU Qwen smoke; preserves all downloads and evidence."""
import argparse
import ctypes as C
from ctypes import wintypes as W
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
# Required for the explicitly owned, suspended process tree; no shell execution.
import subprocess  # nosec B404
import sys
import threading
import time
import http.client

GIB = 1024 ** 3


class Performance(C.Structure):
    _fields_ = [('cb', W.DWORD)] + [(n, C.c_size_t) for n in
        ('CommitTotal', 'CommitLimit', 'CommitPeak', 'PhysicalTotal',
         'PhysicalAvailable', 'SystemCache', 'KernelTotal', 'KernelPaged',
         'KernelNonpaged', 'PageSize')] + [(n, W.DWORD) for n in
        ('HandleCount', 'ProcessCount', 'ThreadCount')]


class BasicLimits(C.Structure):
    _fields_ = [('ProcessTime', C.c_int64), ('JobTime', C.c_int64),
                ('Flags', W.DWORD), ('MinWorkingSet', C.c_size_t),
                ('MaxWorkingSet', C.c_size_t), ('ActiveLimit', W.DWORD),
                ('Affinity', C.c_size_t), ('Priority', W.DWORD), ('Scheduling', W.DWORD)]


class Limits(C.Structure):
    _fields_ = [('Basic', BasicLimits), ('Io', C.c_uint64 * 6)] + [
        (n, C.c_size_t) for n in ('ProcessMemory', 'JobMemory', 'PeakProcess', 'PeakJob')]


class Accounting(C.Structure):
    _fields_ = [(n, C.c_int64) for n in ('User', 'Kernel', 'PeriodUser', 'PeriodKernel')] + [
        (n, W.DWORD) for n in ('Faults', 'Total', 'Active', 'Terminated')]


def api(dll, name, restype, *args):
    function = getattr(dll, name)
    function.restype, function.argtypes = restype, args
    return function


class OwnedJob:
    def __init__(self, budget):
        self.kernel = C.WinDLL('kernel32', use_last_error=True)
        self.close = api(self.kernel, 'CloseHandle', W.BOOL, W.HANDLE)
        self.query = api(self.kernel, 'QueryInformationJobObject', W.BOOL,
                         W.HANDLE, C.c_int, C.c_void_p, W.DWORD, C.c_void_p)
        self.assign = api(self.kernel, 'AssignProcessToJobObject', W.BOOL, W.HANDLE, W.HANDLE)
        self.kill = api(self.kernel, 'TerminateJobObject', W.BOOL, W.HANDLE, W.UINT)
        self.resume = api(C.WinDLL('ntdll'), 'NtResumeProcess', C.c_long, W.HANDLE)
        self.handle = api(self.kernel, 'CreateJobObjectW', W.HANDLE,
                          C.c_void_p, W.LPCWSTR)(None, None)
        if not self.handle:
            raise C.WinError(C.get_last_error())
        limits = Limits()
        limits.Basic.Flags = 0x2000 | 0x200  # KILL_ON_JOB_CLOSE | JOB_MEMORY
        limits.JobMemory = budget
        if not api(self.kernel, 'SetInformationJobObject', W.BOOL, W.HANDLE,
                   C.c_int, C.c_void_p, W.DWORD)(self.handle, 9, C.byref(limits), C.sizeof(limits)):
            self.close(self.handle)
            raise C.WinError(C.get_last_error())

    def spawn(self, command, **kwargs):
        if kwargs.get('shell') or not isinstance(command, (list, tuple)) or not command:
            raise ValueError('Owned children require an argv list without a shell')
        executable = Path(command[0])
        if not executable.is_absolute() or not executable.is_file() or executable.suffix.lower() != '.exe':
            raise ValueError('Owned children require an existing absolute Windows executable')
        # Suspended creation prevents any descendant escaping before assignment.
        # Callers supply validated absolute executables and fixed serve/prompt argv;
        # user prompt data goes only to stdin. No shell or generated command is run.
        process = subprocess.Popen(command, creationflags=0x08000000 | 0x4, **kwargs)  # nosec B603
        if not self.assign(self.handle, W.HANDLE(int(process._handle))):
            process.kill()
            process.wait(timeout=5)
            raise C.WinError(C.get_last_error())
        if self.resume(W.HANDLE(int(process._handle))) != 0:
            process.kill()
            process.wait(timeout=5)
            raise RuntimeError('Cannot resume owned suspended process')
        return process

    def stats(self):
        accounting, limits = Accounting(), Limits()
        for number, value in ((1, accounting), (9, limits)):
            if not self.query(self.handle, number, C.byref(value), C.sizeof(value), None):
                raise C.WinError(C.get_last_error())
        return {'active_processes': accounting.Active, 'total_processes': accounting.Total,
                'peak_job_commit_bytes': limits.PeakJob}

    def cleanup(self):
        if not self.kill(self.handle, 1):
            raise C.WinError(C.get_last_error())
        deadline = time.monotonic() + 10
        result = self.stats()
        while result['active_processes'] and time.monotonic() < deadline:
            time.sleep(.1)
            result = self.stats()
        self.close(self.handle)
        self.handle = None
        if result['active_processes']:
            raise RuntimeError('Owned job descendants did not exit within cleanup deadline')
        return dict(result, verified_empty=True, method='Windows Job Object terminate/kill-on-close')


def memory():
    value = Performance()
    value.cb = C.sizeof(value)
    if not api(C.WinDLL('psapi', use_last_error=True), 'GetPerformanceInfo', W.BOOL,
               C.POINTER(Performance), W.DWORD)(C.byref(value), value.cb):
        raise C.WinError(C.get_last_error())
    return {'reserve_bytes': (value.CommitLimit - value.CommitTotal) * value.PageSize,
            'physical_available_bytes': value.PhysicalAvailable * value.PageSize}


def listener_pid(port):
    size = W.DWORD()
    function = api(C.WinDLL('iphlpapi'), 'GetExtendedTcpTable', W.DWORD,
                   C.c_void_p, C.POINTER(W.DWORD), W.BOOL, W.ULONG, C.c_int, W.ULONG)
    if function(None, C.byref(size), False, 2, 3, 0) != 122:
        raise RuntimeError('Cannot size Windows listener ownership table')
    buffer = C.create_string_buffer(size.value)
    if function(buffer, C.byref(size), False, 2, 3, 0):
        raise RuntimeError('Cannot inspect Windows listener ownership table')
    raw = buffer.raw
    rows = [struct.unpack_from('<6I', raw, 4 + index * 24)
            for index in range(struct.unpack_from('<I', raw)[0])]
    return [row[5] for row in rows if socket.ntohs(row[2] & 65535) == port
            and socket.inet_ntoa(struct.pack('<I', row[1])) == '127.0.0.1']


def proposal_valid(value):
    if not isinstance(value, dict) or set(value) != {'action', 'brief', 'intent_map', 'questions', 'reasons'}:
        return False
    if value['action'] not in ('keep', 'rewrite', 'clarify') or not isinstance(value['brief'], str):
        return False
    for key in ('questions', 'reasons'):
        if not isinstance(value[key], list) or not all(isinstance(x, str) for x in value[key]):
            return False
    return isinstance(value['intent_map'], list) and all(
        isinstance(x, dict) and set(x) == {'source_quote', 'brief_quote'}
        and all(isinstance(v, str) for v in x.values()) for x in value['intent_map'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ('kritic', 'ollama', 'work-dir'):
        parser.add_argument('--' + flag, type=Path, required=True)
    parser.add_argument('--model', default='qwen2.5:1.5b')
    parser.add_argument('--margin-gib', type=float, default=2)
    parser.add_argument('--deadline-seconds', type=int, default=900)
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('This helper requires Windows Job Objects and commit telemetry')
    if not 1 <= args.margin_gib <= 16 or not 60 <= args.deadline_seconds <= 1800:
        parser.error('margin must be 1..16 GiB; deadline must be 60..1800 seconds')
    for path in (args.kritic, args.ollama, args.work_dir):
        if not path.is_absolute():
            parser.error('All paths must be absolute')
    if not args.kritic.is_file() or not args.ollama.is_file():
        parser.error('Both executable paths must exist')
    if args.work_dir.exists() and any(args.work_dir.iterdir()):
        parser.error('Use a fresh empty work directory; existing test work is preserved')
    args.work_dir.mkdir(parents=True, exist_ok=True)
    evidence_path = args.work_dir / 'evidence.json'
    try:
        evidence_path.open('x', encoding='utf8').close()
    except FileExistsError:
        parser.error('Use a fresh work directory; prior evidence is never overwritten')
    evidence = {'status': 'failed', 'model': args.model, 'results': [], 'resource_samples': [],
                'inference': 'Owned CPU local inference; OpenAI-compatible is a loopback API protocol, not billed external cloud',
                'declared_job_margin_bytes': int(args.margin_gib * GIB), 'commit_floor_bytes': 6 * GIB}

    def persist():
        temporary = evidence_path.with_suffix('.pending.json')
        with temporary.open('w', encoding='utf8') as stream:
            json.dump(evidence, stream, indent=2)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, evidence_path)

    job, monitor = None, None
    done, aborted = threading.Event(), threading.Event()
    started = time.monotonic()
    handles = []
    try:
        initial = memory()
        evidence['resource_samples'].append(dict(initial, elapsed_seconds=0))
        persist()
        if initial['reserve_bytes'] < 6 * GIB + int(args.margin_gib * GIB):
            raise RuntimeError('Insufficient commit reserve: require 6 GiB plus declared job margin')
        job = OwnedJob(int(args.margin_gib * GIB))

        def guard():
            while not done.wait(.25):
                try:
                    sample = dict(memory(), elapsed_seconds=round(time.monotonic() - started, 2))
                    evidence['resource_samples'].append(sample)
                    if sample['reserve_bytes'] < 6 * GIB + GIB // 4:
                        raise RuntimeError('Commit reserve reached abort threshold above 6 GiB floor')
                    if time.monotonic() - started > args.deadline_seconds:
                        raise RuntimeError('Overall smoke deadline exceeded')
                except Exception as error:
                    evidence['abort_reason'] = str(error)
                    aborted.set()
                    job.kill(job.handle, 1)
                    return

        monitor = threading.Thread(target=guard, daemon=True)
        monitor.start()
        env = {k: v for k, v in os.environ.items() if not k.startswith(('KRITIC_', 'OLLAMA_'))}
        for name in ('models', 'cache', 'temp', 'home'):
            (args.work_dir / name).mkdir(exist_ok=True)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        base = f'http://127.0.0.1:{port}'
        env.update(OLLAMA_HOST=base, OLLAMA_MODELS=str(args.work_dir / 'models'),
                   OLLAMA_NO_CLOUD='1', OLLAMA_MAX_LOADED_MODELS='1', OLLAMA_NUM_PARALLEL='1',
                   OLLAMA_CONTEXT_LENGTH='4096', OLLAMA_KEEP_ALIVE='5m', OLLAMA_VULKAN='false',
                   CUDA_VISIBLE_DEVICES='-1', HIP_VISIBLE_DEVICES='-1', ROCR_VISIBLE_DEVICES='-1',
                   GGML_VK_VISIBLE_DEVICES='-1', OLLAMA_LLM_LIBRARY='cpu',
                   HOME=str(args.work_dir / 'home'), USERPROFILE=str(args.work_dir / 'home'),
                   XDG_CACHE_HOME=str(args.work_dir / 'cache'), TEMP=str(args.work_dir / 'temp'),
                   TMP=str(args.work_dir / 'temp'), OLLAMA_TMPDIR=str(args.work_dir / 'temp'),
                   APPDATA=str(args.work_dir / 'cache'), LOCALAPPDATA=str(args.work_dir / 'cache'),
                   NO_PROXY='127.0.0.1,localhost')
        log = (args.work_dir / 'ollama.log').open('xb')
        handles.append(log)
        server = job.spawn([str(args.ollama), 'serve'], env=env, cwd=args.work_dir,
                           stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
        def request(path, body=None, timeout=5):
            if aborted.is_set():
                raise RuntimeError(evidence['abort_reason'])
            data = None if body is None else json.dumps(body).encode('utf8')
            connection = http.client.HTTPConnection('127.0.0.1', port, timeout=timeout)
            connection.request('GET' if body is None else 'POST', path, body=data,
                               headers={'Content-Type': 'application/json', 'Connection':'close'})
            response = connection.getresponse()
            if response.status >= 400:
                response.close(); connection.close()
                raise RuntimeError(f'Owned model server returned HTTP {response.status}')
            return response

        ready_until = time.monotonic() + 30
        while True:
            try:
                with request('/api/version') as response:
                    evidence['ollama_version'] = json.load(response)
                break
            except OSError:
                if server.poll() is not None or time.monotonic() > ready_until or aborted.is_set():
                    raise RuntimeError('Owned Ollama failed readiness; see ollama.log')
                time.sleep(.2)
        evidence['listener_pids'] = listener_pid(port)
        if evidence['listener_pids'] != [server.pid] or server.poll() is not None:
            raise RuntimeError('Loopback endpoint is not owned by this Ollama process')
        with request('/api/pull', {'model': args.model, 'stream': True}, timeout=30) as response:
            last = None
            while line := response.readline(65537):
                if len(line) > 65536 or aborted.is_set():
                    raise RuntimeError('Pull exceeded line bound or resource deadline')
                last = json.loads(line)
                if last.get('error'):
                    raise RuntimeError('Model pull failed: ' + str(last['error']))
            if not last or last.get('status') != 'success':
                raise RuntimeError('Model pull did not complete')
        with request('/api/tags') as response:
            models = json.load(response)['models']
        model = next((m for m in models if args.model in (m.get('name'), m.get('model'))), None)
        if not model or not model.get('digest'):
            raise RuntimeError('Pulled model digest missing')
        evidence['model_digest'] = model['digest']
        persist()
        config = args.work_dir / 'smoke.toml'
        config.write_text('[capabilities]\nprompts=true\n[prompt]\ntimeout_s=120\n', encoding='utf8')
        env.update(KRITIC_CONFIG=str(config), KRITIC_CAP_PROMPTS='true', KRITIC_PROMPT_MODEL=args.model,
                   KRITIC_PROMPT_TIMEOUT_S='120', KRITIC_OLLAMA_URL=base,
                   KRITIC_PROMPT_CLOUD_FORMAT='json_schema',
                   KRITIC_KEV_REQUEST_S='120', KRITIC_CLOUD_MAX_OUTPUT_TOKENS='2048',
                   KRITIC_CLOUD_PROVIDER='openai-compatible', KRITIC_CLOUD_MODEL=args.model,
                   KRITIC_CLOUD_URL=base + '/v1/chat/completions', KRITIC_CLOUD_KEY_ENV='SMOKE_DUMMY_KEY',
                   SMOKE_DUMMY_KEY='local-loopback-no-billing')
        original = 'Please list 3 short parser test ideas. Please list 3 short parser test ideas.'
        for backend in ('local', 'cloud'):
            if aborted.is_set():
                raise RuntimeError(evidence['abort_reason'])
            env['KRITIC_PROMPT_BACKEND'] = backend
            out_path, err_path = [args.work_dir / (backend + suffix) for suffix in ('.stdout', '.stderr')]
            with out_path.open('xb') as out, err_path.open('xb') as err:
                child = job.spawn([str(args.kritic), 'prompt'], env=env, cwd=args.work_dir,
                                  stdin=subprocess.PIPE, stdout=out, stderr=err)
                try:
                    child.communicate(original.encode('utf8'), timeout=130)
                except subprocess.TimeoutExpired:
                    raise RuntimeError('Kritic prompt deadline exceeded') from None
            if out_path.stat().st_size > 1048576 or err_path.stat().st_size > 65536:
                raise RuntimeError('Kritic prompt output exceeded bounds; see owned output files')
            stdout, stderr = out_path.read_bytes(), err_path.read_bytes()
            if child.returncode:
                raise RuntimeError('Kritic prompt failed: ' + stderr[-4000:].decode('utf8', 'replace'))
            report = json.loads(stdout)
            evidence['results'].append({'backend': backend, 'report': report})
            persist()
            if report.get('provider_error') or report.get('status') not in ('keep','suggestion','clarification') or not proposal_valid(report.get('proposal')):
                raise RuntimeError('Prompt provider failed or omitted a schema-valid proposal')
            if report.get('original') != original or report.get('original_sha256') != hashlib.sha256(original.encode()).hexdigest():
                raise RuntimeError('Exact original/hash was not retained')
            if report.get('replacement_allowed') is not False or report.get('authority') != 'original_user_prompt':
                raise RuntimeError('Original prompt authority was not retained')
            with request('/api/ps') as response:
                placement = json.load(response)['models']
            evidence.setdefault('cpu_placement', []).append(placement)
            if not placement or any(m.get('size_vram') != 0 for m in placement):
                raise RuntimeError('CPU-only placement was not confirmed')
        if aborted.is_set():
            raise RuntimeError(evidence['abort_reason'])
        evidence['status'] = 'passed'
    except Exception as error:
        evidence['error'] = str(error)
    finally:
        done.set()
        if monitor:
            monitor.join(timeout=2)
        if job:
            try:
                evidence['cleanup'] = job.cleanup()
            except Exception as error:
                evidence['cleanup'] = {'error': str(error), 'verified_empty': False}
                evidence['status'] = 'failed'
                if job.handle:
                    job.close(job.handle)
        for handle in handles:
            handle.close()
        if aborted.is_set():
            evidence['status'] = 'failed'
        evidence['elapsed_seconds'] = round(time.monotonic() - started, 2)
        persist()
    print(json.dumps({'status': evidence['status'], 'evidence': str(evidence_path)}))
    return 0 if evidence['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
