# Resource sampler cleanup ownership

An observed resource run launched an interpreter-based bounded capture. Two unrelated
interpreter-based editor connectors started during sampling. The report attributed both
connectors to the run and described them as killed. The private capture, paths and report
are deliberately excluded here; the offline regression uses synthetic creation times.

Previously, `phase3::resource` called `roots_for(command)`, which included the interpreter's
installation directory. `hygiene::snapshot` / `leaks` treated a new process in that directory
with a recent start time as a run-owned leak. Sharing an interpreter and starting recently
does not prove ancestry. Native cleanup also used a bare root PID tree and sysinfo's
Windows termination path delegates to `taskkill /PID`, leaving a PID-reuse race.

The sampler tracks the root and descendants observed while their authenticated
parents are present. It retains Windows process handles or Linux pidfds so cleanup
can target the same processes after their parents exit. No interpreter directory,
cwd, machine-wide baseline or process-age heuristic grants run ownership.
Attach mode remains observational.

The sampler reports actual termination-request status. Termination request success does
not assert that process exit or port release has completed. Its intentional sampling-window
stop is not itself an orphan leak. Surviving observed descendants after root exit and
termination failures remain reportable.

Offline process-tracking regressions cover ownership and cleanup using the same
implementation as resource sampling. The phase3 regression checks cleanup-status
wording.

Polling can miss descendants whose parent exits before ancestry is observed.
The sampler leaves those processes alone. The independent hygiene audit and
legacy snapshot APIs have different scopes.

A finite command can complete successfully when the root exits with code 0,
the tracked tree drains, and the requested budgets pass. A forced cleanup
does not turn an incomplete run into a successful exit.
