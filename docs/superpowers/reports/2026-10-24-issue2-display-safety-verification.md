# Issue #2 display-safety implementation verification

Date: 2026-10-24. Repository: cwangfr/zenbook-duo26-Ubuntu26.04.
Branch: `codex/issue2-display-safety-design`. User selected Native execution in the existing feature branch.

## Scope and provenance

Approved design: `../specs/2026-10-24-issue2-display-safety-design.md`.
Implementation plan: `../plans/2026-10-24-issue2-display-safety.md`.
Pre-implementation revision: `2b95a38`; original code: `e0b1176`.
Initial implementation commits: `2094c07`, `85d4828`, `9cab964`, `8e0829d`, `1c3e703`.

This implements the minimal display-safety prerequisite, not all of issue #2 and not issue #1 reconnection. GDM handoff, kernel/Xe recovery and actual suspend/resume acceptance are outside this delivery. No runtime installation, service restart, radio mutation or real modeset was performed by the implementation/verification workflow.

The installed machine runtime was not rebuilt or replaced. Its observed kernel was `7.0.0-38-generic`. Automated tests exercise the checked-out source with temporary state/DRM fixtures, real Unix sockets and a fake hardware driver; they are not hardware acceptance evidence.

## Verification evidence before independent review

- Baseline: 84 library tests passed.
- Final initial implementation: `cargo test --offline --manifest-path ui-tauri-react/src-tauri/Cargo.toml`: **109 library tests passed**, all binary and doc-test targets passed.
- `cargo check --offline --manifest-path ui-tauri-react/src-tauri/Cargo.toml --bins`: passed.
- `cargo fmt --manifest-path ui-tauri-react/src-tauri/Cargo.toml --check`: passed.
- `npm run vite:build` from `ui-tauri-react`: passed, with a bundle chunk-size advisory. Missing local TypeScript tooling was restored using `npm ci --no-audit --no-fund`; the lockfile did not change. npm noted an unapproved optional msw postinstall; no approval override was used or required for this successful build.
- `bash tests/install-stdin-test.sh`: PASS.
- `bash tests/xe-backlight-args-test.sh`: PASS.
- `git diff --check`: passed.

RED evidence was observed for missing persistence/observation behavior, scoped guard/commit logic, Agent gate retention/dedup, child reaping on timeout, readback verification, UI error propagation, Agent-side Degraded propagation before receipt consumption, lifecycle/input publication, clock rollback, explicit old-protocol rejection, contradictory kernel/logical state and restrictive file permissions. The durability-stage test was also mutation-checked by removing its file-sync error check and observing failure before restoring it.

The real 12-second daemon response deadline was exercised over a fake Agent socket: an operation remained active, no second modeset was sent, uncertainty was persisted and late verified cleanup restored only explicit manual access. No arbitrary short sleep was substituted for that timeout regression.

## Implementation rulings

1. Native work stays in the explicitly selected current feature branch. This avoids unnecessary worktree/cache duplication; main is unaffected. Risk: local implementation edits before task commits.
2. The installed task-start wrapper tried to execute a non-executable task-brief script. The same helper was run via bash and task bases recorded manually; no installed skill files changed. No runtime cost.
3. Backend readback/reporting lives in a focused `display_backend.rs` module. This separates evidence from gate state. No behavior change from the intended design.
4. Attached compatibility fallback is disabled because the current code cannot reliably distinguish a rejected, unexecuted command from partial/unknown execution. Cost: older Mutter can return an explicit failure rather than try fallback.
5. The daemon executor is per RuntimeState rather than process-global. It still serializes the single daemon and allows isolated real-function tests. No intended production behavior difference.
6. UI/profile direct display-write fallbacks were removed: otherwise daemon rejection would bypass the Agent gate. Cost: callers now see explicit errors instead of applying an unsafe fallback.
7. Manual in-flight operations get a durable pending flag as well. A timed-out manual operation must not disappear across daemon restart. Cost: uncertain manual outcomes may require reboot.
8. Legacy tests expecting unscoped Dock/retry were replaced with scoped socket-level tests. Registration responsiveness, startup lock coverage and orientation deadlock regression remain covered through the new path.
9. Snapshot ordering uses a separate monotonic revision, not the diagnostic wall clock. Clock rollback cannot invalidate valid saves; stale conflicting snapshots are refused.
10. A minimal suspended flag accompanies lifecycle epochs, blocking new layout writes between Pre and Post. It does not implement the deferred full resume readiness/reconnect state machine; #1 remains necessary.

