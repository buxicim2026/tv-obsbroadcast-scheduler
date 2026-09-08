--[[
tv_obsbroadcast_scheduler.lua — OBS Studio script frontend.

前端用 OBS 的官方 Lua 脚本 API（而不是原生二进制插件）实现：脚本由 OBS
自身加载执行，不存在 DLL 导出符号、libobs ABI / 结构体布局匹配、VC++
运行库等一类问题，因此不会出现"编译通过但 OBS 加载不了"的情况。

职责：
  * 注册一个控制用 source（可在『添加来源』里看到）
  * 提供属性面板填写 obs-websocket 凭据 / 目标媒体源 / 调度开关
  * 拉起并监控 Rust 引擎子进程，把配置通过 HTTP 推给引擎
真正的调度逻辑仍在 Rust 引擎里（通过 obs-websocket 改媒体源）。

安装：
  1. 本文件旁放好 engine/ 目录（内含 Rust 引擎二进制）
  2. OBS 菜单：工具 -> 脚本 -> "+" -> 选择本文件
  3. 场景里：添加来源 -> Broadcast Scheduler Control
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

------------------------------------------------------------------ HTTP ---
-- 引擎暴露本地 HTTP API。用 curl（Win10+ / macOS / 常见 Linux 都自带）
-- 完成请求，避免依赖 LuaSocket。

local function curl_bin()
  return IS_WIN and "curl.exe" or "curl"
end

local TMP_FILE = nil
local function tmp_file()
  if TMP_FILE == nil then TMP_FILE = script_dir() .. "_tvbs_body.json" end
  return TMP_FILE
end

local function http(method, api_path, body, bootstrap_token)
  local cmd = curl_bin() .. " -s -m 4 -X " .. method
  if body ~= nil then
    cmd = cmd .. ' -H "Content-Type: application/json"'
  end
  if bootstrap_token ~= nil then
    cmd = cmd .. ' -H "X-Bootstrap-Token: ' .. bootstrap_token .. '"'
  end
  if body ~= nil then
    local tf = tmp_file()
    local f = io.open(tf, "w")
    if f ~= nil then f:write(body); f:close() end
    cmd = cmd .. ' -d "@' .. tf .. '"'
  end
  cmd = cmd
    .. ' -w "\\n%{http_code}" http://'
    .. ENGINE_HOST .. ":" .. ENGINE_PORT .. api_path
  if not IS_WIN then cmd = cmd .. " 2>/dev/null" end

  local handle = io.popen(cmd)
  local out = ""
  if handle ~= nil then
    out = handle:read("*a") or ""
    handle:close()
  end
  local code = string.match(out, "(%d%d%d)%s*$")
  return out, code
end

---------------------------------------------------------------- 引擎进程 ---

local function engine_alive()
  local _, code = http("GET", "/healthz")
  return code == "200"
end

local function spawn_engine()
  local exe = engine_path()
  if not file_exists(exe) then
    obs.blog(obs.LOG_WARNING, LOG_TAG .. "engine binary not found: " .. exe)
    return false
  end
  if IS_WIN then
    os.execute("start " .. q("") .. " /MIN " .. q(exe))
  else
    os.execute(q(exe) .. " >/dev/null 2>&1 &")
  end
  obs.blog(obs.LOG_INFO, LOG_TAG .. "engine spawned: " .. exe)
  return true
end

local function ensure_engine()
  if engine_alive() then
    return true
  end
  return spawn_engine()
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

-- 把当前 settings 推给引擎（bootstrap + 调度开关）。
local function push_settings(settings)
  if settings == nil then return false end
  ensure_engine()

  local token = ensure_token(settings)
  local payload = string.format(
    '{"bootstrap_token":"%s","host":"%s","port":%d,"password":"%s",'
      .. '"tls":%s,"target_input":"%s"}',
    json_escape(token),
    json_escape(obs.obs_data_get_string(settings, "ws_host")),
    obs.obs_data_get_int(settings, "ws_port"),
    json_escape(obs.obs_data_get_string(settings, "ws_password")),
    obs.obs_data_get_bool(settings, "ws_tls") and "true" or "false",
    json_escape(obs.obs_data_get_string(settings, "target_input"))
  )

  local _, code = http("POST", "/api/bootstrap", payload)
  if code ~= "200" then
    obs.blog(obs.LOG_WARNING,
      LOG_TAG .. "bootstrap failed (HTTP " .. tostring(code)
        .. ") — engine running? see http://" .. ENGINE_HOST .. ":" .. ENGINE_PORT .. "/healthz")
    return false
  end

  local enabled = obs.obs_data_get_bool(settings, "scheduler_enabled")
  http("POST", "/api/scheduler/enable",
    string.format('{"enabled":%s}', enabled and "true" or "false"), token)
  obs.blog(obs.LOG_INFO, LOG_TAG .. "settings pushed (scheduler_enabled="
    .. tostring(enabled) .. ")")
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
  local body, code = http("GET", "/healthz")
  obs.blog(obs.LOG_INFO, LOG_TAG .. "healthz -> HTTP " .. tostring(code) .. " " .. tostring(body))
  if code == "200" and script_settings ~= nil then
    push_settings(script_settings)
  end
  return true
end

function on_open_admin_clicked(props, property)
  ensure_engine()
  local url = "http://" .. ENGINE_HOST .. ":" .. ENGINE_PORT .. "/admin"
  if IS_WIN then
    os.execute("start " .. q("") .. " " .. q(url))
  elseif IS_MAC then
    os.execute("open " .. q(url))
  else
    os.execute("xdg-open " .. q(url) .. " >/dev/null 2>&1 &")
  end
  obs.blog(obs.LOG_INFO, LOG_TAG .. "opened " .. url)
  return true
end

------------------------------------------------------------ source 类型 ---

local source_info = {
  id = "tvbs_scheduler_control",
  type = obs.OBS_SOURCE_TYPE_INPUT,
  output_flags = obs.OBS_SOURCE_VIDEO,
}

source_info.get_name = function()
  return "Broadcast Scheduler Control"
end

source_info.create = function(settings, source)
  return {}
end

source_info.destroy = function(data)
end

source_info.get_width = function(data)
  return 1
end

source_info.get_height = function(data)
  return 1
end

-- 该源不产出画面，视频由用户指定的 Media Source 播放。
source_info.video_render = function(data, effect)
end

source_info.get_defaults = function(settings)
  obs.obs_data_set_default_string(settings, "ws_host", "127.0.0.1")
  obs.obs_data_set_default_int(settings, "ws_port", DEFAULT_WS_PORT)
  obs.obs_data_set_default_string(settings, "ws_password", "")
  obs.obs_data_set_default_bool(settings, "ws_tls", false)
  obs.obs_data_set_default_string(settings, "target_input", "main_media")
  obs.obs_data_set_default_bool(settings, "scheduler_enabled", false)
  obs.obs_data_set_default_string(settings, "bootstrap_token", "")
end

source_info.get_properties = function(data)
  local props = obs.obs_properties_create()
  obs.obs_properties_add_text(props, "tvbs_hint",
    "控制源：不输出画面。视频由下面填写的 Media Source 播放。",
    obs.OBS_TEXT_INFO)
  return add_fields(props)
end

source_info.update = function(data, settings)
  if script_settings == nil then script_settings = settings end
  push_settings(settings)
end

-------------------------------------------------------- 脚本生命周期 ---

function script_description()
  return "TV Broadcast Scheduler —— 让一个媒体源按节目表毫秒级自动硬切。\n\n"
    .. "填写 obs-websocket 凭据与目标媒体源后，在场景里『添加来源 → "
    .. "Broadcast Scheduler Control』即可。"
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
  obs.obs_register_source(source_info)
  ensure_engine()
end

function script_update(settings)
  script_settings = settings
  push_settings(settings)
end

function script_save(settings)
  script_settings = settings
  push_settings(settings)
end
