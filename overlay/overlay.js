// overlay/overlay.js — sync with engine /ws and update the bar.

const wsUrl = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`;
const dot = document.getElementById('o-dot');
const name = document.getElementById('o-name');
const next = document.getElementById('o-next');
const ticker = document.getElementById('o-remaining');

let playlistCache = [];

function connect() {
    const ws = new WebSocket(wsUrl);
    ws.addEventListener('open', () => console.log('[overlay] ws open'));
    ws.addEventListener('close', () => setTimeout(connect, 1000));
    ws.addEventListener('message', ev => {
        try {
            const msg = JSON.parse(ev.data);
            if (msg.kind === 'snapshot') apply(msg);
        } catch {}
    });
}

async function bootstrap() {
    try {
        const r = await fetch('/api/playlist');
        const p = await r.json();
        playlistCache = p.items || [];
    } catch {}
}

function apply(msg) {
    const st = msg.scheduler || {};
    const onAir = st.scheduler_state === 'Playing' ||
                  st.scheduler_state === 'Interstitial';
    dot.classList.toggle('idle', !onAir);

    name.textContent = st.current_program_name || '— 待机 —';
    ticker.textContent = formatRemaining(st.current_remaining_ms);

    const upcoming = pickUpcoming();
    next.textContent = upcoming ? upcoming.name : '—';
}

function pickUpcoming() {
    const now = Date.now();
    return playlistCache
        .filter(p => p.start_at_ms > now)
        .sort((a, b) => a.start_at_ms - b.start_at_ms)[0];
}

function formatRemaining(ms) {
    if (ms == null || ms < 0) return '--:--:--';
    const s = Math.floor(ms / 1000);
    const m = Math.floor(s / 60);
    const sec = s % 60;
    return `${pad(m)}:${pad(sec)}`;
}

function pad(n) { return String(n).padStart(2, '0'); }

bootstrap();
connect();
