// admin/app.js — frontend SPA for tv-obsbroadcast-scheduler.
//
// Vanilla ES module. Tabs (Dashboard / Playlist / Timeline / Settings) are
// switched via the top nav. A live WebSocket keeps the dashboard in sync
// with the engine; mutated settings go back via REST.

import { initTimeline, renderTimeline } from '/admin/timeline.js';
import { renderBumpers } from '/admin/bumpers.js';

const API = {
    status: '/api/status',
    playlist: '/api/playlist',
    playlistItem: '/api/playlist/item',
    enable: '/api/scheduler/enable',
    health: '/healthz',
};

const PROGRAM_KIND_LABEL = {
    primary: 'Primary',
    interstitial: 'Interstitial',
    standalone: 'Standalone',
};

let ws = null;
let wsReconnectTimer = null;
let lastSnapshot = null;
let playlistCache = [];

/* -------------------------------------------------------------------------- */
/* Boot                                                                      */
/* -------------------------------------------------------------------------- */

window.addEventListener('DOMContentLoaded', () => {
    setupTabs();
    setupDashboardActions();
    setupPlaylistAdd();
    setupSettings();
    initTimeline();
    refreshAll();
    startWs();
});

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
            if (tab === 'timeline') renderTimeline(playlistCache);
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
        renderStatus(status);
        renderDashboard(status);
        renderSettingsForm();
        renderPlaylistRows();
        renderBumpers(playlist.bumpers || []);
        if (!document.querySelector('[data-tab="timeline"]').classList.contains('hidden')) {
            renderTimeline(playlistCache);
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

async function apiPost(url, body) {
    const res = await fetch(url, {
        method: 'POST',
        headers: authHeaders(),
        body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json().catch(() => ({}));
}

async function apiDelete(url, body) {
    const res = await fetch(url, {
        method: 'DELETE',
        headers: authHeaders(),
        body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json().catch(() => ({}));
}

/* -------------------------------------------------------------------------- */
/* Dashboard                                                                 */
/* -------------------------------------------------------------------------- */

function renderDashboard(s) {
    const sch = s.scheduler || {};
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

function clearUpcoming() {
    document.getElementById('dash-next-name').textContent = '—';
    document.getElementById('dash-next-path').textContent = '—';
    document.getElementById('dash-next-kind').textContent = '—';
    document.getElementById('dash-next-start').textContent = '--:--:--';
    document.getElementById('dash-next-duration').textContent = '--:--';
}

function setupDashboardActions() {
    document.getElementById('btn-armed').addEventListener('click', toggleArmed);
    document.getElementById('btn-pause').addEventListener('click', () => {
        log('暂停功能将在下一版本接通（引擎侧 /api/scheduler/pause）');
    });
    document.getElementById('btn-next').addEventListener('click', () => {
        log('跳到下一档将在下一版本接通（引擎侧 /api/scheduler/next）');
    });
    document.getElementById('btn-restart').addEventListener('click', async () => {
        const b = document.getElementById('btn-restart');
        b.disabled = true;
        b.textContent = '刷新中…';
        await refreshAll();
        b.disabled = false;
        b.textContent = '重新载入';
    });
    document.getElementById('btn-clear-log').addEventListener('click', () => {
        document.getElementById('dash-log').innerHTML = '';
    });
    document.getElementById('btn-manage-bumpers').addEventListener('click', () => {
        document.querySelector('.nav-tab[data-tab="timeline"]')?.click();
    });
}

async function toggleArmed() {
    const enable = !(lastSnapshot?.scheduler?.scheduler_running);
    try {
        await apiPost(API.enable, { enabled: enable });
        log(`已${enable ? '启用' : '停用'}自动播出`);
        await refreshAll();
    } catch (e) {
        log(`toggleArmed failed: ${e}`);
    }
}

/* -------------------------------------------------------------------------- */
/* Status pills (header right)                                               */
/* -------------------------------------------------------------------------- */

function renderStatus(s) {
    const eng = document.getElementById('status-eng');
    const obs = document.getElementById('status-obs');
    const cfg = document.getElementById('status-cfg');
    eng.textContent = s.scheduler ? `引擎 ${s.scheduler.scheduler_state || 'Idle'}` : '引擎 无响应';
    eng.className = `pill ${s.scheduler?.obs_connected ? 'pill-ok' : 'pill-warn'}`;
    obs.textContent = `OBS ${s.scheduler?.obs_connected ? '已连' : '未连'}`;
    obs.className = `pill ${s.scheduler?.obs_connected ? 'pill-ok' : 'pill-warn'}`;
    cfg.textContent = s.target_input
        ? `目标: ${s.target_input}`
        : 'OBS-WS 待配置';
    cfg.className = `pill ${s.target_input ? 'pill-ok' : 'pill-muted'}`;
}

/* -------------------------------------------------------------------------- */
/* Playlist                                                                  */
/* -------------------------------------------------------------------------- */

function setupPlaylistAdd() {
    document.getElementById('btn-add-program').addEventListener('click', async () => {
        const name = document.getElementById('playlist-add-name').value.trim();
        const path = document.getElementById('playlist-add-path').value.trim();
        if (!name || !path) {
            log('新增节目需要名称和路径');
            return;
        }
        try {
            await apiPost(API.playlistItem, {
                id: cryptoRandomId(),
                name,
                file_path: path,
                start_at_ms: Date.now() + 60_000, /* default +60s */
                declared_duration_ms: 30 * 60 * 1000,
                kind: 'primary',
            });
            log(`已新增：${name}`);
            document.getElementById('playlist-add-name').value = '';
            document.getElementById('playlist-add-path').value = '';
            await refreshAll();
        } catch (e) {
            log(`add failed: ${e}`);
        }
    });

    // Bulk-import a playlist file (JSON or TSV/TXT). Kept local — nothing is
    // uploaded to a server, rows are upserted one-by-one through the same
    // authenticated REST endpoint the "+" button uses.
    const importBtn = document.getElementById('btn-import-open');
    const fileInput = document.getElementById('playlist-import');
    if (importBtn && fileInput) {
        importBtn.addEventListener('click', () => fileInput.click());
        fileInput.addEventListener('change', async () => {
            const file = fileInput.files && fileInput.files[0];
            fileInput.value = '';
            if (!file) return;
            try {
                const text = await file.text();
                const rows = parsePlaylistFile(file.name, text);
                if (!rows.length) {
                    log(`导入文件「${file.name}」里没解析到任何节目行`);
                    return;
                }
                let ok = 0;
                const base = Date.now();
                for (let i = 0; i < rows.length; i++) {
                    const r = rows[i];
                    // If the file does not provide absolute start times, lay
                    // the rows back-to-back from +60 s so the playlist is
                    // immediately drivable.
                    const start = r.start_at_ms != null
                        ? r.start_at_ms
                        : base + 60_000 + i * (r.declared_duration_ms || 30 * 60 * 1000);
                    try {
                        await apiPost(API.playlistItem, {
                            id: cryptoRandomId(),
                            name: r.name,
                            file_path: r.file_path,
                            start_at_ms: start,
                            declared_duration_ms: r.declared_duration_ms || 30 * 60 * 1000,
                            kind: r.kind || 'primary',
                        });
                        ok++;
                    } catch (e) {
                        log(`导入第 ${i + 1} 条失败（${r.name}）：${e}`);
                    }
                }
                log(`✅ 导入完成：成功 ${ok}/${rows.length} 条（${file.name}）`);
                await refreshAll();
            } catch (e) {
                log(`导入失败：${e}`);
            }
        });
    }
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

function renderPlaylistRows() {
    const tbody = document.getElementById('playlist-tbody');
    tbody.innerHTML = '';
    playlistCache.forEach((p, i) => {
        const tr = document.createElement('tr');
        tr.className = 'hover:bg-surface-2 transition';
        tr.innerHTML = `
            <td class="py-2 pr-2 font-mono text-muted">${i + 1}</td>
            <td class="py-2 pr-2">${escapeHtml(p.name)}</td>
            <td class="py-2 pr-2 font-mono text-xs truncate max-w-md">${escapeHtml(p.file_path)}</td>
            <td class="py-2 pr-2 text-xs">
                <span class="legend legend-${p.kind}"></span>
                ${PROGRAM_KIND_LABEL[p.kind] || p.kind}
            </td>
            <td class="py-2 pr-2 font-mono text-xs">${new Date(p.start_at_ms).toLocaleTimeString()}</td>
            <td class="py-2 pr-2 font-mono text-xs">${msToHMS(p.declared_duration_ms)}</td>
            <td class="py-2 pr-2 font-mono text-xs text-muted">${p.detected_duration_ms ? msToHMS(p.detected_duration_ms) : '—'}</td>
            <td class="py-2 pr-2 text-right">
                <button class="btn btn-link" data-act="up" data-id="${p.id}">↑</button>
                <button class="btn btn-link" data-act="down" data-id="${p.id}">↓</button>
                <button class="btn btn-link text-red" data-act="del" data-id="${p.id}">删除</button>
            </td>`;
        tbody.appendChild(tr);
    });
    tbody.querySelectorAll('button[data-act="del"]').forEach(b => {
        b.addEventListener('click', async () => {
            await apiDelete(API.playlistItem, { id: b.dataset.id });
            await refreshAll();
        });
    });
}

/* -------------------------------------------------------------------------- */
/* Settings                                                                  */
/* -------------------------------------------------------------------------- */

function setupSettings() {
    document.getElementById('cfg-save').addEventListener('click', saveCfg);
    document.getElementById('cfg-test').addEventListener('click', testCfg);
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
    if (!snap || !snap.obs_ws) return;
    const active = document.activeElement;
    const setVal = (id, val, guard) => {
        if (active && active.id === id) return; // user typing in this field
        const el = document.getElementById(id);
        if (el) el.value = val;
    };
    setVal('cfg-host', snap.obs_ws.host || '127.0.0.1');
    setVal('cfg-port', snap.obs_ws.port != null ? snap.obs_ws.port : 4455);
    setVal('cfg-password', snap.obs_ws.password || '');
    const tlsEl = document.getElementById('cfg-tls');
    if (tlsEl && active && active.id !== 'cfg-tls') tlsEl.checked = !!snap.obs_ws.tls;
    setVal('cfg-target-input', snap.target_input || '');
    const sc = snap.scheduler_cfg || {};
    setVal('cfg-lead-in', sc.lead_in_ms != null ? sc.lead_in_ms : 200);
    setVal('cfg-clock-offset', sc.clock_offset_ms != null ? sc.clock_offset_ms : 0);
    setVal('cfg-missing-policy', sc.on_missing_file || 'skip_to_next');
}

async function saveCfg() {
    const token = engineToken();
    const payload = {
        bootstrap_token: token,
        host: document.getElementById('cfg-host').value,
        port: parseInt(document.getElementById('cfg-port').value, 10) || 4455,
        password: document.getElementById('cfg-password').value,
        tls: document.getElementById('cfg-tls').checked,
        target_input: document.getElementById('cfg-target-input').value,
        scheduler: {
            lead_in_ms: parseInt(document.getElementById('cfg-lead-in').value, 10) || 200,
            clock_offset_ms: parseInt(document.getElementById('cfg-clock-offset').value, 10) || 0,
            on_missing_file: document.getElementById('cfg-missing-policy').value,
        },
    };
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
    document.getElementById('cfg-status').textContent = '正在探测…';
    try {
        const r = await fetch(API.health);
        const j = await r.json();
        document.getElementById('cfg-status').textContent =
            `引擎在线：${j.service} v${j.version}`;
    } catch (e) {
        document.getElementById('cfg-status').textContent = `不可达：${e}`;
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
        ws.addEventListener('open', () => log('WebSocket 已连接'));
        ws.addEventListener('message', ev => {
            try {
                const msg = JSON.parse(ev.data);
                if (msg.kind === 'snapshot') {
                    lastSnapshot = msg;
                    renderStatus(msg);
                    renderDashboard(msg);
                }
            } catch (_) {}
        });
        ws.addEventListener('close', () => {
            log('WebSocket 断开，1 秒后重连…');
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
    const isErr = /失败|错误|拒绝|401|500|not|err/i.test(s) && !/成功/.test(s);
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
