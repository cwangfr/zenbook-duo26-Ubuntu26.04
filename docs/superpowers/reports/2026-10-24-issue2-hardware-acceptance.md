# Issue #2 受控部署执行记录：部署前阻塞

日期：2026-10-24。执行方式：Native。用户已要求“开始执行”。

**结论：BLOCKED_BEFORE_DEPLOYMENT。未安装候选二进制、未停止/重启服务、未重启电脑、未执行真实显示写入。实机功能验收尚未开始。**

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

## 验收矩阵

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
