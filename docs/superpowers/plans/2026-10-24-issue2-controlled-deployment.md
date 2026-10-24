# Issue #2 安全前置修复：受控部署与实机验收计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans for Native execution of this plan. Steps use checkbox (`- [ ]`) syntax for tracking. This is an operations/acceptance runbook, not a new feature implementation plan. Do not manufacture fault injection or production changes to satisfy a software TDD template.

**Goal:** 将已验证的安全前置修复部署到本机，证明版本一致、普通 Dock/旋转路径可用、保护机制没有被绕过，并产出可追溯的实机验收结论。

**Architecture:** 同一提交构建全部参与进程，先形成候选包、备份和具体切换清单，再停止旧进程并统一替换。首次验收使用一次明确安排的正常重启建立新的 boot 边界，随后逐项操作、逐项取证；遇到 Unknown/pending 不重发显示变更，不清状态解锁。

**Tech Stack:** 现有 Rust/Tauri、npm lockfile、systemd system/user units、Unix socket protocol 3、DRM sysfs、journal。仅使用仓库已有构建链，不新增部署框架。

**Spec:** `../specs/2026-10-24-issue2-display-safety-design.md`，尤其部署/回滚边界；已完成实现的证据为 `../reports/2026-10-24-issue2-display-safety-verification.md`。本计划是该设计的操作验收补充，不新增 #1 或完整 #2 功能。

## 当前基线和方案选择

- 原候选 `db73a686d69ddc5e3fbb8540e1b0ec71372385c5` 已因真实双屏解析缺陷废弃；修复后的新候选固定为 `d117a43685226ac45f57047fddcd7e94323e42d0`。运行时安全修复仍包含提交 `4c96f31`。
- 原候选 115 项库测试通过；解析修复后固定候选 116 项通过、本机捕获文本的诊断回归通过，binary check、前端构建和五项离线 release 构建通过。统一安装已完成，等待重启及实机验收。
- 沿用用户已选择的 Native 执行方式。用户随后明确要求开始并继续执行部署，确认工作已保存、可以正常重启。
- **推荐：分阶段构建、统一切换、逐项验收。** 可以在停服务前审查全部候选产物与回滚材料。
- 全量运行 `install.sh` 会涉及额外系统配置；直接运行 `install-rust-runtime.sh` 会自行安装并重启 daemon/lifecycle/Agent，且不统一更新 UI。两者均不作为本轮切换命令。
- 先继续实现 #1 可推迟部署，但无法获得当前修复的硬件证据，且扩大待验证变更；本轮只完成这份部署验收。

## Global Constraints

- 本计划已进入用户要求的执行阶段；重启与真实显示操作仍按下述阶段门逐项核验。
- 所有五个参与二进制来自同一候选源码：daemon、session-agent、lifecycle、usb-remap-helper、Tauri control UI；禁止混用旧 UI/tray。
- 不运行全量安装器，不升级内核、固件、驱动或依赖，不改 GDM、udev、蓝牙策略、休眠策略；不启用原来禁用的功能或旧服务。
- 不做 suspend/hibernate、反复重启、强杀、故意磁盘满、损坏真实状态或实际后端超时注入；这些故障继续使用已完成的模拟测试证据。
- 不删除、手工清除或恢复旧 pending/ownership 字段以通过验收。新 boot 清理旧证据不代表硬件健康。
- 一次只做一个物理/界面操作；等待终态、保存证据后再继续。发生错误停止测试，不循环点击重试。
- 初次验收安排一次用户可参与的正常重启；用户需先保存工作。重启不是 suspend 验收，也不由工具未经执行指令自动触发。
- 回滚是恢复完整匹配版本及原有 unit/配置；有不确定在途结果时不能在同一 boot 启动旧实现。
- 本轮不 push、merge 或关闭 issue；不声称 #1 或完整 #2 已修复。

## Review Focus

