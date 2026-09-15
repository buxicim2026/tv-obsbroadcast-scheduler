--[[
tv_obsbroadcast_scheduler.lua — OBS Studio script frontend.

前端用 OBS 的官方 Lua 脚本 API（而不是原生二进制插件）实现：脚本由 OBS
自身加载执行，不存在 DLL 导出符号、libobs ABI / 结构体布局匹配、VC++
运行库等一类问题，因此不会出现"编译通过但 OBS 加载不了"的情况。

职责：
  * 提供脚本属性面板填写 obs-websocket 凭据 / 目标媒体源 / 调度开关
  * 拉起并监控 Rust 引擎子进程，把配置通过 HTTP 推给引擎
真正的调度逻辑仍在 Rust 引擎里（通过 obs-websocket 改媒体源）。

安装：
  1. 本文件旁放好 engine/ 目录（内含 Rust 引擎二进制）
  2. OBS 菜单：工具 -> 脚本 -> "+" -> 选择本文件
  3. 在脚本属性里点「Open Admin in Browser」，用网页面板操作。
     不需要往场景里添加任何来源 —— 视频由你指定的媒体源播放。
--]]

local obs = obslua

local ENGINE_HOST  = "127.0.0.1"
local ENGINE_PORT  = 8789
local LOG_TAG      = "[TVBS] "
local DEFAULT_WS_PORT = 4455

local IS_WIN = package.config:sub(1, 1) == "\\"
local IS_MAC = false
if not IS_WIN then
  local h = io.popen("uname -s 2>/dev/null")
  if h ~= nil then
    local name = h:read("*l")
    h:close()
    IS_MAC = (name == "Darwin")
  end
end

------------------------------------------------------------------ 路径 ---

local function script_dir()
  local src = debug.getinfo(1, "S").source or ""
  if src:sub(1, 1) == "@" then src = src:sub(2) end
  local dir = src:match("^(.*[/\\])")
  if dir == nil or dir == "" then dir = "./" end
  return dir
end

local function engine_path()
  if IS_WIN then
    return script_dir() .. "engine\\tv-obsbroadcast-scheduler.exe"
  end
  return script_dir() .. "engine/tv-obsbroadcast-scheduler"
end

local function q(s)
  return '"' .. tostring(s) .. '"'
end

local function file_exists(p)
  local f = io.open(p, "rb")
  if f ~= nil then f:close(); return true end
  return false
end

-------------------------------------------------------------- 文件桥 ----
-- OBS 的 Lua 没有进程 / 网络接口，任何 os.execute / io.popen 在 Windows 上
-- 都要经由 cmd.exe —— 而 OBS 自己没有控制台，于是每调一次就闪一个黑框：
-- 启动时探测 healthz 就要调十几次，退出时再来一次。
--
-- 所以日常工作改成**纯文件读写**（不起任何子进程）：
--   脚本  --写-->  bridge.json  --读-->  引擎
--   引擎  --写-->  status.json  --读-->  脚本
-- 唯一还会起进程的地方是"引擎确实没在运行、需要拉起它"，而且只在那时。

local SEP = IS_WIN and "\\" or "/"
local BRIDGE_NAME = "bridge.json"
local STATUS_NAME = "status.json"

-- 目录统一以分隔符结尾，拼接文件名时才不会把路径和文件名粘在一起。
local function norm_dir(d)
  if d == nil or d == "" then return d end
  if d:sub(-1) ~= SEP then d = d .. SEP end
  return d
end

