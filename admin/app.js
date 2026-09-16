// admin/app.js — frontend SPA for tv-obsbroadcast-scheduler.
//
// Vanilla ES module. Tabs (Dashboard / Playlist / Timeline / Settings) are
// switched via the top nav. A live WebSocket keeps the dashboard in sync
// with the engine; mutated settings go back via REST.

import { renderBumpers } from '/admin/bumpers.js';

const API = {
    status: '/api/status',
    playlist: '/api/playlist',
    playlistItem: '/api/playlist/item',
    enable: '/api/scheduler/enable',
    start: '/api/scheduler/start',
    pause: '/api/scheduler/pause',
    timeSync: '/api/time/sync',
    next: '/api/scheduler/next',
    reload: '/api/scheduler/reload',
    reorder: '/api/playlist/reorder',
    verify: '/api/playlist/verify',
    upload: '/api/playlist/upload',
    fsBrowse: '/api/fs/browse',
    fsStat: '/api/fs/stat',
    health: '/healthz',
};

// 节目类型 — 顺序即下拉里的顺序。播放顺序只由时间决定，与类型无关。
const PROGRAM_KINDS = [
    ['primary', '正片'],
    ['public_service', '公益广告'],
    ['channel_id', '频道ID'],
    ['preview', '节目预告'],
    ['promo', '宣传片'],
    ['interstitial', '插播内容'],
];
const PROGRAM_KIND_LABEL = Object.fromEntries(PROGRAM_KINDS);

/// 北京时间显示（播出时间一律按本机时间呈现）。
/// 本机时间显示。调度器判定节目窗口用的就是本机时钟，所以界面显示也一律用本机时间，
/// 不做任何时区换算 —— 否则机器时区不是东八区时，"看到的播出时间"和"引擎判定的
/// 时间"就对不上。调度器判定节目窗口用的就是本机时钟，所以界面显示也一律用
/// 本机时间，不做任何时区换算 —— 否则机器时区一旦不是东八区，"看到的播出时间"
/// 和"引擎判定的时间"就对不上，节目看起来就是不触发。
function fmtLocal(ms) {
    if (ms == null) return '—';
    try {
        return new Date(ms).toLocaleString('zh-CN', {
            hour12: false,
            year: 'numeric', month: '2-digit', day: '2-digit',
            hour: '2-digit', minute: '2-digit', second: '2-digit',
        });
    } catch (_) {
        return new Date(ms).toString();
    }
}

/// 把 <input type="datetime-local"> 的值按**本机时间**解析成 epoch ms。
function parseLocalInput(v) {
    if (!v) return null;
    const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(String(v));
    if (!m) return null;
    return new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5], 0, 0).getTime();
}

let ws = null;
let wsReconnectTimer = null;
let lastSnapshot = null;
/// 临时的主控台错误提示（比引擎的 last_error 优先级高，十几秒后自动让位）。
let dashErrorOverride = null;
let playlistCache = [];
let bumpersCache = [];

/* -------------------------------------------------------------------------- */
/* Boot                                                                      */
/* -------------------------------------------------------------------------- */

window.addEventListener('DOMContentLoaded', () => {
    // Each wiring step is isolated on purpose: if one of them throws, the
    // rest of the console must still work. Previously a single failure here
    // aborted the whole boot and left every button looking dead.
    const steps = [
        ['页签切换', setupTabs],
        ['名称浮窗', setupPlaylistTooltip],
        ['主控台按钮', setupDashboardActions],
        ['节目表', setupPlaylistAdd],
        ['设置', setupSettings],
    ];
    for (const [label, fn] of steps) {
        try {
            fn();
        } catch (e) {
            showGlobalError(`「${label}」初始化失败，该部分功能不可用`, e);
        }
    }
    refreshAll();
    startWs();
    tickClock();
    setInterval(tickClock, 1000);
});

// Friendly banner: the message is meant for a human, the stack goes to the
// browser console only (a wall of raw stack text on screen looks like a bug).
function showGlobalError(friendly, err) {
    console.error('[admin]', friendly, err);
    const el = document.getElementById('global-error');
    if (!el) return;
    el.textContent = `⚠️ ${friendly}。请把浏览器控制台（F12）里的报错发给我们。`;
    el.hidden = false;
}

function setupTabs() {
    const buttons = document.querySelectorAll('.nav-tab');
    buttons.forEach(btn => {
        btn.addEventListener('click', () => {
            const tab = btn.dataset.tab;
            document.querySelectorAll('.nav-tab')
                .forEach(b => b.classList.toggle('nav-tab-active', b === btn));
            document.querySelectorAll('[data-tab]')
                .forEach(panel => {
                    if (!panel.classList.contains('tab-panel')) return;
                    panel.classList.toggle('hidden', panel.dataset.tab !== tab);
                });
        });
    });

    // Cross-tab helpers.
    document.querySelectorAll('[data-jump-tab]').forEach(b => {
        b.addEventListener('click', () => {
            const tab = b.dataset.jumpTab;
            document.querySelector(`.nav-tab[data-tab="${tab}"]`)?.click();
        });
    });
}

/* -------------------------------------------------------------------------- */
/* REST                                                                      */
/* -------------------------------------------------------------------------- */

async function refreshAll() {
    try {
        const [status, playlist] = await Promise.all([
            fetch(API.status).then(r => r.json()),
            fetch(API.playlist).then(r => r.json()),
        ]);
        lastSnapshot = status;
        playlistCache = playlist.items || [];
        bumpersCache = playlist.bumpers || [];
        renderStatus(status);
        renderDashboard(status);
        renderConsole(status);
        renderSettingsForm();
        await verifyPlaylist();
        renderPlaylistRows();
        renderBumpers(bumpersCache);
        // Once the channel is really on air, a leftover "target source does not
        // exist" complaint is stale by definition — clear it instead of leaving
        // a fixed configuration looking broken.
        const schNow = status.scheduler || status.status || {};
        const statusEl = document.getElementById('playlist-status');
        if (statusEl && schNow.scheduler_running
            && /目标媒体源|不是媒体源/.test(statusEl.textContent || '')) {
            statusEl.textContent = '';
        }
    } catch (e) {
        log(`refreshAll failed: ${e}`);
    }
}

// Write endpoints are guarded by the engine (see schedule_api::require_token)
// and expect the bootstrap token, which the status snapshot carries.
function authHeaders() {
    const token =
        (lastSnapshot && lastSnapshot.bootstrap_token) ||
        window.__TVBS_TOKEN__ ||
        '';
    return {
        'Content-Type': 'application/json',
        'X-Bootstrap-Token': token,
    };
}

/// Refresh the cached snapshot (which carries the bootstrap token) and return
/// the fresh token. Cheap, and it is what recovers a stale/missing token.
async function reloadToken() {
    try {
        const s = await fetch(API.status).then(r => r.json());
        lastSnapshot = { ...(lastSnapshot || {}), ...s };
    } catch (_) {}
    return (lastSnapshot && lastSnapshot.bootstrap_token) || '';
}

/// One write attempt. `send` performs the actual fetch with the given token.
async function writeWithAuth(send) {
    let res = await send(authHeaders());
    if (res.status !== 401 && res.status !== 403) return res;
    // Token was stale/lost (e.g. a WS frame replaced the snapshot before the
    // engine started sending the token). Re-read it and retry exactly once.
    await reloadToken();
    res = await send(authHeaders());
    return res;
}

async function apiPost(url, body) {
    const res = await writeWithAuth((headers) => fetch(url, {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
    }));
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json().catch(() => ({}));
}

async function apiDelete(url, body) {
    const res = await writeWithAuth((headers) => fetch(url, {
        method: 'DELETE',
        headers,
        body: JSON.stringify(body),
    }));
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json().catch(() => ({}));
}

/* -------------------------------------------------------------------------- */
/* Dashboard                                                                 */
/* -------------------------------------------------------------------------- */

function renderDashboard(s) {
    // Same compatibility shim as renderStatus (old key: `status`).
    const sch = s.scheduler || s.status || {};
    const onAir = sch.scheduler_state === 'Playing' ||
                  sch.scheduler_state === 'Interstitial';
    document.getElementById('dash-on-air-dot')?.classList.toggle('bg-red-500', onAir);
    document.getElementById('dash-on-air-dot')?.classList.toggle('bg-zinc-700', !onAir);
    document.getElementById('dash-on-air-dot')?.classList.toggle('pulse', onAir);
    document.getElementById('on-air-dot')?.classList.toggle('opacity-30', !onAir);
    document.getElementById('on-air-dot')?.classList.toggle('shadow-red-glow', onAir);
    document.getElementById('on-air-dot')?.classList.toggle('pulse', onAir);

    document.getElementById('dash-program-name').textContent =
        sch.current_program_name || '— 待机 —';
    document.getElementById('dash-program-meta').textContent =
        onAir ? `当前状态：${sch.scheduler_state}` : '等待调度器启动';

    document.getElementById('dash-remaining').textContent =
        formatRemaining(sch.current_remaining_ms);

    const btn = document.getElementById('btn-armed');
    btn.textContent = sch.scheduler_running ? '停用自动播出' : '启用自动播出';
    btn.classList.toggle('btn-primary', !sch.scheduler_running);
    btn.classList.toggle('btn-secondary', sch.scheduler_running);

    const playlistName = playlistCache[playlistCache.length - 1];
    if (playlistName) {
        // pick the closest future item
        const now = Date.now();
        const upcoming = playlistCache
            .filter(p => p.start_at_ms > now)
            .sort((a, b) => a.start_at_ms - b.start_at_ms)[0];
        if (upcoming) {
            document.getElementById('dash-next-name').textContent = upcoming.name;
            document.getElementById('dash-next-path').textContent = upcoming.file_path;
            document.getElementById('dash-next-kind').textContent =
                PROGRAM_KIND_LABEL[upcoming.kind] || upcoming.kind;
            document.getElementById('dash-next-start').textContent =
                new Date(upcoming.start_at_ms).toLocaleTimeString();
            document.getElementById('dash-next-duration').textContent =
                msToHMS(upcoming.declared_duration_ms);
        } else {
            clearUpcoming();
        }
    } else {
        clearUpcoming();
    }
}