## Deployment and rollback boundary

Before deployment, stop all old UI/tray and runtime processes and build all protocol-3 participants from the same revision. An already running old UI can still contain a direct-backend fallback; installing new files alone does not replace that process. Capture actual running executable hashes, installed hashes, ExecStart, Cargo.lock, source revision, boot ID, locale and kernel.

For rollback, preserve the implementation branch and use the matching runtime/UI from `2b95a38`. Do not run old code over an unresolved operation in the same boot; use a clean reboot before returning to the old runtime. The old implementation retains its original safety limitations.

## Independent review

A fresh-context independent reviewer reviewed `2b95a38..1c3e703` and requested changes: one P1 and three P2 findings. All four were accepted as Important and addressed in one author-executed fix pass, commit `4c96f31`. The Native workflow uses regression tests and the full suite after that pass; no second independent review was performed.

| Finding | Repair and evidence |
| --- | --- |
| Sensor uncertainty lost on Agent replacement (P1) | Scoped sensor intent goes through the daemon executor and persists `manual_pending` before sending to the Agent. Unknown and in-flight daemon loss retain the obligation across same-boot daemon/Agent replacement. Automatic Degraded and the Agent's in-gate dual-panel check remain enforced. Original direct sensor admission failed the durable-pending assertion; the replacement production path passes. |
| Repeated cleanup consumes unsaved receipt (P2) | Every consumption attempt rejects storage faults and durably saves the obligation first. The second-cleanup regression failed with a missing Agent receipt before the fix and now retains pending and receipt. |
| Sensor contention abandons latest Dock target (P2) | Retain current intent for exact Busy/RevisionChanged NotStarted results and wake healthy unsent targets on bounded cleanup ticks. A held sensor initially left zero Dock calls; it now converges after release. Dispatch-time revision rejection is also covered by a regression mutation-checked against the removed fix. Uncertain dispatched operations are never resent. |
| Delayed registration overwrites newer keyboard input (P2) | Registration/startup replay samples the authoritative current keyboard state and scale under the target-publication lock, after binding. Paused-bind regression initially overwrote attached status; it now retains the latest observation and applies the correct direction. |

Final source verification at the tree committed as `4c96f31`:

- `cargo test --offline --manifest-path ui-tauri-react/src-tauri/Cargo.toml`: **115 passed; 0 failed**, 12.43 seconds; all binary and doc-test targets passed.
- `cargo check --offline --manifest-path ui-tauri-react/src-tauri/Cargo.toml --bins`: passed, no warnings.
- `cargo fmt --manifest-path ui-tauri-react/src-tauri/Cargo.toml --check`: passed.
- `npm run vite:build`: passed; existing chunk-size advisory remains.
- Both shell tests and `git diff --check`: passed.

The full suite was repeated after routing startup through the same current-observation replay helper. The only later changes are this verification documentation and plan/ledger bookkeeping. No deferred Minor review findings.

Historical independent findings are preserved in `2026-10-24-issue2-display-safety-independent-review.md`; their line numbers refer to reviewed commit `1c3e703`, not the repaired source. The execution record is preserved in `2026-10-24-issue2-display-safety-execution-ledger.md`.

## Final review rulings

11. Sensor writes reuse durable `manual_pending` and the daemon executor; no second queue is needed. Actual execution and topology checks remain in the Agent gate. Cost: uncertain sensor outcomes conservatively require reboot.
12. Physical Xe/Mutter, suspend recovery and installed provenance remain unverified: no deployment/hardware action was authorized. Cost: hardware acceptance is still required.
13. Issue #1 reconnection/full readiness stays deferred as approved. Cost: missing backend readiness can still prevent progress.
14. Cosmetic cleanup and dead legacy diagnostic helpers were left outside the functional fix pass. No functional cost identified.
15. Hostile protocol clients and external compositor writes remain outside the stated runtime-owned-writer guarantee. Cost: external mutations can invalidate observations.