1. 磁盘文件已经替换但旧进程仍运行，或 desktop/autostart 实际启动另一份 UI：必须逐 PID 校验 `/proc/<pid>/exe`，不能仅看 PATH/文件名。Task 3、4 验证。
2. systemd unit/drop-in、旧 shell daemon、sleep hook 或 user autostart 继续带入旧 writer：Task 1 清点，Task 3 逐项停用/保留，不改无关服务。
3. eDP-2 在测试开始前已 Disabled/Unknown：Task 1、4 判为阻塞，不用自动打开来制造健康基线；保护性拒绝与功能验收失败分开记录。
4. 超时后后台仍执行或 pending 尚未解决：Task 4、5 只读追踪终态，不 resend、不重启解锁；硬件故障不做破坏性复现。
5. 回滚材料版本不匹配，或者旧状态覆盖当前证据：Task 2 建立完整版本清单；Task 6 先归档故障状态，再按 boot 边界恢复，不把备份 state 写回运行中的新系统。

## 文件与产物

- 参考：`install-rust-runtime.sh` 的安装路径、unit 模板及 sleep hook；`install-ui.sh` 的 UI 安装位置；`ui-tauri-react/src-tauri/tauri.conf.json` 的构建入口。
- 只创建操作产物：`local-reports/issue2-deployment/<UTC-run-id>/`（已被 gitignore 忽略），权限 0700；含 `baseline/`、`candidate/`、`backup/`、`cases/` 和 `manifest.json`。
- `manifest.json` 必须记录 `sourceCommit`、`protocolVersion: 3`、`kernel`、`bootIdBefore`/`bootIdAfter`、`sessionBackend`、`cargoLockSha256`、`packageLockSha256`、五项 `built/installed/running` SHA-256、真实安装路径、units/drop-ins/enabled 状态、构建命令/退出码、配置修改与恢复情况。
- 提交最终脱敏报告：`docs/superpowers/reports/2026-10-24-issue2-hardware-acceptance.md`。原始 journal、状态、配置和主机信息留在本地；不得提交凭据、完整环境变量或无关个人信息。
- 默认不改产品代码、安装器或 systemd 语义。如发现缺陷，先保存可复现证据并另行诊断，不在部署流程中临时绕过保护。

### Task 1：只读预检与实际安装清点

**Interfaces:** 输出 `baseline/` 和明确的 GO/BLOCKED 结论，供后续任务使用；此阶段不停止或重启任何进程。

- [ ] 记录 `git status --porcelain`、候选提交是否存在、当前 branch、`uname -r`、当前 boot ID、locale、桌面/会话类型、当前用户和 UID；不假定先前记录的内核仍是当前内核。
- [ ] 用 `systemctl cat/show/is-enabled` 清点 `zenbook-duo-rust-daemon.service`、`zenbook-duo-rust-lifecycle.service`、旧 `zenbook-duo.service`；用 `systemctl --user` 清点 `zenbook-duo-session-agent.service`、旧 `zenbook-duo-user.service`。保存 FragmentPath、drop-ins、ExecStart/ExecStop、MainPID、active/enabled 状态。不存在项如实记录。
- [ ] 检查 `/usr/lib/systemd/system-sleep/zenbook-duo-rust-lifecycle`、旧 `duo` hook、system/user desktop autostart 和 `zenbook-duo-control` 的 PATH/desktop/package 解析结果。用完整命令行枚举相关进程，不依赖可能截断名称的 `pgrep -x`。
- [ ] 为实际相关 PID 记录 executable 路径和 `/proc/<pid>/exe` hash；检查重复 daemon/Agent/UI 与旧脚本 writer。只保存相关命令行，不导出完整进程环境。
- [ ] 读取 `/var/lib/zenbook-duo/state.json`、`~/.config/zenbook-duo/settings.json`、可读 DRM eDP connector 的 `enabled/status`，保存 kernel/system/user journal 时间窗口。保留缺失/解析失败原貌，禁止读取失败后生成默认 state。
- [ ] 用户观察当前两个面板、主屏可用性、物理键盘状态和方向；需要人为观察的项目标为待填写，不能从 sysfs 猜测画面正常。
- [ ] **通过标准：** 主屏可用、当前显示基线可解释、没有无法确认的活动写入/Unknown/pending，恢复入口可用。若已存在显示故障、状态损坏或不可解释的 ownership，标记 BLOCKED，先做诊断；本计划不授权自动修复该故障。

