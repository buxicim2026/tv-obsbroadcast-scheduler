# 架构总览

## 设计原则

1. **电视台式自动播出**：节目表 → 调度器 → 媒体源切换，毫秒精度
2. **零黑帧硬切**：lead-in 提前切换文件，到点 restart，单帧过渡
3. **OBS 友好复用**：不重新实现 ffmpeg 解码器，直接调 OBS 自带 `Media Source`
4. **崩溃恢复**：所有调度状态每 5 秒落盘；OBS 重启可继续

## 数据流

```
                      ┌────────────────────────────────────────┐
                      │              OBS Studio                │
                      │  ┌──────────────┐  ┌─────────────────┐ │
                      │  │ Media Source │  │ Scheduler Ctrl  │ │
                      │  │ (ffmpeg)     │  │ (plugin source) │ │
                      │  └──────────────┘  └─────────────────┘ │
                      │          ▲                  │          │
                      │          │ WS 5             │ bootstrap│
                      │          │                  ▼          │
                      │  ┌────────────────────────────────────┐ │
                      │  │  obs-websocket 5 server (9455)     │ │
                      │  └────────────────────────────────────┘ │
                      └────────────┬───────────────────────────┘
                                   │ WS 5
                      ┌────────────▼─────────────┐
                      │   Rust Engine (subproc)  │
                      │  ┌─────────────────────┐ │
                      │  │ Scheduler loop      │ │  50ms tick
                      │  │  (Rust)             │ │
                      │  └─────────────────────┘ │
                      │  ┌────────────┐ ┌───────┐│
                      │  │ axum       │ │ WS    ││
                      │  │ HTTP :8789 │ │ /ws   ││
                      │  └─────┬──────┘ └───┬───┘│
                      └────────┼────────────┼────┘
                               │            │
              ┌────────────────▼──┐    ┌────▼─────────────────┐
              │  admin/ 网页       │    │  overlay/ 浮窗        │
              │  (vanilla HTML/JS) │    │  (vanilla HTML/JS)    │
              └───────────────────┘    └──────────────────────┘
```

## 模块分层

### C 薄壳插件 (`plugin/`)

| 文件 | 职责 |
| --- | --- |
| `src/plugin.c` | `obs_module_load/unload`，注册 source kind、拉起并监控引擎 |
| `src/source.c` | 注册 *Broadcast Scheduler Control* source kind |
| `src/source_properties.c` | Qt 属性面板：OBS-WS 配置、target_input、scheduler 开关、测试连接、Open Admin 按钮 |
| `src/engine_proc.c` + `src/platform/*` | 拉起 / 终结 Rust 引擎子进程；Win 用 JobObject，POSIX 用 fork+prctl，macOS 用 posix_spawn |

> **Admin UI 的入口**：OBS 没有公开的纯 C dock API（`obs_register_dock_id` 并不存在），
> 因此当前版本不做 in-OBS dock，改用属性面板的 *Open Admin* 按钮 + 浏览器打开
> `http://127.0.0.1:8789/admin`。把 admin 页面嵌入 OBS（C++ + Qt/QWebEngine）
> 列为未来路线。

### Rust 引擎 (`engine/`)

```
engine/src/
├── main.rs               # CLI 启动 + 启动编排
├── lib.rs                # 模块导出
├── config.rs             # 节目表 + WS 配置 + 便携路径
├── playlist.rs           # 节目表 CRUD + JSON 持久化
├── scheduler.rs          # 时间轴推进引擎 / 状态机
├── interrupt.rs          # 插播触发 / 续接计算
├── media_probe.rs        # 通过 obs-websocket 探测 ffmpeg_source 时长
├── app_status.rs         # 全局共享状态
├── embedded.rs           # include_dir! 内嵌 admin / overlay
├── obs_ws/
│   ├── mod.rs
│   ├── messages.rs       # 协议消息类型
│   └── codec.rs          # 帧编解码 / 类型化 API
├── server/
│   ├── mod.rs            # axum Router
│   ├── bootstrap.rs      # C 薄壳 → 引擎 bootstrap (POST /api/bootstrap)
│   ├── schedule_api.rs   # 节目表 REST
│   ├── ws_push.rs        # 实时状态推送
│   └── health.rs         # /healthz
└── bin/
    └── main.rs
```

### 前端

| 路径 | 角色 | 内嵌 |
| --- | --- | --- |
| `admin/` | 高级编辑页（Dashboard / 时间轴 / 批量导入 / 报表） | 通过 axum 服务 |
| `overlay/` | 播出状态浮窗（ON AIR + 倒计时 + 下一档） | 通过 axum 服务 |

## 关键流程

### 启动

1. OBS 启动 → 加载 `tv-obsbroadcast-scheduler(.dll/.so/.dylib)`
2. `obs_module_load` →
   - 注册 *Broadcast Scheduler Control* source kind
   - 拉起 Rust 引擎子进程（包含 JobObject 防止 OBS 崩溃时孤儿）
   - 启动引擎监控线程（引擎崩溃后自动拉起）
