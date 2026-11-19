# Agent message contract v1

An optional, model-free PreToolUse hook requires compact JSON message envelopes
for supported agent messaging and delegation tools. It rejects invalid messages
before transmission, with a specific correction. It never silently rewrites,
truncates or removes evidence. No GPU or service is required.

Generate the canonical JSON Schema with `python -I contract.py`. Keep the native
tool's recipient/task ID; do not duplicate it in the message. The JSON object is
the message string, without Markdown fences or introductory text.

| kind | Required fields | Word ceiling |
|---|---|---|
| `task` | `goal`, nonempty `scope`, nonempty `done_when` | 240 |
| `result` | `summary`, `changes`, nonempty `checks`, nonempty `evidence` | 180 |
| `blocker` | `problem`, nonempty `evidence`, nonempty `needs` | 120 |
| `question` | `question` | 80 |
| `update` | `summary` | 80 |

All messages have `kind`; unknown/duplicate fields, placeholders and empty required
strings are rejected. Maximum UTF-8 payload is 4096 bytes. Prose fields have a
300-character limit; reference fields allow 600. Lists have at most eight entries
(question options: four). Check status is `pass`, `fail` or `not_run`; failures and
unrun checks require `detail`. State exactly what was checked. Evidence is an exact
artifact path/line, revision, command observation or other source reference—not a
claim that the validator verified it. Put long logs/code/research in permanent
project artifacts and send their references. Split independent tasks if needed.

Optional fields: task `worktree`, `constraints`, `context`; result `remaining`;
blocker `attempted`; question `options`, `context`; update `next`, `refs`.
Use `result` for completed work and validation claims, `update` for progress.
Do not send acknowledgment-only messages unless they convey a needed state change.
Omit greetings, please, thanks, sign-offs, repeated context and narrative.
Preserve exact identifiers, failure text, constraints and uncertainty. Paths and
evidence are not sanitized for courtesy words.

```json
{"kind":"task","goal":"Reject malformed imports at the boundary","scope":["src/imports/","tests/imports.rs"],"done_when":["Valid imports work and malformed inputs fail"],"constraints":["Preserve other agents changes"]}
```

```json
{"kind":"result","summary":"Import validation added","changes":["src/imports/parser.rs"],"checks":[{"name":"cargo test imports","status":"pass"}],"evidence":["reports/import-tests.txt"],"remaining":["Large-file performance not measured"]}
```

## Coverage and limitations

Enroll explicitly with `python deploy.py --runtime <new permanent directory>`.
This adds Codex/Claude PreToolUse hooks while retaining existing hook groups.
Optional `--hermes-home <actual home>` installs the native plugin and needs PyYAML;
the runtime validator itself uses only Python's standard library. Reconnect clients
and restart Hermes to load new hooks/plugins. Defaults target `~/.codex` and
`~/.claude`; use `--help` for overrides. Run tests before enrollment:
`python -B -m unittest discover -s integrations/agent-messages -p 'test_*.py'`.
For an upgrade, choose a new permanent runtime and pass `--replace-runtime
<previous runtime>`. The installer refuses known competing versions unless that
exact prior runtime is selected, then replaces only its managed hooks. Re-enrollment
also removes an old restricted matcher for the same contract before installing
unrestricted message coverage. Unrelated hooks remain intact.

The native adapter checks Codex `spawn_agent`, `send_message`, `followup_task`,
Codex app thread prompts, Claude `Task`/`Agent`/`SendMessage`, and Hermes
`message_agent`/`delegate_task`. New assignments require `kind=task`.
Hermes delegation uses the JSON envelope in `goal`; put references in the JSON
`context` array rather than its unchecked external `context` string. Every child
in a Hermes batch is checked. List/stop and shutdown control operations are left
alone. Nonmessage tools and user-facing replies are unaffected.
Codex `fork_thread` carries no message/prompt; it is left alone. Any subsequent
thread message is validated separately.

Only enrolled supported hook paths are enforced. Calls hidden inside an opaque
code-mode wrapper and automatic subagent final responses may bypass native
PreToolUse visibility; the global directive applies there, but the hook cannot
claim to intercept them. This contract checks structure, size, explicit fields and
obvious prose pleasantries. It cannot prove truth, relevance, semantic completeness,
actual test execution, ownership or permission to contact another session.
Existing worktree-scope checks still apply. Human authorization to message an
existing task remains required by the host. No model is used to guess or discard
information, and no general token-saving percentage is claimed.
