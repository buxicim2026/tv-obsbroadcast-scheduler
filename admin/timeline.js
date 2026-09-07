// admin/timeline.js — 24h horizontal timeline view.

let cachedItems = [];
let dayStart = startOfDay(Date.now());

function startOfDay(ms) {
    const d = new Date(ms);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
}

export function initTimeline() {
    const tl = document.getElementById('timeline-canvas');
    const hours = document.getElementById('timeline-hours');
    if (hours && !hours.children.length) {
        for (let i = 0; i < 24; i++) {
            const span = document.createElement('span');
            span.className = 'text-center text-muted text-xs border-l border-border';
            span.textContent = String(i).padStart(2, '0');
            hours.appendChild(span);
        }
    }
    if (tl) {
        tl.addEventListener('click', () => {
            dayStart = startOfDay(Date.now());
            renderTimeline(cachedItems);
        });
    }
    document.getElementById('btn-tl-prev-day')?.addEventListener('click', () => {
        dayStart -= 24 * 60 * 60 * 1000;
        renderTimeline(cachedItems);
    });
    document.getElementById('btn-tl-today')?.addEventListener('click', () => {
        dayStart = startOfDay(Date.now());
        renderTimeline(cachedItems);
    });
    document.getElementById('btn-tl-next-day')?.addEventListener('click', () => {
        dayStart += 24 * 60 * 60 * 1000;
        renderTimeline(cachedItems);
    });
}

export function renderTimeline(items) {
    cachedItems = items || [];
    const tl = document.getElementById('timeline-canvas');
    const now = document.getElementById('timeline-now');
    if (!tl) return;
    // clear old blocks
    tl.querySelectorAll('.tl-block').forEach(el => el.remove());

    const dayEnd = dayStart + 24 * 60 * 60 * 1000;
    cachedItems.forEach(p => {
        const startMs = p.start_at_ms;
        const endMs = startMs + p.declared_duration_ms;
        if (endMs <= dayStart || startMs >= dayEnd) return;
        const left = Math.max(0, ((startMs - dayStart) / (24 * 60 * 60 * 1000)) * 100);
        const width = Math.max(
            0.4,
            ((Math.min(endMs, dayEnd) - Math.max(startMs, dayStart)) /
                (24 * 60 * 60 * 1000)) * 100
        );
        const block = document.createElement('div');
        block.className = `tl-block tl-${p.kind}`;
        block.style.left = `${left}%`;
        block.style.width = `${width}%`;
        block.title = `${p.name}  ·  ${new Date(startMs).toLocaleTimeString()}  ·  ${msToHMS(p.declared_duration_ms)}`;
        block.textContent = p.name;
        block.addEventListener('click', () => {
            // jump-to-program (stub)
        });
        tl.appendChild(block);
    });

    // now-line
    const nowMs = Date.now();
    if (nowMs >= dayStart && nowMs <= dayEnd) {
        now.style.left = `${((nowMs - dayStart) / (24 * 60 * 60 * 1000)) * 100}%`;
        now.style.display = 'block';
    } else {
        now.style.display = 'none';
    }
    // refresh every 30s so the now-line moves.
    if (!window.__TL_TICK__) {
        window.__TL_TICK__ = setInterval(() => renderTimeline(cachedItems), 30_000);
    }
}

function msToHMS(ms) {
    const s = Math.floor(ms / 1000);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    return `${pad(h)}:${pad(m)}:${pad(sec)}`;
}
function pad(n) { return String(n).padStart(2, '0'); }
