# Issue #2 受控部署与实机验收记录

日期：2026-10-24。执行方式：Native。用户已要求“开始执行”。

**当前结论：BLOCKED_AFTER_BOOT。** 修复后的候选 `d117a43` 已统一安装并完成一次正常重启；启动时的自动 Dock 遇到 GNOME 命令错误，显示安全门进入 `Degraded/Unknown`。主屏正常，但基础功能验收未通过，后续显示操作已停止。下文保留旧候选的部署前阻塞记录。

## 已完成的准备

- 固定候选源码 `db73a686d69ddc5e3fbb8540e1b0ec71372385c5`，独立导出 staging，未修改主工作区产品代码。
- 本机：`7.0.0-38-generic`，GNOME/Wayland；两块 eDP 面板 sysfs 均为 connected/enabled。用户确认系统显示双屏开启、键盘盖在下屏上；用户随后确认主屏画面正常、方向正常、能够打开终端；画面与恢复入口的预检条件已满足。下屏物理画面因遮挡留待后续验收。
- 清点了实际 system/user unit、sleep hook、进程和状态文件；旧 shell services/hook 未发现处于运行状态。
- daemon 和 Agent 都 active，但 protocol-2 daemon 报告 Agent disconnected。现有 state 的 ownership=true 与双屏开启不能作为新版本 ownership 证据，未采用或修改它。
- 四个 runtime release 二进制及 Tauri UI release 均离线构建成功；五项产物已复制到候选目录并计算 hash。Cargo.lock/package-lock.json 前后 hash 相同。
- 原候选完整测试：**115 passed, 0 failed**；全部 binary/doc targets 通过。`cargo check --locked --offline --bins` 通过。结束前再次核对：4 个已安装 runtime 文件 hash 与备份相同，5 项候选 hash 与 manifest 相同。
- 当前四个已安装 runtime 二进制、units、hook 目标、配置与 state 的只读副本已保存。未发现已安装 UI/desktop 入口，因此不存在可声称“已验证完整匹配”的旧五项回滚 cohort。

原始证据目录（仅本机、不提交原始数据）：
`local-reports/issue2-deployment/20261024T143414Z/`。

## 新发现的部署阻塞：双屏 gdctl 输出漏解析主屏

只读证据链：

1. DRM sysfs：eDP-1 与 eDP-2 都 enabled。
2. `gdctl show` 原始输出：Logical monitor #1 包含 eDP-1，Primary=yes、Transform=180、Scale≈1.6667；Logical monitor #2 包含 eDP-2。
3. 旧 Agent 只读 `get_display_layout` 回复：eDP-1 enabled=false、primary=false、scale=1、transform=0；与原始输出矛盾。
4. 在候选源码的**另一个独立诊断副本**中，加入只使用捕获文本的测试 `captured_dual_monitor_gdctl_preserves_primary_panel`。测试实际调用原候选 `parse_gdctl_output`，断言主屏仍 enabled/primary，结果 **FAIL**：eDP-1 被解析为 enabled=false。没有调用真实后端写入。

根因位于 `ui-tauri-react/src-tauri/src/hardware/display_config.rs` 的逻辑显示器 connector 提取：对行先 `split_whitespace().next()`，但非末尾树分支的 connector 行形如 `│      └──eDP-1 (...)`，第一个 token 是 `│`，不是 connector。最后一支 eDP-2 没有该竖线前缀，因而能够被识别。既有测试仅覆盖单 logical monitor，未暴露这种分支结构。

影响：候选 `dock_transaction::drain_one` 要求逻辑布局中 eDP-1 enabled 才认为 backend ready。此错误会在真实双屏布局下阻塞自动 Dock；手动布局/方向也不能信任错误的基线。不能通过删掉 readiness 检查或借 sysfs enabled 强行放行。

该问题存在于候选使用的既有 parser，不是此次部署产生的现场修改。原有 115 项测试全绿不能证明捕获到的真实输入受支持。

本轮保留失败回归在本地诊断副本，**未修改候选源码、已构建产物或主工作区产品代码**。计划明确规定发现缺陷后另行诊断修复，不在部署过程中临时绕过保护，因此没有继续安装。

## 其他执行条件

- `sudo -n true` 返回“interactive authentication is required”；当前通道不能读取 root daemon 的 `/proc/<pid>/exe` hash。系统级安装与 root 运行文件核验仍需要用户在本机终端认证；不要求用户在聊天中发送密码。
- 新解析问题是首要阻塞，即使完成 sudo 认证也不应安装当前候选。
- 旧 daemon/Agent 不自动重连的问题继续作为 #1 的已有基线记录，本轮未尝试重启它们规避问题。

## 后续恢复顺序

1. 单独修复 connector 行的树形前缀解析，先将本机捕获失败用例固化为可提交、脱敏的回归；补充多 logical monitor/connector enabled、primary、scale、position 和 transform 断言。
2. 修复后运行完整测试与检查，形成新提交；更新部署候选 SHA，重新构建全部五项产物并核验锁文件/hash。当前候选不得直接沿用。
3. 主屏/方向/终端观察已由用户补齐；后续补齐 root 运行 hash 和完整回滚材料，再重新判定预检 GO。
4. 从统一切换阶段继续；安排一次正常 reboot 后才做计划中的基础实机验收。本次不执行 suspend。

## 原候选阶段的验收矩阵

| 项目 | 结果 |
| --- | --- |
| 现有环境/服务/文件只读清点 | 已采集，存在待补证据 |
| 原候选离线 runtime/UI release 构建 | PASS |
| 原候选既有 115 项库测试及 binary/doc targets | PASS |
| 捕获的真实双屏 gdctl 输入回归 | FAIL，阻塞部署 |
| 系统安装、统一切换、reboot | NOT_STARTED |
| Dock 3 轮、自动旋转、手动布局、轻量竞争 | NOT_STARTED |
| 回滚实操 | NOT_NEEDED（系统安装未改变） |

