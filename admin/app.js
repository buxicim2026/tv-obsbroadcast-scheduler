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
        ['主控台按钮', setupDashboardActions],
        ['节目表', setupPlaylistAdd],
        ['设置', setupSettings],
        ['时间轴', initTimeline],
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
            if (tab === 'timeline') renderTimeline(timelineItems());
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
        renderSettingsForm();
        renderPlaylistRows();
        renderBumpers(bumpersCache);
        if (!document.querySelector('[data-tab="timeline"]').classList.contains('hidden')) {
            renderTimeline(timelineItems());
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
    // Old engines returned the runtime state under `status`; the current
    // engine (and the /ws snapshot) uses `scheduler`. Accept both.
    const sch = s.scheduler || s.status || {};
    const eng = document.getElementById('status-eng');
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
    let start = Date.now() + 60_000;
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

// Browsers never expose a file's absolute path, so the user tells us the
// folder once (remembered locally) and we join it with the picked file name.
function mediaRootInput() {
    return document.getElementById('media-root');
}
function mediaPathFor(file) {
    const rootEl = mediaRootInput();
    const root = (rootEl && rootEl.value || '').trim();
    const rel = file.webkitRelativePath || file.name;
    if (!root) return rel;
    const sep = /[\\/]$/.test(root) ? '' : (root.includes('\\') ? '\\' : '/');
    return root + sep + rel;
}
function hasMediaRoot() {
    const rootEl = mediaRootInput();
    return !!(rootEl && rootEl.value.trim());
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
            document.getElementById('playlist-add-name')?.focus();
        } else if (k === 's') {
            e.preventDefault();
            saveCfg();
        }
    });

    // Remember the media folder between sessions.
    const rootEl = mediaRootInput();
    if (rootEl) {
        try { rootEl.value = localStorage.getItem('tvbs.mediaRoot') || ''; } catch (_) {}
        rootEl.addEventListener('change', () => {
            try { localStorage.setItem('tvbs.mediaRoot', rootEl.value.trim()); } catch (_) {}
        });
    }

    document.getElementById('btn-add-program').addEventListener('click', async () => {
        const nameEl = document.getElementById('playlist-add-name');
        const pathEl = document.getElementById('playlist-add-path');
        const name = (nameEl && nameEl.value || '').trim();
        const path = (pathEl && pathEl.value || '').trim();
        if (!name || !path) {
            setPlaylistStatus('请填写节目名称和文件路径（或点「选择文件」挑一个视频）', 'err');
            return;
        }
        setPlaylistStatus('正在新增…');
        try {
            const dur = defaultDurationMs();
            await addProgram(name, path, nextStartAt(dur), dur, selectedKind());
            if (nameEl) nameEl.value = '';
            if (pathEl) pathEl.value = '';
            setPlaylistStatus(`✅ 已新增「${name}」`, 'ok');
            log(`已新增：${name}`);
            await refreshAll();
        } catch (e) {
            const msg = friendlyWriteError(e);
            setPlaylistStatus(`新增失败：${msg}`, 'err');
            log(`新增失败：${msg}`);
        }
    });

    // Pick a single media file to fill the form.
    const pickBtn = document.getElementById('btn-pick-media');
    const mediaInput = document.getElementById('media-file-input');
    let pickMode = 'fill';
    if (pickBtn && mediaInput) {
        pickBtn.addEventListener('click', () => { pickMode = 'fill'; mediaInput.click(); });
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

            if (pickMode === 'fill') {
                const f = files[0];
                const nameEl = document.getElementById('playlist-add-name');
                const pathEl = document.getElementById('playlist-add-path');
                if (nameEl && !nameEl.value.trim()) {
                    nameEl.value = f.name.replace(/\.[^.]+$/, '');
                }
                if (pathEl) pathEl.value = mediaPathFor(f);
                setPlaylistStatus(
                    hasMediaRoot()
                        ? `已选择：${f.name}（路径已填好，点「新增节目」即可）`
                        : `已选择：${f.name}。请先在旁边的框里填好文件所在目录，路径才是完整的`,
                    hasMediaRoot() ? 'ok' : 'err'
                );
                return;
            }

            // Batch: one program per selected media file, laid back-to-back.
            setPlaylistStatus(`正在导入 ${files.length} 个媒体文件…`);
            const dur = defaultDurationMs();
            const base = nextStartAt(dur);
            let ok = 0;
            for (let i = 0; i < files.length; i++) {
                const f = files[i];
                try {
                    await addProgram(
                        f.name.replace(/\.[^.]+$/, ''),
                        mediaPathFor(f),
                        base + i * dur,
                        dur,
                        selectedKind()
                    );
                    ok++;
                } catch (e) {
                    log(`导入「${f.name}」失败：${friendlyWriteError(e)}`);
                }
            }
            const tail = hasMediaRoot() ? '' : '（未填媒体目录，路径可能不完整）';
            setPlaylistStatus(`✅ 已导入 ${ok}/${files.length} 个文件${tail}`, ok ? 'ok' : 'err');
            log(`媒体文件导入：成功 ${ok}/${files.length}${tail}`);
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

function renderPlaylistRows() {
    const tbody = document.getElementById('playlist-tbody');
    tbody.innerHTML = '';

    if (!playlistCache.length) {
        const tr = document.createElement('tr');
        tr.innerHTML = '<td colspan="8" class="py-4 text-center text-muted text-xs">' +
            '还没有节目。用上方「新增节目」或「📁 选择文件」添加，也可以用「导入节目表文件」批量导入。</td>';
        tbody.appendChild(tr);
        return;
    }

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
                <button class="btn btn-link" data-act="up" data-id="${p.id}"
                        ${i === 0 ? 'disabled' : ''} title="上移">↑</button>
                <button class="btn btn-link" data-act="down" data-id="${p.id}"
                        ${i === playlistCache.length - 1 ? 'disabled' : ''} title="下移">↓</button>
                <button class="btn btn-link text-red" data-act="del" data-id="${p.id}">删除</button>
            </td>`;
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
    const sc = snap.scheduler_cfg || (snap.config && snap.config.scheduler) || {};
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
        ws.addEventListener('open', () => log('WebSocket 已连接'));
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
                    // The engine only pushes status, not the playlist. If the
                    // item count changed (another client, or the scheduler
                    // probing durations), pull the list again.
                    if (typeof msg.playlist_size === 'number'
                        && msg.playlist_size !== playlistCache.length) {
                        refreshAll();
                    }
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