/* ---------------------- 播控台（电视台风格主界面） ---------------------- */

function setText(id, v) {
    const el = document.getElementById(id);
    if (el) el.textContent = v;
}

function upcomingProgram() {
    const now = Date.now();
    return playlistCache
        .filter(p => (p.start_at_ms || 0) > now)
        .sort((a, b) => a.start_at_ms - b.start_at_ms)[0] || null;
}

function fmtCountdown(ms) {
    if (ms == null || ms < 0) return '00:00:00';
    const s = Math.floor(ms / 1000);
    return [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60]
        .map(n => String(n).padStart(2, '0'))
        .join(':');
}

function renderConsole(s) {
    const sch = (s && (s.scheduler || s.status)) || {};
    const running = !!sch.scheduler_running;

    // 左侧节目表
    const tbody = document.getElementById('ctl-tbody');
    if (tbody) {
        tbody.innerHTML = '';
        if (!playlistCache.length) {
            const tr = document.createElement('tr');
            tr.innerHTML = '<td colspan="6" class="ctl-empty">暂无节目 — 点左上角「📁 导入素材」导入视频</td>';
            tbody.appendChild(tr);
        } else {
            playlistCache.forEach((p, i) => {
                const [label] = programState(p, sch);
                const durMs = p.detected_duration_ms || p.declared_duration_ms || 0;
                const onAir = sch.current_program_id === p.id && running;
                const tr = document.createElement('tr');
                if (onAir) tr.className = 'ctl-row-onair';
                tr.innerHTML =
                    `<td class="ctl-num">${String(i + 1).padStart(3, '0')}</td>` +
                    `<td class="ctl-name" title="${escapeHtml(p.file_path)}">${escapeHtml(p.name)}</td>` +
                    `<td>${PROGRAM_KIND_LABEL[p.kind] || p.kind}</td>` +
                    `<td class="ctl-mono">${msToHMS(durMs)}</td>` +
                    `<td class="ctl-mono">${fmtLocal(p.start_at_ms)}</td>` +
                    `<td class="ctl-state">${label}</td>`;
                tbody.appendChild(tr);
            });
        }
    }

    const total = playlistCache.reduce(
        (a, p) => a + (p.detected_duration_ms || p.declared_duration_ms || 0), 0);

    setText('ctl-playlist-count', `${playlistCache.length} 条`);
    setText('sb-count', String(playlistCache.length));
    setText('ctl-info-total', msToHMS(total));
    setText('ctl-sb-total', msToHMS(total));
    const paused = !!sch.paused;
    setText('sb-state', !running ? '待机'
        : (paused ? '暂停' : (sch.scheduler_state === 'Interstitial' ? '插播中' : '播出中')));
    setText('ctl-onair-text', !running ? '待机' : (paused ? 'PAUSED' : 'ON AIR'));

    // 暂停键是双态的：播出中显示「暂停」，暂停中显示「继续」。
    const pauseBtn = document.getElementById('btn-pause');
    if (pauseBtn) {
        pauseBtn.textContent = paused ? '继续' : '暂停';
        pauseBtn.classList.toggle('on', paused);
    }
    setText('ctl-info-current', sch.current_program_name || '—');
    setText('sb-obs', sch.obs_connected ? '已连接' : '未连接');

    // 引擎最近一次失败直接摆在主控台上：用户点了「启用自动播出」没反应时，
    // 原因（OBS 未连接 / 目标源不对 / 节目时间都过去了）就在这里。
    const errEl = document.getElementById('ctl-error');
    if (errEl) {
        const override = dashErrorOverride && Date.now() < dashErrorOverride.until
            ? dashErrorOverride.msg
            : '';
        const err = override || sch.last_error || '';
        errEl.textContent = err;
        errEl.hidden = !err;
    }

    const badge = document.getElementById('ctl-onair-badge');
    if (badge) badge.classList.toggle('on', running);

    // 工具条上的播出按钮：待机=绿色「启用」，播出中=红色「停用」
    const armedBtn = document.getElementById('btn-armed');
    if (armedBtn) {
        armedBtn.classList.toggle('on', running);
        armedBtn.textContent = running ? '停用自动播出' : '启用自动播出';
    }
    const dot = document.getElementById('dash-on-air-dot');
    if (dot) {
        dot.classList.toggle('bg-red-500', running);
        dot.classList.toggle('bg-zinc-700', !running);
        dot.classList.toggle('pulse', running);
    }

    const up = upcomingProgram();
    setText('ctl-info-nextcd', up ? fmtCountdown(up.start_at_ms - Date.now()) : '--:--:--');
}

/// 每秒走一次：主时钟 + 备播倒计时 + 当前剩余时间本地递减（WS 会校正）。
function tickClock() {
    // 主时钟显示本机时间：和调度器用的是同一个时钟，一眼就能对上。
    setText('ctl-clock', new Date().toLocaleTimeString('zh-CN', { hour12: false }));
    const up = upcomingProgram();
    setText('ctl-info-nextcd', up ? fmtCountdown(up.start_at_ms - Date.now()) : '--:--:--');
    const sch = (lastSnapshot && (lastSnapshot.scheduler || lastSnapshot.status)) || {};
    if (sch.scheduler_running && typeof sch.current_remaining_ms === 'number'
        && sch.current_remaining_ms > 0) {
        sch.current_remaining_ms = Math.max(0, sch.current_remaining_ms - 1000);
        setText('dash-remaining', formatRemaining(sch.current_remaining_ms));
    }
}

function clearUpcoming() {
    document.getElementById('dash-next-name').textContent = '—';
    document.getElementById('dash-next-path').textContent = '—';
    document.getElementById('dash-next-kind').textContent = '—';
    document.getElementById('dash-next-start').textContent = '--:--:--';
    document.getElementById('dash-next-duration').textContent = '--:--';
}

function setupDashboardActions() {
    document.getElementById('btn-armed').addEventListener('click', toggleArmed);
    document.getElementById('btn-pause').addEventListener('click', async () => {
        const paused = !!(lastSnapshot && lastSnapshot.scheduler && lastSnapshot.scheduler.paused);
        const b = document.getElementById('btn-pause');
        if (b) b.disabled = true;
        try {
            await apiPost(API.pause, { paused: !paused });
            log(paused ? '已继续播出' : '已暂停播出（画面暂停，计时已冻结）');
            await refreshAll();
        } catch (e) {
            log(`暂停/继续失败：${e.message}`, 'warn');
        } finally {
            if (b) b.disabled = false;
        }
    });
    document.getElementById('btn-next').addEventListener('click', async () => {
        const b = document.getElementById('btn-next');
        if (b) { b.disabled = true; b.textContent = '切换中…'; }
        try {
            await apiPost(API.next, {});
            log('已跳到下一档');
            await refreshAll();
        } catch (e) {
            log(`跳档失败：${e.message}`, 'warn');
        } finally {
            if (b) { b.disabled = false; b.textContent = '下一档'; }
        }
    });
    document.getElementById('btn-restart').addEventListener('click', async () => {
        const b = document.getElementById('btn-restart');
        b.disabled = true;
        b.textContent = '刷新中…';
        await refreshAll();
        b.disabled = false;
        b.textContent = '重新载入';
    });
    const clearLog = document.getElementById('btn-clear-log');
    if (clearLog) {
        clearLog.addEventListener('click', () => {
            const l = document.getElementById('dash-log');
            if (l) l.innerHTML = '';
        });
    }
    // 播控台工具条
    const ctlImport = document.getElementById('ctl-import');
    if (ctlImport) {
        ctlImport.addEventListener('click', () => {
            const f = document.getElementById('media-file-input');
            if (f) f.click();
        });
    }
    const ctlGoto = document.getElementById('ctl-goto-playlist');
    if (ctlGoto) {
        ctlGoto.addEventListener('click', () => {
            document.querySelector('.nav-tab[data-tab="playlist"]')?.click();
        });
    }
    // 「重新载入」不只是刷新页面：它让引擎重读磁盘上的 config.json，
    // 这样在面板外（手工改文件）的改动也会生效，并重算时间轴。
    const ctlReload = document.getElementById('ctl-reload');
    if (ctlReload) {
        ctlReload.addEventListener('click', async () => {
            const original = ctlReload.textContent;
            ctlReload.disabled = true;
            ctlReload.textContent = '载入中…';
            try {
                await apiPost(API.reload, {});
                await refreshAll();
                log('已重新载入 config.json，时间轴已对齐当前时间');
            } catch (e) {
                log(`重新载入失败：${e.message}`, 'warn');
            } finally {
                ctlReload.disabled = false;
                ctlReload.textContent = original;
            }
        });
    }
    const ctlDiag = document.getElementById('ctl-diag');
    if (ctlDiag) ctlDiag.addEventListener('click', runDiagnostics);
    const ctlClearLog = document.getElementById('ctl-clear-log');
    if (ctlClearLog) {
        ctlClearLog.addEventListener('click', () => {
            const l = document.getElementById('dash-log');
            if (l) l.innerHTML = '';
        });
    }
    // The 24h timeline tab was dropped; the bumper list lives on the console, so
    // "manage" now leads to the playlist where the schedule is edited.
    document.getElementById('btn-manage-bumpers').addEventListener('click', () => {
        document.querySelector('.nav-tab[data-tab="playlist"]')?.click();
    });
    // Support button: the sponsor link is not decided yet, so acknowledge the
    // click in place instead of navigating nowhere.
    const supportBtn = document.getElementById('support-btn');
    if (supportBtn) {
        supportBtn.addEventListener('click', () => {
            const original = supportBtn.textContent;
            supportBtn.textContent = '感谢支持 · 链接待开放';
            log('感谢支持！赞助链接即将开放，敬请期待。');
            setTimeout(() => { supportBtn.textContent = original; }, 2500);
        });
    }
}

