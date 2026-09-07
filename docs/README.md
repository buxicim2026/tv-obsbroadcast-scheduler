# 用户文档

> ⚠️ 本文档随版本迭代持续补充。完成的功能请以最新 Releases 的 `CHANGELOG.md` 为准。

## 目录

1. [快速开始](#快速开始)
2. [安装](#安装)
3. [配置 obs-websocket](#配置-obs-websocket)
4. [添加场景源](#添加场景源)
5. [编辑节目表](#编辑节目表)
6. [启用自动播出](#启用自动播出)
7. [高级操作](#高级操作)

## 快速开始

1. 在 OBS 中启用 obs-websocket 插件（**工具 → obs-websocket 设置**，记下端口和密码）
2. 在 OBS 场景里加一个 **媒体源**（Media Source / VLC Source），命名为你想要的即可
3. 启动本插件后，在场景里加一个 **Broadcast Scheduler Control** 源，填入 obs-websocket 的连接信息
4. 在弹出的属性面板里开始添加节目项（每项有文件路径 + 起始时间 + 声明时长）
5. 启用自动播出 → 引擎接管

## 安装

参见根目录 [README.md §安装](../README.md#安装)。

跨平台插件目录：

| 平台 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\obs-studio\plugins\tv-obsbroadcast-scheduler\` |
| Linux | `~/.config/obs-studio/plugins/tv-obsbroadcast-scheduler/` |
| macOS | `~/Library/Application Support/obs-studio/plugins/tv-obsbroadcast-scheduler/` |

OBS 启动时插件会自动拉起 Rust 引擎子进程，无需额外操作。

## 编译环境 (Windows GNU 方案)

默认目标：`x86_64-pc-windows-gnu`（MinGW-w64）。这套方案比 Visual Studio
Build Tools 装得轻，CI 在 GitHub Actions 上用 ubuntu runner 更稳。

1. 安装 [MinGW-w64 / WinLibs](https://winlibs.com/) — 解压后把 `mingw64/bin`
   加入 `PATH`。
2. `rustup target add x86_64-pc-windows-gnu`
3. 在项目根下执行：
   ```bash
   cargo build --release --manifest-path engine/Cargo.toml
   ```

`.cargo/config.toml` 已经把 `build.target` 默认设到 GNU。如果你想切回
MSVC，临时加 `--target x86_64-pc-windows-msvc` 即可。

## 配置 obs-websocket

OBS Studio 28+ 自带 obs-websocket 5 插件。首次启用：

1. OBS 菜单 → **工具** → **obs-websocket 设置**
2. 勾选 **启用 obs-websocket 服务器**
3. 记住 **服务器端口**（默认 `4455`）和 **服务器密码**（首次启用时显示）
4. 点 **应用**

## 添加场景源

在 OBS 场景中：

1. **媒体源**（第一步先准备，确保视频能正常播放）：右键场景 → 添加 → 媒体源 → 命名（例如 `main_media`）
2. **Broadcast Scheduler Control**（第二步让插件调度它）：右键场景 → 添加 → Broadcast Scheduler Control

`main_media` 必须是 **媒体源**（ffmpeg_source）才支持自动切文件。其它类型源不支持硬切。

## 编辑节目表

**面板 1：原生属性面板**

在场景中选中 *Broadcast Scheduler Control* → 右键 → **属性**：

- 顶部 **OBS WebSocket 连接**：主机 / 端口 / 密码 + **连接测试** 按钮
- **受控媒体源名称**：填场景里那个媒体源的名称（例如 `main_media`）
- **节目清单**：表格视图，列：名称 / 文件路径 / 类型 / 起始时间 / 时长
- 选中行右侧 **详细编辑**：每个字段都有输入框
- 底部 **状态条**：下一档 + 剩余时长

**面板 2：Dock 网页**

OBS 菜单 → **视图** → **停靠部件** → **Custom Browser Docks** → 新建 →

- 名称：`Broadcast Scheduler`
- URL：`http://127.0.0.1:8789/admin`

Dock 视图提供：

- 24h 横向时间轴（毫秒刻度）
- 节目块彩色区分（正片 / 插播 / 广告）
- 实时"现在线"（红色细线）
- 批量导入（拖放文件夹 / CSV / JSON）
- 插播快捷触发面板
- 报表（实际开播时间 vs 计划）

## 启用自动播出

- **面板 1** 顶部：**启用自动播出** 复选框
- 或者 **面板 2** Dock 主页右上角大号 **ON AIR** 开关

启用后调度器立即接管：

- 当前时间 < 第一条起始 → 等待
- 当前时间落在某条节目区间内 → 立即切到该节目
- 当前时间超过最后一条 → 等价于"演出结束"，用户可手动跳到下一档或改写编排表

## 高级操作

| 操作 | 位置 |
| --- | --- |
| 即时插播 | Dock 主页 → 选"主节目" → 选"插播条目" → **立刻插入** |
| 跳到下一条 | 原生属性面板底部按钮 / Dock 控制条 |
| 跳过当前 | Dock 控制条 |
| 暂停 / 继续 | 原生属性面板底部 / Dock 控制条 |
| 调整 lead-in | 原生属性面板 → 设置 → lead-in 毫秒（默认 200ms） |
| 时钟偏移对齐 | 原生属性面板 → 设置 → 时钟偏移毫秒 |
| 导出节目表 | Dock 主页 → 导出 → CSV / JSON |
| 查看日志 | 引擎日志位于 `plugins/tv-obsbroadcast-scheduler/engine/engine.log` |
