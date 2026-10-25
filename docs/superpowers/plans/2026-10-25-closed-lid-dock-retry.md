# 合盖拒绝保留停靠目标 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 键盘仍盖在下屏上时，合盖拒绝只推迟这一次停靠；盖子传感器放开后下一次尝试仍能关掉下屏，并且不把这次开机标成降级。

**Architecture:** 合盖拒绝继续是 `KnownFailure`，不降级。`drain_dock_targets` 不再在工作线程里睡满 4 次后删掉目标。每次只做一次尝试，然后把目标留在 `display_safety.target`。`start_dock_worker` 看 `closed_lid_retry_not_before`，到点才由现有的 1 秒清理节拍重新启动。宽限期内 1 秒后再试；宽限结束后再给 45 秒尾段，每 5 秒一次。没有宽限，或尾段已过，才删掉目标。画面若已变化，原有的 `Unknown` 降级路径不动。

**Tech Stack:** 现有 Rust runtime、protocol 3、`DisplaySafetyState::resume_grace_until`、daemon 每秒 `cleanup_display_work` → `resume_dock_target`。

**Spec:** 本次硬件记录 `local-reports/issue2-deployment/20261024T231830Z/cases/08-suspend-8/result.json`。开机 `ea424e4d`，休眠 `2026-10-24T23:38:03Z` 至 `23:38:04Z`。`operation=9..13`、`target=9`、`source=Resume` 连续五次 `KnownFailure`，原文 `Failed to apply configuration: Refusing to activate a closed laptop panel`，revision 9 到 13。第 5 次后目标被清空。结束时未降级，代理仍是第 1 代，`eDP-2` 仍开启，`secondaryPanelDisabledByRuntime` 为 false。既有约束：合盖且画面未变不得进入 Degraded；画面变了或错误原文不同仍是 Unknown。

## Global Constraints

- 合盖拒绝的判定字符串保持 `Refusing to activate a closed laptop panel`。`before == after`、执行前不是 Unknown、主屏开启，才是 `KnownFailure`。
- 这次拒绝不得设置 `degraded`，不得设置 gate `blocked`，不得把 `manual_allowed` 设为 false。
- 宽限期仍是 `RESUME_GRACE_WINDOW` = 15 秒，由 Post/Thaw 写到 `display_safety.resume_grace_until`。
- 宽限内重试间隔 1 秒。宽限结束后的尾段是 45 秒，间隔 5 秒。尾段从 `resume_grace_until` 起算，所以从 Post 起最长约 60 秒。
- 没有 `resume_grace_until` 时，第一次合盖拒绝就删掉目标，不进入尾段。
- 不读 lid sysfs，不新增依赖，不改探针断连逻辑，不在本计划里部署或清当前开机状态。
- `ponytail:` 尾段是固定 45 秒。盖子传感器若在 Post 之后超过约 60 秒才放开，这次停靠会停掉，下一次 Post 或键盘事件才会再派。要更长就加大这个常量，不要改成无界重试。

## Review Focus

1. 合盖拒绝但执行前后画面已经不同：仍是 `Unknown` 并降级。现有 `closed_laptop_panel_refusal_stays_unknown_when_panel_changes_or_reason_differs` 必须原样通过，本任务不改 `confirmed_report`。
2. 错误原文不是合盖拒绝的 `KnownFailure`：仍走原来的降级路径。本任务只在 `closed_laptop_panel_refusal` 为真时保留目标。
3. 没有 Post、`resume_grace_until` 为 None：第一次拒绝就删掉目标，调用次数为 1，不降级。现有 `closed_laptop_panel_refusal_outside_resume_grace_does_not_degrade` 保持这个断言。
4. 宽限期还在时连续拒绝 5 次：目标还在，不降级，调用次数正好是泵送次数。由 Task 1 的 `closed_lid_refusal_keeps_target_for_five_attempts_inside_grace` 覆盖。旧的 `closed_panel_retries < 4` 会在第 5 次清掉目标，这个测试就是用来否掉它的。
5. 尾段已过仍拒绝：删掉目标，不降级，不再自动重试。由 `closed_lid_refusal_after_grace_tail_drops_target_without_degrading` 覆盖。尾段内下一次尝试成功时，目标清空、下屏所有权记下、gate 不阻塞。由 `closed_lid_refusal_in_tail_retries_then_verifies` 覆盖。

