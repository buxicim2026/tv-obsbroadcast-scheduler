<div align="center">

📺 **B站：[不息传播](https://space.bilibili.com/385015308)** ｜ 💬 **微信公众号：不息传播**

# 电视塔（tv-obsbroadcast-scheduler）

**在你的 OBS 里，播你自己的电视台。**

一款面向电视迷爱好者的 OBS Studio 自动播出插件。排好节目单，到点自动播，中间还能插广告，整点还会报时——就像真正的电视台一样。

[![GitHub release](https://img.shields.io/github/v/release/buxicim2026/tv-obsbroadcast-scheduler?style=flat-square)](https://github.com/buxicim2026/tv-obsbroadcast-scheduler/releases)
[![GitHub stars](https://img.shields.io/github/stars/buxicim2026/tv-obsbroadcast-scheduler?style=social)](https://github.com/buxicim2026/tv-obsbroadcast-scheduler)
[![Sponsor](https://img.shields.io/static/v1?label=Sponsor&message=%E2%9D%A4&logo=GitHub&color=%23fe8e86)](https://github.com/sponsors/buxicim2026)

</div>

---

## 目录

- [项目介绍](#项目介绍)
- [主要特点](#主要特点)
- [适合谁用](#适合谁用)
- [安装（3 步）](#安装3-步)
- [5 分钟上手](#5-分钟上手)
- [播出浮窗与报时器](#播出浮窗与报时器)
- [常见问题](#常见问题)
- [支持项目](#支持项目)
- [联系与反馈](#联系与反馈)
- [许可证](#许可证)

---

## 项目介绍

**电视塔（tv-obsbroadcast-scheduler）** 是一个专注于 OBS Studio 的电视台式自动播出插件。

它的目标很简单：**让你在 OBS 里搭一个属于自己的电视台。**

你只需要在 OBS 里放一个普通的**媒体源**，剩下的交给它——把一堆视频排成节目单，像电视台一样到点自动硬切、播完自动接下一条，中间还能插播广告。

后台是一个本地 Rust 引擎 + **admin 网页节目单编辑器**：你只在网页里选文件、排顺序，到点 OBS 就自动播，不需要手动拖时间轴，也不用碰 OBS 的播放按钮。

项目名称：

- 英文名：`tv-obsbroadcast-scheduler`
- 中文名：**电视塔**

一句话介绍：

> **电视塔，让你的 OBS 像电视台那样自己会播节目。**

---

## 主要特点

### 📺 顺序连续播出

一个媒体源按节目单从上到下播放，前一条播完立刻切下一条（硬切，无转场）。

### ⏱ 自动识别时长

导入视频时用浏览器本地解码读出真实时长，不用手填。

### 🎬 6 种节目类型

正片 / 公益广告 / 频道ID / 节目预告 / 宣传片 / 插播内容。

### 📅 本机时间节目单

设定开播时间后自动排出每条的播出时间；启用自动播出只负责按这张时间表开播，随时可提前排期。

### 🟢 播出状态一目了然

每条节目显示 等待中 / 播出中 / 已完成 / 异常（文件丢失会标红）。

### 🖱 拖拽排序 + 批量删除

鼠标直接拖动行首的 ⠿ 调整播出顺序，勾选后批量删除。

### 🔄 人工干预自动顺排

暂停会把后面节目整体顺延、跳档会把后面节目紧凑重排，拖拽后自动衔接——不会留下黑屏空档。

### 🕐 电视台报时器

整点 / 半点在画面上浮出一分钟报时（精确到秒），字体、字号、背景透明度、显示位置可调，随时开关。

### 🌐 国家授时中心授时

通过 NTP 与授时中心对时，报时器按授时时间显示（本机时间不准也不影响）。

### 💾 崩溃恢复

状态落盘，OBS 或引擎重启后从当前时间点接着排。

### 🖥 三平台

Windows 10+ / Linux（x64、arm64）/ macOS 13+ Apple Silicon。

### 🔐 凭据本地保存

OBS 密码、节目单都只存在本机的 `config.json`。

---

## 适合谁用

电视塔适合以下使用场景：

- 电视迷想在自己的 OBS 里模拟电视台播出；
- 长时间挂机直播，按节目单自动轮播；
- 需要整点报时、插播广告的仿电视直播；
- 活动、展会、店铺的循环展示；
- 任何你想“排好节目单，到点自动播”的场景。

简单来说：

> 只要你想在 OBS 里像电视台一样播节目，电视塔就能帮到你。

---

## 安装（3 步）

> 前端是 **OBS Lua 脚本**（不是二进制 DLL），由 OBS 自己加载执行，因此不存在“DLL 导出符号 / libobs ABI 不匹配 / 缺 VC++ 运行库”这类加载失败问题。

**1. 下载**

从 [Releases](https://github.com/buxicim2026/tv-obsbroadcast-scheduler/releases) 取对应平台的文件：

- `tv-obsbroadcast-scheduler-windows-x64.zip`
- `tv-obsbroadcast-scheduler-linux-x64.tar.gz`
- `tv-obsbroadcast-scheduler-macos-arm64.tar.gz`

**2. 解压，保持这个结构**（`engine` 必须和 lua 在同一层）：
tv-obsbroadcast-scheduler
├── tv_obsbroadcast_scheduler.lua ← 在 OBS 里加载这个文件
└── engine
└── tv-obsbroadcast-scheduler.exe ← Rust 引擎，由脚本自动拉起（不会弹黑窗）

**3. 加载脚本**

OBS 菜单 **工具 → 脚本 → 右下角「＋」→ 选择 `tv_obsbroadcast_scheduler.lua`**。加载后脚本会自动拉起引擎（无窗口、后台运行）。OBS 日志里出现 `[TVBS] settings pushed` 即表示引擎已接上凭据。

---

## 5 分钟上手

### 第 1 步：在场景里放一个「媒体源」

场景内 **＋ → 添加来源 → 媒体源（Media Source）**，名字随意（例如 `main_media`），**不要勾“本地文件”以外的远程串流**。

> ⚠️ 请务必用 **媒体源（ffmpeg_source）**，不要用 VLC 视频源——引擎通过 obs-websocket 的媒体控制接口驱动它。

### 第 2 步：开启 OBS WebSocket

**工具 → WebSocket 服务器设置 → 启用 WebSocket 服务器**，记下端口（默认 `4455`）和密码。

### 第 3 步：在脚本面板填凭据

加载脚本后，在脚本面板里填：

| 项 | 值 |
|---|---|
| OBS WebSocket Host | `127.0.0.1` |
| OBS WebSocket Port | `4455` |
| Password | 第 2 步的密码 |
| **Media Source Name** | 第 1 步那个媒体源的名字（如 `main_media`） |

点 **Test Connection**：成功 → OBS 日志出现 `Test Connection OK`；失败 → 日志会写清 HTTP 状态码。

### 第 4 步：编排节目单

点 **Open Admin in Browser**，或浏览器打开 `http://127.0.0.1:8789/admin`：

1. 进「节目清单」页
2. 点「**选择文件（可多选，直接导入）**」，一次选中今天要播的所有视频 → **选完即导入**（文件会传到引擎自己的 `media` 目录，名称和时长自动填好，不用填任何路径）
3. 不想上传（文件已在本机、体积很大）时，用「**从本机已有文件选择**」直接引用
4. 表格里给每条选「节目类型」（默认正片），用 ↑↓ 排顺序
5. 需要定点开播：设定「开播时间（本机时间）」→ 点「⏱ 按开播时间重排」

### 第 5 步：开始自动播出

回「主控台」点 **启用自动播出**：

- 它只是**启用**：节目按节目表里已经设定好的时间播，**不会**从你点击的那一刻重排
- 没到点的节目会等着，到点自动开播并逐条切下去
- 若当前已在某一档的时间窗口内，会立刻接管播放这一档
- 若所有节目时间都已过去，界面会提示你去重排（不会默默什么都不做）

---

## 播出浮窗与报时器

### 播出浮窗（Overlay）

地址：`http://127.0.0.1:8789/overlay`

在 OBS 里加一个**浏览器源**指向它（建议 1920×120，放在画面底部），会显示：ON AIR 灯、当前节目名、下一档、剩余时间。

### 报时器（Clock）

地址：`http://127.0.0.1:8789/clock`

像电视台那样在**整点与半点**浮出一分钟报时。在 OBS 场景里加一个**浏览器源**，地址填上面的 `/clock`（建议 960×240），到 admin 的**系统设置 → 电视台报时器** 里打开开关，可调字体、字号、底板样式、背景透明度、显示时长和位置。

---

## 常见问题

**Q：完全不播放？**
A：① 目标源填错 → 设置页从**下拉**里选真实来源；② 用的不是媒体源（VLC 源不支持）→ 换成媒体源；③ 文件路径不对 → 该条会显示“异常”，改用「浏览本机文件」重新添加。

**Q：节目条显示“异常”？**
A：文件被移动/删除，或者浏览器选文件时“媒体所在目录”没填对。用「浏览本机文件」添加可避免。

**Q：OBS 一启动就自己开播？**
A：不应该发生：引擎每次启动都会先解除自动播出，保持待命，等你在面板里点「启用自动播出」。

**Q：OBS 一直“未连接”？**
A：工具 → WebSocket 服务器设置是否启用；端口/密码是否与脚本面板一致；引擎是否在运行。

**Q：新增节目提示“没有通过授权”？**
A：在 OBS 脚本面板点一次 **Test Connection**（引擎会把授权令牌交给脚本）。

**Q：时间到了没切？**
A：检查是否点了「启用自动播出」；节目单的播出时间是否还在未来。

> 日志位置：OBS 菜单 **帮助 → 日志文件 → 查看当前日志**，筛选 `[TVBS]`。

---

## 支持项目

如果这个项目对你有帮助，欢迎通过 GitHub Sponsors 支持我继续维护：

[![Sponsor](https://img.shields.io/static/v1?label=Sponsor&message=%E2%9D%A4&logo=GitHub&color=%23fe8e86)](https://github.com/sponsors/buxicim2026)

你也可以在仓库页面点击右上角的 **Sponsor** 按钮进行捐赠。

> 提示：仓库右上角 Sponsor 按钮需要先在 GitHub 仓库的 **Settings → Sponsorships** 中启用，并确保 `.github/FUNDING.yml` 包含：
>
> ```yaml
> github: buxicim2026
> ```

---

## 联系与反馈

- B站：[不息传播](https://space.bilibili.com/385015308)
- 微信公众号：不息传播
- GitHub Issues：[提交问题](https://github.com/buxicim2026/tv-obsbroadcast-scheduler/issues)

---

## 许可证

本项目采用 MIT 许可证。详见 [LICENSE](LICENSE) 文件。
