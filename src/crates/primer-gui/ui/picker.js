// Launch-screen session picker.
//
// Loaded before settings.js + app.js so it can expose
// `window.PrimerPicker` to the chat shell. Shown by `main()` in
// app.js when no session is active on launch; hidden once the user
// picks a row to resume or finishes the "Start new" → Settings → save
// flow.
//
// The picker stays stateless across show/hide cycles — every show()
// re-fetches the sessions list and re-renders. That keeps "what's on
// the picker" trivially aligned with disk; a session deleted out of
// band between two shows will simply disappear from the list.

const { invoke } = window.__TAURI__.core;

// Default placeholder name a fresh learner profile carries until the
// user supplies their own. Mirrors `primer_core::consts::learner::DEFAULT_NAME`
// — keep in sync when changing. Used to suppress the personalised
// "Welcome back, {name}" greeting for unconfigured installs.
const DEFAULT_LEARNER_NAME = "Explorer";

const dom = {
  screen: document.getElementById("picker-screen"),
  title: document.getElementById("picker-title"),
  subtitle: document.getElementById("picker-subtitle"),
  loading: document.getElementById("picker-loading"),
  error: document.getElementById("picker-error"),
  empty: document.getElementById("picker-empty"),
  list: document.getElementById("picker-list"),
  settingsBtn: document.getElementById("picker-settings"),
  startNewBtn: document.getElementById("picker-start-new"),
};

const state = {
  /// Callback invoked on successful resume_session — picker hands
  /// the new SessionInfo off to the chat shell, which clears bubbles
  /// and replays the loaded turns.
  onResumed: null,
  /// Callback invoked on successful start_session (via the settings
  /// modal). Same job as onResumed but without the turn-replay step.
  onStarted: null,
};

wireFooterButtons();
window.PrimerPicker = { show, hide };

/// Show the picker overlay. Re-fetches the session list every call so
/// "what's on screen" stays trivially aligned with disk. Callbacks
/// are stored on `state` and fired once the user picks an action.
async function show({ onResumed, onStarted } = {}) {
  state.onResumed = onResumed ?? null;
  state.onStarted = onStarted ?? null;
  dom.screen.hidden = false;
  showLoading();
  await refresh();
}

/// Hide the picker overlay without resetting state. Callers that want
/// to surface the picker again (future: switch-session-from-chat) just
/// call show() — the next refresh() will re-fetch from disk.
function hide() {
  dom.screen.hidden = true;
}

function showLoading() {
  dom.loading.hidden = false;
  dom.error.hidden = true;
  dom.empty.hidden = true;
  dom.list.hidden = true;
}

function showError(msg) {
  dom.error.textContent = msg;
  dom.error.hidden = false;
  dom.loading.hidden = true;
}

/// Reload the picker contents from disk + settings. Public-ish: called
/// internally by show() and after a failed resume attempt to repaint
/// the list. Independent error handling per fetch — see comment below.
async function refresh() {
  // Fetch both in parallel but keep error reporting independent —
  // a settings-load failure shouldn't masquerade as a sessions-load
  // error, since the picker can still render its list with the
  // generic "Welcome to Primer" header while the settings round-trip
  // is broken.
  const [sessionsResult, settingsResult] = await Promise.allSettled([
    invoke("list_sessions"),
    invoke("get_settings"),
  ]);

  if (settingsResult.status === "fulfilled") {
    applyHeader(settingsResult.value);
  } else {
    console.warn("get_settings failed:", settingsResult.reason);
    applyHeader(null);
  }

  if (sessionsResult.status === "fulfilled") {
    renderSessions(sessionsResult.value ?? []);
  } else {
    showError(`Couldn't load sessions: ${formatErr(sessionsResult.reason)}`);
  }
}

function applyHeader(settings) {
  const name = settings?.learner?.name?.trim();
  if (name && name !== DEFAULT_LEARNER_NAME) {
    dom.title.textContent = `Welcome back, ${name}`;
    dom.subtitle.textContent = "Pick up where you left off, or start fresh.";
  } else {
    dom.title.textContent = "Welcome to Primer";
    dom.subtitle.textContent =
      "Start a new session — or open Settings to customise your setup first.";
  }
}

