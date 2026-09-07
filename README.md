# tv-obsbroadcast-scheduler

> **电视台式自动播出控制系统** —— 为 OBS Studio 打造的毫秒级精度的节目编排插件。

把准备好的视频文件编排成节目表（含正片 + 广告插播），本插件会按时间表自动切换
OBS 场景中的 `ffmpeg_source`，节目之间**硬切**（无任何过渡转场），单媒体源即可完成整档播出。
OBS 重启后调度器自动从中断点恢复。

**特性一览**

| 功能 | 说明 |
| --- | --- |
| 毫秒级自动切换 | 本地时钟 + 可配置 lead-in（默认 200ms），确保零黑帧硬切 |
| 节目表编排 | 增删改节目项，文件首次播放后自动探测实际时长并回写 |
| 插播（interstitial） | 正片某时刻自动 / 即时插入广告 / 短片，播完后回到主节目剩余部分继续 |
| 双 UI | OBS 原生属性面板（核心 CRUD，无网络依赖）+ 自定义 Dock 网页（高级时间轴 / 批量导入） |
| 播出浮窗 | `overlay/` 提供"正在播出 / 下一档 / 倒计时"，可叠在直播画面上 |
| 三平台 | Windows 10 1809+ / Linux glibc 2.31+（x64 & arm64）/ macOS 13+ Apple Silicon |
| 凭据本地化 | OBS WebSocket 凭据、节目表 JSON 都存在本地，便携模式 + 用户配置 fallback |
| 崩溃恢复 | 调度器每次状态变更落盘；OBS 重启后从中断点继续编排表 |

**安装**

从 [Releases](../../releases) 下载平台对应的插件包（`tv-obsbroadcast-scheduler-<平台>-<版本>.zip/.tar.gz`），
解压后把整个文件夹放进 OBS 插件目录：

| 平台 | 插件目录 |
| --- | --- |
| Windows | `%APPDATA%\obs-studio\plugins\` |
| Linux | `~/.config/obs-studio/plugins/` |
| macOS | `~/Library/Application Support/obs-studio/plugins/` |

详见 [docs/README.md](docs/README.md)。

**架构概览**

双进程分层：

- **C 薄壳插件** (`plugin/`)：在 OBS 主进程运行，注册 *Broadcast Scheduler Control* source / dock、Qt 属性面板、拉起并管理 Rust 引擎子进程
- **Rust 引擎** (`engine/`)：独立子进程，承担调度推进、节目表 CRUD、OBS WebSocket 5 通信、本地 HTTP/WS 服务（axum）

详见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

**本地编译**

```bash
# 任意平台只需要 Rust 1.74+
cargo build --release --manifest-path engine/Cargo.toml

# C 薄壳（需要 OBS Studio 28+ 源码 headers）
cmake -S plugin -B plugin/build \
  -DLIBOBS_INCLUDE_DIR=<path-to-obs-studio>/libobs \
  [-DOBS_IMPORT_LIB=<obs.lib> | -DOBS_STUB_LIB=<libobs.so>]
cmake --build plugin/build --config Release
```

默认 Rust target = `x86_64-pc-windows-gnu`（开发机用 MinGW-w64 即可）。
CI / 正式发布用 `x86_64-pc-windows-msvc`，加 `--target` 即可。

详见 [docs/ARCHITECTURE.md § 编译](docs/ARCHITECTURE.md) 与
[docs/README.md § 编译环境](docs/README.md#编译环境-windows-gnu-方案)。

**许可**: MIT
