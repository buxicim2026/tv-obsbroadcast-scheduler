// clock/clock.js — station clock (整点 / 半点报时).
//
// Two things this file is careful about:
//
// 1. It never uses `hidden`/`display:none`. An OBS browser source keeps the
//    last frame it painted, so a page that stops drawing appears frozen — the
//    clock used to sit on its final second forever. Visibility is opacity, and
//    we keep painting even while invisible.
//
// 2. The window is centred on the hour: with the default settings the display
//    runs 11:59:30 → 12:00:30, counting the channel into the news, rather than
//    starting only once the hour has already struck.

const cfg = {
    enabled: false,
    font_family: '',
    font_size_px: 72,
    bg_opacity_percent: 45,
    duration_s: 60,
    lead_s: 30,
    plate_style: 'pill',
    position: 'top_right',
};

const POSITIONS = [
    'top_left', 'top_right', 'bottom_left', 'bottom_right',
    'top_center', 'bottom_center',
];
const PLATE_STYLES = ['pill', 'rounded', 'rect', 'none'];

/// Which segments are lit for each digit (a=top, b=top-right, c=bottom-right,
/// d=bottom, e=bottom-left, f=top-left, g=middle).
const SEGMENTS = {
    '0': 'abcdef',
    '1': 'bc',
    '2': 'abdeg',
    '3': 'abcdg',
    '4': 'bcfg',
    '5': 'acdfg',
    '6': 'acdefg',
    '7': 'abc',
    '8': 'abcdefg',
    '9': 'abcdfg',
};

/// Correction (ms) from the NTP sync against 国家授时中心. Applied to the
/// machine clock, so a PC whose time is wrong still strikes the hour on time.
let ntpOffsetMs = 0;

const slot = document.getElementById('clock-slot');
const plate = document.getElementById('clock-plate');
const digits = document.getElementById('clock-digits');
const offEl = document.getElementById('clock-off');

/* ------------------------------------------------------------------ DOM --- */

/// One seven-segment digit plus the two leading zeros: the layout never
/// changes, only which segments light up, so there is no reflow while ticking.
let digitEls = [];
let colonEls = [];

function buildDigits() {
    digits.innerHTML = '';
    digitEls = [];
    colonEls = [];
    for (let i = 0; i < 8; i++) {
        if (i === 2 || i === 4) {
            const colon = document.createElement('div');
            colon.className = 'seg-colon';
            colon.innerHTML = '<i></i><i></i>';
            digits.appendChild(colon);
            colonEls.push(colon);
            continue;
        }
        const d = document.createElement('div');
        d.className = 'seg-digit';
        for (const name of 'abcdefg') {
            const seg = document.createElement('i');
            seg.className = `seg ${name}`;
            d.appendChild(seg);
        }
        digits.appendChild(d);
        digitEls.push(d);
    }
}

function paint(text) {
    // text is "HH:MM:SS"
    const chars = text.split('');
    let digitIndex = 0;
    for (const ch of chars) {
        if (ch === ':') continue;
        const el = digitEls[digitIndex++];
        if (!el) continue;
        const lit = SEGMENTS[ch] || '';
        for (const name of 'abcdefg') {
            const seg = el.querySelector(`.seg.${name}`);
            if (seg) seg.classList.toggle('on', lit.includes(name));
        }
    }
}

/* --------------------------------------------------------------- timing --- */

/// Seconds past the hour, with sub-second precision so the last tick before a
/// window closes is exact.
function secondsIntoHour(now) {
    return now.getMinutes() * 60 + now.getSeconds() + now.getMilliseconds() / 1000;
}

/// Is the clock due right now? Anchors are :00 and :30 — and because the lead
/// can push the window before the anchor, we also check the *next* hour's :00
/// so 11:59:30 lights up instead of waiting for midnight-style rollover.
function windowState(now) {
    const secs = secondsIntoHour(now);
    const lead = Math.max(0, cfg.lead_s);
    const total = Math.max(5, cfg.duration_s);
    for (const anchor of [0, 1800, 3600]) {
        const offset = secs - anchor; // negative = anchor still ahead
        if (offset >= -lead && offset < total - lead) {
            return { show: true, remaining: total - lead - offset };
        }
    }
    return { show: false };
}

function pad(n, width) {
    return String(n).padStart(width, '0');
}

/* -------------------------------------------------------------- painting --- */

function applyConfig(c) {
    if (!c) return;
    Object.assign(cfg, c);

    if (!POSITIONS.includes(cfg.position)) cfg.position = 'top_right';
    POSITIONS.forEach(p => slot.classList.remove(p));
    slot.classList.add(cfg.position);

    let style = String(cfg.plate_style || 'pill').toLowerCase();
    if (!PLATE_STYLES.includes(style)) style = 'pill';
    PLATE_STYLES.forEach(s => plate.classList.remove(s));
    plate.classList.add(style);

    const size = Math.max(12, Math.min(400, cfg.font_size_px || 72));
    plate.style.fontSize = `${size}px`;
    if (cfg.font_family) plate.style.fontFamily = cfg.font_family;

    const op = Math.max(0, Math.min(100, cfg.bg_opacity_percent ?? 45));
    plate.style.setProperty('--plate-opacity', String(op / 100));

    offEl.hidden = !!cfg.enabled;
}

let lastPaint = '';
let lastVisible = null;

function tick() {
    const now = new Date(Date.now() + ntpOffsetMs);
    if (!cfg.enabled) {
        if (lastVisible !== false) {
            slot.classList.remove('visible');
            lastVisible = false;
        }
        return;
    }

    const { show } = windowState(now);
    // Visibility through opacity, and the text keeps updating while hidden so
    // the page always has something fresh to paint.
    if (show !== lastVisible) {
        slot.classList.toggle('visible', show);
        lastVisible = show;
    }

    const text = `${pad(now.getHours(), 2)}:${pad(now.getMinutes(), 2)}:${pad(now.getSeconds(), 2)}`;
    if (text !== lastPaint) {
        lastPaint = text;
        paint(text);
    }
}

/* ------------------------------------------------------------------ boot --- */

function applyNtp(n) {
    if (!n) return;
    if (typeof n.offset_ms === 'number') ntpOffsetMs = n.offset_ms;
}

async function loadConfig() {
    try {
        const r = await fetch('/api/status');
        const s = await r.json();
        if (s.clock) applyConfig(s.clock);
        applyNtp(s.ntp);
    } catch (_) {}
}

function connect() {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.addEventListener('message', ev => {
        try {
            const msg = JSON.parse(ev.data);
            if (msg.kind !== 'snapshot') return;
            if (msg.clock) applyConfig(msg.clock);
            applyNtp(msg.ntp);
        } catch (_) {}
    });
    ws.addEventListener('close', () => setTimeout(connect, 1000));
}

buildDigits();
loadConfig();
connect();
// 100ms keeps the seconds honest without giving the browser source any reason
// to throttle; the wall clock itself comes from the machine, not from the WS.
setInterval(tick, 100);
tick();
