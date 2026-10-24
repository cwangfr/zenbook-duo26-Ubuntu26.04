# Issue #2：自动切屏最小安全前置设计

日期：2026-10-24；源码基线：`e0b11760ad23898761e7acedd322dc412ced604a`。
目标仓库：`cwangfr/zenbook-duo26-Ubuntu26.04`。
状态：方案 A 已由用户选择；本文待用户批准，不代表实现或部署完成。

## 1. 意图与范围

为 #1 后续连接自愈建立可验证的切屏安全边界：恢复通信不能绕过未知下屏保护，旧操作不能覆盖当前目标，IPC 等待超时不能让两个显示写操作重叠。

本次实现范围：

- startup、registration、Post/Thaw、monitor 四类自动 Dock 入口共享事务和保护。
- Agent 端所有布局写操作共享执行边界，包括 Dock、手动布局、手动旋转、传感器自动旋转。
- 分开记录目标是否完成与操作确实造成的显示变化；ownership 只有一个提交点。
- 明确请求超时、迟到结果、后端 Error、会话替换、部分执行失败的结果语义。
- 添加生产调用链上的模拟测试及必要诊断信息。

不包含：#1 自动重新注册、完整 suspend/readiness 状态机、GDM/Plymouth 修改、Xe/内核/DKMS/MOK 修改、可信首次交接、新的显示任务队列、日志系统全面重构或 UI 重做。

本设计不宣称 sysfs、GNOME logical layout 或 ownership 能证明物理显示管线健康。未知状态下拒绝开屏是安全降级，不是恢复成功。

## 2. 已知事实与证据边界

上轮临时源码副本测试运行了真实 daemon 函数，只替换 sysfs 输入、持久化路径及 Agent socket。四个安全断言均失败：

1. `E=disabled/O=false` 的注册仍发送 detached Dock。
2. 持有 replay 锁时 startup 仍发送 Dock。
3. 会话代际变化后旧 ACK 仍授予 ownership。
4. backend Error 被 `probe_session_agent` 判为成功，并被 startup 用作 ready。

现有 ownership 6 项、GNOME 解析/验证 3 项测试通过，尚未覆盖这些跨入口情形。上述证据不等于实机卡死复现。临时测试已清理，实施时须将相关断言重新固化为正式测试。

源码补充事实：Agent 每连接 spawn task；自动旋转直接写后端；attached 主命令、fallback、verify 的预算合计可达 14 秒，daemon 当前响应预算为 12 秒。Agent 端已有 `kill_on_drop(true)`，但杀死 gdctl 进程不作为已撤销 compositor 操作的证据。

当前机器读取到的内核是 `7.0.0-38-generic`；已读服务 ExecStart 与安装二进制 hash，但未证明它们对应本源码。部署验收必须重新记录和比较，不能引用 README 的其他内核结果代替。

## 3. 方案选择

用户在对话中选择 **A：局部收敛**。

- A：复用 daemon 最新 Dock target 与 replay executor，补全入口；Agent 增加共享执行门和有界操作结果记录。保留现有运行时结构，围绕已验证缺口改动。
- B：把所有显示意图迁入统一任务队列，重构协议和调度。可集中管理，但改动与迁移成本更大，用户没有选择。

A 仍需要少量协议结果信息，原因是当前无身份的 `Ack` 无法回答“超时的那次操作后来完成了吗”。这不是持久任务系统：不增加 worker 队列、磁盘任务库、通用调度框架或新依赖。

## 4. 安全契约

### 4.1 观察值与归属

下屏观察至少区分 `Enabled / Disabled / Unknown`；找不到目标、读取失败不能冒充 Disabled。保留现有 boot 绑定的 ownership 字段和旧文件默认值。

- `O=true` 表示本 boot 曾有可信 runtime 关屏证据，不表示 Xe 健康。
- detached 自动操作在 `Disabled/Unknown + 无有效 ownership` 下不下发开屏。
- Unknown、已知在途/结果未定、会话失效或已知显示失败均不得因 O=true 而直接放行。
- `Enabled + O=false` 允许正常布局同步；不将 Enabled 宣称为物理健康证据。
- attach 只能在可信执行与 verify 结果下保留或新建 O。原来已经关闭且无 O 的下屏，重复 attached 同步不授予 O。
- 成功恢复双屏清除 O；boot 不一致清除 O；未知结果不能新授予 O。

不得根据错误消息把正常 disabled 一律描述为 Xe 故障。

### 4.2 三种身份，不混用