### Task 2：构建候选包、备份和具体切换清单

**Interfaces:** 消费 baseline，输出五个候选二进制、hash manifest、完整备份及逐项安装/恢复路径；不写系统安装位置。

- [ ] 在本地 staging 从固定候选提交导出源码；不 reset 当前功能分支。记录独立源码目录，禁止混用另一个工作区的旧 `dist` 或 release 产物。
- [ ] 使用现有锁文件构建：Rust 使用 `cargo build --locked --offline --release --manifest-path <source>/ui-tauri-react/src-tauri/Cargo.toml --bin zenbook-duo-daemon --bin zenbook-duo-session-agent --bin zenbook-duo-lifecycle --bin zenbook-duo-usb-remap-helper`；前端在导出的 UI 目录执行 `npm ci --offline --no-audit --no-fund`（缓存不足时停止并记录缺项），再 `npm run build:local -- --ci`。Tauri Rust 构建设置 `CARGO_NET_OFFLINE=true`，前后比对 Cargo.lock/package-lock hash，任何改动均阻止候选通过。
- [ ] 运行候选源码的 `cargo test --locked --offline --manifest-path <source>/ui-tauri-react/src-tauri/Cargo.toml`、`cargo check --locked --offline --manifest-path <source>/ui-tauri-react/src-tauri/Cargo.toml --bins`；预期 115 个库测试通过，所有 binary/doc targets 通过；UI release 构建成功且五个二进制均存在。缺依赖不自动运行 apt/rustup 或在线更新。
- [ ] 对五个产物保存 SHA-256、大小、权限、构建日志和锁文件 hash；此时不运行候选程序连接真实 daemon/显示后端。
- [ ] 备份实际安装的完整旧 cohort、units/drop-ins、symlink 目标、desktop/autostart、enabled 状态和配置；备份 state 作为诊断证据，不作为运行时覆盖模板。记录备份文件 hash、owner/mode，并核实恢复路径与可用终端入口。
- [ ] 实际版本无法证明一致时，不把零散二进制称为有效回滚包。优先完成可证明一致的完整备份；必要时另建 pre-implementation `2b95a38` 的全套匹配构建及验证，不能未经验证将其称为可用恢复版本。
- [ ] 将 Task 1 发现的真实 ExecStart/UI 路径映射成具体的五项替换清单；常规 runtime 路径为 `/usr/local/libexec/zenbook-duo/`，UI 路径以本机解析结果为准，不制造第二份 PATH 覆盖。unit 无需变化则原样保留，缺失 unit 时先生成可审阅模板再决定是否采用。
- [ ] **通过标准：** 五项产物和回滚包可核验，切换清单完整，无锁文件变化、无缺失依赖。形成可审阅结果后才进入部署；只要求准备时在此结束。

### Task 3：统一切换，并建立新的 boot 边界

**Interfaces:** 消费已审核的切换清单，输出安装后 hash、停止/启动记录和重启交接说明。会影响真实会话，仅在用户明确要求执行部署后执行。

- [ ] 确认用户已保存工作，并安排这一次正常重启。记录当前键盘位置、屏幕状态、原始 auto-rotate 设置；不偷偷改变设置、睡眠策略或触发显示恢复。
- [ ] 停止旧 UI/tray 和清单中的旧 writer，防止 autostart/watchdog 拉回。先确认没有正在执行的显示操作；不能确认时停止切换、保存证据。需要暂时 mask 的**具体已发现服务**记录原值与恢复命令，不 blanket kill 或停用其他用户服务。
- [ ] 有序停止 Agent、相关 daemon 和 lifecycle。注意 lifecycle unit 的 `ExecStop=... shutdown` 会执行真实生命周期操作：必须作为已授权切换的一部分记录；不得调用安装器以隐藏这一步，也不通过修改 unit 语义跳过记录。
- [ ] 用候选产物替换清单中的五个位置，保持 owner/mode；同文件系统临时文件加 rename 安装。逐项验证 installed hash 等于 candidate hash；任何半途失败都不启动混合 cohort，保持停机并恢复整套备份。
- [ ] 不主动 `restart lifecycle` 来模拟 boot。保持原有正确 unit/hook 配置；只在确有 unit 变更时 daemon-reload。仅解除候选新服务的临时 mask，按切换清单设置其下次启动；旧实现 daemon/user 服务与旧 sleep hook 在候选版本运行期间保持停用/隔离，不能随临时 mask 一并恢复。记录原值，仅在完整回滚时恢复旧启动配置。保留无关服务的原状态，确认下次启动将使用同一候选 cohort。
- [ ] 重启前再次检查相关旧进程均已退出，desktop/autostart 指向候选 UI；生成重启后的继续步骤和证据目录位置，避免会话中断后丢失执行位置。由用户执行一次正常重启，不执行 suspend/hibernate。
- [ ] **通过标准：** 所有安装文件 hash 匹配、无旧 writer，重启交接完整；若未完成 reboot，状态是 WAITING_FOR_REBOOT，不能开始硬件验收。

