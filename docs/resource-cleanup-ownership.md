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

The sampler now owns only its authenticated root and descendants observed while their
authenticated parents were present. It retains exact descendant identities after a parent
exits. Windows creation time and parent PID are queried through one kernel handle; an
additional validation pass rejects identities replaced during enumeration. Cleanup opens
a handle, verifies creation time, and terminates through that same handle. Linux uses
one `/proc/PID/stat` read for ancestry and creation ticks and signals a validated pidfd.
No interpreter directory, cwd, machine-wide baseline or process-age heuristic grants
resource-run ownership. Attach mode remains observational.

The sampler reports actual termination-request status. Termination request success does
not assert that process exit or port release has completed. Its intentional sampling-window
stop is not itself an orphan leak. Surviving observed descendants after root exit and
termination failures remain reportable.

Offline regressions in `native::ownership::tests` cover unrelated new interpreter processes,
multi-generation ownership, retained observed orphans, reused root/child PIDs and stale parent
links. The phase3 regression checks truthful cleanup-status wording. They never spawn,
inspect or terminate real workloads.

Limitations: polling can miss descendants whose parent exits before ancestry is observed.
Such processes are not inferred or killed. Windows/Linux supply authenticated identities;
other platforms currently return an explicit identity-unavailable error. Linux kernels
without pidfd support fail cleanup closed. The independent hygiene audit and legacy snapshot
APIs are unchanged and must not be treated as authenticated resource-run cleanup. Live
process tests are deliberately excluded from this fix's validation while other work runs.

Successful bounded-workload classification is a separate API issue: the current resource
API has no explicit bounded-job versus long-running-workload contract, and still reports
an exit during sampling as a budget violation. This ownership fix does not reinterpret it.
