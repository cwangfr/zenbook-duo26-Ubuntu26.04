# 手动布局确定失败不得降级 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 命令明确失败且读回布局与执行前一致时，把手动布局记为确定失败，不进入自动切屏 Degraded，界面显示命令原文。

**Architecture:** 在 `run_manual` 用纯函数根据命令结果、执行前布局和读回布局分类。只有读回证明没有布局变化时才是 `KnownFailure`。读回缺失、读回与执行前不同、或命令成功但不符合请求，仍是 `Unknown`，daemon 继续降级。`apply_manual` 只对这种 `KnownFailure` 清除 `manual_pending` 并返回命令原文。

**Tech Stack:** 现有 Rust runtime、`gdctl`、protocol 3、`MutationStatus::KnownFailure`。

**Spec:** `docs/superpowers/specs/2026-10-24-issue2-display-safety-design.md`，第 5.2 节终态区分和第 5.3 节降级边界。

## Global Constraints

- 结果未知（超时、部分执行或验证不足）使自动布局写入进入 Degraded；不靠重启 daemon、重新注册或清状态解锁。
- 失败但结果确定时，必须能说明是否发生显示变更；不得只凭错误字符串猜测有没有变更。
- 未知终态不解锁写入。本阶段不增加强制恢复按钮，也不在当前 boot `d8f69865` 上部署、清 state 或重试显示操作。
- 屏幕编号相反是独立问题，本计划不改 `eDP-1`/`eDP-2` 的画布位置。

## Review Focus

1. 命令返回明确错误，但读回的缩放、位置或开关已经变了：必须仍是 `Unknown` 并降级。由 Task 1 的 `changed_readback_stays_unknown` 覆盖。
2. 命令成功，读回仍是旧缩放：必须仍是 `Unknown` 并降级。由 Task 1 的 `unmatched_success_stays_unknown` 覆盖。
3. `gdctl` 没有读回：必须是 `Unknown`，不能当成确定未变更。由 Task 1 的 `missing_readback_stays_unknown` 覆盖。
4. 命令成功且读回符合请求时仍是 `Verified`，不能被新分类误伤。由 Task 1 的 `manual_layout_known_failure_not_used_for_matching_success` 覆盖。
5. 用户看到的是命令原文，并且本次 `manual_pending` 被清除；已经存在的 Degraded 不会进入这条路径，因为 `apply_manual` 在降级时于下发前拒绝。由 daemon 测试的返回值和 `degraded.is_none()` 覆盖。

---

### Task 1：确定未变更的手动布局失败

**Files:**

- Modify: `ui-tauri-react/src-tauri/src/runtime/display_backend.rs`（`run_manual` 的布局分类）
- Modify: `ui-tauri-react/src-tauri/src/runtime/dock_transaction.rs`（`apply_manual` 对 `KnownFailure` 的处理，约 615–652 行）
- Test: 同两个文件内的现有 `#[cfg(test)]` 模块

**Interfaces:**

- Consumes: `MutationStatus::{Verified, NotStarted, KnownFailure, Unknown}`，现有布局验证条件（connector、enabled、transform、scale 绝对差 `< 0.02`）
- Produces: `pub(crate) fn classify_manual_layout(command: &Result<(), String>, previous: &DisplayLayout, actual: Result<&DisplayLayout, &str>, requested: &DisplayLayout) -> MutationStatus`

布局未变指同一组 connector，且每个 connector 的 `enabled`、`primary`、`transform`、`x`、`y`、`current_mode.mode_id` 相同，`scale` 绝对差 `< 0.02`。

- [x] **Step 1：写失败测试**

在 `display_backend.rs` 的测试模块加四个纯函数测试，名称都含 `manual_layout_known_failure`：