### Task 4：重启后启动、版本和空闲状态验收

**Interfaces:** 消费新的 boot ID 与运行中 cohort，输出第一份实机 GO/BLOCKED 结果。

- [ ] 登录原桌面会话；记录新的 boot ID，确认与切换前不同。检查 daemon、lifecycle、Agent 的 active 状态和 unit 真实 ExecStart。不要用反复重启 Agent 弥补 #1 未实现的重连。
- [ ] 对实际运行 daemon/Agent/UI 及 helper/lifecycle 启动日志做版本追踪。常驻 PID hash 必须等于本轮 candidate；oneshot 无存活 PID 时记录执行路径/installed hash和本次执行 journal，不虚构 running hash。
- [ ] 使用现有只读界面与 protocol-3 查询获取 daemon status、Agent `GetDisplayExecutionState` 和 `GetDisplayLayout`。Unix socket 查询只发送只读类型；验证响应 protocolVersion=3、Agent/scope 对应当前 boot/daemon。不可用或不匹配时立即 BLOCKED，不发送写命令试探。
- [ ] 检查 state JSON 的 `displaySafety.pending`、`manual_pending`、`receipt_to_consume`、`degraded`，以及 Agent 的 active/receipt/blocked。取两次相隔至少 2 秒的空闲快照，确认已完成事务收尾；状态文件和 gate 快照需要结合读取，不能把一次瞬时 pending 当作永久故障。
- [ ] 比较逻辑 layout、DRM 观察与用户看到的两个面板。若 eDP-2 已 Disabled/Unknown 且没有本轮可信 ownership，记录保护性拒绝，不能强行打开以继续用例。保护正确不等于双屏功能通过。
- [ ] **通过标准：** 同版本单实例、会话 scope 正确、主屏正常，系统达到可解释的 idle，未进入 Degraded。若 Agent 未注册/未就绪，归类为 #1/启动就绪阻塞，带日志结束本轮，不自动更改范围。

### Task 5：小范围真实功能验收

**Interfaces:** 每例输出 `cases/<编号>/`，包含时间、用户动作、boot/scope/op/revision、前后 state/gate/layout/DRM/journal 和用户画面观察；结论为 PASS/BLOCKED/FAIL。