3. 引擎启动 → 读取本地 `config.json`（凭据 / 节目表）→ 起 axum 服务 :8789
4. C 端在 source settings 写有 WS 凭据时 → 发 **bootstrap** 通知引擎

### 启用自动播出

1. 用户在原生属性面板勾上"启用自动播出" → 写 source settings
2. C 端发 `POST /api/scheduler/enable` 给引擎（或通过 bootstrap）
3. 引擎加载节目表，遍历找到当前 `start_at ≤ now < end_at` 的条目，若有则立即切；否则找下一条，进入 Armed 状态
4. 主循环每 50ms tick，推进时间轴

### 切节目

```rust
// 1. 切到下一条前 200ms（lead-in）
SetInputSettings {
    input_name: "main_media",
    settings: { local_file: next_path, is_local_file: true, looping: false, ... }
}
sleep(10ms)
// 2. 到点：restart
TriggerMediaInputAction {
    input_name: "main_media",
    action: "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_RESTART"
}
```

OBS 的媒体源切换流程是文件解析→seek→解码。`SetInputSettings` 让 OBS 后台准备好新解码器，到点 `RESTART` 触发 seek+play，理论黑场窗口 ≤ 1 帧。

### 插播（interstitial）

调度器维护：

```rust
enum SchedulerState {
    Idle,
    Armed { target_id, fire_at },                           // 等下一档
    Playing { current_id, started_at, end_at },             // 当前正片
    InterstitialPlaying { main_id, bumper_id, main_resume_offset_ms },
    Error { ... },
}
```

触发流程：

1. 调度器扫 `BumperSlot`，命中 → 状态切到 `InterstitialPlaying`
2. 把当前正片的"已播放时长"记作 `main_offset_ms`
3. 引擎执行切节目到插播项
4. 插播播完 → 状态切回 `Playing`，根据正片剩余时长计算 resume offset：
   ```
   remaining_ms = declared_duration_ms - main_offset_ms - bumper_duration_ms
   ```
   或者更精细：通过 `GetMediaInputStatus` 拿当前 cursor 估算剩余
5. 媒体源切回原节目 + `seek_to(max(0, main_offset_ms))`

### 崩溃恢复

引擎启动时：

1. 读本地 `config.json`：找到上次"已落盘"标记
2. 读 `playlist.json`：完整节目表
3. `now()` 落在已播节目区间 → 跳过；从下一条开始
4. 节目表无变化项 → 继续

调度器每个状态变更都落盘（debounce 5 秒），崩溃后从最新一份继续。

### OBS 重启

OBS 退出时：

- C 薄壳被卸载
- C 薄壳把 Rust 引擎子进程一起终结（JobObject 关闭 → 杀进程）

OBS 重启后：

- 用户重新打开 Broadcast Scheduler Control 源
- 引擎子进程被重新拉起
- 重新加载节目表，状态从中断点续接

## 编译

### Rust 引擎

```bash
cargo build --release --manifest-path engine/Cargo.toml
# 产物：engine/target/release/tv-obsbroadcast-scheduler(.exe)
```

### C 薄壳

需要 OBS Studio 28+ 源码提供 `libobs/` 头文件。打包脚本会在 CI 中自动从
`https://github.com/obsproject/obs-studio` 拉取对应版本的源代码。

```bash
# Windows
cmake -S plugin -B plugin/build -G "Visual Studio 17 2022" -A x64 \
  -DLIBOBS_INCLUDE_DIR=C:/path/to/obs-studio/libobs \
  -DOBS_IMPORT_LIB=C:/path/to/obs.lib
cmake --build plugin/build --config Release

# Linux
cmake -S plugin -B plugin/build \
  -DLIBOBS_INCLUDE_DIR=~/obs-studio/libobs \
  -DOBS_STUB_LIB=~/libobs-stub/libobs.so
cmake --build plugin/build --config Release

# macOS
cmake -S plugin -B plugin/build \
  -DLIBOBS_INCLUDE_DIR=~/obs-studio/libobs
cmake --build plugin/build --config Release
```

## 安全性

- 引擎只监听 `127.0.0.1`，不暴露到 LAN
- obs-websocket 凭据只在本地落盘（`config.json`），不上传任何服务
- bootstrap 接口鉴权用一次性 shared secret（C 薄壳启动时生成 / 写入 source settings，引擎首次收到 bootstrap 后丢弃）

## 已知限制

- 单源切换（用户明确要求"一个媒体源"）；架构预留多源扩展点
- 4K 内容需要 OBS 启用硬件解码（ffmpeg_source 默认 `hw_decode=true`，4K HEVC 可能仍卡顿）
- 不支持 HDR（用户明确不考虑）
- 不支持 macOS Intel（社区字幕插件也不支持）
