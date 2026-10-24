# Issue #2 Display Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved local-convergence design for safe automatic Dock operations, without implementing #1 reconnect or deploying to the live machine.

**Architecture:** Keep the daemon's latest-target executor and route all automatic Dock entry points through it. Add an Agent-local gate shared by every layout writer, scoped operation receipts, and durable pending/Degraded state. Separate confirmed physical/logical effects from completion of the current desired target.

**Tech Stack:** Rust 2021, existing Tokio/Serde/nix/chrono/std; Tauri/React build compatibility. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-24-issue2-display-safety-design.md`, approved by the user with “spec ok”. Baseline source `e0b11760ad23898761e7acedd322dc412ced604a`; design commit `975f8f3`.

## Global Constraints

- “startup、registration、Post/Thaw、monitor 四类自动 Dock 入口共享事务和保护。”
- “Agent 端所有布局写操作共享执行边界，包括 Dock、手动布局、手动旋转、传感器自动旋转。”
- “分开记录目标是否完成与操作确实造成的显示变化；ownership 只有一个提交点。”
- “自动测试使用可注入 sysfs/时钟/backend 与临时路径，不能写真实 `/var/lib`、发真实 gdctl 或操作蓝牙。”
- “不包含：#1 自动重新注册、完整 suspend/readiness 状态机、GDM/Plymouth 修改、Xe/内核/DKMS/MOK 修改、可信首次交接、新的显示任务队列、日志系统全面重构或 UI 重做。”
- “本阶段不提供‘结果未知后无证据自动恢复’。” Once bounded reconciliation latches Degraded, automatic layout writes, including sensor rotation, remain blocked for that boot. A late verified receipt may restore explicit manual layout access only.
- Existing boot-scoped ownership remains backward compatible; no unknown or stale-scope result grants ownership. Protocol mismatch fails closed.
- No installation, service restart, suspend, push, merge, or live hardware test in this execution. Provide separate deployment instructions after code verification.

## Review Focus

1. A query overtakes a sent-but-unaccepted mutation: NotFound must not permit resend or another mutation. Pin in Task 4.
2. Caller cancellation or disconnected reply socket: backend execution still owns the gate, and its result remains retrievable. Pin in Task 3.
3. Disk-full, corrupt state, or crash at receipt consumption: cannot turn uncertainty into clean defaults. Pin in Tasks 1 and 4.
4. Rotation begins while Dock changes topology: recheck under the same execution gate; stale dual-screen observations cannot reopen a panel. Pin in Task 3.
5. Target or lifecycle changes while an old attach succeeds: preserve eligible same-scope effect evidence without declaring the new target complete; changed scope cannot grant O. Pin in Tasks 4 and 5.

---

## File map and shared interfaces

All paths below are repository-relative. Do not split existing large modules except for the focused units below.

- Create `ui-tauri-react/src-tauri/src/runtime/store.rs`: durable snapshots and explicit load failures; injectable store path.
- Create `ui-tauri-react/src-tauri/src/runtime/display_safety.rs`: observation/guard/commit decisions and the Agent gate/receipt state, with tests using controlled futures.
- Modify `ipc/protocol.rs`: wire DTOs and protocol version 3.
- Modify `runtime/state.rs`: defaulted safety state, store handle excluded from serialization, boot validation.
- Modify `hardware/sysfs.rs`: structured secondary-panel observation; keep bool wrapper for existing non-safety consumers.
- Modify `runtime/session_agent.rs`: registration scope, scoped Dock dispatch, receipt/status queries, common write gate, verified backend results.
- Modify `runtime/daemon.rs`: shared Dock transaction, persistence-before-send, latest-target drain, timeout reconciliation, scoped commit, manual mutation result handling.
- Modify `runtime/monitor.rs`, `runtime/policy.rs`, `runtime/probe.rs`: publish keyboard intent independently of slow side work and use bounded probes.
- Modify `runtime/client.rs`: validate response protocol; update callers/tests of registration and session responses.
- Modify `runtime/mod.rs`: export new modules.
- Modify README runtime/development sections: safety outcomes, version alignment, verification and rollback notes.

Use existing inline Rust test modules. Use a test utility module only if shared fixtures would otherwise be duplicated. Real production seams accept injectable store/backend/observation dependencies; no production behavior may branch on a test environment variable.

Shared DTO names are fixed by this plan; internal layout remains implementer-owned:

- `PanelObservation`: Enabled, Disabled, Unknown { reason: String }.
- `DisplayScope`: boot_id, daemon_id, agent_id (String), registration_generation and lifecycle_epoch (u64).
- `DockOperation`: scope, operation_id and target_generation (u64), attached (bool), scale (f64), ownership_authorized (bool), expected_revision (u64). Agent rechecks observed panel state under its gate; the authorization is not permission to ignore Unknown or local uncertainty.
- `DisplayMutationKind`: Dock, ManualLayout, ManualOrientation, SensorRotation.
- `MutationReport`: before/after panel observation and logical layout verification, revision_before/after, and terminal status: Verified, NotStarted { reason }, KnownFailure { reason }, Unknown { reason }. A backend rejection is only KnownFailure when no mutation is established; otherwise use Unknown.
- `DockReceipt`: original DockOperation and MutationReport.
- `DisplayExecutionSnapshot`: agent_id, revision, active mutation identity, optional Dock receipt, last mutation kind/report, and blocked/uncertain state. Query never waits for the writer lock.
- `DisplaySafetyState`: current boot, lifecycle epoch, pending operation, optional receipt-consumption obligation, Degraded reason and explicit-manual permission. All new serialized fields have safe defaults; boot mismatch invalidates historical operation evidence.

IDs: use OS randomness available via std filesystem (`/proc/sys/kernel/random/uuid`) for per-process identities; missing identity source fails closed. Operation IDs are monotonic within daemon identity, and never reused under it. Wall-clock timestamps are diagnostics only.

## Task 1: Durable, injectable runtime state and panel observations

**Files:** Create `runtime/store.rs` and the initial `runtime/display_safety.rs` containing only PanelObservation; modify `runtime/state.rs`, `runtime/mod.rs`, `hardware/sysfs.rs`, and `runtime/daemon.rs` initialization.

**Interfaces:**
- Produce `RuntimeStore::new(path: PathBuf)`, `load(&self) -> Result<Option<RuntimeState>, String>`, `save(&self, state: &RuntimeState) -> Result<(), String>`.
- RuntimeState owns a cloneable `Arc<RuntimeStore>` excluded from serialization. Existing `save()` delegates to it; loaded states use the same injected store. Production Default uses `paths::state_file_path()`; test fixtures always supply a temporary store.
- Produce `observe_secondary_panel(drm_root: &Path) -> PanelObservation`; production wrapper supplies `/sys/class/drm`. Existing bool function returns true only for Enabled.
- Produce `RuntimeState::load_from(store: Arc<RuntimeStore>) -> Result<Self, String>`; only missing file gives initial state. Initialization must expose a persistence-fault state that prevents display writes rather than silently defaulting and overwriting a malformed file.

- [x] **Step 1: Write failing persistence/observation tests.** Assert that missing state creates a safe initial state, valid old JSON loads, malformed/unreadable existing state returns Err, Unknown is distinct from Disabled, and invalid writes cannot truncate the prior committed JSON. Add injected save-stage failures for write, file-sync, rename and directory-sync. For each failure, pending data is either old-complete or new-complete, never partial; failure never reports success.
- [x] **Step 2: Run:** `cargo test --lib runtime::store::tests` and `cargo test --lib hardware::sysfs::tests`. New behaviors must fail before implementation; existing ordinary parsing tests may remain green.
- [x] **Step 3: Implement atomic saves** using same-directory unique create_new temp file, serialize/write_all, sync_all, rename and parent-directory sync. Clean up abandoned temp files without deleting the committed snapshot. Propagate errors to safety callers. Do not erase a corrupt input file during fault-mode initialization. Add a test-only faulting filesystem seam inside store tests, not a global env override.
- [x] **Step 4: Route every RuntimeState writer through the store.** Existing daemon tests receive temporary stores before any save-capable function runs. Confirm concurrent saves use the same store synchronization and existing RuntimeState locks; do not allow an older cloned snapshot to overwrite newer pending safety fields.
- [x] **Step 5: Run focused tests** plus existing state ownership tests, then `git diff --check`. Commit `fix(runtime): persist display safety state atomically`.

## Task 2: Scoped protocol and pure safety decisions

**Files:** Modify `ipc/protocol.rs`, `runtime/state.rs`, `runtime/client.rs`; extend Task 1’s `runtime/display_safety.rs`; update affected fixtures/callers for compilation.

**Interfaces:**
- Define the shared DTOs above; protocol version becomes exactly 3.
- Extend registration with Agent identity; reply with accepted DisplayScope, without waiting for layout replay.
- Session commands: scoped Dock operation, `GetDisplayExecutionState`, exact scope/ID `ConsumeDockReceipt`, and context binding for registration/lifecycle. Reject legacy unscoped automatic SetDockMode.
- Session responses: scoped Dock receipt, execution snapshot, registration/context acknowledgement, and explicit reject/error. Ordinary manual replies must identify revision and effect evidence needed to invalidate ownership.
- Produce `automatic_dock_allowed(attached: bool, observed: &PanelObservation, ownership: bool, uncertain: bool) -> bool` and `classify_dock_commit(current_scope: &DisplayScope, latest_target: u64, receipt: &DockReceipt) -> CommitDecision`.
- Define `CommitDecision` outcomes EvidenceAndTargetComplete, EvidenceOnly, NotStarted, RejectStaleScope and Unknown; define `DisplayReject` reasons Busy, StaleScope, RevisionChanged, InvalidOperation and Degraded. Each carries a diagnostic reason where needed. These outcomes separate eligible evidence from matching-target completion. Do not use an old target-generation mismatch alone to discard a verified same-scope close effect.

- [x] **Step 1: Write failing decision and wire tests.** Round-trip the new operation, snapshot and receipt; reject protocol 2. Assert `allowed(false, Disabled, false, false)==false`, Unknown is blocked even with O=true, Enabled/O=false sync is allowed, and matching scope plus verified old-target close yields evidence but not target completion. Each individual scope field mismatch rejects new O. Error response is never a readiness success.
- [x] **Step 2: Run:** `cargo test --lib ipc::protocol` and `cargo test --lib runtime::display_safety::tests`. Record expected failures.
- [x] **Step 3: Implement DTOs and pure decisions.** Add defaulted safety fields and boot reset rules. Validate protocol on daemon, Agent and client response paths; no silent fallback to protocol 2. Update existing match arms and tests so intermediate code compiles, while unimplemented scoped mutations explicitly reject rather than bypass safety.
- [x] **Step 4: Run tests** above and `cargo check --bins`. Commit `feat(runtime): scope display operations and outcomes`.

## Task 3: Agent execution gate, receipts and verified backends

**Files:** Modify `runtime/display_safety.rs`, `runtime/session_agent.rs`, relevant bounded command helpers in `hardware/display_config.rs`.

**Interfaces:**
- Produce `AgentDisplayGate::new(agent_id: String)`, `bind_scope(&self, scope: DisplayScope) -> Result<(), String>`, `snapshot(&self) -> DisplayExecutionSnapshot`, and `consume(&self, scope: &DisplayScope, operation_id: u64) -> Result<(), String>`.
- Produce `start_write<F, Fut>(self: &Arc<Self>, kind: DisplayMutationKind, scope: DisplayScope, operation: Option<DockOperation>, run: F) -> Result<tokio::task::JoinHandle<MutationReport>, DisplayReject>`, where F is a Send+'static FnOnce returning a Send+'static future yielding MutationReport. Start owns the gate before reporting acceptance; the spawned execution owns it through verify/result recording. Recheck expected_revision inside the gate; RevisionChanged means NotStarted, never execute with stale authorization. Use immediate try-lock rejection, not a mutation queue.
- Identical pending operation ID returns existing state/receipt; differing parameters under that ID reject. Gate state stores only active work plus one unconsumed Dock receipt; manual work and sensor work cannot replace it.
- Backend execution returns structured MutationReport. GNOME verifies logical state and samples panel observation inside the gate; KDE/Niri use their current readback paths and conservatively report Unknown if they cannot prove the expected layout.

- [x] **Step 1: Write failing gate tests with controllable futures.** Start a blocked Dock and assert maximum concurrent writes is 1 across all four mutation kinds; status responds without waiting. Drop the response waiter and assert execution stays active and stores its receipt. Busy must never execute later. Duplicate operation executes once. Wrong consume ID cannot delete a receipt. Rebinding scope never silently unlocks active/unknown work.
- [x] **Step 2: Run:** `cargo test --lib runtime::display_safety::tests::agent` and verify the missing gate behavior is red.
- [x] **Step 3: Implement gate and route all actual writers through it.** Registration passes one shared gate to session handlers and sensor watcher. Sensor dual-screen check occurs inside execution, not just before acquiring the gate. Move synchronous backend work off async workers; owned blocking task holds the gate until completion even if a caller is cancelled. Manual layout always invalidates automatic ownership through its reported revision; orientation retains O only with a proven unchanged enabled-set.
- [x] **Step 4: Add failing command-runner tests.** A timed-out helper is terminated and reaped before the local execution releases; no fallback after timeout/ambiguous partial execution. Since no reliable not-executed classifier exists initially, disable automatic attached fallback conservatively rather than guessing from stderr. Readback failure after a successful command yields Unknown, never new O. Use a harmless local sleeping test executable or controlled runner, never real gdctl.
- [x] **Step 5: Implement explicit child cleanup and structured failure classification.** Removing unbounded command fallbacks is allowed; no timeout path may silently call an unbounded shell alternative. Keep query and status handlers responsive under fake blocking backend work.
- [x] **Step 6: Run gate/backend tests, existing GNOME/KDE/Niri argument and locale tests**, then `cargo check --bins`. Commit `fix(runtime): serialize agent display mutations and retain outcomes`.

## Task 4: Unified daemon transaction and reconciliation

**Files:** Modify `runtime/daemon.rs`, `runtime/state.rs`, `runtime/monitor.rs`, `runtime/display_safety.rs`.

**Interfaces:**
- Produce `submit_dock_target(state: &Arc<RwLock<RuntimeState>>, attached: bool, scale: f64, source: DockSource) -> Result<u64, String>`: publish latest target and ensure one drain worker; return generation without awaiting backend completion.
- Produce `drain_dock_targets(state: Arc<RwLock<RuntimeState>>) -> Result<(), String>`: sole automatic operation sender and ownership commit point.
- Produce `reconcile_pending_dock(state: &Arc<RwLock<RuntimeState>>, mode: ReconcileMode) -> Result<(), String>`: Active or LateCleanup; receipt queries only.
- Fixed initial budgets: automatic mutation response 12 seconds retained; Active reconciliation at most 3 queries, each capped at 1200 ms, with 200 ms between queries. LateCleanup runs at most once per second, one capped query per tick. Agent snapshot is cheap; no larger timeout is the safety fix.
- DockSource covers Monitor, Startup, Registration, Resume; ReconcileMode distinguishes bounded active recovery from permanent automatic Degraded cleanup.

- [x] **Step 1: Write failing socket-level entry tests.** All four sources, Disabled/Unknown with O=false, send zero mutations; normal Enabled attach→verified close→detach grants then clears O. Startup cannot bypass held executor. Use the actual production submit/drain/registration functions with temporary store and fake Agent socket. Assert outcomes, not only a helper's Boolean.
- [x] **Step 2: Write failing interleaving tests.** Delay old attach, publish new detach while work runs, release old same-scope receipt, then assert latest target alone completes and its enable can use the legitimate prior close evidence. Switch any scope field before reply and assert no current O is granted. Superseded target cannot retry after a new target is visible.
- [x] **Step 3: Run:** `cargo test --lib runtime::daemon::tests` with only test fixtures that cannot access production paths. Record RED evidence for the newly introduced cases.
- [x] **Step 4: Implement common target publication and drain.** Startup keeps its bounded readiness observation but cannot fall through to an unguarded mutation. Remove monitor's duplicate O submission. Use one RuntimeState lock boundary for target/scope/ownership checks and commit. Release state locks over I/O; take the Agent under-gate observations as operation evidence rather than trusting the outer sample. Persist pending before writing request bytes; save failure aborts dispatch.
- [x] **Step 5: Add failing timeout/crash tests.** Response timeout leaves pending and sends no second mutation. NotFound cannot unlock it. Test all 3 active queries, then the Degraded latch and 1Hz late cleanup. Receipt completion after exhaustion enables only verified explicit manual access, never automatic replay/sensor rotation. Persist fails before dispatch or before receipt consumption: zero unsafe actions. Crash between persisted outcome and consume resumes exact-ID cleanup without reexecution.
- [x] **Step 6: Implement reconciliation and durable acknowledgement obligation.** Store terminal evidence/consumption obligation before consuming, then durably clear that obligation after acknowledgement. Duplicate consumption is harmless for the same ID and never affects another receipt. On daemon restart unresolved old-daemon pending becomes Degraded; it may be cleaned only as evidence, never adopted as current O. No loop re-registers Agent or sends mutation as a health check.
- [x] **Step 7: Update manual mutation processing.** Refuse while safety storage fails, active/unknown work persists, or current scope cannot be verified. A verified same-scope late completion may restore explicit manual access while auto Degraded stays latched. Manual completion applies revision-based O invalidation before automatic intent resumes. The transaction checks snapshot revision and last-mutation report too, so a lost/delayed manual reply cannot preserve stale O; an unexplained revision advance conservatively revokes O. Reject operation revision mismatches at the Agent gate. Add an explicit manual-completion-before-daemon-reply race test. Inform sensor gate of the persistent auto block during context binding.
- [x] **Step 8: Run focused daemon tests, store/gate regressions and `cargo check --bins`.** Commit `fix(runtime): unify safe dock transactions and timeout reconciliation`.

## Task 5: Lifecycle/input integration, full regression and delivery

**Files:** Modify `runtime/daemon.rs`, `runtime/monitor.rs`, `runtime/policy.rs`, `runtime/probe.rs`, `runtime/session_agent.rs`, README and approved spec progress checkboxes.

**Interfaces:**
- Produce `advance_display_epoch(state: &Arc<RwLock<RuntimeState>>, phase: LifecyclePhase) -> Result<(), String>`: persist new epoch and communicate context before new writes; lifecycle event publication precedes fallible backlight work.
- Monitor publishes latest keyboard/target independently of slow layout/BT probes. Reuse sysfs-only `keyboard_attached()` for this fast signal; slow status probes must not overwrite a newer keyboard observation when committing their result.
- Policy's hardware side work uses owned bounded subprocess runners. Existing functions can change async signatures to use Tokio; update every caller together, without a new Bluetooth policy setting.

- [x] **Step 1: Write failing lifecycle/progress tests.** Inject backlight Err and a never-finishing BT/probe process: latest Dock target still updates. In-flight attach spanning Pre→Post yields no new O, even if it later verifies; previously committed same-boot O is not erased solely by suspend. Context update racing a write must leave the Agent rejecting stale queued scope, and daemon cannot dispatch until context acknowledgement.
- [x] **Step 2: Run lifecycle/monitor/policy tests and verify RED**, then implement epoch publication, nonblocking fast keyboard intent and bounded side work. In particular `probe::current_status()` currently invokes slow subprocesses; moving only policy work would leave the input publication blocked, so split or isolate the hardware-only signal and bound remaining probes.
- [x] **Step 3: Add and run end-to-end simulated sequence tests.** Four-source contention plus sensor/manual work never overlaps; malformed protocol/state/backend responses fail closed; old protocol receives explicit mismatch; repeated same-boot daemon/Agent restarts cannot unlock uncertain work. New boot invalidates old pending/O without falsely claiming actual screen recovery. No source path sends legacy unscoped SetDockMode.
- [x] **Step 4: Update README** with pending/Degraded meanings, manual-access limitation, protocol-3 binary alignment, source/build provenance checks, and rollback to the pre-implementation commit. Explain that #1 reconnect and hardware recovery remain separate. Keep product-facing messages plain; operation IDs belong in logs, not UI instructions.
- [x] **Step 5: Run the complete verification set** from `ui-tauri-react/src-tauri`: `cargo fmt --check`, `cargo test`, `cargo check --bins`; from `ui-tauri-react`: `npm run vite:build`; repository root: `bash tests/install-stdin-test.sh`, `bash tests/xe-backlight-args-test.sh`, `git diff --check`. Restore existing cached dependencies with lockfile-preserving commands if required; no system installs.
- [x] **Step 6: Perform an independent whole-branch review** against the approved spec. Address important issues with targeted regression tests, repeat affected tests, then repeat the full suite if fixes change runtime behavior. Report any remaining limitation or failed check by name, including baseline failures.
- [x] **Step 7: Commit** `fix(runtime): integrate display safety across lifecycle and input`; attach final test evidence and review outcome to the execution ledger. Leave the implementation branch local and reviewable. Do not deploy or claim #1/hardware suspend recovery is fixed.

## Self-review and execution status

- [x] Spec coverage: §§4–7 map to Tasks 1–5; §8's 13 test classes each have an owning task.
- [x] Interface consistency: operation/scope/receipt definitions above are shared; query never takes mutation lock; only daemon drain commits automatic O.
- [x] Review Focus: all five conditions have explicit failure assertions in named tasks.
- [x] Scope: no reconnect, GDM, kernel, dependency additions or live deployment.
- [x] User reviewed the plan and chose Native: “native 开始”.
- [x] Execution uses the explicitly selected current feature branch; baseline 84 tests passed; per-plan ledger created.

Execution method: Native, selected by the user. These five tasks are closely coupled, with an independent final branch review; all tasks share the same protocol, scope and receipt contracts, so maintaining one implementation context reduces integration churn. The approved design remains the authority if a detail in this plan conflicts with it.

## Delivery evidence

Native execution completed through source commit `4c96f31`. Independent review found four Important issues; one regression-driven fix pass addressed all four. Final source suite: 115 library tests plus all binary/doc targets pass. Binary check, formatting, frontend build, both shell tests and diff checks pass. See `../reports/2026-10-24-issue2-display-safety-verification.md` for findings, evidence, rulings and remaining hardware/deployment limits. Branch remains local; nothing deployed or pushed.
