// ── Theme management ─────────────────────────────────────────────────────────
// Cycles: system → light → dark → system
// <html class="dark"> is the hook; CSS in app.css does the rest.

const THEME_ICONS = { light: '☀️', dark: '🌙', system: '💻' };
const THEME_TITLES = { light: 'Light mode', dark: 'Dark mode', system: 'System theme' };

function applyTheme(theme) {
  const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  const useDark = theme === 'dark' || (theme === 'system' && prefersDark);
  document.documentElement.classList.toggle('dark', useDark);
  localStorage.setItem('theme', theme);
  const btn = document.getElementById('theme-toggle');
  if (btn) {
    btn.textContent = THEME_ICONS[theme] || '💻';
    btn.title = THEME_TITLES[theme] || 'Toggle theme';
  }
}

function cycleTheme() {
  const current = localStorage.getItem('theme') || 'system';
  applyTheme({ system: 'light', light: 'dark', dark: 'system' }[current] || 'light');
}

function initTheme() {
  applyTheme(localStorage.getItem('theme') || 'system');
}

// Re-apply if OS preference changes while the page is open
window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
  if ((localStorage.getItem('theme') || 'system') === 'system') applyTheme('system');
});

// ── API helper ────────────────────────────────────────────────────────────────
const API = '/api';

async function apiFetch(method, path, body) {
  const opts = { method, headers: { 'Content-Type': 'application/json' } };
  if (body !== undefined) opts.body = JSON.stringify(body);
  const res = await fetch(API + path, opts);
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error || 'Request failed');
  }
  return res.status === 204 ? null : res.json();
}

// ── Toast notifications ───────────────────────────────────────────────────────
function showToast(message, kind = 'info') {
  const container = document.getElementById('toast-container');
  if (!container) return;
  const bg = { success: 'bg-green-600', error: 'bg-red-600', info: 'bg-blue-600' };
  const el = document.createElement('div');
  el.className = `toast px-4 py-2 rounded shadow-lg text-white text-sm font-medium ${bg[kind] || bg.info}`;
  el.textContent = message;
  container.appendChild(el);
  setTimeout(() => { el.style.transition = 'opacity 0.3s'; el.style.opacity = '0'; }, 3200);
  setTimeout(() => el.remove(), 3500);
}

// ── Confirm + execute ─────────────────────────────────────────────────────────
async function confirmAction(message, fn) {
  if (!confirm(message)) return;
  try {
    await fn();
    window.location.reload();
  } catch (e) {
    showToast(e.message, 'error');
  }
}

// ── Availability calendar ─────────────────────────────────────────────────────
function availCalendar(initialDates) {
  return {
    selected: new Set(initialDates),
    toggle(dateStr) {
      if (this.selected.has(dateStr)) this.selected.delete(dateStr);
      else this.selected.add(dateStr);
    },
    isSelected(dateStr) { return this.selected.has(dateStr); },
    async save(reunionId) {
      try {
        await apiFetch('PUT', `/reunions/${reunionId}/availability/me`,
          { dates: Array.from(this.selected) });
        showToast('Availability saved!', 'success');
      } catch (e) {
        showToast(e.message, 'error');
      }
    },
  };
}

// ── Location vote ─────────────────────────────────────────────────────────────
async function voteLocation(reunionId, locId, score) {
  try {
    await apiFetch('PUT', `/reunions/${reunionId}/locations/${locId}/vote`, { score, comment: null });
    window.location.reload();
  } catch (e) {
    showToast(e.message, 'error');
  }
}

// ── Slot claim / release ──────────────────────────────────────────────────────
async function claimSlot(reunionId, blockId, slotId) {
  try {
    await apiFetch('POST', `/reunions/${reunionId}/schedule/${blockId}/slots/${slotId}/claim`);
    window.location.reload();
  } catch (e) {
    showToast(e.message, 'error');
  }
}

async function releaseSlot(reunionId, blockId, slotId) {
  try {
    await apiFetch('DELETE', `/reunions/${reunionId}/schedule/${blockId}/slots/${slotId}/claim`);
    window.location.reload();
  } catch (e) {
    showToast(e.message, 'error');
  }
}

// ── Phase advance ─────────────────────────────────────────────────────────────
async function advancePhase(reunionId) {
  if (!confirm('Advance this reunion to the next phase?')) return;
  try {
    await apiFetch('POST', `/reunions/${reunionId}/advance-phase`);
    window.location.reload();
  } catch (e) {
    showToast(e.message, 'error');
  }
}

// ── Activity vote ─────────────────────────────────────────────────────────────
async function voteActivity(reunionId, actId, score) {
  try {
    await apiFetch('PUT', `/reunions/${reunionId}/activities/${actId}/vote`, { interest_score: score });
    window.location.reload();
  } catch (e) {
    showToast(e.message, 'error');
  }
}

// ── Today-view SSE ────────────────────────────────────────────────────────────
function startTodaySSE(reunionId) {
  const container = document.getElementById('today-content');
  if (!container) return;
  const es = new EventSource(`/api/reunions/${reunionId}/today`);
  es.onmessage = (e) => {
    try {
      const data = JSON.parse(e.data);
      renderTodaySnapshot(data, container);
    } catch (err) {
      console.error('SSE parse error', err);
    }
  };
  es.onerror = () => {
    console.warn('SSE connection lost, reconnecting...');
  };
}

// Initialise theme as soon as app.js is parsed (bottom of <body>)
initTheme();

function renderTodaySnapshot(blocks, container) {
  const now = new Date();
  const nowMins = now.getHours() * 60 + now.getMinutes();

  if (!blocks.length) {
    container.innerHTML = '<p class="text-gray-500 text-center py-8">No events scheduled for today.</p>';
    return;
  }

  const colors = { group:'bg-blue-50 border-blue-300', optional:'bg-green-50 border-green-300',
                   meal:'bg-amber-50 border-amber-300', travel:'bg-purple-50 border-purple-300' };

  container.innerHTML = blocks.map(b => {
    const [sh, sm] = b.start_time.split(':').map(Number);
    const [eh, em] = b.end_time.split(':').map(Number);
    const startMins = sh * 60 + sm;
    const endMins   = eh * 60 + em;
    const isCurrent = nowMins >= startMins && nowMins < endMins;
    const borderCls = colors[b.block_type] || 'bg-gray-50 border-gray-300';
    const ring = isCurrent ? ' ring-2 ring-amber-500' : '';

    const slots = (b.slots || []).map(sl => {
      const names = (sl.signups || []).map(s => `<span class="bg-gray-100 px-1 rounded text-xs">${s.user_id}</span>`).join(' ');
      return `<div class="text-sm mt-1"><span class="font-medium">${sl.role_name}</span> ${names}</div>`;
    }).join('');

    return `<div class="p-4 rounded border ${borderCls}${ring} mb-3">
      <div class="flex justify-between items-start">
        <div>
          <h3 class="font-semibold text-gray-900">${b.title}</h3>
          ${b.description ? `<p class="text-sm text-gray-600 mt-0.5">${b.description}</p>` : ''}
          ${slots}
        </div>
        <div class="text-sm text-gray-500 whitespace-nowrap ml-4">
          ${b.start_time.slice(0,5)} – ${b.end_time.slice(0,5)}
          ${isCurrent ? '<span class="ml-1 text-amber-600 font-semibold">● Now</span>' : ''}
        </div>
      </div>
    </div>`;
  }).join('');
}
