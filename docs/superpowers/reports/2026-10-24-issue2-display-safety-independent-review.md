# Independent final review — 2b95a38..1c3e703

Reviewer: single independent Rust/Linux concurrency reviewer. Reviewed commit `1c3e703106a4f4f4ebb37c04d4e8e8617880bbb8`, the full implementation range, approved design and plan, and execution ledger/Rulings. Implementation, Git state, and live services were not modified. Existing tests were not rerun; the recorded verification is accepted as build/test evidence, not evidence for the untested interleavings below.

**Verdict: changes required.** Four actionable findings: one P1 and three P2. The implementation substantially improves the boundary, but sensor uncertainty does not survive Agent replacement, and three production paths lose required evidence or current intent.

## Findings

### 1. [P1] Persist sensor uncertainty before allowing the next Agent to write

**Primary location:** `ui-tauri-react/src-tauri/src/runtime/session_agent.rs:1319–1327`.
Supporting locations: `runtime/display_safety.rs:529–530`, `runtime/daemon.rs:596–605` (all relative to `ui-tauri-react/src-tauri/src/`).

Sensor rotation calls the Agent gate directly with no durable operation record. An Unknown result sets only `AgentDisplayGate.inner.blocked`; the watcher discards the report except for its Verified test. The daemon's periodic status path reads layout, not execution state, so an idle system need never persist this Unknown outcome. Registration only degrades when daemon `pending` or `manual_pending` is set; sensor work sets neither.

**Trigger:** enable auto-rotation; let a sensor modeset time out or fail verification while both panels remain observably enabled; restart the Agent before a subsequent Dock transaction discovers its blocked snapshot. The new gate starts unblocked, registers against a daemon with no pending/degraded state, and may immediately rotate or accept automatic Dock again. A crash during the rotation has the same missing durable boundary.

**Impact:** same-boot Agent replacement erases an uncertain automatic display operation and permits another display write without reconciliation. This violates the approved permanent automatic Degraded contract and the explicit prohibition on using Agent restart to clear uncertainty. It is an actual safety hole, rather than merely missing diagnostics.

**Fix direction:** establish a durable pre-execution obligation for sensor writes and resolve it using scoped evidence, or conservatively block replacement when the previous Agent's write history cannot be established. Reporting Unknown only after completion is insufficient for the crash window. No general task queue is needed.

**Required regression:** a sensor backend becomes Unknown (and separately dies in flight), replace its gate/Agent and register it in the same boot, and assert zero new automatic/manual writes until the required safety resolution. Include daemon restart as well.

### 2. [P2] Do not consume an obligation whose save failed

**Primary location:** `ui-tauri-react/src-tauri/src/runtime/dock_transaction.rs:466–473`.
Supporting locations: the same file at `388–389` and `426–460`.

`finish_receipt` puts the terminal receipt into the live `receipt_to_consume` field before saving. If that save fails, the first reconciliation correctly returns without consuming. However, the next cleanup tick sees the in-memory field and calls `consume_saved_receipt` directly. That function does not check `storage_fault` or establish that the obligation was durably saved: it pushes the blocked policy and sends Consume anyway. It then clears both pending fields before another save that necessarily fails while `storage_fault` remains set.

**Trigger:** pending was persisted and the Agent completed; disk-full/write/sync failure occurs while saving the terminal receipt; run a second reconciliation/normal one-second cleanup tick with the Agent reachable.

**Impact:** the Agent's only terminal receipt is deleted although its evidence/consumption obligation never reached disk. A later daemon restart finds the old pending operation but can no longer retrieve its terminal evidence, making otherwise reconcilable work permanently unresolved. The current fault latch prevents an immediate unsafe display unlock, so this is P2 rather than a claim of clean defaults. It nevertheless violates the explicit save-before-consume safety contract.

**Fix direction:** distinguish a persisted consumption obligation from a merely staged one, and prohibit consumption after a failed safety-state save. Preserve pending/receipt evidence in memory until durable completion is established. The existing `persistence_failure_prevents_dispatch_and_consumption` test calls reconciliation only once, which misses this path.

**Required regression:** inject the terminal-evidence save failure, invoke reconciliation twice (or run the cleanup loop), and assert the Agent still retains the receipt and the daemon retains its pending record.

### 3. [P2] Resume the current Dock target after transient sensor contention

**Primary location:** `ui-tauri-react/src-tauri/src/runtime/dock_transaction.rs:240–241`.
Supporting locations: the same file at `188–202` and `372–376`; `runtime/monitor.rs:618–619`.

If a sensor rotation owns the Agent gate when Dock reads the snapshot, `drain_one` returns Busy. The drain stops and clears `worker_running` unless a different target happened to arrive. There is no subsequent wakeup for the existing target: the keyboard monitor only submits on a physical edge, and late cleanup only handles pending operations. If rotation starts after the snapshot instead, a Busy/RevisionChanged NotStarted result also discards the matching current target.

**Trigger:** rotate the device and attach the keyboard during the sensor backend operation; leave the keyboard attached after rotation completes. The automatic attach can stop without ever sending its modeset. The analogous dispatch-time revision race also abandons the latest intent.

**Impact:** ordinary rotation/Dock contention leaves the bottom panel in the wrong state until another physical transition, registration, or explicit user action. The common executor has serialized writes but has lost convergence on the latest target.

