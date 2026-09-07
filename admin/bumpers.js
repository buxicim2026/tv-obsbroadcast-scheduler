// admin/bumpers.js — dashboard widgets for bumpers + a "trigger now" button.

export function renderBumpers(bumpers) {
    const list = document.getElementById('dash-bumpers-list');
    if (!list) return;
    list.innerHTML = '';
    if (!bumpers.length) {
        const empty = document.createElement('div');
        empty.className = 'text-muted text-sm col-span-full';
        empty.innerHTML =
            '暂无插播条目，先在 <button class="btn btn-link" data-jump-tab="timeline">时间轴</button> 或 <button class="btn btn-link" data-jump-tab="playlist">节目表</button> 添加。';
        list.appendChild(empty);
        list.querySelectorAll('[data-jump-tab]').forEach(b => {
            b.addEventListener('click', () => {
                const tab = b.dataset.jumpTab;
                document.querySelector(`.nav-tab[data-tab="${tab}"]`)?.click();
            });
        });
        return;
    }
    bumpers.forEach(b => {
        const card = document.createElement('button');
        card.className = 'text-left bg-bg border border-border rounded-lg p-3 hover:border-red transition';
        card.innerHTML = `
            <div class="font-bold text-sm mb-1">${escapeHtml(b.content.name)}</div>
            <div class="text-xs text-muted">@ +${(b.at_into_program_ms / 1000).toFixed(1)}s · ${msToHM(b.content.declared_duration_ms)}</div>`;
        card.addEventListener('click', () => {
            // trigger bumper via REST (stub — endpoint added by `c-plugin-properties` todo)
            alert('插播触发：' + b.content.name + '\n（实际调 POST /api/bumper/<id>/fire）');
        });
        list.appendChild(card);
    });
}

function msToHM(ms) {
    const s = Math.floor(ms / 1000);
    const m = Math.floor(s / 60);
    const sec = s % 60;
    return `${m}m${sec}s`;
}

function escapeHtml(s) {
    return String(s ?? '').replace(/[&<>"']/g, c => (
        { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
    ));
}