async function toggleArmed() {
    const running = !!(lastSnapshot && lastSnapshot.scheduler && lastSnapshot.scheduler.scheduler_running);
    const btn = document.getElementById('btn-armed');
    if (btn) {
        btn.disabled = true;
        btn.textContent = running ? '停止中…' : '启动中…';
    }
    try {
        if (running) {
            await apiPost(API.enable, { enabled: false });
            log('已停用自动播出');
        } else {
            // Refuse to arm into a state that cannot possibly play, with the
            // reason spelled out (missing/incorrect target source is by far the
            // most common cause of "nothing happens").
            // 这一步只提醒、不再拦截。以前它会直接中断启用：OBS 刚启动还没
            // 连上、或名字稍后才改，用户就会看到"点了没反应、按钮弹回去"。
            // 现在照常启用，真有问题由引擎报错并显示在主控台。
            const check = await checkTargetInput();
            if (!check.ok) {
                log(`提醒（不影响启用）：${check.message}`);
                showDashError(check.message);
            } else {
                dashErrorOverride = null;
            }
            // Arming re-bases the list on *this* moment: later than planned ->
            // everything 顺延; earlier -> everything 提前. No manual fixups.
            await apiPost(API.start, {
                ids: playlistCache.map(p => p.id),
                from_now: true,
            });
            log('已开始自动播出（节目单已按当前时刻对齐）');
            // Clear any stale pre-flight complaint: it used to stay on the
            // playlist page forever, so a fixed configuration still looked broken.
            setPlaylistStatus('已开始自动播出，节目单已按当前时刻对齐', 'ok');
            // Verify rather than assume: if the engine didn't actually come up,
            // say why instead of leaving the operator staring at a button that
            // quietly went back to "启用自动播出".
            setTimeout(reportArmResult, 2500);
        }
        await refreshAll();
        // Pull once more a moment later: the scheduler needs a tick to flip
        // scheduler_running, and the button label reads from that snapshot.
        setTimeout(refreshAll, 600);
    } catch (e) {
        log(`切换自动播出失败：${friendlyWriteError(e)}`);
    } finally {
        if (btn) {
            btn.disabled = false;
            btn.textContent = running ? '启用自动播出' : '停用自动播出';
        }
    }
}

/// 把一条失败原因同时摆到主控台和活动日志上。前端自己拦下来的问题
/// （比如目标源不对）以前只写在「节目清单」页，在主控台点按钮的人根本看不见。
function showDashError(msg) {
    dashErrorOverride = msg ? { msg, until: Date.now() + 15_000 } : null;
    const el = document.getElementById('ctl-error');
    if (el) {
        el.textContent = msg || '';
        el.hidden = !msg;
    }
}

/// 点了「启用自动播出」之后真的去核对一次：如果引擎没起来，把原因说出来，
/// 而不是让按钮悄悄变回「启用自动播出」让人以为网络卡了。
async function reportArmResult() {
    await refreshAll();
    const sch = (lastSnapshot && (lastSnapshot.scheduler || lastSnapshot.status)) || {};
    if (sch.scheduler_running) return;
    const why = sch.last_error ? `（${sch.last_error}）` : '';
    let extra = '';
    if (!sch.obs_connected) {
        extra = 'OBS 未连接：检查 obs-websocket 的端口与密码（设置页点「测试连接」）。';
    } else if (!playlistCache.length) {
        extra = '节目单是空的：先导入节目文件。';
    }
    const msg = `自动播出没有启动${why}${extra ? ' ' + extra : ''}`;
    log(msg);
    setPlaylistStatus(msg, 'err');
    showDashError(msg);
}

/// 一键自检：把"为什么播不出来"要看的东西一次全列出来。
/// 播出起不来基本就那几种原因（OBS 没连上 / 目标源名字不对 / 文件不见了 /
/// 节目时间都过去了），与其一个个猜，不如一次全查。
async function runDiagnostics() {
    log('—— 开始自检 ——');
    let snap = {};
    try {
        snap = await fetch(API.status).then(r => r.json());
        lastSnapshot = snap;
    } catch (e) {
        log(`✗ 引擎没有响应：${(e && e.message) || e}`);
        showDashError('引擎没有响应：确认 OBS 正在运行（引擎由 OBS 脚本拉起）');
        return;
    }

    const lines = [];
    const bad = [];
    const sch = snap.scheduler || snap.status || {};
    lines.push(`引擎：状态 ${sch.scheduler_state || 'Idle'}｜调度${sch.scheduler_running ? '运行中' : '未运行'}`);

    if (sch.obs_connected) {
        lines.push('OBS：已连接 ✓');
    } else {
        const why = sch.obs_error ? `（${sch.obs_error}）` : '';
        lines.push(`OBS：未连接 ✗${why}`);
        bad.push(`OBS 未连接${why}：到「设置」页核对 obs-websocket 的端口与密码，再点「测试连接」`);
    }

    const target = snap.target_input || '';
    try {
        const j = await fetch('/api/obs/inputs').then(r => r.json());
        const list = j.inputs || [];
        if (!list.length) {
            lines.push('目标源：无法枚举（OBS 未连接）');
        } else {
            const want = String(target).trim().toLowerCase();
            const hit = list.find(i => String(i.inputName || '').trim().toLowerCase() === want);
            if (hit) {
                lines.push(`目标源：${target} ✓（类型 ${hit.inputKind || '未知'}）`);
            } else {
                lines.push(`目标源：${target || '(空)'} ✗`);
                bad.push(`目标媒体源「${target || '(空)'}」在 OBS 里不存在。OBS 现有来源：`
                    + list.map(i => i.inputName).join('、') + '。请到「设置」页重新选择并保存');
            }
        }
    } catch (_) {
        lines.push('目标源：查询失败');
    }

    let items = [];
    try {
        const p = await fetch(API.playlist).then(r => r.json());
        items = p.items || [];
        playlistCache = items;
    } catch (_) {}
    lines.push(`节目单：共 ${items.length} 条`);
    if (!items.length) {
        bad.push('节目单是空的：先点「导入素材」把节目加进来');
    } else {
        try {
            const v = await apiPost(API.verify, {});
            const missing = (v && v.missing) || [];
            if (missing.length) {
                lines.push(`节目单：${missing.length} 条文件丢失 ✗`);
                bad.push(`有 ${missing.length} 条节目的文件找不到了（节目表里标红那几条）：换文件或删掉它们`);
            } else {
                lines.push('节目单：文件都在 ✓');
            }
        } catch (_) {}
        const now = Date.now();
        const pending = items.filter(p => p.start_at_ms > now);
        lines.push(`节目单：待播 ${pending.length} 条`
            + (pending.length ? `，首条 ${fmtLocal(pending[0].start_at_ms)}` : ''));
        if (!pending.length) {
            bad.push('节目单里所有节目的播出时间都已过去：点「启用自动播出」会按当前时刻重排，然后再试');
        }
    }

    const ntp = snap.ntp || {};
    if (ntp.error) lines.push(`授时：${ntp.error}`);
    else if (ntp.offset_ms != null) lines.push(`授时：${ntp.server}｜偏差 ${ntp.offset_ms}ms`);

    if (sch.last_error) lines.push(`引擎最近报错：${sch.last_error}`);

    for (const l of lines) log(`  ${l}`);
    if (bad.length) {
        log('—— 自检发现问题 ——');
        for (const b of bad) log(`  ✗ ${b}`);
        showDashError(bad[0]);
        setPlaylistStatus(bad[0], 'err');
    } else {
        log('—— 自检通过，没有发现明显问题 ——');
        showDashError('');
    }
}

/* -------------------------------------------------------------------------- */
/* Status pills (header right)                                               */
/* -------------------------------------------------------------------------- */

function renderStatus(s) {
    // The header status pills were removed (ON AIR is the only indicator), so
    // there is nothing to render here. Kept as a no-op for compatibility.
    const eng = document.getElementById('status-eng');
    if (!eng) return;
    // Old engines returned the runtime state under `status`; the current
    // engine (and the /ws snapshot) uses `scheduler`. Accept both.
    const sch = s.scheduler || s.status || {};
    const obs = document.getElementById('status-obs');
    const cfg = document.getElementById('status-cfg');
    // Plain labels — no coloured status lights (ON AIR is the only lamp).
    eng.textContent = `引擎 ${sch.scheduler_state || 'Idle'}`;
    eng.className = 'pill';
    obs.textContent = `OBS ${sch.obs_connected ? '已连' : '未连'}`;
    obs.className = 'pill';
    const target = s.target_input || (s.config && s.config.target_input) || '';
    cfg.textContent = target ? `目标: ${target}` : 'OBS-WS 待配置';
    cfg.className = 'pill';
}

/* -------------------------------------------------------------------------- */
/* Playlist                                                                  */
/* -------------------------------------------------------------------------- */

// Inline feedback right next to the buttons. Relying only on the dashboard
// activity log made actions look like "nothing happened" from other tabs.
function setPlaylistStatus(msg, kind) {
    const el = document.getElementById('playlist-status');
    if (!el) return;
    el.textContent = msg || '';
    el.className = 'text-xs mt-2 ' + (kind === 'ok' ? 'text-emerald-400' : kind === 'err' ? 'text-red' : 'text-muted');
}

// Turn an HTTP failure into something actionable instead of "HTTP 401".
function friendlyWriteError(e) {
    const s = String(e && e.message ? e.message : e);
    if (/401/.test(s)) {
        return '没有通过授权：请先在 OBS 的脚本面板里点一次「Test Connection」（它会把授权令牌交给引擎），然后再试';
    }
    if (/403/.test(s)) return '引擎拒绝了该操作（HTTP 403）';
    if (/404/.test(s)) return '引擎没有这个接口（HTTP 404）——可能是旧版引擎，请更新到最新包';
    if (/Failed to fetch|NetworkError|ECONNREFUSED/i.test(s)) {
        return '连不上引擎：引擎进程可能没在跑，请在 OBS 脚本里点「Test Connection」或重启 OBS';
    }
    return s;
}