- daemon 实例身份：本次 daemon 生命周期唯一，重启不复用；与 boot ID 一起标识协调方。
- Agent 实例及注册代际：进程重启改变实例身份，每次有效注册产生新代际。
- lifecycle epoch：Boot/Pre/Post/Thaw 更新，属于操作 scope；已提交的同 boot ownership 不因普通休眠自动清零，但跨 epoch 的在途结果不得新授予 O。
- Dock target generation 与 operation ID：目标可被替换；operation ID 标识一次具体操作，在网络下发前分配。

沿用现有代际与路径机制，补充缺失身份；具体 Rust 类型/命名留给实现者。注册握手将有效实例/代际信息交给两端。Agent 执行前复核上下文；daemon 提交时复核同一上下文。scope 明确包含 boot、daemon 实例、Agent 实例、注册代际和 lifecycle epoch；target generation 单独表示意图版本。socket 路径相同不能证明实例相同。

### 4.3 单一自动 Dock 事务

所有自动入口只提交意图，由共用事务完成：

1. 更新最新 target；取得 daemon executor，不持有 RuntimeState 写锁跨 IPC await。
2. 读取当前 target、boot、daemon/Agent 身份、注册代际、最新键盘观察、下屏观察和 ownership。
3. 检查身份、在途状态、开屏保护及后端可用性。
4. 在发出任何可能执行的请求字节前，原子持久化 pending operation 与 scope；持久化失败则不下发。下发带 operation ID 的操作；处理结构化结果，重新观察布局。
5. 在 RuntimeState 写锁下复核身份与 target，再提交操作证据及目标结果。
6. 有更新目标则继续处理最新目标；被替代、阻断、失败均返回独立 outcome，不伪装为成功。

startup 不再直接调用 `forward_session_command(SetDockMode)`。monitor 不再在事务外重复采样/提交 ownership。注册先完成握手回复，再异步提交当前意图，保持现有避免注册互等的行为。

最新物理键盘观察必须可以在旧 Dock 执行期间更新：monitor 不得因 await 整个 Dock 操作停止发布键盘状态。只保留一个最新 target，不积压历史 attach/detach。锁内执行前再次检查 target；Agent 已开始的操作不靠取消 Rust future 假装撤销。

新目标替代旧目标，不代表旧操作造成的事实消失。daemon 不在已经过期的目标上继续重试。

## 5. Agent 执行门与超时结果

### 5.1 共享门

Agent 内一个执行门保护所有布局写操作及其 verify；自动旋转也经此门。同步后端操作不能阻塞执行状态查询，使用现有 Tokio 的阻塞任务能力或等价隔离。阻塞任务不因外层 timeout 被丢弃就被标记为结束，锁由真正执行任务持有到结束。

这里的保证是本 Agent 发出的显示写操作不重叠，不约束 GNOME 设置、GDM 或其他外部程序。所有后端保留支持；无法核实的结果降级，不能把非 GNOME 裸 ACK 当作 GNOME verify 证据。

请求可有界等待门，超时且尚未受理时返回 Busy/NotStarted；不留下一个会在调用方放弃后悄悄执行的排队操作。门内复核已注册身份及过期条件。

不对健康检查、操作状态查询、通知或亮度 sysfs 写入使用该布局写锁。读取布局不能用作“没有在途操作”的替代证据。

### 5.2 有界结果记录

Agent 至少维护：当前执行状态、当前 display revision、一个尚未由 daemon 确认消费的自动 Dock operation 及其终态。其他布局写操作递增 revision，防止把先前快照当成最新证据。

自动 Dock 受理后，在发送网络回复之前存储其结果；socket 关闭不取消记录。daemon 按 operation ID 查询、消费结果。未消费前不接收第二个自动 Dock，从而无需无限 receipt 缓存。同一 operation ID 的重复请求只返回当前状态或已有终态，不再次执行；参数/scope 不一致的同 ID 请求拒绝。结果消费需精确匹配 ID，错误或重复消费不得删除其他记录。手动操作/自动旋转在未消费窗口返回 Busy 或跳过，不能抢先覆盖证据。窗口可能因故障延长，按下文迟到结果清理处理，不能承诺始终短暂。

终态至少区分：

- 未执行：身份过期、门忙、保护阻断等，没有开屏副作用。
- 已验证完成：实际 operation ID、scope、执行前后观察、verify 结果和 revision。
- 失败但结果确定：可说明是否已发生显示变更，不凭错误字符串猜测。
- 结果未知：超时、部分执行或验证不足；自动布局写入进入降级状态。

只读状态查询有界且不等待布局锁，因而 daemon 可以区分 Busy、明确终态和无法联系。返回 Error 仅能说明有进程响应，不代表后端 ready。

### 5.3 超时处理

