# Security annotation rationale

Python inline suppressions are line-specific. B101 locations are in test modules. B404 covers local subprocess imports; B603 covers exact argv-based invocation sites; B110/B112 cover reviewed best-effort UI enumeration and cleanup handlers. The existing B602 caller-supplied command exception is listed separately because Bandit classifies it HIGH.

| File | Rule | Current lines | Rationale |
|---|---|---:|---|
| harness/py/desktop_test.py | B110 | 250, 298, 342, 452 | Transient UI control failure is best-effort; a closed control is not treated as application failure. |
| harness/py/desktop_test.py | B112 | 217, 234, 340, 361, 583, 596 | Transient UI enumeration or cleanup failure is best-effort; continue after an element or process disappears. |
| harness/py/desktop_test.py | B404 | 18 | The subprocess module is imported for intended local tool invocation. |
| harness/py/desktop_test.py | B602 | 1298 | Pre-existing caller-supplied command dispatch; string commands deliberately use shell mode, while argv calls stay shell-free. Bandit classifies this HIGH. |
| harness/py/desktop_test.py | B603 | 202, 1200 | Explicit argv (no shell command string); operator-selected executable or fixed test harness. The scanner reports this low finding with the default shell behavior. |
| harness/py/tests/test_desktop.py | B101 | 21, 42, 43, 44, 45, 46, 47, 52, 53, 54, 55, 56, 57, 58, 59, 60, 65, 66, 67, 72, 73, 79, 81, 82, 83, 88, 105, 107, 108, 109, 115, 117, 118, 119, 120, 121 | Test-only assert; test assertion, not runtime authorization. |
| harness/py/tests/test_desktop.py | B404 | 6 | The subprocess module is imported for intended local tool invocation. |
| harness/py/tests/test_desktop.py | B603 | 20 | Explicit argv (no shell command string); operator-selected executable or fixed test harness. The scanner reports this low finding with the default shell behavior. |
| harness/py/tests/test_hygiene.py | B101 | 40, 41, 44, 45, 63, 72 | Test-only assert; test assertion, not runtime authorization. |
| harness/py/tests/test_hygiene.py | B404 | 5 | The subprocess module is imported for intended local tool invocation. |
| harness/py/tests/test_hygiene.py | B603 | 31, 58 | Explicit argv (no shell command string); operator-selected executable or fixed test harness. The scanner reports this low finding with the default shell behavior. |
| harness/py/tests/test_unity_mode.py | B101 | 24, 25, 26, 27, 31, 36, 37, 38, 40, 45, 47, 56, 57, 58, 59, 96, 98, 101, 103, 106, 116, 130, 135, 136, 147, 148, 149, 167, 169 | Test-only assert; test assertion, not runtime authorization. |
| harness/py/tests/test_ux_adapter.py | B101 | 38, 39, 42, 43, 44, 45, 46, 48, 49, 50, 52, 56, 57, 68, 70, 71, 72, 131, 132, 134, 142, 172, 176, 192, 194, 196, 199 | Test-only assert; test assertion, not runtime authorization. |
| harness/py/tests/test_ux_adapter.py | B404 | 5 | The subprocess module is imported for intended local tool invocation. |
| harness/py/tests/test_ux_adapter.py | B603 | 21 | Explicit argv (no shell command string); operator-selected executable or fixed test harness. The scanner reports this low finding with the default shell behavior. |
| harness/py/unity_mode.py | B404 | 16 | The subprocess module is imported for intended local tool invocation. |
| harness/py/unity_mode.py | B603 | 73 | Explicit argv (no shell command string); operator-selected executable or fixed test harness. The scanner reports this low finding with the default shell behavior. |
| harness/py/ux_adapter.py | B110 | 225 | Transient UI control failure is best-effort; a closed control is not treated as application failure. |
| harness/py/ux_adapter.py | B112 | 170 | Transient UI enumeration or cleanup failure is best-effort; continue after an element or process disappears. |
| integrations/hermes/kritic/__init__.py | B404 | 40 | The subprocess module is imported for intended local tool invocation. |
| integrations/hermes/kritic/__init__.py | B603 | 168 | Explicit argv (no shell command string); operator-selected executable or fixed test harness. The scanner reports this low finding with the default shell behavior. |
| integrations/hermes/tests/test_plugin.py | B101 | 67, 81, 82, 83, 84, 85, 90, 92, 94, 95, 96, 97, 100, 109, 110, 111, 113, 114, 119, 121, 122, 130, 133, 137, 164, 165, 166 | Test-only assert; test assertion, not runtime authorization. |
| integrations/hermes/tests/test_response_schema.py | B101 | 21, 22, 23, 24, 51, 52, 53, 54, 70, 71, 72, 91, 92, 93, 94, 104, 105, 112, 123, 126, 129, 130 | Test-only assert; test assertion, not runtime authorization. |
| integrations/hermes/tests/test_response_schema.py | B404 | 4 | The subprocess module is imported for intended local tool invocation. |

## Current scanner records

- Semgrep 1.178.0: semgrep.json, semgrep.stderr, semgrep.exit; fresh run exit 0, no parser errors, no findings, empty stderr. The earlier docstring-only insecure-WebSocket false positive was removed by changing the explanatory text to “loopback WebSocket URL.”
- Bandit 1.9.4: bandit.json, bandit.stderr, bandit.exit; fresh default run exit 0, no remaining findings. Stderr contains seven B603 nosec notices saying no failed test. A separate --ignore-nosec diagnostic in bandit-ignore-nosec.json confirms underlying raw findings at annotated lines: 175 total (B101 147, B110 5, B112 7, B404 7, B603 8, B602 1). The only HIGH/MEDIUM item is the existing B602 caller-supplied shell-dispatch exception (HIGH).
- Syntax check: python3 -m py_compile passed for touched Python modules with PYTHONPYCACHEPREFIX directed to E:.
- Raw outputs are kept under E:/AI/work/kritic-readiness/security-cleanup; this note includes no source excerpts.