/// Start time for the next program: never earlier than +60 s, and never before
/// the current last program ends — otherwise every new entry piled up on the
/// same timestamp and the scheduler played them all at once.
function nextStartAt(durationMs) {
    const dur = durationMs || 30 * 60 * 1000;
    // 5s, not a minute: importing a file and then pressing "启用自动播出" should
    // put something on screen right away. Waiting 60 seconds made a working
    // setup look broken.
    let start = Date.now() + 5_000;
    if (playlistCache.length) {
        const last = playlistCache[playlistCache.length - 1];
        const lastEnd = (last.start_at_ms || 0) + (last.declared_duration_ms || 0);
        if (lastEnd + 5_000 > start) start = lastEnd + 5_000;
    }
    return start;
}

function selectedKind() {
    const el = document.getElementById('playlist-add-kind');
    return (el && el.value) || 'primary';
}

/// Declared duration from the "minutes" field. The engine probes the real
/// length after the first play, but the schedule needs an estimate until then —
/// a wrong fixed 30 min for every entry is what makes the timeline drift.
function defaultDurationMs() {
    const el = document.getElementById('playlist-add-duration');
    const min = parseInt(el && el.value, 10);
    return (isFinite(min) && min > 0 ? min : 30) * 60 * 1000;
}

// Add one program, used by both the "+" button and file imports.
async function addProgram(name, path, startAtMs, durationMs, kind) {
    return apiPost(API.playlistItem, {
        id: cryptoRandomId(),
        name,
        file_path: path,
        start_at_ms: startAtMs,
        declared_duration_ms: durationMs || 30 * 60 * 1000,
        kind: kind || 'primary',
    });
}

function setupPlaylistAdd() {
    // Keyboard shortcuts advertised in the UI (Ctrl/⌘ + N / + S).
    document.addEventListener('keydown', (e) => {
        if (!(e.ctrlKey || e.metaKey)) return;
        const k = (e.key || '').toLowerCase();
        const t = e.target;
        const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT');
        if (k === 'n' && !typing) {
            e.preventDefault();
            document.querySelector('.nav-tab[data-tab="playlist"]')?.click();
            // The manual name/path inputs are gone (import is pick-and-upload),
            // so focus the import button instead.
            document.getElementById('btn-pick-media')?.focus();
        } else if (k === 's') {
            e.preventDefault();
            saveCfg();
        }
    });

    // 「新增节目」同样是选文件：名称取文件名、时长自动探测，不让用户手填。
    document.getElementById('btn-add-program').addEventListener('click', () => {
        const input = document.getElementById('media-file-input');
        if (input) input.click();
    });

    // Pick a single media file to fill the form.
    const pickBtn = document.getElementById('btn-pick-media');
    const mediaInput = document.getElementById('media-file-input');
    let pickMode = 'fill';
    if (pickBtn && mediaInput) {
        pickBtn.addEventListener('click', () => { pickMode = 'batch'; mediaInput.click(); });
    }
    const importMediaBtn = document.getElementById('btn-import-media');
    if (importMediaBtn && mediaInput) {
        importMediaBtn.addEventListener('click', () => { pickMode = 'batch'; mediaInput.click(); });
    }
    if (mediaInput) {
        mediaInput.addEventListener('change', async () => {
            const files = Array.from(mediaInput.files || []);
            mediaInput.value = '';
            if (!files.length) return;

            // Pick -> upload -> import. The browser cannot reveal an absolute
            // path, so we hand the bytes to the engine and use the path it
            // reports back. No folder typing, no 异常 rows.
            setPlaylistStatus(`正在读取 ${files.length} 个文件…`);
            const probed = await Promise.all(files.map(probeMediaFile));
            const broken = probed.filter(p => !p.ok);
            if (broken.length) {
                log(`无法读取（可能损坏或格式不支持）：${broken.map(p => p.file.name).join('、')}`);
            }

            let cursor = nextStartAt(0);
            let ok = 0;
            for (let i = 0; i < probed.length; i++) {
                const item = probed[i];
                const label = item.file.name;
                const name = label.replace(/\.[^.]+$/, '');
                const dur = item.durationMs > 0 ? item.durationMs : 30 * 60 * 1000;
                setPlaylistStatus(`正在导入 ${i + 1}/${probed.length}：${label}…`);
                let path = '';
                try {
                    path = await uploadMediaFile(item.file, (p) => {
                        setPlaylistStatus(
                            `正在上传 ${i + 1}/${probed.length}：${label}（${Math.round(p * 100)}%）`
                        );
                    });
                } catch (e) {
                    log(`上传「${label}」失败：${friendlyWriteError(e)}`);
                    continue;
                }
                try {
                    await addProgram(name, path, cursor, dur, 'primary');
                    cursor += dur;
                    ok++;
                } catch (e) {
                    log(`导入「${label}」失败：${friendlyWriteError(e)}`);
                }
            }
            setPlaylistStatus(
                `✅ 已导入 ${ok}/${probed.length} 个文件（时长自动识别，已保存到引擎 media 目录）`,
                ok ? 'ok' : 'err'
            );
            log(`导入完成：成功 ${ok}/${probed.length}`);
            await refreshAll();
        });
    }

    // 本机文件浏览器：路径由引擎返回，导入后不可能出现"异常"。
    const browseBtn = document.getElementById('btn-browse');
    if (browseBtn) browseBtn.addEventListener('click', () => openFsBrowser(''));
    const fsClose = document.getElementById('fs-close');
    if (fsClose) {
        fsClose.addEventListener('click', () => {
            const m = document.getElementById('fs-modal');
            if (m) m.hidden = true;
        });
    }
    const fsModal = document.getElementById('fs-modal');
    if (fsModal) {
        fsModal.addEventListener('click', (e) => {
            if (e.target === fsModal) fsModal.hidden = true;
        });
    }

    // 开播时间（本机时间）→ 按该时刻重排整张节目单
    const applyStartBtn = document.getElementById('btn-apply-start');
    if (applyStartBtn) {
        applyStartBtn.addEventListener('click', async () => {
            const raw = (document.getElementById('schedule-start-at') || {}).value || '';
            const base = parseLocalInput(raw);
            if (base == null) {
                setPlaylistStatus('请先选择开播时间（按本机时间填写）', 'err');
                return;
            }
            setPlaylistStatus('正在按开播时间重排…');
            try {
                await apiPost('/api/playlist/reorder', {
                    ids: playlistCache.map(p => p.id),
                    base_start_ms: base,
                    from_now: false,
                });
                setPlaylistStatus('✅ 已按开播时间重排节目单', 'ok');
                await refreshAll();
            } catch (e) {
                setPlaylistStatus(`重排失败：${friendlyWriteError(e)}`, 'err');
            }
        });
    }

    // 批量勾选 / 批量删除
    const selectAll = document.getElementById('select-all');
    if (selectAll) {
        selectAll.addEventListener('change', () => {
            document.querySelectorAll('#playlist-tbody input.row-select')
                .forEach(cb => { cb.checked = selectAll.checked; });
        });
    }
    const delSelected = document.getElementById('btn-delete-selected');
    if (delSelected) {
        delSelected.addEventListener('click', async () => {
            const ids = Array.from(
                document.querySelectorAll('#playlist-tbody input.row-select:checked')
            ).map(cb => cb.dataset.id);
            if (!ids.length) {
                setPlaylistStatus('请先勾选要删除的节目', 'err');
                return;
            }
            if (!window.confirm(`确定删除选中的 ${ids.length} 条节目？该操作不可撤销。`)) return;
            let ok = 0;
            for (const id of ids) {
                try {
                    await apiDelete(API.playlistItem, { id });
                    ok++;
                } catch (e) {
                    log(`删除失败：${friendlyWriteError(e)}`);
                }
            }
            setPlaylistStatus(`✅ 已删除 ${ok}/${ids.length} 条`, ok ? 'ok' : 'err');
            const sa = document.getElementById('select-all');
            if (sa) sa.checked = false;
            await refreshAll();
        });
    }

    // Import a playlist LIST file (JSON or TXT) — same authenticated endpoint.
    const importBtn = document.getElementById('btn-import-open');
    const fileInput = document.getElementById('playlist-import');
    if (importBtn && fileInput) {
        importBtn.addEventListener('click', () => fileInput.click());
        fileInput.addEventListener('change', async () => {
            const file = fileInput.files && fileInput.files[0];
            fileInput.value = '';
            if (!file) return;
            setPlaylistStatus('正在导入节目表…');
            try {
                const text = await file.text();
                const rows = parsePlaylistFile(file.name, text);
                if (!rows.length) {
                    setPlaylistStatus(`「${file.name}」里没有解析到任何节目行`, 'err');
                    return;
                }
                let ok = 0;
                const base = nextStartAt(30 * 60 * 1000);
                for (let i = 0; i < rows.length; i++) {
                    const r = rows[i];
                    const start = r.start_at_ms != null
                        ? r.start_at_ms
                        : base + i * (r.declared_duration_ms || 30 * 60 * 1000);
                    try {
                        await addProgram(r.name, r.file_path, start, r.declared_duration_ms, r.kind);
                        ok++;
                    } catch (e) {
                        log(`导入第 ${i + 1} 条失败（${r.name}）：${friendlyWriteError(e)}`);
                    }
                }
                setPlaylistStatus(`✅ 导入完成：成功 ${ok}/${rows.length} 条（${file.name}）`, ok ? 'ok' : 'err');
                await refreshAll();
            } catch (e) {
                setPlaylistStatus(`导入失败：${friendlyWriteError(e)}`, 'err');
            }
        });
    }
}

/* ---------------------- 本机文件浏览器（引擎侧路径） ---------------------- */

async function openFsBrowser(path) {
    const modal = document.getElementById('fs-modal');
    if (modal) modal.hidden = false;
    await loadFs(path || '');
}