- daemon 等待超时：保留 pending operation 与可能仍在执行的状态，不立即重复下发，不授予 O、不宣布完成。
- daemon 对同一 Agent/operation 有界查询结果。实现者可选择小次数退避，必须有固定上限；健康轮询不触发布局重放。
- 取得可信终态并消费后，才允许处理下一目标。状态 NotFound 不证明请求没有执行：查询可能早于旧请求被受理，因此这时保持 pending/Degraded，不重发也不放行新自动操作。
- 查询超过预算仍不能确认、Agent 换实例、gdctl 超时或部分执行无法核实：进入明确 Degraded，保留诊断证据，停止自动写布局；不靠重新注册、重启 daemon、成功 Ping 或增加 sleep 清除此状态。
- 有界主动核对耗尽后，仍允许每秒最多一次有界的只读迟到结果核对；仅访问当前已注册 Agent，不主动重注册、不触发布局、不重发原命令。读到终态时先持久化证据，再按 ID 消费；这是清理通道，不是自动恢复通道。Agent 不可达则保持 pending 并去重报告，重新可达时继续只读核对。
- 迟到结果为同 scope 的可验证成功，且无后续 revision 冲突时，receipt 消费后可解除 Agent 的 Busy，使用户显式手动布局再次可用；daemon 的自动切屏 Degraded 仍保留到新 boot。本阶段不把迟到证据用于自动重试，也不把用户操作清除 O 当成清除 Degraded。传感器自动旋转仍属于自动写入，保持暂停。
- 迟到结果仍未知或身份/scope 不匹配时，只能清理可确定属于旧操作的终态 receipt；所有无法证明安全的自动/手动布局请求均明确拒绝，不能靠清理 receipt 解锁。用户可重启建立新 boot；本阶段不增加危险的强制恢复按钮。
- 后端命令明确超时后不再自动执行 attached fallback。只有明确的“不支持该参数”等未执行兼容性错误才允许 fallback；无法可靠分类则不 fallback。
- Agent 内命令超时后等待子进程退出/回收；不据此宣称 Mutter 已完成/撤销请求。未知结果仍降级。

本前置阶段不提供“结果未知后无证据自动恢复”。用户会看到降级原因；重启机器会建立新 boot，未知状态不会成为跨 boot ownership。持久化 pending/uncertain 标记，使同 boot 的 daemon 重启不会解锁不明操作。Agent 重启也不能清除 daemon 端标记。后续 #1 在此边界上恢复通信。

## 6. 旧结果与 ownership 提交

区分两件事：

- 操作证据：哪次可信操作在什么 scope 下，确实把 Enabled 变成 Disabled（或恢复双屏）。
- 当前目标完成：操作结果是否匹配当前有效目标。

同一有效 Agent/scope 内，旧 attach 已 verify 成功、只是 target 改为 detach：不能宣布当前 detach 完成，但可以记录该 attach 的关屏事实；在确认没有后续/外部冲突的可用证据后，用于最新 detach 的保护判断。证据与 target 完成在同一提交点分别处理。

boot、daemon 实例、Agent 实例、注册代际或 lifecycle epoch 已变化：旧 ACK 不授予当前 ownership；保留取证信息，必要时降级。即使命令可能已切屏，也不能凭新 session 的一次 show 把旧操作追认为当前 ownership。

手动布局成功后撤销自动 ownership，避免之后把用户关屏误认为 runtime 关屏；旋转若不能证明保持了现有开关状态，同样撤销 O。传感器旋转只在执行门内重新确认双屏仍有效时执行，不能复用锁外陈旧判断。

提交与 target/身份变化使用共同状态同步边界；不能先读取 generation、释放锁，再单独写 O。boot 不变的同 daemon 正常操作无需反复创建新的 boot epoch。

## 7. 生命周期与协议边界

- 不让 lifecycle 背光失败阻止发布 display lifecycle 状态/目标；记录背光错误并独立处理，不假装整体成功。
- Boot/Pre/Post/Thaw 改变有效事务 epoch，使旧操作不能在恢复后被误当成当前完成。在途操作跨 Pre/Post 时保持 pending，仍等待同一执行门的实际结果；结果只作取证，不补授 O，按未知结果降级。旧 scope 的终态可被精确消费以清理 Agent receipt，但不解除 daemon 的 Degraded。完整 resume-ready 等待与 #1 重连机制不在此次实现范围。
- 蓝牙操作不得挡住最新键盘目标发布；显示意图先发布，现有射频操作隔离并有界。无需增加新的蓝牙设置。
- 协议为相关操作增加 scope/operation/outcome 和只读 execution-state 查询。升级协议版本，daemon、Agent、lifecycle/helper/UI 统一构建。版本不匹配明确拒绝，不向旧 Agent 降级发送无上下文的自动 Dock。
- 旧 state 文件可加载；新安全字段缺失不伪造 pending 或 ownership。旧文件已有且 boot 有效的 O 仍遵守新保护，不当作全面健康证明。
- 未知操作跨同 boot daemon 重启保持 Degraded；操作记录不跨 boot 被重用。receipt 消费与 daemon 状态保存的顺序必须允许保守恢复：先持久化待核对/完成信息，再确认消费；崩溃窗口宁可降级，不允许重复执行。
- RuntimeState 的安全字段不能继续依赖现有原地 `fs::write` 与“加载失败就默认正常”的契约。所有状态保存采用同目录唯一临时文件→完整写入→文件 sync→原子 rename→目录 sync；共享现有保存入口以免其他写者再次截断安全字段。
- 仅首次不存在的状态文件可按初始状态启动；已有文件读取/解析失败进入持久化故障降级，禁止显示写操作，不静默当成干净状态，也不以普通默认状态覆盖损坏文件。磁盘保存或 sync 失败时不下发新显示变更、不消费未落盘结果；已在途操作继续收集结果但不宣告安全提交。恢复存储访问本身不自动解除已有 uncertainty；需完整核对，不能丢掉 pending。