**Fix direction:** preserve the latest target and arrange bounded re-observation/wakeup for positively NotStarted contention. Retry authorization against a fresh snapshot/revision and latest target; do not retry uncertain, sent-but-unconfirmed operations or superseded targets.

**Required regression:** hold a sensor task at the gate, submit a single attach target, finish the sensor task, and assert that the same latest target eventually completes without another keyboard edge. Cover a revision change between snapshot and admission too.

### 4. [P2] Do not publish a registration-time keyboard snapshot after newer input

**Primary location:** `ui-tauri-react/src-tauri/src/runtime/daemon.rs:613–622`.
Supporting locations: the same file at `609–611`; `runtime/dock_transaction.rs:154–160`.

Registration captures `attached` before spawning its replay work, then awaits context binding before submitting that captured value. The independent keyboard monitor can publish a newer physical observation during that await. The delayed registration/startup task then submits the old value as a *newer* target generation, and `submit_dock_target` even writes it back into `status.keyboard_attached`. Generation checks at dispatch cannot detect that this newer generation represents older input.

**Trigger:** registration observes detached; pause its Bind reply; attach the keyboard and allow monitor publication; then release the registration replay. It can replace the current attach intent with detached. The inverse interleaving is also possible.

**Impact:** the runtime may apply the opposite layout after already learning the current physical state, producing an unnecessary open/close cycle. The next 250 ms sample can repair the target, but an already accepted backend mutation is deliberately not cancelled, so polling does not remove the regression. This defeats the design's latest-observation guarantee.

**Fix direction:** publish replay intent from the current observation under the same state boundary, or carry an observation version and reject a replay whose sampled input has been superseded. Replays should not overwrite authoritative keyboard observations with captured values.

**Required regression:** pause registration context binding, publish the opposite keyboard state, resume registration, and assert that no stale-direction operation is admitted and that the newer keyboard observation remains authoritative.

## Review Focus assessment

1. **Sent-but-unaccepted mutation / NotFound:** daemon reconciliation preserves pending and never resends on a missing receipt. No contrary issue found in the Dock path.
2. **Caller cancellation / disconnected reply:** accepted work runs in an independently spawned task owning the gate through backend completion and receipt recording. Dropping the socket waiter does not cancel it. This does not cover process death; finding 1 concerns that durable gap for sensor work.
3. **Disk-full / corrupt state / consumption crash:** atomic replacement and corrupt-load refusal are appropriate; a persisted consume obligation survives acknowledgement loss. Finding 2 breaks save-before-consume on the second cleanup pass.
4. **Rotation versus Dock topology:** sensor readback and dual-panel check occur inside the owned gate, preventing the specified stale outside-gate topology check. Finding 3 concerns missed progress after safe rejection; finding 1 concerns replacement after uncertain rotation.
5. **Old attach versus target/scope changes:** receipt classification separates same-scope effect evidence from target completion, while lifecycle/registration changes reject new ownership under one RuntimeState boundary. Finding 4 is an earlier input-version problem: stale intent can receive a fresh target generation.

## Spec-compliance verdict

**Not yet compliant.** The main scoped Dock protocol, single Agent gate, durable pending Dock/manual work, conservative Unknown result handling, atomic state store, same-scope effect handling, removal of direct UI write fallbacks, protocol upgrade, and independent keyboard publication match the approved design. Findings 1 and 2 contradict the explicit uncertainty/persistence contract; findings 3 and 4 leave the latest-target behavior incomplete. The recorded module and execution-method Rulings are reasonable and do not justify these gaps. No unrelated scope expansion requiring rollback was found.

## Code-quality verdict

**Good structural direction, changes required for state-transition correctness.** Focused gate, backend-evidence, transaction, and persistence modules are useful boundaries. Owned execution and nonblocking snapshots are appropriate concurrency choices. The problematic edges come from encoding staged versus durable evidence in the same fields and from treating transient refusal as the end of current-intent processing. Tests cover many happy and selected failure paths but do not exercise a second cleanup pass after persistence failure, sensor restart uncertainty, sensor contention followed by drain resumption, or delayed replay publication. No repository-specific AGENTS.md/CONTRIBUTING/CODING_STANDARDS source was found, and no purely stylistic finding is raised.

## Verification evidence and limits

The ledger records 109 passing library tests plus binary/doc targets, `cargo check --bins`, formatting, frontend build, both shell tests, and diff checks at the reviewed revision. Existing cargo/build logs were inspected; frontend reports only its documented chunk-size advisory. No test, command exercising real display hardware, installation, service restart, suspend, Git mutation, or additional agent was run for this review. Findings are based on the concrete production control flow; proposed regressions have not been executed.

## Declined to judge

- Physical Xe/Mutter health, real suspend recovery, installed-binary provenance, and live GNOME/KDE/Niri behavior: no hardware/deployment actions were authorized for this review; simulated and build evidence cannot establish them.
- Issue #1 reconnection and full resume readiness: explicitly outside the approved scope.
- Cosmetic refactors, dead legacy diagnostic helpers, and format/lint issues already covered by tools: no demonstrated user impact, and this review prioritizes safety and regressions.
- Hypothetical hostile local protocol clients or external compositor writers: outside the stated guarantee. Findings above use the actual runtime's own paths.