async function loadFs(path) {
    const list = document.getElementById('fs-list');
    const pathEl = document.getElementById('fs-path');
    const upBtn = document.getElementById('fs-up');
    if (!list) return;
    list.innerHTML = '<div class="text-muted text-xs" style="padding:10px">加载中…</div>';
    let j = {};
    try {
        const url = API.fsBrowse + (path ? '?path=' + encodeURIComponent(path) : '');
        const r = await fetch(url);
        j = await r.json();
    } catch (e) {
        list.innerHTML = `<div class="text-red text-xs" style="padding:10px">无法读取目录：${escapeHtml(String(e))}</div>`;
        return;
    }
    if (pathEl) pathEl.textContent = j.path || '请选择磁盘或目录';
    if (upBtn) {
        upBtn.disabled = !j.parent;
        upBtn.onclick = () => { if (j.parent) loadFs(j.parent); };
    }
    list.innerHTML = '';
    if (j.error) {
        const d = document.createElement('div');
        d.className = 'text-red text-xs';
        d.style.padding = '10px';
        d.textContent = j.error;
        list.appendChild(d);
    }
    (j.dirs || []).forEach(d => {
        const row = document.createElement('button');
        row.type = 'button';
        row.className = 'fs-row fs-dir';
        row.textContent = '📁 ' + d.name;
        row.addEventListener('click', () => loadFs(d.path));
        list.appendChild(row);
    });
    (j.files || []).forEach(f => {
        const row = document.createElement('button');
        row.type = 'button';
        row.className = 'fs-row fs-file';
        row.innerHTML = `<span>🎬 ${escapeHtml(f.name)}</span>` +
            `<span class="text-muted">${fmtSize(f.size)}</span>`;
        row.addEventListener('click', () => addFromBrowser(f));
        list.appendChild(row);
    });
    if (!(j.dirs || []).length && !(j.files || []).length && !j.error) {
        const d = document.createElement('div');
        d.className = 'text-muted text-xs';
        d.style.padding = '10px';
        d.textContent = '该目录下没有可用的视频/图片。';
        list.appendChild(d);
    }
}

function fmtSize(n) {
    if (n == null) return '';
    const mb = n / (1024 * 1024);
    return mb >= 1024 ? (mb / 1024).toFixed(2) + ' GB' : mb.toFixed(1) + ' MB';
}

async function addFromBrowser(f) {
    setPlaylistStatus('正在添加…');
    const name = f.name.replace(/\.[^.]+$/, '');
    try {
        // Real absolute path from the engine; duration is filled in by the
        // engine's probe once the file plays for the first time.
        await addProgram(name, f.path, nextStartAt(0), 30 * 60 * 1000, 'primary');
        setPlaylistStatus(`✅ 已添加「${name}」（时长将在首次播出后自动探测）`, 'ok');
        await refreshAll();
    } catch (e) {
        setPlaylistStatus(`添加失败：${friendlyWriteError(e)}`, 'err');
    }
}

/// Hand the file bytes to the engine and get back the absolute path it was
/// stored at. This is what lets "pick files" be the whole workflow.
/// Uses XHR (not fetch) so the admin can show real upload progress.
async function uploadMediaFile(file, onProgress) {
    // Make sure we send a fresh token: a WS frame may have replaced the
    // snapshot since the page loaded.
    await reloadToken();
    const token = engineToken();
    return new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.open('POST', API.upload, true);
        xhr.setRequestHeader('Content-Type', 'application/octet-stream');
        // encodeURIComponent keeps non-ASCII names header-safe.
        xhr.setRequestHeader('X-File-Name', encodeURIComponent(file.name));
        xhr.setRequestHeader('X-Bootstrap-Token', token);
        if (xhr.upload && onProgress) {
            xhr.upload.onprogress = (e) => {
                if (e.lengthComputable) onProgress(e.loaded / e.total);
            };
        }
        xhr.onload = () => {
            if (xhr.status >= 200 && xhr.status < 300) {
                try {
                    resolve(JSON.parse(xhr.responseText).path);
                } catch (e) {
                    reject(new Error('服务器返回无法解析'));
                }
                return;
            }
            let msg = `HTTP ${xhr.status}`;
            try {
                const j = JSON.parse(xhr.responseText);
                if (j.error) msg = j.error;
            } catch (_) { /* keep the status line */ }
            if (xhr.status === 401 || xhr.status === 403) {
                msg = '没有通过授权：请在 OBS 脚本面板点一次 Test Connection';
            }
            if (xhr.status === 413) {
                msg = '文件太大，被引擎拒绝';
            }
            reject(new Error(msg));
        };
        xhr.onerror = () => reject(new Error('上传失败（引擎可能没在运行）'));
        xhr.send(file);
    });
}

/// Pre-flight check before arming: the target must exist in OBS and be a media
/// source, otherwise nothing will ever play and the reason is invisible.
async function checkTargetInput() {
    // Read the target from a *fresh* status call, not the local snapshot: the
    // snapshot is replaced by every /ws frame, so after saving a new target
    // source in Settings the check could still be looking at the old name — and
    // report "source does not exist" even though everything is configured
    // correctly.
    let target = (lastSnapshot && lastSnapshot.target_input) || '';
    try {
        const s = await fetch(API.status).then(r => r.json());
        lastSnapshot = { ...(lastSnapshot || {}), ...s };
        target = s.target_input || target;
    } catch (_) {}
    if (!target) {
        return { ok: false, message: '还没有设置目标媒体源：到「设置」页从下拉里选择你的媒体源并保存' };
    }
    try {
        const j = await fetch('/api/obs/inputs').then(r => r.json());
        const list = j.inputs || [];
        if (!list.length) return { ok: true, message: '' }; // OBS 未连接，交给其它提示
        // OBS source names tolerate stray spaces and case differ, so compare
        // loosely — an exact match made "main_media " look like a missing source.
        const want = target.trim().toLowerCase();
        const hit = list.find(i => String(i.inputName || '').trim().toLowerCase() === want);
        if (!hit) {
            return {
                ok: false,
                message: `目标媒体源「${target}」在 OBS 里不存在。当前 OBS 的来源：` +
                    list.map(i => i.inputName).join('、') + '。请到「设置」页重新选择',
            };
        }
        const kind = (hit.inputKind || '').toLowerCase();
        if (!/ffmpeg|vlc|media/.test(kind)) {
            return {
                ok: false,
                message: `「${target}」不是媒体源（类型：${hit.inputKind}）。请选择「媒体源」，VLC 视频源也可`,
            };
        }
        return { ok: true, message: '' };
    } catch (_) {
        return { ok: true, message: '' };
    }
}

/// Read a media file's real duration in the browser (no upload, no ffprobe).
/// Images have no duration, so they get a short default slot.
const IMAGE_SLOT_MS = 10_000;

function probeMediaFile(file) {
    return new Promise((resolve) => {
        const isImage = /^image\//.test(file.type || '');
        if (isImage) {
            resolve({ file: file, durationMs: IMAGE_SLOT_MS, ok: true });
            return;
        }
        let url;
        try {
            url = URL.createObjectURL(file);
        } catch (_) {
            resolve({ file: file, durationMs: 0, ok: false });
            return;
        }
        const v = document.createElement('video');
        v.preload = 'metadata';
        let settled = false;
        const finish = (ms, ok) => {
            if (settled) return;
            settled = true;
            // Free the blob immediately — dozens of kept object URLs are a
            // real leak when importing a whole day's worth of programmes.
            try { URL.revokeObjectURL(url); } catch (_) {}
            v.removeAttribute('src');
            v.load();
            resolve({ file: file, durationMs: ms, ok: ok });
        };
        v.onloadedmetadata = () => {
            const d = v.duration;
            const good = Number.isFinite(d) && d > 0;
            finish(good ? Math.round(d * 1000) : 0, good);
        };
        v.onerror = () => finish(0, false);
        setTimeout(() => finish(0, false), 10_000);
        v.src = url;
    });
}

// Parse a playlist file into {name,file_path,start_at_ms?,declared_duration_ms?,kind?}[].
function parsePlaylistFile(name, raw) {
    const lower = (name || '').toLowerCase();
    const text = raw.replace(/\r\n/g, '\n').replace(/^\uFEFF/, '');
    if (lower.endsWith('.json')) {
        const j = JSON.parse(text);
        const arr = Array.isArray(j) ? j : (j.items || []);
        return arr
            .map(x => ({
                name: String(x.name ?? '').trim(),
                file_path: String(x.file_path ?? x.path ?? '').trim(),
                start_at_ms: x.start_at_ms != null ? Number(x.start_at_ms) : null,
                declared_duration_ms: x.declared_duration_ms != null ? Number(x.declared_duration_ms) : 30 * 60 * 1000,
                kind: x.kind || 'primary',
            }))
            .filter(x => x.name && x.file_path);
    }
    // Plain text / TSV: one program per line, `name<TAB>path`. If there is no
    // tab, split on the first space-delimited run of '|' or the first 2+ spaces.
    return text
        .split('\n')
        .map(line => line.trim())
        .filter(line => line && !line.startsWith('#'))
        .map((line, i) => {
            let sep = line.indexOf('\t');
            if (sep < 0) {
                const pipe = line.indexOf('|');
                if (pipe >= 0) {
                    sep = pipe;
                    line = line.slice(0, pipe) + '\t' + line.slice(pipe + 1);
                } else {
                    const m = line.match(/^(\S+)\s+(.+)$/);
                    if (m) { sep = m[1].length + 1; }
                }
            }
            const name = (sep > 0 ? line.slice(0, sep) : line).trim();
            const path = (sep > 0 ? line.slice(sep + 1) : '').trim();
            if (!name || !path) return null;
            return { name, file_path: path, start_at_ms: null, declared_duration_ms: 30 * 60 * 1000, kind: 'primary' };
        })
        .filter(Boolean);
}

/// Items drawn on the 24h timeline: programs plus their bumpers (an
/// interstitial scheduled at an offset inside a program would otherwise be
/// invisible in the timeline view).
function timelineItems() {
    const byId = new Map(playlistCache.map(p => [p.id, p]));
    const bumpers = (bumpersCache || []).map(b => {
        const target = byId.get(b.target_program_id);
        if (!target) return null;
        return {
            id: b.id,
            name: b.content.name,
            start_at_ms: (target.start_at_ms || 0) + (b.at_into_program_ms || 0),
            declared_duration_ms: b.content.declared_duration_ms || 0,
            kind: 'bumper',
        };
    }).filter(Boolean);
    return [...playlistCache, ...bumpers];
}

