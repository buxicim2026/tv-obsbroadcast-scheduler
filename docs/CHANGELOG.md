# 更新日志

## [0.1.0] - 进行中
### 已完成
- 项目骨架初始化（workspace Cargo / CMake / Rust 引擎 / C 薄壳分层）
- 文档：用户文档 + 架构总览

### 进行中
- Rust 引擎核心：obs-websocket 5 客户端 + 调度主循环 + 节目表 CRUD + axum HTTP/WS
- C 薄壳插件：source 注册 + Qt 属性面板 + Dock 入口 + 引擎子进程管理

### 待办
- 插播调度 + 媒体时长探测 + 状态机 + 崩溃恢复
- admin/ 高阶级编辑网页
- overlay/ 播出状态浮窗
- 三平台打包脚本 + CI 自动发布