function renderSessions(sessions) {
  dom.loading.hidden = true;
  if (sessions.length === 0) {
    dom.empty.hidden = false;
    dom.list.hidden = true;
    dom.list.replaceChildren();
    return;
  }
  dom.empty.hidden = true;
  dom.list.hidden = false;
  dom.list.replaceChildren(...sessions.map(renderRow));
}

function renderRow(s) {
  const li = document.createElement("li");
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "picker-row";
  btn.dataset.sessionId = s.session_id;

  const lastActivity = new Date(s.last_activity);
  const startedAt = new Date(s.started_at);

  const when = document.createElement("span");
  when.className = "picker-row-when";
  when.textContent = relativeTime(lastActivity);

  const summaryEl = document.createElement("span");
  summaryEl.className = "picker-row-summary";
  if (s.summary && s.summary.trim().length > 0) {
    summaryEl.textContent = s.summary;
  } else if (s.turn_count === 0) {
    summaryEl.textContent = "Started but no turns yet.";
    summaryEl.classList.add("is-empty");
  } else {
    summaryEl.textContent = "No summary yet — still in the early turns.";
    summaryEl.classList.add("is-empty");
  }

  const meta = document.createElement("span");
  meta.className = "picker-row-meta muted";
  const turnLabel = s.turn_count === 1 ? "1 turn" : `${s.turn_count} turns`;
  meta.textContent = `${turnLabel} · started ${relativeTime(startedAt)}`;

  btn.appendChild(when);
  btn.appendChild(summaryEl);
  btn.appendChild(meta);
  btn.addEventListener("click", () => resume(s.session_id));
  btn.title = `Session ${s.session_id}`;

  li.appendChild(btn);
  return li;
}

/// Resume a session by id. Disables UI for the duration so a fast
/// second click can't fire a parallel resume_session, then hands the
/// returned SessionInfo to `state.onResumed`. On failure repaints the
/// list (in case disk state changed) and surfaces the error inline.
async function resume(sessionId) {
  // Disable every row + footer button so the user can't fire a
  // second resume_session while the first is still constructing.
  setRowsBusy(true);
  showLoading();
  try {
    const info = await invoke("resume_session", { sessionId });
    hide();
    if (state.onResumed) {
      try {
        await state.onResumed(info);
      } catch (cbErr) {
        console.warn("onResumed callback threw:", cbErr);
      }
    }
  } catch (err) {
    setRowsBusy(false);
    // Re-render the list rather than dropping into the global empty
    // state — refresh() will swap loading→list and surface any
    // freshly-changed listings as a bonus.
    await refresh();
    showError(`Couldn't resume session: ${formatErr(err)}`);
  }
}

function setRowsBusy(busy) {
  for (const btn of dom.list.querySelectorAll(".picker-row")) {
    btn.disabled = busy;
  }
  dom.startNewBtn.disabled = busy;
  dom.settingsBtn.disabled = busy;
}

function wireFooterButtons() {
  // Both footer buttons open the same Settings modal — "Start new"
  // is the primary path and lands directly on the start-new flow
  // when the user clicks "Save & start new session" inside the
  // modal; "Settings" is for users who want to tweak before deciding.
  // Both supply the same onSessionRestarted callback so the modal
  // can hand the chat shell its new session.
  dom.startNewBtn.addEventListener("click", () => openSettings());
  dom.settingsBtn.addEventListener("click", () => openSettings());
}

function openSettings() {
  window.PrimerSettings.open({
    onSessionRestarted: () => {
      hide();
      if (state.onStarted) {
        try {
          state.onStarted();
        } catch (cbErr) {
          console.warn("onStarted callback threw:", cbErr);
        }
      }
    },
  });
}

// ─── Helpers ────────────────────────────────────────────────────

/// Compact relative-time formatting tuned for the picker's "when did
/// I last use this" use case. < 1 min → "just now"; < 1 hour → "Nm
/// ago"; < 24 hours → "Nh ago"; < 7 days → "Nd ago"; else absolute
/// date.
function relativeTime(date) {
  const now = Date.now();
  const diffMs = now - date.getTime();
  const sec = Math.floor(diffMs / 1000);
  if (sec < 60) return "just now";
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d ago`;
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

function formatErr(err) {
  if (typeof err === "string") return err;
  if (err && typeof err.message === "string") return err.message;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}
