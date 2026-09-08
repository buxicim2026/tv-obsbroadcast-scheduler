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
| 双 UI | OBS 原生属性面板（凭据 / 目标源 / 开关 / 测试连接）+ 浏览器 admin（`http://127.0.0.1:8789/admin`，节目表编排 / 时间轴 / 批量导入） |
| 播出浮窗 | `overlay/` 提供"正在播出 / 下一档 / 倒计时"，可叠在直播画面上 |
| 三平台 | Windows 10 1809+ / Linux glibc 2.31+（x64 & arm64）/ macOS 13+ Apple Silicon |
| 凭据本地化 | OBS WebSocket 凭据、节目表 JSON 都存在本地，便携模式 + 用户配置 fallback |
| 崩溃恢复 | 调度器每次状态变更落盘；OBS 重启后从中断点继续编排表 |

**安装（OBS 脚本，3 步）**

> 前端是 **OBS Lua 脚本**（不是二进制 DLL）。它由 OBS 自己加载执行，
> 因此不存在"DLL 没导出符号 / libobs ABI 不匹配 / 缺 VC++ 运行库"这类
> 二进制插件特有的加载失败问题。

1. 从 [Releases](../../releases) 下载 **`tv-obsbroadcast-scheduler-<平台>.zip/.tar.gz`**。
2. 解压到任意位置（例如 `C:\obs-scheduler\`），**保持这个结构**：

   ```
   tv-obsbroadcast-scheduler\
   ├── tv_obsbroadcast_scheduler.lua        ← 在 OBS 里加载这个文件
   └── engine\
       └── tv-obsbroadcast-scheduler.exe    ← Rust 引擎（脚本自动拉起）
   ```

3. OBS 菜单 **工具 → 脚本 → 右下角 "＋" → 选择 `tv_obsbroadcast_scheduler.lua`**。
   加载后脚本会自动拉起引擎。

**快速上手**

1. 在场景里**添加一个"媒体源"（Media Source）**，记住它的名字（例如 `main_media`）。
2. 开启 OBS WebSocket：**工具 → WebSocket 服务器设置** → 勾选"启用 WebSocket 服务器"，
   记下端口（默认 `4455`）与密码。
3. 在脚本面板（或在场景中 **＋ → 添加来源 → Broadcast Scheduler Control** 的属性里）填写：
   - OBS WebSocket Host `127.0.0.1`、Port `4455`、Password
   - **Media Source Name**：`main_media`（步骤 1 的名字）
   - 点 **Test Connection**（结果写在 OBS 日志：帮助 → 日志文件 → 查看当前日志）
   - 勾选 **Enable scheduler**
4. 编排节目表：点 **Open Admin in Browser**（或浏览器打开 `http://127.0.0.1:8789/admin`），
   在 admin 里加节目、设插播、导入脚本——调度器即按时间表硬切媒体源。

**排错**

| 现象 | 检查 |
| --- | --- |
| OBS 报脚本加载失败 | `.lua` 与 `engine\` 是否在同一层；OBS 是否 ≥ 28 |
| 脚本在但引擎不起 | 手动双击 `engine\tv-obsbroadcast-scheduler.exe` 看能否运行；是否被杀软拦截 |
| Test Connection 失败 | 引擎没起来，或 WebSocket 未启用 / 端口密码不对（日志里有 `[TVBS]` 前缀的信息） |
| 添加来源里没有该源 | 脚本是否已加载（工具 → 脚本 里应能看到它）；source 是在脚本加载时注册的 |

详见 [docs/README.md](docs/README.md)。

**架构概览**

双进程分层：

- **Lua 脚本前端** (`obs-script/`)：由 OBS 加载，注册 *Broadcast Scheduler Control* source、提供属性面板、拉起并监控 Rust 引擎、把凭据推给引擎
- **Rust 引擎** (`engine/`)：独立子进程，承担调度推进、节目表 CRUD、OBS WebSocket 5 通信、本地 HTTP/WS 服务（axum）

> 早期的前端是 **C 二进制插件**（`plugin-c-legacy/`）：在真机上无法被 OBS 加载
> （DLL 导出 / libobs ABI / 运行库依赖一类问题），已归档，**不再参与构建**。

详见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

**本地编译**

```bash
# 只需要 Rust 1.74+（前端是 Lua 脚本，不需要 OBS SDK / C 编译）
cargo build --release --manifest-path engine/Cargo.toml

# 产出 target/release/tv-obsbroadcast-scheduler(.exe)，
# 与 obs-script/tv_obsbroadcast_scheduler.lua 一起放到 engine/ 同级即可使用。
```

默认 Rust target = `x86_64-pc-windows-gnu`（开发机用 MinGW-w64 即可）。
CI / 正式发布用 `x86_64-pc-windows-msvc`，加 `--target` 即可。

详见 [docs/ARCHITECTURE.md § 编译](docs/ARCHITECTURE.md) 与
[docs/README.md § 编译环境](docs/README.md#编译环境-windows-gnu-方案)。

**许可**: MIT