本轮操作裁决：按批准的操作 runbook 使用实际检查和既有回归，不制造真实故障或新增产品代码；预检有缺口时仅继续无系统副作用的离线构建/备份；发现确定 parser 缺陷后停止部署。代价是本轮未完成实机验收，需修复后重新生成候选。

## 解析阻塞的后续处理（同日）

用户要求继续。针对上述捕获的双屏结构，已在主工作区新增脱敏回归 `parses_both_logical_monitors_with_nested_tree_prefixes`，先观察到主屏 enabled/primary 断言失败，再修正逻辑 monitor connector 行中树形前缀的提取，观察到该测试通过。在独立诊断副本中，原始本机 `gdctl show` 捕获文本的回归也从 FAIL 变 PASS。主工作区完整库测试现为 **116 passed, 0 failed**，全部 binary/doc targets、Rust binary check、格式和前端构建通过。修复提交：`d117a43685226ac45f57047fddcd7e94323e42d0`。

旧候选 `db73a68` 仍禁止安装。修复后的固定候选 `d117a43` 从独立源码导出，五项 release 产物离线构建成功；候选源码 **116 passed, 0 failed**，binary/doc targets、cargo check、UI 构建通过，锁文件 hash 未变化。

用户在本机终端完成只读 root 预检：运行中的旧 daemon/Agent 的 executable hash 与已备份的安装文件一致，备份、候选文件及 boot ID 均匹配。用户确认工作已保存、可正常重启。在同一终端执行受控切换后，daemon、Agent、lifecycle 均停止；五个已安装文件的 hash 由脚本和独立只读核对均与新候选一致。原有 system/user units 仍 enabled，当前进程未启动新旧混合 cohort。安装清单和原始日志保存在本地 `local-reports/issue2-deployment/20261024T143414Z/`。

统一切换完成时状态为 **WAITING_FOR_REBOOT**，旧 boot ID 为 `a6d3e086-4eba-4d53-8e5f-2597b20b31a4`。用户随后执行了一次正常重启。

## 重启后阻塞：GNOME 的无效 `--mode off`

新 boot ID `f280c70f-eab4-48ee-9fb2-e17549d9feb8` 与旧值不同。daemon、Agent active 且各只有一个进程；lifecycle unit active/exited。五项 installed hash 再次等于候选，protocol 3 的 daemon 与 Agent 只读请求正常，Agent 已注册到本 boot。两块 eDP 均 connected/enabled，`gdctl show` 与协议布局都显示 eDP-1 为主屏、eDP-2 仍启用。用户确认主屏画面和方向正常，键盘仍盖在下屏，系统显示设置中下屏仍开启。

启动时自动 Dock 的唯一一次 operation 返回 `Unknown`：`Failed to create configuration: No mode off available for eDP-2`。Agent gate 的 revision=1、`active=null`、`receipt=null`，`blocked=Display result could not be verified`；daemon state 的 `displaySafety.degraded=Display result cannot be adopted: Unknown`、`manual_allowed=false`。两次相隔超过两秒的只读快照一致，后续只读复核仍一致，没有继续派发新 operation。安全阻断按设计起效，但 Dock 功能未成功，下屏仍开启。未清理 state、重启服务或重发显示变更；没有继续做物理 Dock、旋转、手动布局或竞争用例。

现场 `/usr/bin/gdctl` 的配置生成器将 `--mode` 的值当作真实显示模式查找；eDP-2 不存在名为 `off` 的模式。GNOME 配置只包含传入的 logical monitors，因此附着模式应只配置 eDP-1，而不为 eDP-2 指定 `--mode off`。源码中原有一个仅用于测试的 primary-only fallback 参数函数，生产 `build_gnome_dock_mode_args` 却仍输出无效参数。针对生产参数构造的回归先 RED，删去附着分支的 eDP-2 参数后 GREEN；完整库测试 **116 passed, 0 failed**、binary/doc targets、cargo check、格式检查通过。修复提交 `8ca7fd4`，**仅在源码中，未部署到本机**。

原始证据留在忽略的本地目录 `local-reports/issue2-deployment/20261024T143414Z/cases/00-postboot/`：两份协议/state 快照、daemon/Agent/kernel journal、`gdctl show`、RED/GREEN 和完整测试日志。`8ca7fd4` 已独立导出并离线构建五项 release 候选；候选源码 116 项库测试、binary/doc targets、cargo check 和 UI build 均通过，锁文件未变化。产物及 hash 清单保存在同一运行目录的 `candidate/next/`，没有安装。当前已安装的仍是 `d117a43` 候选，安全门保持阻断。由于 `Unknown` 的在途结果边界，未在同一 boot 回滚到旧实现，也不靠再次重启绕开阻断；下一轮须审核新候选及回滚条件，再单独安排受控部署、新 boot 和物理验收。主屏-only GNOME 命令的真实硬件效果尚未验证。

| 当前阶段项目 | 结果 |
| --- | --- |
| 统一切换、五项 installed hash、新 boot/单实例/protocol 3 | PASS |
| 主屏与方向用户观察 | PASS |
| 启动自动 Dock、下屏关闭 | FAIL：`gdctl --mode off` 无效 |
| Unknown 后停止自动重发与手动写入 | PASS（只读证据） |
| 3 轮 Dock、旋转、手动布局、轻量竞争 | BLOCKED，未执行 |
| 完整 #2 suspend/resume | NOT_STARTED（不在本轮范围） |