- [ ] **A：普通 Dock，3 个完整循环。** 从健康双屏、键盘分离开始；连接键盘，确认主屏持续可用、下屏关闭、Verified 后才获得本 boot ownership、pending/receipt 收尾；分离键盘，确认下屏恢复、ownership 清除。每一边等待终态再做下一步。每个操作观测窗口最多 20 秒；20 秒只表示停止下一步并取证，不取消/重发后台任务，也不作为性能承诺。
- [ ] **B：自动旋转，一次往返。DEFERRED（2026-10-24，用户确认延后）。** 本机没有可用的方向传感器证据，本轮不临时打开 auto-rotate，也不把该用例记为通过。后续单独安排，不阻塞手动方向往返。
- [x] **C：手动布局与手动方向，各一次往返。** 2026-10-24 在 `3acd3b4`、启动 `5cc47b0a` 上通过：Apply Layout `Verified` revision 3，下屏 `y=1080`；Left `Verified` revision 6，下屏 `(0, 1728)`；回到 Normal `Verified` revision 7，恢复上屏 `180 (0, 0)`、下屏 `0 (0, 1080)`。主屏保持启用，安全门空闲。
- [x] **D：轻量串行竞争，一次。** 2026-10-24 在 `3acd3b4`、启动 `5cc47b0a` 上：Normal `Verified` revision 11（`21:05:34Z`），随后键盘合上 `operation=7` `Verified` revision 12（`21:05:39Z`）。最终方向 Normal，上屏开、下屏关，本 boot 拥有关闭权，安全门空闲，同一 operation 没有重发。两笔写屏没有重叠（方向结束后约 3 秒才观察到键盘），并行互斥断言 **NOT_PROVEN**。画面正常不能补上这个断言。机器保持键盘合上、方向 Normal，不再为收尾额外切屏。
- [ ] 每次实际发生 Unknown、超时或 Degraded 时，停止后续动作，保存 journal/state/gate，验证没有自动重发/自动解锁；不故意在本机制造故障。异常拒绝的深层崩溃/存储故障行为引用既有模拟回归证据，不能写成实机已通过。
- [ ] 恢复本轮临时修改的 auto-rotate 和其他明确记录的测试设置；若恢复需要显示写入且当前 Degraded/Unknown，保持保护状态，记录待恢复，不绕过执行门。
- [ ] **通过标准：** 每例均有双重证据（协议/状态与用户画面观察），主屏保持可用，无新内核/Xe 显示错误，无未解释的 pending/Degraded。内核日志缺少权限时标记证据缺口，不默认为无错误。本轮不运行十轮 suspend 测试。

### Task 6：停止条件、回滚和最终结论

**Interfaces:** 输出完整验收报告、当前安装状态和下一阶段建议；不以重启后能亮屏代替事务安全证据。

- [ ] 任一版本不一致、主屏异常、backend Unknown、scope 异常、持久化错误、内核新显示错误或未解释 pending：立即停止新动作并归档，不清 state、不重试 modeset、不用 Agent/daemon 重启解除保护。
- [ ] 区分“功能 FAIL”“保护性 BLOCKED”“证据不足 NOT_PROVEN”。先保存故障时 state、boot、hash 和 journal，再决定保持新版本保护停机还是恢复旧 cohort；不能自动回滚到保护更弱的旧代码继续测试。
- [ ] 如需回滚，先形成并核对具体恢复清单；停止全部相关 writer，恢复完整匹配的五项二进制、原 units/drop-ins/hook/desktop/autostart 及原 enabled 状态。保留故障 state 证据，不把旧备份 state 覆盖上去。存在 Unknown/pending 时，旧实现只能在正常重启建立新 boot 后启动；未安排重启则保持相关服务停止并交接。
- [ ] 回滚后同样核对 installed/running hashes、新 boot ID 与主屏基础可用性；回滚不意味着旧代码拥有新安全保证。记录缺失材料/无法恢复项，不能报告成功。
- [ ] 最终报告记录候选提交、全部通过/阻塞用例、真实内核与后端、原始证据位置、保护触发情况、当前运行版本、配置恢复结果。只提交脱敏报告及必要文档，不提交主机原始数据。
- [ ] **结束标准：** 全部基础用例通过则交付“安全前置修复基础实机验收通过”；下一阶段设计/实现 #1 的重连与就绪检查，然后再单独做 #2 完整 suspend/resume 验收。若失败则根据证据先修复具体阻塞，保持单一变量。

## 执行前自查

- [x] 已读取已批准设计、实现验证报告、实际安装器、构建入口、unit 模板和状态路径。
- [x] 每阶段有输入、输出、通过标准、失败停止条件；五项 Review Focus 均有对应核验。
- [x] 现有 Native 偏好保留，不再要求重新选择执行模式。
- [x] 构建、切换、重启和真实验收明确分段；本次只交付计划。
- [x] 实机不注入崩溃/磁盘/超时故障；软件回归与硬件证据分开。
- [x] 未实施安装、真实布局变更、服务重启、reboot 或新的代码功能。

## 本次执行进度（2026-10-24）

