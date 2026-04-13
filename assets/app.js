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
function availCalendar(initialDates, reunionId) {
  return {
    selected: new Set(initialDates),
    inverted: false,       // true = user is marking UNavailable days (shown red)
    reunionId,
    _dragging: false,
    _dragMode: 'add',      // 'add' | 'remove'
    _saveTimer: null,

    isSelected(dateStr) { return this.selected.has(dateStr); },

    // Called on mousedown / touchstart on a day cell
    startDrag(dateStr) {
      this._dragging = true;
      this._dragMode = this.selected.has(dateStr) ? 'remove' : 'add';
      this._applyDate(dateStr);
    },
    // Called on mouseenter while dragging
    applyDrag(dateStr) {
      if (this._dragging) this._applyDate(dateStr);
    },
    stopDrag() {
      if (this._dragging) {
        this._dragging = false;
        this._scheduleSave();
      }
    },

    _applyDate(dateStr) {
      if (this._dragMode === 'add') this.selected.add(dateStr);
      else this.selected.delete(dateStr);
    },

    // Toggling inverted flips the selection so the server-side meaning is preserved.
    // (selected = available in normal mode; selected = unavailable in inverted mode)
    onInvertedChange() {
      const all = Array.from(this.$el.querySelectorAll('[data-date]')).map(el => el.dataset.date);
      this.selected = new Set(all.filter(d => !this.selected.has(d)));
      // No save needed: the set of available dates is unchanged after flipping both
      // the mode and the selection simultaneously.
    },

    // Returns the dates to send to the API (always the "available" dates).
    _availableDates() {
      if (!this.inverted) return Array.from(this.selected);
      const all = Array.from(this.$el.querySelectorAll('[data-date]')).map(el => el.dataset.date);
      return all.filter(d => !this.selected.has(d));
    },

    _scheduleSave() {
      clearTimeout(this._saveTimer);
      this._saveTimer = setTimeout(() => this._doSave(), 1200);
    },

    async _doSave() {
      try {
        await apiFetch('PUT', `/reunions/${this.reunionId}/availability/me`,
          { dates: this._availableDates() });
        showToast('Saved', 'success');
      } catch (e) {
        showToast(e.message, 'error');
      }
    },

    // Attach non-passive touchmove so we can prevent scroll mid-drag
    init() {
      const self = this;
      this.$el.addEventListener('touchmove', function(e) {
        if (!self._dragging) return;
        e.preventDefault();
        const t = e.touches[0];
        const el = document.elementFromPoint(t.clientX, t.clientY);
        const d = el?.closest('[data-date]')?.dataset?.date || el?.dataset?.date;
        if (d) self._applyDate(d);
      }, { passive: false });
    },
  };
}

