## Kritic TEST RELEASE

Optional `technical_debt_critic` is disabled by default. Use it for an explicit
maintenance review, offline unless the user authorizes public package metadata lookup.
Inspect coverage gaps and source/date/status evidence; validate urgency and estimated effort.
Never invent installed versions, migration requirements or environment-variable values, and
never apply an upgrade merely because the report lists a newer release. See docs/technical-debt.md.

Optional review_history is disabled by default. Record only an explicitly selected
saved report and exact input corpus/context. Historical hashes are not fresh verification or
proof that a report ran over those inputs. jcode-format exports are separate low-trust files,
not a live integration; never reuse a historical pass as current acceptance.

Use the Kritic skill to test its viability. Independently validate every critic claim against
source, executed checks and the task requirements. Never interpret approval as permission to ship.
Inspect status and available tools first; use artifact_validate(use_model=false), contract_critic
and only installed, enabled checks. Missing dependencies, empty tests and skipped coverage are
unverified. Do not install models or API credentials merely to satisfy the skill.

Reproduce Kritic defects, fix them in the Kritic repository with meaningful regression coverage,
and submit a focused PR to https://github.com/TmRxJD/kritic. Keep private project data out of
public reports. Report actual evidence, false positives, fixes and remaining limitations.

On test.4 and later, use repository_critic for explicit repository policy and staged scope;
it is disabled by the minimal profile. Follow the repository's own
naming, layers and purposes. Review commit hunks for relevance and semantic cohesion, excessive
file size, modular ownership and non-obvious function explanations. Install hooks only through
the explicit repository workflow; budgets and scope declarations do not prove those qualities.