用户已要求开始执行。首次预检阶段状态：**BLOCKED_BEFORE_DEPLOYMENT**。Task 1 只读预检发现真实双屏 `gdctl` 输出的主屏解析缺陷，独立捕获回归已失败；Task 2 的离线五项 release 构建、既有 115 项测试及只读备份已完成主要部分，完整回滚/root 核验仍待补齐。该阶段 Task 3—5 未开始，没有安装、停服或 reboot。按 Task 6 的停止条件保存证据；先另行修复 parser、更新候选 SHA 后继续。详见 `../reports/2026-10-24-issue2-hardware-acceptance.md`。

### 继续执行记录（同日）

用户要求“继续工作”，已授权消除本次预检发现的解析阻塞。固定脱敏双屏测试先 RED，再以一处 connector 提取修复 GREEN；本机原始捕获文本在独立诊断副本中也从 RED 变 GREEN。修复提交 `d117a43`，新候选从该提交导出；原 `db73a68` 候选产物仅作为诊断证据，不能安装。新的部署预检仍需完成 root 运行文件 hash、完整回滚和统一切换条件。

### 统一切换记录（同日）

修复候选的五项 release 产物、116 项库测试和二进制检查均通过；锁文件不变。用户在本机终端完成只读 root 预检，确认旧 daemon/Agent 的运行文件与旧安装备份一致、当前 boot 未变。用户确认已保存工作并可重启。随后停止旧 Agent、lifecycle、daemon，原子安装五项新二进制；再次独立核对五项 installed SHA-256 全部等于候选，旧进程全部退出，原有三个 unit 保持 enabled。切换完成时状态为 **WAITING_FOR_REBOOT**。原始 manifest、部署日志和回滚文件位于忽略的本地运行目录。

### 重启后停止条件（同日）

用户正常重启后，新 boot 中 daemon/Agent、protocol 3、双屏和主屏画面均可用，但启动自动 Dock 的 GNOME 命令 `--monitor eDP-2 --mode off` 返回 `No mode off available`。Agent gate 与 daemon state 一致进入 `Degraded/Unknown`；没有重复派发，按 Task 6 停止 Task 5。当前安装仍是 `d117a43`，安全门保持阻断，未在同一 boot 回滚或重启服务。只读证据与脱敏结论见 `../reports/2026-10-24-issue2-hardware-acceptance.md`。

无效参数的生产构造已按 RED→GREEN 测试修正于源码提交 `8ca7fd4`。从该提交独立导出并离线构建了五项匹配候选，候选源码 116 项测试、cargo check 与 UI build 通过，锁文件不变；**此修复未部署**。下一次部署须审核当前 `Unknown` 状态、这五项候选和完整回滚条件，并安排新的受控 boot。不能把本次安全阻断记为 Dock 功能通过。

### 第二轮部署与物理操作停止条件（同日）

用户授权第二轮部署后，`8ca7fd4` 五项候选完成 root 预检、完整备份、统一切换和正常重启。新 boot 中版本、protocol 3、单实例、安全门、attached 启动 Dock 及用户画面观察均通过；底屏正确关闭。第 1 轮键盘分离后双屏画面与最终状态正常，但 daemon 日志显示 USB 探测抖动为 detached → attached → detached，产生三个串行且均 Verified 的 Dock operation。按“一次物理操作、等待终态”和“不连续重发”的约束停止后续用例，键盘保持分离、双屏健康。

根因是 250ms USB 轮询将每个单次样本变化立即发布，没有枚举稳定窗口。现场序列的回归先 RED，随后在进入 policy 前增加连续三个相同差异样本的去抖。代码审查发现 lifecycle 也会独立写入 keyboard 状态；第二个 RED→GREEN 回归确保去抖器每次以 RuntimeState 当前值为基线，避免缓存状态与 lifecycle 分叉。完整测试更新为 118 项通过，cargo check、格式和 diff 检查通过；复审没有剩余 Critical 或 Important 问题。固定提交 `559df2e` 的五项离线 release 候选已构建，候选源码 118 项测试与 binary check 再次通过，锁文件不变，尚未部署。下一轮必须重新建立 boot 边界，不能把第二轮第 1 个 Dock 循环记为完整 PASS。
