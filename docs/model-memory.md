# Local Kev memory profiles and cooperative yielding

The compatible native Laya/Kev server supports `--memory-profile auto` by default.
Kritic uses the server's optional reservation protocol to yield Kev's allocations before
local vision chat or the deterministic `ui_test` workload. No new Kritic configuration
keys are required: discovery uses the configured local Laya URL and `/health`.

The optional [native source patch and build instructions](../extras/native-kev/README.md)
provide a reproducible compatible server at its stated upstream revision.

These are server capabilities, not features guaranteed by every Laya binary. Older
servers, unreachable services, remote URLs, native MLX, and off/cloud Kev backends are
skipped. Cloud vision calls do not request a local memory lease. Kritic does not start,
restart, or stop a service to obtain a reservation.

## Selecting a profile

Pass the profile to the compatible native server CLI or its matching launcher:

| Profile | Behavior |
| --- | --- |
| `auto` (default) | Starts with streaming on supported Vulkan devices; adapts at request boundaries using successful usage history and measured headroom. |
| `low-commit` | Keeps eligible large Q8_0 parameter matrices streamed through a bounded GPU buffer. Lower committed memory costs additional transfer latency. |
| `high-commit` | Keeps GPU weights resident for faster repeated inference. It still honors memory reservations and configured idle unload. |

The native CLI also accepts `resident` as an alias for `high-commit`, and the older
`--stream-weights` flag selects `low-commit`. CPU execution retains its mapped-weight
path; this prototype's GPU streaming requires Vulkan and eligible Q8_0 matrices.
Do not assume these profiles enable streaming for arbitrary devices or quantizations.

Explicit high-commit is a performance preference, not permission to ignore another
workload's acknowledged memory reservation. Releasing a reservation allows a later
decision request to reload Kev according to its configured profile; release and health
checks do not themselves warm a model.

## What auto measures

Only successful inference observations update exponentially weighted averages of input
tokens, forward count, sequence bucket, and intervals between successful request starts.
Health polling does not train the policy or trigger a profile switch. Idle unload resets
usage history to a conservative cold start.

The initial defaults are deliberately specific: **a conservative Q8_0 / RTX 2070 starting
profile, not universal safe admission limits**.

| Decision | Starting rule |
| --- | --- |
| Promotion eligibility | At least four successful observations. |
| Sustained demand | Recent average request interval at most 30 seconds; or average input at least 1,024 tokens, at least four forwards, or sequence bucket at least 512. |
| Promotion headroom | At least 16 GiB available system commit and 4.75 GiB free device memory. |
| Immediate demotion | Available commit at most 12 GiB, free device memory at most 0.75 GiB, or unknown headroom. |
| Switching cooldown | 60 seconds; pressure/unknown-headroom demotion bypasses it. |

The separate promotion/demotion thresholds accommodate the measured resident-versus-
streaming footprint difference and reduce switching caused by the model's own allocation.
These thresholds do not reserve the remaining memory against other processes. Operating
system commit, GPU budgets, model size, backend workspace, and workload shape can change.
Unknown headroom keeps auto conservative. Do not interpret total GPU heap capacity or a
configured pagefile size as measured free memory.

## Measured tradeoffs

On the tested Windows RTX 2070 / Kev-4B Q8_0 setup:

| Mode or check | Observed result |
| --- | --- |
| Resident smoke workload | About 4.7–4.9 GiB private commit. |
| Streaming smoke workload | About 1.25 GiB private commit; larger inputs need more workspace. |
| Broader development cases | Median 0.36 s resident versus 1.63 s streaming. |
| Auto regression | All 14 complete answer payloads matched the resident baseline; promotion occurred on request five. |
| Vision reservation | Released 4.48 GiB from the loaded resident process before acknowledging the lease. |
| Real browser smoke | All four page requests observed an unloaded Kev with an active lease; cleanup released it afterward. |

The broader 86-question comparison preserved all selected decisions, with one
probability payload differing by at most 0.0018. Both modes answered only 49/86
labelled cases correctly: equivalence preserves existing model limitations and
does not establish critic reliability. These observations are scoped to those
fixtures and hardware, not universal memory or latency guarantees.

## Reservation lifecycle

1. Kritic probes the existing local server for
   `memory_management: {"version": 1, "reservations": true}` in `/health`.
2. Before heavy work starts, Kritic sends `POST /v1/memory/reserve` with a unique client
   `id`, an `owner`, and `ttl_seconds`. Work starts only after the matching ID and
   `reserved: true` acknowledgement.
3. The server unloads Kev's engines before acknowledging the yield. New decision requests
   receive HTTP 503 with `memory_reserved` while **any** lease remains active.
4. Kritic explicitly awaits `POST /v1/memory/release` on completion or error, before control
   returns to a path that can perform Kev triage. Independent callers hold independent IDs;
   releasing one lease cannot release another caller's reservation.
5. Cancellation attempts a bounded asynchronous release. Expiry handles a crashed client
   or unavailable cleanup runtime. Native cancellation tombstones prevent a delayed reserve
   request from resurrecting a lease that has already been released.

Vision leases last 360 seconds: the 300-second client timeout plus a 60-second margin.
UI leases use `ui.timeout_s + 60`, capped at the native maximum of 3,600 seconds. When a
lease is acquired, Kritic enforces a work deadline 30 seconds before that TTL expires,
including reservation acknowledgement time, and awaits release after a timeout. A UI job
with an unusually large configured timeout must therefore fit within the reservation's
shorter deadline. Unsupported servers retain the existing workload timeout. Control requests have bounded
timeouts; redirects, credentials in URLs, and non-loopback destinations are rejected.
No service credentials are sent by this control client.

Once a server advertises support, reservation rejection or an invalid acknowledgement
fails the heavy operation before it starts. Check the local server and retry; Kritic does
not silently bypass a refused yield. A release failure is also reported; the guard attempts
cleanup again and the lease eventually expires. Unsupported capability is a harmless skip.

## Limits

A reservation yields only the cooperating Kev server's model allocations. It cannot create
RAM, evict unrelated applications, change the OS pagefile, or guarantee capacity beyond
currently measured free commit and device budgets. Other processes may allocate memory
immediately after those measurements. Additional local Kev endpoints and third-party model
servers need their own coordination; this client reserves the configured primary Laya server.

Mapped source pages may remain in the working set between normal inference requests to
avoid repeated page-fault latency. Native idle unload and acknowledged reservations release
the engine and its mapping. Lower private commit and lower active physical memory are
different measurements; compare both alongside latency when evaluating a profile.