```rust
#[test]
fn manual_layout_known_failure_when_readback_unchanged() {
    let previous = sample_layout(1.666667);
    let requested = sample_layout(1.5);
    let status = classify_manual_layout(
        &Err("gdctl set failed: scale rejected".into()),
        &previous,
        Ok(&previous),
        &requested,
    );
    assert_eq!(
        status,
        MutationStatus::KnownFailure { reason: "gdctl set failed: scale rejected".into() }
    );
}

#[test]
fn manual_layout_known_failure_not_when_readback_changed() {
    let previous = sample_layout(1.666667);
    let mut changed = previous.clone();
    changed.displays[1].scale = 1.5;
    let status = classify_manual_layout(
        &Err("gdctl set failed: partial".into()),
        &previous,
        Ok(&changed),
        &changed,
    );
    assert!(matches!(status, MutationStatus::Unknown { .. }));
}

#[test]
fn manual_layout_known_failure_not_when_success_unmatched() {
    let previous = sample_layout(1.666667);
    let status = classify_manual_layout(&Ok(()), &previous, Ok(&previous), &sample_layout(1.5));
    assert!(matches!(status, MutationStatus::Unknown { .. }));
}

#[test]
fn manual_layout_known_failure_not_when_readback_missing() {
    let previous = sample_layout(1.666667);
    let status = classify_manual_layout(
        &Err("gdctl set failed: timeout".into()),
        &previous,
        Err("gdctl show failed"),
        &sample_layout(1.5),
    );
    assert!(matches!(status, MutationStatus::Unknown { .. }));
}

#[test]
fn manual_layout_known_failure_not_used_for_matching_success() {
    let requested = sample_layout(1.5);
    let status = classify_manual_layout(&Ok(()), &sample_layout(1.666667), Ok(&requested), &requested);
    assert_eq!(status, MutationStatus::Verified);
}
```

`sample_layout(scale)` 构造 eDP-1 与 eDP-2 都启用、eDP-1 为 primary；只有 eDP-2 的 scale 使用参数。比较用到的 `primary`、`transform`、`x`、`y`、`mode_id` 都填实。

在 `dock_transaction.rs` 照现有 Fixture 增加一个测试：Agent 对手动布局返回 `MutationComplete`，`status` 为 `KnownFailure { reason: "gdctl set failed: scale rejected" }`，`before` 与 `after` 都是 `Enabled`。

```rust
#[tokio::test]
async fn manual_layout_known_failure_does_not_degrade() {
    let err = apply_and_get_daemon_error(&fixture).await;
    assert_eq!(err, "gdctl set failed: scale rejected");
    let state = fixture.state.read().await;
    assert!(state.display_safety.degraded.is_none());
    assert!(!state.display_safety.manual_pending);
}
```

- [x] **Step 2：运行测试并确认失败**

Run: `cargo test --offline --manifest-path ui-tauri-react/src-tauri/Cargo.toml --lib manual_layout_known_failure`

Expected: 编译失败或断言失败。分类函数尚不存在，且现有 `apply_manual` 会把 `KnownFailure` 降级成 `Display operation is degraded`。

- [x] **Step 3：实现分类并接入两条路径**

在 `display_backend.rs` 实现 `classify_manual_layout`。`run_manual` 的布局分支在读回之后调用它，不再无条件把未验证结果改成 `Unknown { reason: "Requested display layout could not be verified" }`。方向分支保持现有验证。

在 `apply_manual` 中，`MutationStatus::KnownFailure { reason }` 且当前 scope 仍有效时：不调用 `degrade`，不写 `degraded`，清除本次设置的 `manual_pending`，保存后 `return Err(reason)`。`Unknown` 仍降级并返回 `Display operation is degraded`。已处于 Degraded 的请求继续在下发前拒绝，本分支不负责清除它。

- [x] **Step 4：运行测试并确认通过**

Run: `cargo test --offline --manifest-path ui-tauri-react/src-tauri/Cargo.toml --lib`

Expected: 全部库测试通过，包含 Step 1 的五个分类测试和一个 daemon 测试。

- [x] **Step 5：提交**

```bash
git add ui-tauri-react/src-tauri/src/runtime/display_backend.rs \
  ui-tauri-react/src-tauri/src/runtime/dock_transaction.rs
git commit -m "$(cat <<'EOF'
fix(runtime): 未变更的布局失败不再降级自动切屏

EOF
)"
```

## 明确不做

- 不部署到当前机器，不清理 `manual_pending`，不重启 daemon、Agent 或 lifecycle。
- 不改控制界面把 eDP-1 固定画在上方的行为。
- 不把 `Refusing stale runtime snapshot` 的防旧写入检查放宽。

## 执行前自查

- [x] 第 5.2、5.3 节的四种终态都有归类；未知仍降级，确定未变更不降级。
- [x] 五个 Review Focus 都有对应测试。
- [x] 当前 boot 的保护状态不在本计划内解除。
- [x] 只包含一个可独立测试的代码任务。