---

### Task 1：合盖拒绝留住目标，到点再试

**Files:**

- Modify: `ui-tauri-react/src-tauri/src/runtime/display_safety.rs`（`DisplaySafetyState` 增加不落盘的重试时刻）
- Modify: `ui-tauri-react/src-tauri/src/runtime/dock_transaction.rs`（`start_dock_worker`、`publish_dock_target`、`drain_dock_targets`，以及文件末尾的合盖测试）
- Test: `ui-tauri-react/src-tauri/src/runtime/dock_transaction.rs` 内现有 `#[cfg(test)]` 模块

**Interfaces:**

- Consumes: `DisplaySafetyState.resume_grace_until: Option<std::time::Instant>`，`closed_laptop_panel_refusal(reason: &str) -> bool`，`resume_dock_target(state: &Arc<RwLock<RuntimeState>>)`，清理节拍已经每秒调用它。测试夹具里已有 `refuse-closed-laptop`：存在时删掉文件并返回一次合盖 `KnownFailure`。
- Produces: `DisplaySafetyState.closed_lid_retry_not_before: Option<std::time::Instant>`，`#[serde(skip)]`，默认 `None`。`start_dock_worker` 在该时刻未到时不启动。`publish_dock_target` 换上新目标时把它清掉。不新增对外函数。

- [ ] **Step 1: 写失败测试**

在 `dock_transaction.rs` 的测试夹具里，`ApplyDockOperation` 若看到 `refuse-closed-laptop-sticky` 就返回合盖 `KnownFailure`，并且不删这个文件。`before` 与 `after` 都是 `Disabled`，主屏开启，副屏关闭。调用计数仍先加。

把现有 `closed_laptop_panel_refusal_retries_within_resume_grace_without_degrading` 改成显式泵送：Post 之后写一次性 `refuse-closed-laptop`，`submit_dock_target(..., DockSource::Resume)`，`idle`。此时 `calls == 1`，目标还在，未降级。把 `closed_lid_retry_not_before` 写成 `Some(std::time::Instant::now() - Duration::from_secs(1))`，再调用 `resume_dock_target` 并 `idle`。最终 `calls == 2`，目标为 `None`，`secondary_panel_disabled_by_runtime` 为真，未降级，gate `blocked` 为 `None`。

新增三个测试：

- `closed_lid_refusal_keeps_target_for_five_attempts_inside_grace`：Post 之后写 `refuse-closed-laptop-sticky`。第一次 `submit_dock_target(..., Resume)` 后 `idle`，再把 `closed_lid_retry_not_before` 拨到过去并 `resume_dock_target` + `idle`，重复到 `calls == 5`。断言目标仍是 `Some`，`degraded` 为 `None`，`manual_allowed` 为真，gate `blocked` 为 `None`。
- `closed_lid_refusal_after_grace_tail_drops_target_without_degrading`：不调用 Post。`reset_resume_grace_for_test`。把 `resume_grace_until` 设为 `Some(std::time::Instant::now() - Duration::from_secs(46))`。写 `refuse-closed-laptop-sticky`，`submit_dock_target(..., Resume)`，`idle`。断言 `calls == 1`，目标为 `None`，`degraded` 为 `None`，`manual_allowed` 为真。
- `closed_lid_refusal_in_tail_retries_then_verifies`：不调用 Post。`resume_grace_until = Some(now - 1s)`。写一次性 `refuse-closed-laptop`，提交并 `idle`。断言 `calls == 1`，目标为 `Some`，`closed_lid_retry_not_before` 距现在至少 4 秒，未降级。再把该时刻拨到过去，`resume_dock_target`，`idle`。断言 `calls == 2`，目标为 `None`，`secondary_panel_disabled_by_runtime` 为真，gate 未阻塞。

