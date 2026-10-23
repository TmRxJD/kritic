"""Model-free launchd contract regressions; never start programs or load weights."""
import copy
import importlib.util
import os
from pathlib import Path
import plistlib
import socket
import stat
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('serve_mlx', Path(__file__).with_name('serve-mlx.py'))
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)


class LaunchdTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.python = self.root / 'kev/.venv/bin/python'
        self.python.parent.mkdir(parents=True)
        self.python.touch()
        self.cfg = {'review': {'backend': 'local'}, 'kev': {
            'local_runtime': 'mlx', 'native_url': 'http://127.0.0.1:8009',
            'native_run': 'jaredpalmer/kev-0.8b@revision', 'native_dtype': 'bfloat16',
            'max_seq_tokens': 1024}}

    def build(self, cfg=None, ram=8 * 1024**3):
        return launcher.build_job(cfg or self.cfg, self.root / 'kev', self.root / 'logs', ram)

    def test_fixed_argv_environment_and_headless_ownership(self):
        job = self.build()
        args = job['ProgramArguments']
        self.assertEqual(args[:2], ['/usr/bin/env', '-i'])
        interpreter = args.index(str(self.python.resolve()))
        self.assertEqual(args[interpreter:], [str(self.python.resolve()), '-m', 'kev.serve', '--run',
            'jaredpalmer/kev-0.8b@revision', '--device', 'mps', '--host', '127.0.0.1', '--port', '8009'])
        self.assertEqual(set(args[2:interpreter]), {
            'HOME=' + str(Path.home()), 'PATH=/usr/bin:/bin:/usr/sbin:/sbin',
            'KEV_BACKEND=mlx', 'KEV_DTYPE=bf16', 'KEV_MERGE=1', 'KEV_TRUNCATE_STATES=0',
            'KEV_PREFIX_CACHE=1', 'KEV_PREFIX_MAX_TOKENS=1024', 'PYTHONUNBUFFERED=1'})
        self.assertNotIn('EnvironmentVariables', job)
        self.assertEqual(job['StandardInPath'], '/dev/null')
        self.assertFalse(job['KeepAlive'])
        self.assertTrue(job['RunAtLoad'])
        self.assertEqual(job['ProcessType'], 'Interactive')
        self.assertNotIn('Shell', job)

    def test_refuses_unsafe_profiles_and_large_model_before_start(self):
        for key, value in [('native_url', 'http://example.com:8009'), ('cpu_only', True),
                           ('native_dtype', 'float32'), ('native_run', 'jaredpalmer/kev-4b@revision'),
                           ('native_run', 'jaredpalmer/kev-0.8b')]:
            cfg = copy.deepcopy(self.cfg)
            cfg['kev'][key] = value
            with self.assertRaises(ValueError):
                self.build(cfg)
        self.python.unlink()
        with self.assertRaises(ValueError):
            self.build()

    def test_serialization_permissions_and_no_overwrite(self):
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            port = probe.getsockname()[1]
        self.cfg['kev']['native_url'] = f'http://127.0.0.1:{port}'
        job = self.build()
        path = launcher.write_job(job, self.root / 'logs')
        self.assertEqual(plistlib.loads(path.read_bytes()), job)
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        with self.assertRaises(FileExistsError):
            launcher.write_job(job, self.root / 'logs')
        self.assertEqual(plistlib.loads(path.read_bytes()), job)

    def test_busy_port_creates_no_job(self):
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            probe.listen()
            self.cfg['kev']['native_url'] = f'http://127.0.0.1:{probe.getsockname()[1]}'
            with self.assertRaises(OSError):
                launcher.write_job(self.build(), self.root / 'logs')
        self.assertFalse((self.root / 'logs').exists())


if __name__ == '__main__':
    unittest.main()