/// Ids the engine reported as "file missing / blank" — shown as 异常.
let missingIds = new Set();

async function verifyPlaylist() {
    try {
        const r = await apiPost('/api/playlist/verify', {});
        missingIds = new Set(r.missing || []);
    } catch (_) {
        missingIds = new Set();
    }
}

/// 播出状态：异常 / 播出中 / 已完成 / 等待中
function programState(p, sch) {
    if (missingIds.has(p.id)) return ['异常', 'text-red'];
    const now = Date.now();
    const start = p.start_at_ms || 0;
    const dur = p.detected_duration_ms || p.declared_duration_ms || 0;
    const end = start + dur;
    const onAir = sch
        && sch.current_program_id === p.id
        && (sch.scheduler_state === 'Playing' || sch.scheduler_state === 'Interstitial');
    if (onAir || (now >= start && now < end && sch && sch.scheduler_running)) {
        return ['播出中', 'text-emerald-400'];
    }
    if (dur > 0 && now >= end) return ['已完成', 'text-muted'];
    return ['等待中', 'text-muted'];
}

/// 节目名单行会被反复重建，所以浮窗用事件委托挂在 tbody 上（只绑一次）。
/// 用 fixed 定位跟随鼠标：表格外面是 overflow-x-auto，普通绝对定位会被裁掉。
function setupPlaylistTooltip() {
    const tip = document.getElementById('pl-tooltip');
    const tbody = document.getElementById('playlist-tbody');
    if (!tip || !tbody || tbody.dataset.tipBound === '1') return;
    tbody.dataset.tipBound = '1';

    const place = (e) => {
        const pad = 14;
        const r = tip.getBoundingClientRect();
        let x = e.clientX + pad;
        let y = e.clientY + pad;
        if (x + r.width > window.innerWidth - 8) x = e.clientX - r.width - pad;
        if (y + r.height > window.innerHeight - 8) y = e.clientY - r.height - pad;
        tip.style.left = `${Math.max(8, x)}px`;
        tip.style.top = `${Math.max(8, y)}px`;
    };

    tbody.addEventListener('mouseover', (e) => {
        const cell = e.target.closest('td.pl-name');
        if (!cell) return;
        tip.innerHTML = '';
        const nameEl = document.createElement('div');
        nameEl.className = 'pl-tip-name';
        nameEl.textContent = cell.dataset.full || cell.textContent || '';
        tip.appendChild(nameEl);
        const path = cell.dataset.path || '';
        if (path) {
            const pathEl = document.createElement('div');
            pathEl.className = 'pl-tip-path';
            pathEl.textContent = path;
            tip.appendChild(pathEl);
        }
        tip.hidden = false;
        place(e);
    });
    tbody.addEventListener('mousemove', (e) => {
        if (!tip.hidden) place(e);
    });
    tbody.addEventListener('mouseout', (e) => {
        if (e.target.closest('td.pl-name')) tip.hidden = true;
    });
    window.addEventListener('scroll', () => { tip.hidden = true; }, true);
}

function renderPlaylistRows() {
    const tbody = document.getElementById('playlist-tbody');
    tbody.innerHTML = '';

    if (!playlistCache.length) {
        const tr = document.createElement('tr');
        tr.innerHTML = '<td colspan="8" class="py-4 text-center text-muted text-xs">' +
            '还没有节目。点上方「📁 选择文件（可多选，直接导入）」一次导入多个视频，' +
            '或用「🗂 从本机已有文件选择」直接引用本机文件。</td>';
        tbody.appendChild(tr);
        return;
    }

    const sch = (lastSnapshot && (lastSnapshot.scheduler || lastSnapshot.status)) || {};
    let dragId = null;
    const clearMarks = () => tbody.querySelectorAll('tr')
        .forEach(r => r.classList.remove('drop-above', 'drop-below'));

    /// Send the new order to the engine. Rows that already aired keep their
    /// slots; everything still to come is laid out back-to-back behind them, so
    /// reordering never leaves a hole (which would show as black).
    const applyOrder = async (items) => {
        const now = Date.now();
        const anchorItem = items.find(p => p.start_at_ms > now);
        setPlaylistStatus('正在应用新的播出顺序…');
        try {
            await apiPost(API.reorder, {
                ids: items.map(p => p.id),
                from_id: anchorItem ? anchorItem.id : null,
                base_start_ms: anchorItem ? anchorItem.start_at_ms : 0,
                from_now: !anchorItem,
            });
            setPlaylistStatus('✅ 已应用新顺序，待播节目已自动顺排衔接', 'ok');
            await refreshAll();
        } catch (e) {
            setPlaylistStatus(`调整顺序失败：${friendlyWriteError(e)}`, 'err');
        }
    };

    const reorderByDrag = (fromId, targetId, after) => {
        const from = playlistCache.findIndex(x => x.id === fromId);
        if (from < 0) return;
        const items = playlistCache.slice();
        const [moved] = items.splice(from, 1);
        const to = items.findIndex(x => x.id === targetId);
        if (to < 0) return;
        items.splice(after ? to + 1 : to, 0, moved);
        applyOrder(items);
    };

    playlistCache.forEach((p, i) => {
        const [stateLabel, stateClass] = programState(p, sch);
        const durMs = p.detected_duration_ms || p.declared_duration_ms || 0;
        // Rows that have already started (or are on air) can't be dragged:
        // moving them rewrites times the scheduler has already acted on.
        const locked = p.start_at_ms <= (sch.now_ms || Date.now());
        const tr = document.createElement('tr');
        tr.className = 'hover:bg-surface-2 transition';
        if (locked) tr.classList.add('row-locked');
        tr.draggable = !locked;
        tr.innerHTML = `
            <td class="py-2 pr-2">
                <input type="checkbox" class="row-select" data-id="${p.id}" aria-label="选择该节目" />
            </td>
            <td class="py-2 pr-2 pl-grip"
                title="${locked ? '该节目已开始播出，不能调整顺序' : '按住这里拖动，即可调整播出顺序'}">${locked ? '' : '⠿'}</td>
            <td class="py-2 pr-2 pl-name"
                data-full="${escapeHtml(p.name)}"
                data-path="${escapeHtml(p.file_path)}">${escapeHtml(p.name)}</td>
            <td class="py-2 pr-2">
                <select class="form-input row-kind" data-id="${p.id}" style="min-width:108px">
                    ${PROGRAM_KINDS.map(([v, label]) =>
                        `<option value="${v}" ${v === p.kind ? 'selected' : ''}>${label}</option>`).join('')}
                </select>
            </td>
            <td class="py-2 pr-2 font-mono text-xs">${msToHMS(durMs)}</td>
            <td class="py-2 pr-2 font-mono text-xs">${fmtLocal(p.start_at_ms)}</td>
            <td class="py-2 pr-2 text-xs ${stateClass}">${stateLabel}</td>
            <td class="py-2 pr-2 text-right">
                <button class="btn btn-link" data-act="up" data-id="${p.id}"
                        ${i === 0 ? 'disabled' : ''} title="上移">↑</button>
                <button class="btn btn-link" data-act="down" data-id="${p.id}"
                        ${i === playlistCache.length - 1 ? 'disabled' : ''} title="下移">↓</button>
                <button class="btn btn-link text-red" data-act="del" data-id="${p.id}">删除</button>
            </td>`;
        // ---- drag & drop reordering -----------------------------------
        tr.addEventListener('dragstart', (e) => {
            if (locked) { e.preventDefault(); return; }
            dragId = p.id;
            tr.classList.add('dragging');
            e.dataTransfer.effectAllowed = 'move';
            try { e.dataTransfer.setData('text/plain', p.id); } catch (_) {}
        });
        tr.addEventListener('dragend', () => {
            dragId = null;
            tr.classList.remove('dragging');
            clearMarks();
        });
        tr.addEventListener('dragover', (e) => {
            if (!dragId || dragId === p.id || locked) return;
            e.preventDefault();
            e.dataTransfer.dropEffect = 'move';
            const r = tr.getBoundingClientRect();
            const after = (e.clientY - r.top) > r.height / 2;
            clearMarks();
            tr.classList.add(after ? 'drop-below' : 'drop-above');
        });
        tr.addEventListener('dragleave', () => {
            tr.classList.remove('drop-above', 'drop-below');
        });
        tr.addEventListener('drop', (e) => {
            if (!dragId || dragId === p.id || locked) return;
            e.preventDefault();
            const r = tr.getBoundingClientRect();
            const after = (e.clientY - r.top) > r.height / 2;
            const from = dragId;
            clearMarks();
            reorderByDrag(from, p.id, after);
        });

        tbody.appendChild(tr);
    });

    // Reorder by swapping the two neighbours' start times: the engine always
    // sorts by start_at_ms, so the broadcast order follows the table order.
    const move = async (id, dir) => {
        const idx = playlistCache.findIndex(p => p.id === id);
        const j = idx + dir;
        if (idx < 0 || j < 0 || j >= playlistCache.length) return;
        const a = playlistCache[idx];
        const b = playlistCache[j];
        setPlaylistStatus('正在调整顺序…');
        try {
            await apiPost(API.playlistItem, { ...a, start_at_ms: b.start_at_ms });
            await apiPost(API.playlistItem, { ...b, start_at_ms: a.start_at_ms });
            setPlaylistStatus(`✅ 已调整「${a.name}」的播出顺序`, 'ok');
            await refreshAll();
        } catch (e) {
            setPlaylistStatus(`调整顺序失败：${friendlyWriteError(e)}`, 'err');
        }
    };

    // 节目类型：行内直接改，改完立即写回引擎
    tbody.querySelectorAll('select.row-kind').forEach(sel => {
        sel.addEventListener('change', async () => {
            const p = playlistCache.find(x => x.id === sel.dataset.id);
            if (!p) return;
            setPlaylistStatus('正在更新节目类型…');
            try {
                await apiPost(API.playlistItem, { ...p, kind: sel.value });
                setPlaylistStatus(`✅ 已设为「${PROGRAM_KIND_LABEL[sel.value] || sel.value}」`, 'ok');
                await refreshAll();
            } catch (e) {
                setPlaylistStatus(`更新失败：${friendlyWriteError(e)}`, 'err');
            }
        });
    });

    tbody.querySelectorAll('button[data-act]').forEach(b => {
        const id = b.dataset.id;
        const act = b.dataset.act;
        b.addEventListener('click', async () => {
            if (act === 'up') return move(id, -1);
            if (act === 'down') return move(id, 1);
            if (act === 'del') {
                const item = playlistCache.find(p => p.id === id);
                const label = item ? item.name : id;
                if (!window.confirm(`确定删除节目「${label}」？该操作不可撤销。`)) return;
                try {
                    await apiDelete(API.playlistItem, { id });
                    setPlaylistStatus(`✅ 已删除「${label}」`, 'ok');
                    await refreshAll();
                } catch (e) {
                    setPlaylistStatus(`删除失败：${friendlyWriteError(e)}`, 'err');
                }
            }
        });
    });
}