`closed_laptop_panel_refusal_outside_resume_grace_does_not_degrade` 的断言不改：无宽限，一次调用，目标清空，不降级。

- [ ] **Step 2: 跑测试，确认新断言失败**

Run: `cargo test --locked --offline --lib closed_lid_refusal -- --test-threads=1`

工作目录：`ui-tauri-react/src-tauri`。

Expected: 新测试编译失败或断言失败。失败原因应是第 5 次拒绝后目标已是 `None`，或尾段内第一次拒绝后目标已被清空。不要为了让测试先绿去改断言。

- [ ] **Step 3: 改 `drain_dock_targets` 的合盖分支**

在 `DisplaySafetyState` 增加 `closed_lid_retry_not_before`，`Default` 为 `None`。

`start_dock_worker` 在现有条件之外，若 `closed_lid_retry_not_before` 仍在将来，直接返回，不把 `worker_running` 设为真。

`publish_dock_target` 在写入新 `target` 之前把 `closed_lid_retry_not_before` 设为 `None`。

删掉 `drain_dock_targets` 里的 `closed_panel_retries` 和循环内的 `sleep` + `continue`。一次尝试返回合盖拒绝时：

- `now < resume_grace_until`：留下同一代目标，`closed_lid_retry_not_before = now + 1s`，`worker_running = false`，返回 `Ok(())`。
- `resume_grace_until` 是 `Some(deadline)` 且 `now < deadline + 45s`：同样留下目标，间隔改为 5 秒，返回 `Ok(())`。
- 其余（宽限是 `None`，或尾段已过）且未降级：若目标仍是这一代，置 `None` 并保存；`worker_running = false`，返回 `Ok(())`。

其它错误路径、`Unknown` 降级、核实成功后清目标和键盘通知，保持原样。成功清目标时把 `closed_lid_retry_not_before` 也清掉。

在 45 秒尾段的赋值旁加一行注释：`ponytail: 45s tail after the 15s resume grace; raise the constant if a closed-lid sensor stays shut longer, do not retry without a deadline.`

- [ ] **Step 4: 跑合盖测试和整库**

Run: `cargo test --locked --offline --lib closed_lid -- --test-threads=1`

Expected: `closed_laptop_panel_refusal_outside_resume_grace_does_not_degrade`、`closed_laptop_panel_refusal_retries_within_resume_grace_without_degrading`、`closed_lid_refusal_keeps_target_for_five_attempts_inside_grace`、`closed_lid_refusal_after_grace_tail_drops_target_without_degrading`、`closed_lid_refusal_in_tail_retries_then_verifies` 都通过。`display_backend` 里两个合盖分类测试也通过。

然后 Run: `cargo test --locked --offline --lib`

Expected: 全部通过。现有数量是 134，加上 3 个新测试后是 137。锁文件哈希不得变化。

- [ ] **Step 5: Commit**

```bash
git add ui-tauri-react/src-tauri/src/runtime/display_safety.rs ui-tauri-react/src-tauri/src/runtime/dock_transaction.rs
git commit -m "$(cat <<'EOF'
fix(runtime): 合盖拒绝在盖子打开前保留停靠目标

EOF
)"
```

只提交这两个源文件。不提交 `local-reports/`，不推送。

---

## 本计划不做的事

当前开机 `ea424e4d` 没有降级，下屏开着是因为目标已被丢掉。实现停在上面的提交。安装、重启和剩下的休眠次数另走一次受控切换，等这份计划被接受后再做。不要在这台还开着的机器上清 state，也不要再休眠来“把下屏关掉”。
