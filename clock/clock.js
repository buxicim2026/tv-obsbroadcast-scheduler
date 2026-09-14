// clock/clock.js — station clock (整点 / 半点报时).
//
// Deliberately self-contained: everything visual is driven by the config that
// comes over /ws, so OBS keeps rendering even if the admin page is closed.
//
// Why second-precision works here: the engine pushes config, but the countdown
// itself is computed locally from the wall clock on every animation frame. A
// once-a-second WebSocket tick would drift visibly against the video.

const cfg = {
    enabled: false,
    font_family: '',
    font_size_px: 72,
    bg_opacity_percent: 45,
    duration_s: 60,
    position: 'top_right',
};

const POSITIONS = [
    'top_left', 'top_right', 'bottom_left', 'bottom_right',
    'top_center', 'bottom_center',
];

const slot = document.getElementById('clock-slot');
const plate = document.getElementById('clock-plate');
const timeEl = document.getElementById('clock-time');
const labelEl = document.getElementById('clock-label');
const offEl = document.getElementById('clock-off');

// Seconds past the last trigger point. The clock shows itself during the first
// `duration_s` seconds of every hour and every half hour.
function secondsIntoWindow(now = new Date()) {
    const m = now.getMinutes();
    const s = now.getSeconds();
    const secsPastHour = m * 60 + s;
    const anchor = secsPastHour >= 1800 ? 1800 : 0; // :00 or :30
    return secsPastHour - anchor;
}

function pad(n) {
    return String(n).padStart(2, '0');
}

function applyConfig(c) {
    if (!c) return;
    Object.assign(cfg, c);
    if (!POSITIONS.includes(cfg.position)) cfg.position = 'top_right';

    // Reset every corner class then set the one we want.
    POSITIONS.forEach(p => slot.classList.remove(p));
    slot.classList.add(cfg.position);

    if (cfg.font_family) {
        plate.style.fontFamily = cfg.font_family;
    }
    const size = Math.max(12, Math.min(400, cfg.font_size_px || 72));
    timeEl.style.fontSize = `${size}px`;
    labelEl.style.fontSize = `${Math.max(11, Math.round(size * 0.28))}px`;

    const op = Math.max(0, Math.min(100, cfg.bg_opacity_percent ?? 45));
    plate.style.background = op === 0
        ? 'transparent'
        : `rgba(8, 10, 14, ${(op / 100).toFixed(2)})`;
    plate.style.borderColor = op === 0
        ? 'transparent'
        : `rgba(255, 255, 255, ${Math.min(0.35, op / 220).toFixed(2)})`;

    offEl.hidden = cfg.enabled;
    if (!cfg.enabled) slot.hidden = true;
}

function tick() {
    if (!cfg.enabled) {
        slot.hidden = true;
        return;
    }
    const now = new Date();
    const into = secondsIntoWindow(now);
    const show = into < Math.max(5, cfg.duration_s);
    slot.hidden = !show;
    if (!show) return;

    timeEl.textContent =
        `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

async function loadConfig() {
    try {
        const r = await fetch('/api/status');
        const s = await r.json();
        if (s.clock) applyConfig(s.clock);
    } catch (_) {}
}

function connect() {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.addEventListener('message', ev => {
        try {
            const msg = JSON.parse(ev.data);
            if (msg.kind === 'snapshot' && msg.clock) applyConfig(msg.clock);
        } catch (_) {}
    });
    ws.addEventListener('close', () => setTimeout(connect, 1000));
}

loadConfig();
connect();
// 10Hz is plenty for a wall clock and keeps the browser source cheap.
setInterval(tick, 100);
tick();