/* -------------------------------------------------------------------------- */
/* Settings                                                                  */
/* -------------------------------------------------------------------------- */

/* ------------------------------------------------------ 报时器字体选择 --- */

// 常见字体回退清单。浏览器只有在用户授权后才能枚举系统字体（Chromium 的
// Local Font Access API），所以在读不到时这份列表仍然让用户能选到常见中文字体。
const CLOCK_FONT_FALLBACK = [
    ['Microsoft YaHei', '微软雅黑'],
    ['Microsoft YaHei UI', '微软雅黑 UI'],
    ['SimHei', '中易黑体'],
    ['SimSun', '中易宋体'],
    ['KaiTi', '楷体'],
    ['FangSong', '仿宋'],
    ['DengXian', '等线'],
    ['PingFang SC', '苹方（macOS）'],
    ['Noto Sans CJK SC', 'Noto Sans CJK'],
    ['Source Han Sans SC', '思源黑体'],
    ['DSEG7 Classic', 'DSEG7 数码管字体（若已安装）'],
    ['Arial', 'Arial'],
    ['Impact', 'Impact'],
    ['Consolas', 'Consolas'],
    ['Courier New', 'Courier New'],
];

/// 下拉里选中某个字体名后写进输入框的 CSS 值（带中英文回退，避免缺字）。
/// 授时状态：让"时间到底准不准"这件事在界面上看得见。
function renderNtpStatus(ntp) {
    const el = document.getElementById('cfg-ntp-status');
    if (!el) return;
    if (!ntp) {
        el.textContent = '';
        return;
    }
    if (ntp.error) {
        el.textContent = `⚠ ${ntp.error}`;
        return;
    }
    if (ntp.offset_ms == null) {
        el.textContent = '尚未同步';
        return;
    }
    const sign = ntp.offset_ms >= 0 ? '+' : '';
    const when = ntp.synced_at ? `｜${fmtLocal(new Date(ntp.synced_at).getTime())}` : '';
    el.textContent = `${ntp.server || '已授时'}｜偏差 ${sign}${ntp.offset_ms}ms${when}`;
}

/// 手动触发一次授时，不用等下一个同步周期。
async function syncTimeNow() {
    const el = document.getElementById('cfg-ntp-status');
    if (el) el.textContent = '正在授时…';
    try {
        const res = await writeWithAuth((headers) => fetch(API.timeSync, {
            method: 'POST',
            headers,
            body: '{}',
        }));
        const text = await res.text().catch(() => '');
        if (!res.ok) {
            const msg = text || `HTTP ${res.status}`;
            if (el) el.textContent = `授时失败：${msg}`;
            log(`授时失败：${msg}`);
            return;
        }
        let body = {};
        try { body = JSON.parse(text); } catch (_) {}
        const sign = (body.offset_ms || 0) >= 0 ? '+' : '';
        if (el) el.textContent = `${body.server || '已授时'}｜偏差 ${sign}${body.offset_ms}ms`;
        log(`授时成功：${body.server}（偏差 ${body.offset_ms}ms）`);
        await refreshAll();
    } catch (e) {
        if (el) el.textContent = `授时失败：${(e && e.message) || e}`;
        log(`授时失败：${(e && e.message) || e}`);
    }
}

function fontCssValue(name) {
    return `'${name}', 'Microsoft YaHei', 'PingFang SC', sans-serif`;
}

function fillClockFontList(fonts) {
    const sel = document.getElementById('cfg-clock-font-list');
    if (!sel) return;
    const previous = sel.value;
    sel.innerHTML = '<option value="">— 从列表选择 —</option>';
    const seen = new Set();
    for (const entry of fonts) {
        const name = entry[0];
        const label = entry[1];
        if (!name || seen.has(name)) continue;
        seen.add(name);
        const o = document.createElement('option');
        o.value = name;
        o.textContent = label ? `${label} · ${name}` : name;
        sel.appendChild(o);
    }
    if (previous) sel.value = previous;
}

/// 尝试读取本机已安装字体。被拒绝（或浏览器不支持）就退回内置清单。
async function scanClockFonts() {
    if (typeof window.queryLocalFonts !== 'function') {
        log('此浏览器不允许读取系统字体列表，请手动填写字体名（内置列表里也有常见字体）');
        return;
    }
    try {
        const fonts = await window.queryLocalFonts();
        const names = new Set();
        for (const f of fonts) {
            if (f && f.family) names.add(f.family);
        }
        const arr = Array.from(names).sort().map(n => [n, '']);
        if (!arr.length) {
            log('没有读到任何系统字体（可能被拒绝授权）');
            return;
        }
        fillClockFontList(arr);
        log(`已读取 ${arr.length} 个系统字体，可从下拉中选择`);
    } catch (e) {
        log(`读取系统字体被拒绝：${(e && e.message) || e}`);
    }
}

function setupClockFontPicker() {
    fillClockFontList(CLOCK_FONT_FALLBACK);
    const sel = document.getElementById('cfg-clock-font-list');
    if (sel) {
        sel.addEventListener('change', () => {
            if (!sel.value) return;
            const input = document.getElementById('cfg-clock-font');
            if (input) input.value = fontCssValue(sel.value);
        });
    }
    const scan = document.getElementById('cfg-clock-font-scan');
    if (scan) scan.addEventListener('click', scanClockFonts);
}

function setupSettings() {
    document.getElementById('cfg-save').addEventListener('click', saveCfg);
    document.getElementById('cfg-test').addEventListener('click', testCfg);
    const refreshInputs = document.getElementById('cfg-refresh-inputs');
    if (refreshInputs) refreshInputs.addEventListener('click', loadObsInputs);
    const clockPreview = document.getElementById('cfg-clock-open');
    if (clockPreview) {
        clockPreview.addEventListener('click', () => window.open('/clock', '_blank'));
    }
    setupClockFontPicker();
    const ntpBtn = document.getElementById('cfg-ntp-sync');
    if (ntpBtn) ntpBtn.addEventListener('click', syncTimeNow);
    loadObsInputs();
}

/// Fill the target-source picker with the inputs OBS actually has. A typo in
/// this field is the #1 reason "nothing plays", so never make the user guess.
async function loadObsInputs() {
    const sel = document.getElementById('cfg-target-input');
    if (!sel) return;
    const current = sel.value || (lastSnapshot && lastSnapshot.target_input) || '';
    let inputs = [];
    let err = '';
    try {
        const r = await fetch('/api/obs/inputs');
        const j = await r.json();
        inputs = j.inputs || [];
        err = j.error || '';
    } catch (e) {
        err = String(e);
    }
    // Prefer real media sources; fall back to everything if OBS reports none.
    const media = inputs.filter(i =>
        /ffmpeg|vlc|media/i.test(i.inputKind || ''));
    const list = media.length ? media : inputs;

    sel.innerHTML = '';
    if (!list.length) {
        const o = document.createElement('option');
        o.value = current;
        o.textContent = current
            ? `${current}（未取到 OBS 来源列表${err ? '：' + err : ''}）`
            : `（未取到 OBS 来源列表${err ? '：' + err : ''}）`;
        sel.appendChild(o);
    } else {
        list.forEach(i => {
            const o = document.createElement('option');
            o.value = i.inputName;
            o.textContent = `${i.inputName} · ${i.inputKind || ''}`;
            sel.appendChild(o);
        });
        if (current && !list.some(i => i.inputName === current)) {
            const o = document.createElement('option');
            o.value = current;
            o.textContent = `${current}（OBS 中不存在！）`;
            sel.appendChild(o);
        }
    }
    if (current) sel.value = current;
}

// The engine's token is what the OBS Lua script generated on first launch;
// the admin reads it back from /api/status so every write is authorised.
function engineToken() {
    return (lastSnapshot && lastSnapshot.bootstrap_token) || '';
}

