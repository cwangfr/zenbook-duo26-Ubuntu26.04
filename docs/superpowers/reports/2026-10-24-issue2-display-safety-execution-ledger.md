# SDD ledger — plan: docs/superpowers/plans/2026-10-24-issue2-display-safety.md
Executor: Native, explicitly selected by user; in-place feature branch selected at handoff, no worktree required. Base 2b95a38; design 975f8f3; source e0b1176. No live deployment.
Pre-flight: Tasks 1→2 share PanelObservation/store; Task 1 creates the observation enum before Task 2 extends types. Compatible.
Pre-flight: Tasks 2→3 share protocol DTOs, scope, receipt, reject and gate contracts. Compatible; protocol upgrades fail closed until integration.
Pre-flight: Tasks 3→4 share execution snapshot, revision, receipt consume. Compatible; pending persists before send, result before consume.
Pre-flight: Tasks 4→5 share nonblocking intent publication and lifecycle epochs. Compatible; partial in-flight results across epochs degrade.
Ruling: Native execution in the current feature branch follows the user's selection of the explicitly proposed in-place mode — avoids unnecessary worktree and duplicated build cache — no impact on main; risk is local uncommitted edits during implementation, mitigated by task commits.
Task 1: in progress.
Ruling: task-start script directly executes a non-executable installed task-brief — use bash task-brief and record BASE manually rather than changing installed skill files — no behavior cost.
Task 1 BASE: 2b95a38.
Task 1: complete (2b95a38..HEAD, baseline 84/84; RED store/observation logs; durability mutation failed as expected; task1-final.log 88/88).
Task 2: in progress.
Task 2: complete (2094c07..HEAD; task2-red and wire-red failures; task2-green.log 93/93, task2-check.log binary check passed).
Task 3: in progress. Gate tests RED via unimplemented gate (task3-red.log).
Task 3: Ruling: readback evidence lives in focused display_backend.rs rather than further expanding session_agent.rs — separates backend evidence from gate state — no behavior cost.
Task 3: Ruling: no reliable not-executed fallback classifier exists, so attached compatibility fallback is disabled — follows approved conservative rule — older Mutter may reject attached changes instead of trying a fallback.
Task 3: complete (85d4828..HEAD; gate/runner/readback RED logs; task3-final.log 98/98; task3-check.log passed).
Task 4: in progress.
Task 4: Ruling: daemon execution lock is per RuntimeState rather than a process-global static — same single-daemon semantics, isolated real-function tests — no production behavior cost.
Task 4: Ruling: remove UI and profile direct-write fallbacks after daemon rejection; they otherwise bypass the approved Agent gate — user sees explicit errors where a fallback previously applied — required for safety.
Task 4: Ruling: track manual_pending durably too, since a manual timeout can outlive its caller and daemon restart — prevents silent uncertainty loss — manual outcome uncertainty may require reboot.
Task 4: Ruling: legacy integration tests expecting unscoped SetDockMode/unsafe retry were replaced by new real-socket scoped transaction tests, preserving registration responsiveness/startup serialization/orientation deadlock coverage — tests now assert the approved protocol rather than removed behavior.
Task 4: complete (9cab964..HEAD; task4-red/fallback-red/policy-red; task4-final.log 101/101 including real 12-second timeout; task4-check passed).
Task 5: in progress.
Task 5: Ruling: snapshot fencing uses a separate monotonic revision rather than last_updated — timestamps are diagnostics per plan; clock rollback must not disable persistence — stale snapshots with conflicting same revisions are refused.
Task 5: Ruling: a minimal suspended flag accompanies lifecycle epochs — prevents new writes between Pre and Post without implementing the deferred full resume-readiness/reconnect state machine — resume without backend readiness can still require #1.
Task 5: implementation and verification complete pending final review; safety/clock/wire/evidence/permission RED logs; full cargo test 109/109 plus all binary/doc targets; cargo check --bins, fmt, frontend, two shell tests, diff checks passed. Frontend restored via npm ci; lockfile unchanged; frontend emits a chunk-size advisory only.

Final review: independent reviewer assessed 2b95a38..1c3e703; 1 P1 and 3 P2. Regraded all four as Important by user-visible safety/convergence impact and accepted all for one fix pass. No Minor findings were raised.
Final: Ruling: reuse durable manual_pending for sensor operations and route scoped sensor intents through the daemon executor — the same pre-execution obligation covers non-Dock writes without a second queue; Agent retains actual execution, automatic Degraded enforcement and in-gate dual-panel check — unresolved sensor outcomes conservatively require reboot, like manual uncertainty.
Final: Ruling: hardware/kernel/compositor health and deployment provenance remain unverified — fake-driver and source tests cannot establish physical Xe/Mutter or suspend recovery; no deployment/hardware action was authorized — hardware acceptance remains required.
Final: Ruling: issue #1 reconnection/full readiness stays outside this prerequisite — approved scope defers it — failed session readiness may still block progress.
Final: Ruling: cosmetic/dead diagnostic helper cleanup is not a demonstrated defect — leave unrelated cleanup out — no functional cost identified.
Final: Ruling: hostile local protocol clients and external compositor writers remain outside the guarantee — this change serializes runtime-owned writers — external mutations may invalidate observed state.
Final RED evidence: second reconciliation consumed an unsaved receipt (review-save-red.log); idle sensor completion left 0 Dock calls (review-busy-red.log); delayed registration overwrote attached status (review-replay-red.log); original direct sensor admission had no durable pending (review-sensor-red.log). Dispatch-time RevisionChanged regression was additionally mutation-checked with the transient-retention condition disabled and failed at calls=1 rather than 2 (review-revision-red.log); condition restored before final suite.

Final: fixed sensor uncertainty disappearing on replacement — original sensor admission RED; sensor_inflight_has_durable_obligation_before_backend_runs, sensor_unknown_survives_agent_and_daemon_replacement and sensor_respects_degraded_automatic_policy_even_when_manual_is_allowed GREEN, suite 115/115.
Final: fixed consume-after-save-failure — persistence_failure_prevents_dispatch_and_consumption second cleanup RED→GREEN, suite 115/115.
Final: fixed latest target stranded/discarded by sensor contention — busy_sensor_keeps_latest_target_and_cleanup_resumes_it RED→GREEN, revision_changed_before_admission_reobserves_latest_target mutation RED→GREEN, suite 115/115.
Final: fixed delayed registration overwriting newer keyboard input — registration_replay_does_not_overwrite_new_keyboard_observation RED→GREEN, suite 115/115.
Task 5: complete (commits 8e0829d..4c96f31; cargo test --offline --manifest-path ui-tauri-react/src-tauri/Cargo.toml → 115/115 library tests, all binary/doc targets passed, 12.43s; check --bins, fmt --check, frontend build, both shell tests, diff --check passed). Final task-done wrapper also hits the installed non-executable sdd-workspace helper; completion recorded manually under the existing helper-permission ruling using the actual final test outputs, without changing installed skills.
Final review disposition: four Important findings addressed in one fix pass and verified by author-run regressions/full suite; no second reviewer dispatched per Native workflow. No deferred Minor findings. Source fix commit 4c96f31.