// ── Location vote + comment ───────────────────────────────────────────────────
async function saveLocationVote(reunionId, locId) {
  const score   = parseInt(document.getElementById(`vote-loc-${locId}`).value);
  const rawComment = document.getElementById(`vote-comment-${locId}`).value.trim();
  const comment = rawComment.length > 0 ? rawComment : null;
  try {
    await apiFetch('PUT', `/reunions/${reunionId}/locations/${locId}/vote`, { score, comment });
    showToast('Vote saved!', 'success');
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
let _lastTodayBlocks = null;
let _lastHidePast = false;
let _todayUse24h = localStorage.getItem('todayUse24h') === 'true';

/** Format "HH:MM[:SS]" as "H:MM AM/PM" (or keep 24h if flag set). */
function fmtTime(t) {
  if (_todayUse24h) return t.slice(0, 5);
  const [h, m] = t.split(':').map(Number);
  const period = h >= 12 ? 'PM' : 'AM';
  const h12 = h % 12 || 12;
  return `${h12}:${String(m).padStart(2, '0')} ${period}`;
}

/** Called by the 24h checkbox; persists preference and re-renders cached data. */
function toggle24h(checked) {
  _todayUse24h = checked;
  localStorage.setItem('todayUse24h', String(checked));
  if (_lastTodayBlocks) {
    const container = document.getElementById('today-content');
    if (container) renderTodaySnapshot(_lastTodayBlocks.blocks || [], container, _lastTodayStartDate, _lastHidePast);
  }
}

/** Initialise the 24h checkbox state from stored preference. */
function initTodayCheckbox() {
  const cb = document.getElementById('use24h');
  if (cb) cb.checked = _todayUse24h;
}

let _lastTodayStartDate = null;

/**
 * Start the today-view SSE stream.
 *
 * opts:
 *   oneShot   {boolean} — close the EventSource after the first successful
 *                         message (use on the overview page; /today keeps it live).
 *   startDate {string|null} — YYYY-MM-DD reunion start date, used to show a
 *                             contextual message when there are no events today.
 */
function startTodaySSE(reunionId, opts = {}) {
  const container = document.getElementById('today-content');
  if (!container) return;

  const oneShot   = opts.oneShot   ?? false;
  const hidePast  = opts.hidePast  ?? false;
  const startDate = opts.startDate ?? null;
  _lastTodayStartDate = startDate;
  _lastHidePast = hidePast;

  let received = false;

  function showEmpty() {
    const today = new Date().toISOString().slice(0, 10);
    let msg = 'No events scheduled for today.';
    if (startDate && today < startDate) {
      msg = 'Reunion hasn\'t started yet — the schedule will appear here on the first day.';
    }
    container.innerHTML =
      `<p class="text-sm text-center py-6" style="color:var(--muted)">${msg}</p>`;
  }

  // Safety net: if no data arrives within 6 s, resolve the spinner rather than
  // hanging indefinitely (covers auth errors, network hiccups, empty schedules).
  const fallbackTimer = setTimeout(() => {
    if (!received) showEmpty();
  }, 6000);

  const es = new EventSource(`/api/reunions/${reunionId}/today`);

  es.onmessage = (e) => {
    received = true;
    clearTimeout(fallbackTimer);
    try {
      const data = JSON.parse(e.data);
      _lastTodayBlocks = data;
      renderTodaySnapshot(data.blocks || [], container, startDate, hidePast);
    } catch (err) {
      console.error('SSE parse error', err);
      showEmpty();
    }
    // Overview only needs a static snapshot; close to avoid a persistent
    // open connection on a page that doesn't benefit from live updates.
    if (oneShot) es.close();
  };

  es.onerror = () => {
    // On any error: stop the fallback race, resolve the UI immediately, and —
    // for one-shot callers — close the EventSource to prevent the browser's
    // automatic infinite-reconnect loop from keeping the page in a broken state.
    clearTimeout(fallbackTimer);
    if (!received) showEmpty();
    if (oneShot) es.close();
  };

  return es;
}

// Initialise theme as soon as app.js is parsed (bottom of <body>)
initTheme();

function renderTodaySnapshot(blocks, container, startDate = null, hidePast = false) {
  const now = new Date();
  const nowMins = now.getHours() * 60 + now.getMinutes();

  if (!blocks.length) {
    const today = now.toISOString().slice(0, 10);
    let msg = 'No events scheduled for today.';
    if (startDate && today < startDate) {
      msg = 'Reunion hasn\'t started yet — the schedule will appear here on the first day.';
    }
    container.innerHTML =
      `<p class="text-sm text-center py-6" style="color:var(--muted)">${msg}</p>`;
    return;
  }

  // On the overview page, hide blocks that have already ended.
  let visibleBlocks = blocks;
  if (hidePast) {
    visibleBlocks = blocks.filter(b => {
      const [eh, em] = b.end_time.split(':').map(Number);
      return (eh * 60 + em) > nowMins;
    });
    if (!visibleBlocks.length) {
      container.innerHTML =
        `<p class="text-sm text-center py-6" style="color:var(--muted)">All done for today! 🎉</p>`;
      return;
    }
  }

  const colors = { group:'bg-blue-50 border-blue-300', optional:'bg-green-50 border-green-300',
                   meal:'bg-amber-50 border-amber-300', travel:'bg-purple-50 border-purple-300' };

  container.innerHTML = visibleBlocks.map(b => {
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
          ${fmtTime(b.start_time)} – ${fmtTime(b.end_time)}
          ${isCurrent ? '<span class="ml-1 text-amber-600 font-semibold">● Now</span>' : ''}
        </div>
      </div>
    </div>`;
  }).join('');
}
