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

**安装（3 步）**

1. 从 [Releases](../../releases) 下载 **`tv-obsbroadcast-scheduler-<平台>.zip/.tar.gz`**
   （这是插件完整包，内含插件本体 + 引擎。文件名带 `engine-` 前缀的只是引擎二进制，一般不需要单独下载）。
2. 解压，得到文件夹 `tv-obsbroadcast-scheduler/`，把它**整个**复制到 OBS 的用户插件目录：

   | 平台 | 插件目录 |
   | --- | --- |
   | Windows | `%APPDATA%\obs-studio\plugins\` |
   | Linux | `~/.config/obs-studio/plugins/` |
   | macOS | `~/Library/Application Support/obs-studio/plugins/` |

   放好后的结构必须是这样（**不要再套一层目录**）：

   ```
   %APPDATA%\obs-studio\plugins\
   └── tv-obsbroadcast-scheduler\
       ├── bin\64bit\tv-obsbroadcast-scheduler.dll   ← 插件本体
       └── data\
           ├── engine\tv-obsbroadcast-scheduler.exe  ← Rust 引擎（插件自动拉起）
           └── locale\en-US.ini                      ← 界面文案
   ```

3. **重启 OBS**（插件只在启动时加载）。

**快速上手**

1. 先在 OBS 里准备好视频：在场景中**添加一个"媒体源"（Media Source）**，记住它的名字（例如 `main_media`）。
2. 开启 OBS WebSocket：OBS 菜单 **工具 → WebSocket 服务器设置**，勾选"启用 WebSocket 服务器"，记下端口（默认 `4455`）与密码。
3. 同一场景点 **＋ → 添加来源**，选择 **Broadcast Scheduler Control**（中文界面下叫"广播调度控制"）→ 确定。
4. 右键该来源 → **属性**，填写：
   - Host `127.0.0.1`、Port `4455`、Password（步骤 2 的密码）
   - **Media Source Name**：填步骤 1 的媒体源名字（如 `main_media`）
   - 点 **Test Connection**，结果会写在 OBS 日志里（帮助 → 日志文件 → 查看当前日志）
   - 勾选 **Enabled** 让调度器开始工作
5. 编排节目表：插件会自动拉起引擎，用浏览器打开 **http://127.0.0.1:8789/admin**
   （属性面板的 *Open Admin in Browser* 按钮也会把该地址打印到日志）。
   在 admin 里添加节目、设置插播、导入脚本，调度器即按时间表硬切媒体源。

> admin 目前是**浏览器里的网页**（不是 OBS 内的 Dock 面板）：OBS 没有给纯 C 插件的 Dock 接口，
> 所以 v0.0.1 用浏览器承载高级 UI，核心开关/凭据仍在 OBS 属性面板里。

**看不到插件？按顺序检查**

| 现象 | 检查 |
| --- | --- |
| 添加来源里没有该插件 | 目录是否多套了一层（应是 `plugins\tv-obsbroadcast-scheduler\bin\64bit\*.dll`）；OBS 是否为 64 位；是否重启过 OBS |
| OBS 日志出现 `os_dlopen ... failed` | 插件 DLL 与 OBS 版本不匹配（需要 OBS 28+ 64 位），或缺少 VC++ 运行库 |
| 插件在，但引擎不启动 | 确认 `data\engine\tv-obsbroadcast-scheduler.exe` 存在；手动双击它能否运行；是否被杀软拦截 |
| Test Connection 失败 | 引擎没起来，或 OBS WebSocket 服务器没启用 / 端口密码不对 |

详见 [docs/README.md](docs/README.md)。

**架构概览**

双进程分层：

- **C 薄壳插件** (`plugin/`)：在 OBS 主进程运行，注册 *Broadcast Scheduler Control* source、提供属性面板、拉起并管理 Rust 引擎子进程
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