## 8. 诊断与验证

必要字段：boot/daemon/Agent identity、registration/target generation、operation ID、来源、阶段、耗时、观察值及其错误、O-before/O-after、verify、outcome。Agent 错误至少进入 user journal；不依赖向 root 的 daemon.log 写入成功。本次不改整个日志存储方案。

自动测试使用可注入 sysfs/时钟/backend 与临时路径，不能写真实 `/var/lib`、发真实 gdctl 或操作蓝牙。

正式测试至少覆盖：

1. 四入口在 Disabled/Unknown + O=false 时零自动开屏；Enabled/O=false 正常同步。
2. 正常 attach→detach 与同 boot ownership 保留；跨 boot 不继承。
3. startup、registration、resume、monitor 同时触发时只走统一事务。
4. Dock 与手动布局、手动/传感器旋转最大并发写操作数为 1；查询可响应。
5. daemon 超时但 Agent 尚在执行：不重复下发；迟到结果可查询；在主动核对预算内确定完成后再处理最新 target；已锁定 Degraded 后按第 12 项只清理。
6. 未开始的 Busy 请求不会稍后自行执行；不明结果、命令超时无无限重试/fallback。
7. 旧 session/实例/target 的结果不能宣布当前完成；旧 attach 的可信副作用证据仍支持同 scope 最新 detach。
8. 收到新 target 时跳过旧重试，monitor 可继续发布最新键盘观察。
9. Error 不算 backend-ready；布局读取成功不等于执行门空闲或物理健康。
10. 手动布局或旋转改变开关状态撤销 O；传感器在拿锁后再次检查双屏。
11. daemon/Agent 重启、原子保存各故障点、状态损坏及 receipt 消费的崩溃窗口不解锁未知操作；旧协议明确拒绝。
12. 有界查询结束后才完成的 receipt 仍能只读清理；可信成功使显式手动布局可用但不自动解除 Degraded；未知终态不解锁写入。
13. 背光错误、蓝牙迟滞不阻止最新显示目标发布；既有 locale 及 runtime 回归通过。

实施检查：Rust library 测试、所有 runtime binary 的 cargo check、必要前端构建与协议回归。不得把测试用的 fake backend 通过当作硬件验收。

部署必须另行执行并记录：源码 SHA、Cargo.lock hash、四个安装二进制 hash/运行进程可执行文件、ExecStart、内核、locale、boot ID。先给出测试结果、差异和回滚步骤，再申请真实安装/服务重启。用户未授权本轮自动部署或休眠设备。

本阶段实机验收聚焦正常插拔、并发操作与保守降级。#1 的连续十次休眠后自动恢复留待其实现后单独验收；本阶段不能声明 #1 已修复。

## 9. 实现者可自行决定

模块拆分、类型/字段命名、具体有界等待常量、测试夹具组织及错误枚举排布；复用已有 Tokio/Serde，无新依赖。常量需有测试边界与清晰理由，不以延长超时替代状态语义。

以上协议范围、Agent 内共享门、少量结果记录、保守 Degraded 与手动操作的归属撤销，是设计者提出的具体实现选择，随本文交用户确认，并非用户此前逐项指定。

## 10. 本次设计流程

- [x] 阅读现有调用链与前轮验证结果。
- [x] 提出 A/B，用户选择 A。
- [x] 写出本设计。
- [x] 独立 builder check，处理 lifecycle scope、原子持久化和迟到结果清理缺口；复审结果：nothing important left。
- [ ] 用户批准设计。
- [ ] 批准后调用 writing-plans，形成实现计划。
- [ ] 按计划实施、测试及代码审阅；部署单独处理。