// Copy the latest engine state into the Settings form. Only overwrite fields
// the user is NOT currently editing, so typing isn't clobbered by polls.
function renderSettingsForm() {
    const snap = lastSnapshot;
    if (!snap) return;
    // Tolerate both the flattened shape and the older nested `config` one.
    const cfgObs = snap.obs_ws || (snap.config && snap.config.obs_ws) || null;
    if (!cfgObs) return;
    const active = document.activeElement;
    const setVal = (id, val, guard) => {
        if (active && active.id === id) return; // user typing in this field
        const el = document.getElementById(id);
        if (el) el.value = val;
    };
    setVal('cfg-host', cfgObs.host || '127.0.0.1');
    setVal('cfg-port', cfgObs.port != null ? cfgObs.port : 4455);
    setVal('cfg-password', cfgObs.password || '');
    const tlsEl = document.getElementById('cfg-tls');
    if (tlsEl && active && active.id !== 'cfg-tls') tlsEl.checked = !!cfgObs.tls;
    setVal('cfg-target-input', snap.target_input || (snap.config && snap.config.target_input) || '');
    // 把"引擎现在到底在用哪个源"直接写出来：选完保存后能一眼确认有没有存进去。
    const effEl = document.getElementById('cfg-target-effective');
    if (effEl) {
        const eff = snap.target_input || (snap.config && snap.config.target_input) || '';
        effEl.textContent = eff
            ? `当前生效：${eff} ✓`
            : '当前生效：（尚未设置 —— 播出时不会有画面，请从上方下拉选择后保存）';
        effEl.className = 'text-xs mt-1 ' + (eff ? 'text-emerald-400' : 'text-red');
    }
    const sc = snap.scheduler_cfg || (snap.config && snap.config.scheduler) || {};
    setVal('cfg-lead-in', sc.lead_in_ms != null ? sc.lead_in_ms : 200);
    setVal('cfg-clock-offset', sc.clock_offset_ms != null ? sc.clock_offset_ms : 0);
    setVal('cfg-missing-policy', sc.on_missing_file || 'skip_to_next');

    // 电视台报时器
    const ck = snap.clock || (snap.config && snap.config.clock) || {};
    const ckEn = document.getElementById('cfg-clock-enabled');
    if (ckEn && (!active || active.id !== 'cfg-clock-enabled')) ckEn.checked = !!ck.enabled;
    setVal('cfg-clock-font', ck.font_family || '');
    setVal('cfg-clock-size', ck.font_size_px != null ? ck.font_size_px : 72);
    setVal('cfg-clock-bg', ck.bg_opacity_percent != null ? ck.bg_opacity_percent : 45);
    setVal('cfg-clock-duration', ck.duration_s != null ? ck.duration_s : 60);
    setVal('cfg-clock-lead', ck.lead_s != null ? ck.lead_s : 30);
    setVal('cfg-clock-plate', ck.plate_style || 'pill');
    setVal('cfg-clock-position', ck.position || 'top_right');
    // 授时
    const ts = snap.time_sync || (snap.config && snap.config.time_sync) || {};
    const ntpEn = document.getElementById('cfg-ntp-enabled');
    if (ntpEn && (!active || active.id !== 'cfg-ntp-enabled')) ntpEn.checked = ts.enabled !== false;
    setVal('cfg-ntp-servers', (ts.servers || []).join(', '));
    setVal('cfg-ntp-interval', ts.interval_min != null ? ts.interval_min : 30);
    renderNtpStatus(snap.ntp);

    // 反查字体下拉：当前值正好是列表里某项生成的 CSS 时才回选，否则留空。
    const fontSel = document.getElementById('cfg-clock-font-list');
    if (fontSel && (!active || active.id !== 'cfg-clock-font-list')) {
        const current = ck.font_family || '';
        const hit = Array.from(fontSel.options)
            .find(o => o.value && fontCssValue(o.value) === current);
        fontSel.value = hit ? hit.value : '';
    }
}

async function saveCfg() {
    const token = engineToken();
    const targetEl = document.getElementById('cfg-target-input');
    const targetVal = targetEl ? String(targetEl.value || '').trim() : '';
    const payload = {
        bootstrap_token: token,
        host: document.getElementById('cfg-host').value,
        port: parseInt(document.getElementById('cfg-port').value, 10) || 4455,
        password: document.getElementById('cfg-password').value,
        tls: document.getElementById('cfg-tls').checked,
        scheduler: {
            lead_in_ms: parseInt(document.getElementById('cfg-lead-in').value, 10) || 200,
            clock_offset_ms: parseInt(document.getElementById('cfg-clock-offset').value, 10) || 0,
            on_missing_file: document.getElementById('cfg-missing-policy').value,
        },
        clock: {
            enabled: document.getElementById('cfg-clock-enabled').checked,
            font_family: document.getElementById('cfg-clock-font').value,
            font_size_px: parseInt(document.getElementById('cfg-clock-size').value, 10) || 72,
            bg_opacity_percent: Math.max(0, parseInt(document.getElementById('cfg-clock-bg').value, 10) || 0),
            duration_s: parseInt(document.getElementById('cfg-clock-duration').value, 10) || 60,
            lead_s: Math.max(0, parseInt(document.getElementById('cfg-clock-lead').value, 10) || 0),
            plate_style: document.getElementById('cfg-clock-plate').value,
            position: document.getElementById('cfg-clock-position').value,
        },
        time_sync: {
            enabled: document.getElementById('cfg-ntp-enabled').checked,
            servers: String(document.getElementById('cfg-ntp-servers').value || '')
                .split(',').map(s => s.trim()).filter(Boolean),
            interval_min: parseInt(document.getElementById('cfg-ntp-interval').value, 10) || 30,
        },
    };
    // Only send a target source when one was really picked. An empty <select>
    // (which is what you get while OBS isn't connected) used to be submitted as
    // "" and wiped a perfectly good configuration — the plugin then looked like
    // it "couldn't recognise" the media source any more.
    if (targetVal) {
        payload.target_input = targetVal;
    } else {
        log('受控媒体源这一栏是空的，已保留原来设置（不会清除）。OBS 未连接时请先连上再选。');
    }
    const status = document.getElementById('cfg-status');
    status.textContent = '保存中…';
    try {
        const res = await fetch('/api/bootstrap', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-Bootstrap-Token': token,
            },
            body: JSON.stringify(payload),
        });
        if (!res.ok) {
            const body = await res.json().catch(() => ({}));
            status.textContent = `保存失败 (HTTP ${res.status})${body.error ? '：' + body.error : ''}`;
            log(`设置保存失败: HTTP ${res.status} ${body.error || ''}`);
            return;
        }
        status.textContent = '已保存 ✓（调度器重读配置）';
        log('设置已保存');
        setTimeout(() => { status.textContent = ''; }, 4000);
        refreshAll();
    } catch (e) {
        status.textContent = `错误：${e}`;
        log(`设置保存出错：${e}`);
    }
}

async function testCfg() {
    const el = document.getElementById('cfg-status');
    el.textContent = '正在探测…';
    try {
        // Report the OBS link too — "test connection" that only proves the
        // engine is up says nothing about obs-websocket, which is the part
        // that actually fails (wrong port / password / server disabled).
        const [h, s] = await Promise.all([
            fetch(API.health).then(r => r.json()),
            fetch(API.status).then(r => r.json()),
        ]);
        const sch = s.scheduler || s.status || {};
        const obsTxt = sch.obs_connected ? 'OBS 已连接' : 'OBS 未连接';
        const why = sch.obs_error ? `（${sch.obs_error}）` : '';
        el.textContent = `引擎在线：${h.service} v${h.version}；${obsTxt}${why}`;
    } catch (e) {
        el.textContent = `不可达：${e}`;
    }
}

/* -------------------------------------------------------------------------- */
/* WebSocket live updates                                                    */
/* -------------------------------------------------------------------------- */

function startWs() {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    const url = `${proto}://${location.host}/ws`;
    try {
        ws = new WebSocket(url);
        ws.addEventListener('open', () => {
            log('WebSocket 已连接');
            const el = document.getElementById('ws-state');
            if (el) { el.textContent = 'WS 已连接'; el.className = 'ws-state ok'; }
        });
        ws.addEventListener('message', ev => {
            try {
                const msg = JSON.parse(ev.data);
                if (msg.kind === 'snapshot') {
                    // Merge, never replace: a push frame does not carry every
                    // field (older engines omit the token / obs_ws settings),
                    // and dropping the token made every write fail with 401.
                    lastSnapshot = { ...(lastSnapshot || {}), ...msg };
                    renderStatus(lastSnapshot);
                    renderDashboard(lastSnapshot);
                    renderConsole(lastSnapshot);
                    // The engine only pushes status, not the playlist. If the
                    // item count changed (another client, or the scheduler
                    // probing durations), pull the list again.
                    if (typeof msg.playlist_size === 'number'
                        && msg.playlist_size !== playlistCache.length) {
                        refreshAll();
                    } else if (!document.querySelector('[data-tab="playlist"]').classList.contains('hidden')) {
                        // Keep 等待中/播出中/已完成 fresh without re-fetching.
                        renderPlaylistRows();
                    }
                }
            } catch (_) {}
        });
        ws.addEventListener('close', () => {
            log('WebSocket 断开，1 秒后重连…');
            const el = document.getElementById('ws-state');
            if (el) { el.textContent = 'WS 断开'; el.className = 'ws-state bad'; }
            wsReconnectTimer = setTimeout(startWs, 1000);
        });
    } catch (e) {
        log(`WebSocket error: ${e}`);
    }
}

/* -------------------------------------------------------------------------- */
/* Tiny utilities                                                            */
/* -------------------------------------------------------------------------- */

function log(msg) {
    const ol = document.getElementById('dash-log');
    if (!ol) return;
    const li = document.createElement('li');
    li.className = 'flex items-start gap-2';
    const s = String(msg);
    const isErr = /失败|错误|拒绝|(HTTP\s*[45]\d\d)/i.test(s) && !/成功/.test(s);
    li.innerHTML = `<span class="text-muted">${new Date().toLocaleTimeString()}</span>` +
        `<span class="${isErr ? 'text-red' : ''}">${escapeHtml(s)}</span>`;
    ol.prepend(li);
    while (ol.children.length > 200) ol.removeChild(ol.lastChild);
}
// Cross-module log channel (used by bumpers.js etc.).
window.addEventListener('tvbs:log', (ev) => log(ev.detail));

function escapeHtml(s) {
    return String(s ?? '').replace(/[&<>"']/g, c => (
        { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
    ));
}

function cryptoRandomId() {
    if (crypto?.randomUUID) return crypto.randomUUID();
    return 'p-' + Math.random().toString(36).slice(2, 10);
}

function msToHMS(ms) {
    if (ms == null) return '--:--';
    const s = Math.floor(ms / 1000);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    return `${pad(h)}:${pad(m)}:${pad(sec)}`;
}

function formatRemaining(ms) {
    if (ms == null) return '--:--.--';
    if (ms < 0) ms = 0;
    const s = Math.floor(ms / 1000);
    const millis = ms % 1000;
    const m = Math.floor(s / 60);
    const sec = s % 60;
    return `${pad(m)}:${pad(sec)}.${pad(millis, 3)}`;
}

function pad(n, w = 2) {
    return String(n).padStart(w, '0');
}