-- 引擎可能在便携模式（配置放在脚本目录下）或用户模式（%APPDATA% / ~/.config），
-- 我们两个位置都写、都读，取最新的那份。
local function bridge_dirs()
  local dirs = { norm_dir(script_dir()) }
  if IS_WIN then
    local appdata = os.getenv("APPDATA")
    if appdata then
      dirs[#dirs + 1] = norm_dir(appdata .. "\\tv-obsbroadcast-scheduler")
    end
  elseif IS_MAC then
    local home = os.getenv("HOME")
    if home then
      dirs[#dirs + 1] = norm_dir(
        home .. "/Library/Application Support/tv-obsbroadcast-scheduler")
    end
  else
    local base = os.getenv("XDG_CONFIG_HOME")
    if (base == nil or base == "") and os.getenv("HOME") then
      base = os.getenv("HOME") .. "/.config"
    end
    if base then
      dirs[#dirs + 1] = norm_dir(base .. "/tv-obsbroadcast-scheduler")
    end
  end
  return dirs
end

local function write_file(path, data)
  local f = io.open(path, "wb")
  if f == nil then return false end
  f:write(data)
  f:close()
  return true
end

local function read_file(path)
  local f = io.open(path, "rb")
  if f == nil then return nil end
  local s = f:read("*a")
  f:close()
  return s
end

-- 引擎每 500ms 刷新一次 status.json；读到 6 秒内的时间戳即认为它活着。
local function engine_alive()
  for _, dir in ipairs(bridge_dirs()) do
    local s = read_file(dir .. STATUS_NAME)
    if s then
      local ts = tonumber(string.match(s, '"ts"%s*:%s*(%d+)'))
      if ts and (os.time() * 1000 - ts) < 6000 then return true end
    end
  end
  return false
end

local function status_text()
  for _, dir in ipairs(bridge_dirs()) do
    local s = read_file(dir .. STATUS_NAME)
    if s then return s end
  end
  return nil
end

---------------------------------------------------------------- 引擎进程 ---

local function spawn_engine()
  local exe = engine_path()
  if not file_exists(exe) then
    obs.blog(obs.LOG_WARNING, LOG_TAG .. "engine binary not found: " .. exe)
    return false
  end
  if IS_WIN then
    -- 这是启动路径上唯一还会创建 cmd.exe 的地方（Lua 只能这样起进程），
    -- 且只在引擎确实没跑时发生一次。/B 表示不新建窗口；引擎本身是
    -- windows-subsystem 程序，不会有自己的控制台。
    os.execute("start " .. q("") .. " /B " .. q(exe))
  else
    os.execute(q(exe) .. " >/dev/null 2>&1 &")
  end
  obs.blog(obs.LOG_INFO, LOG_TAG .. "engine spawned: " .. exe)
  return true
end

-- 先看看引擎是不是已经在跑（读文件，不起进程）；只有没在跑才拉起它。
local function ensure_engine()
  if engine_alive() then return true end
  spawn_engine()
  -- 轮询 status.json 而不是 HTTP：不起任何子进程。引擎正常时不到一秒就
  -- 会写出 status.json；只有它起不来才会等满。
  local deadline = os.clock() + 4
  while os.clock() < deadline do
    if engine_alive() then return true end
    local until_ = os.clock() + 0.2
    while os.clock() < until_ do end
  end
  return engine_alive()
end

------------------------------------------------------------------ 配置 ---

local script_settings = nil

local function json_escape(s)
  s = tostring(s or "")
  s = s:gsub("\\", "\\\\"):gsub('"', '\\"')
  return s
end

local function ensure_token(settings)
  local token = obs.obs_data_get_string(settings, "bootstrap_token")
  if token == nil or token == "" then
    math.randomseed(os.time())
    local chars = "0123456789abcdef"
    local parts = {}
    for i = 1, 32 do
      parts[i] = chars:sub(math.random(1, 16), math.random(1, 16))
    end
    token = table.concat(parts)
    obs.obs_data_set_string(settings, "bootstrap_token", token)
  end
  return token
end

-- 把当前 settings 交给引擎：写一份 bridge.json，引擎自己来取。
-- apply_enabled=false：只送凭据，**不要**恢复上次的自动播出状态。
-- OBS 启动（脚本加载）后应处于待命，由操作员在面板/网页里手动开启。
-- open_admin=true：顺便请引擎帮我们打开浏览器（自己起进程会闪命令行窗口）。
local function build_payload(settings, apply_enabled, open_admin, force)
  local token = ensure_token(settings)
  local enabled = (apply_enabled ~= false)
    and obs.obs_data_get_bool(settings, "scheduler_enabled")

  -- Blank fields are written as JSON `null` so the engine skips them. Writing
  -- "" instead made an empty box in the script properties overwrite whatever
  -- the operator had saved through the web panel — including the target media
  -- source, which then came back as "source does not exist" after an OBS restart.
  local function opt_str(v)
    if v == nil or v == "" then return "null" end
    return '"' .. json_escape(v) .. '"'
  end
  local port = obs.obs_data_get_int(settings, "ws_port")
  local port_json = port > 0 and tostring(port) or "null"

  return string.format(
    '{\n  "bootstrap_token": "%s",\n  "host": %s,\n  "port": %s,\n'
      .. '  "password": "%s",\n  "tls": %s,\n  "target_input": %s,\n'
      .. '  "enabled": %s,\n  "open_admin": %s,\n  "force": %s,\n  "ts": %d\n}\n',
    json_escape(token),
    opt_str(obs.obs_data_get_string(settings, "ws_host")),
    port_json,
    json_escape(obs.obs_data_get_string(settings, "ws_password")),
    obs.obs_data_get_bool(settings, "ws_tls") and "true" or "false",
    opt_str(obs.obs_data_get_string(settings, "target_input")),
    enabled and "true" or "false",
    open_admin and "true" or "false",
    force and "true" or "false",
    os.time() * 1000
  )
end

local function push_settings(settings, apply_enabled, open_admin, force)
  if settings == nil then return false end
  local payload = build_payload(settings, apply_enabled, open_admin == true, force == true)
  local written = 0
  for _, dir in ipairs(bridge_dirs()) do
    if write_file(dir .. BRIDGE_NAME, payload) then
      written = written + 1
    end
  end
  if written == 0 then
    obs.blog(obs.LOG_WARNING,
      LOG_TAG .. "could not write bridge.json next to the script — settings not delivered")
    return false
  end
  return true
end

-------------------------------------------------------------------- UI ---

local function add_fields(props)
  obs.obs_properties_add_text(props, "ws_host", "OBS WebSocket Host", obs.OBS_TEXT_DEFAULT)
  obs.obs_properties_add_int(props, "ws_port", "OBS WebSocket Port", 1, 65535, 1)
  obs.obs_properties_add_text(props, "ws_password", "Password", obs.OBS_TEXT_PASSWORD)
  obs.obs_properties_add_bool(props, "ws_tls", "Use TLS (wss://)")
  obs.obs_properties_add_text(props, "target_input", "Media Source Name", obs.OBS_TEXT_DEFAULT)
  obs.obs_properties_add_bool(props, "scheduler_enabled", "Enable scheduler")
  obs.obs_properties_add_button(props, "test_connection", "Test Connection", on_test_clicked)
  obs.obs_properties_add_button(props, "open_admin", "Open Admin in Browser", on_open_admin_clicked)
  return props
end

function on_test_clicked(props, property)
  if not ensure_engine() then
    obs.blog(obs.LOG_WARNING, LOG_TAG .. "engine could not be started")
    return true
  end
  -- force = true：操作员按了 Test Connection，就是要以脚本属性为准，
  -- 平时引擎以网页面板的设置为主（否则两边会互相覆盖）。
  if script_settings ~= nil then push_settings(script_settings, false, false, true) end
  local s = status_text()
  if s == nil then
    obs.blog(obs.LOG_WARNING,
      LOG_TAG .. "engine has not written status.json yet — try again in a second")
    return true
  end
  local version = string.match(s, '"engine_version"%s*:%s*"([^"]*)"') or "?"
  local connected = string.match(s, '"obs_connected"%s*:%s*(%a+)')
  local state = string.match(s, '"scheduler_state"%s*:%s*"([^"]*)"') or "?"
  obs.blog(obs.LOG_INFO, string.format(
    "%sTest Connection: engine v%s, obs_connected=%s, state=%s — admin: http://%s:%d/admin",
    LOG_TAG, version, tostring(connected), state, ENGINE_HOST, ENGINE_PORT))
  return true
end

function on_open_admin_clicked(props, property)
  ensure_engine()
  -- 请引擎帮我们打开浏览器：脚本自己 os.execute 会闪一个命令行窗口。
  if script_settings ~= nil then
    push_settings(script_settings, false, true)
  end
  obs.blog(obs.LOG_INFO, LOG_TAG .. "asked the engine to open http://"
    .. ENGINE_HOST .. ":" .. ENGINE_PORT .. "/admin")
  return true
end

-------------------------------------------------------- 脚本生命周期 ---

-- 以前这里还注册过一个隐藏的 input source（"Broadcast Scheduler Control"），
-- 用户必须把它加进场景，否则脚本不生效。现在调度完全由 Rust 引擎通过
-- obs-websocket 驱动用户指定的媒体源，不再需要任何占位源 —— 注册一个不产出
-- 画面的 source 只会污染『添加来源』列表、让用户找不到该选什么。

function script_description()
  return "TV Broadcast Scheduler —— 让一个媒体源按节目表毫秒级自动硬切。\n\n"
    .. "填写 obs-websocket 凭据与目标媒体源即可，不需要往场景里添加来源。\n"
    .. "面板：Admin UI http://" .. ENGINE_HOST .. ":" .. ENGINE_PORT .. "/admin"
end

function script_properties()
  local props = obs.obs_properties_create()
  obs.obs_properties_add_text(props, "tvbs_status",
    "Admin UI: http://" .. ENGINE_HOST .. ":" .. ENGINE_PORT .. "/admin",
    obs.OBS_TEXT_INFO)
  return add_fields(props)
end

function script_defaults(settings)
  obs.obs_data_set_default_string(settings, "ws_host", "127.0.0.1")
  obs.obs_data_set_default_int(settings, "ws_port", DEFAULT_WS_PORT)
  obs.obs_data_set_default_string(settings, "ws_password", "")
  obs.obs_data_set_default_bool(settings, "ws_tls", false)
  obs.obs_data_set_default_string(settings, "target_input", "main_media")
  obs.obs_data_set_default_bool(settings, "scheduler_enabled", false)
  obs.obs_data_set_default_string(settings, "bootstrap_token", "")
end

function script_load(settings)
  script_settings = settings
  ensure_engine()
  -- Push on load as well: otherwise the engine only ever sees the credentials
  -- (and the bootstrap token the admin needs for writes) when the user happens
  -- to open the script panel and press Test Connection.
  -- 第二参数 false = 只送凭据，不自动开播（保持待命）。
  if settings ~= nil then
    push_settings(settings, false)
  end
end

function script_update(settings)
  script_settings = settings
  push_settings(settings)
end

function script_save(settings)
  script_settings = settings
  push_settings(settings)
end
