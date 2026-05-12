// Vanilla-JS chat shell for the Primer GUI.
//
// Step 4 wires:
//   • Auto-start a session on launch (uses persisted gui-config.json).
//   • Render child + Primer bubbles in the scroll area.
//   • Listen for `primer://chunk` events and append to the live bubble.
//   • Finalise on `primer://turn_complete`.
//
// Tauri 2 exposes `invoke` / `listen` on `window.__TAURI__` because
// `app.withGlobalTauri = true` is set in tauri.conf.json — no npm
// toolchain needed.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const dom = {
  body: document.body,
  sessionInfo: document.getElementById("session-info"),
  chatScroll: document.getElementById("chat-scroll"),
  emptyState: document.getElementById("empty-state"),
  errorBanner: document.getElementById("error-banner"),
  composer: document.getElementById("composer"),
  input: document.getElementById("input"),
  send: document.getElementById("send"),
  sidebarToggle: document.getElementById("sidebar-toggle"),
  signals: {
    empty: document.getElementById("sidebar-empty"),
    lagHint: document.getElementById("signals-lag-hint"),
    intentCard: document.getElementById("signals-intent-card"),
    intentBadge: document.getElementById("signals-intent-badge"),
    engagementCard: document.getElementById("signals-engagement-card"),
    engagementState: document.getElementById("signals-engagement-state"),
    engagementFill: document.getElementById("signals-engagement-fill"),
    engagementValue: document.getElementById("signals-engagement-value"),
    engagementReasoning: document.getElementById("signals-engagement-reasoning"),
    engagementModel: document.getElementById("signals-engagement-model"),
    conceptsCard: document.getElementById("signals-concepts-card"),
    conceptsChild: document.getElementById("signals-concepts-child"),
    conceptsPrimer: document.getElementById("signals-concepts-primer"),
    extractorModel: document.getElementById("signals-extractor-model"),
    comprehensionCard: document.getElementById("signals-comprehension-card"),
    comprehensionList: document.getElementById("signals-comprehension-list"),
    comprehensionModel: document.getElementById("signals-comprehension-model"),
  },
};

// Live state — `streamingPrimerEl` points at the currently-streaming
// Primer bubble (if any) so chunk events can append to its text node
// without re-querying the DOM.
const state = {
  sessionId: null,
  streamingPrimerEl: null,
};

main();

async function main() {
  setupChunkListener();
  setupTurnCompleteListener();
  setupComposer();
  setupAutogrow();
  setupSidebarToggle();
  await openOrStartSession();
  // Render whatever's already on the DM (resumed sessions land here
  // with populated last_* accessors); first-launch shows the empty state.
  refreshSignals();
}

async function openOrStartSession() {
  try {
    let info = await invoke("current_session_info");
    if (info === null) {
      info = await invoke("start_session");
    }
    renderSessionInfo(info);
    enableComposer();
  } catch (err) {
    showError(formatErr(err));
  }
}

function renderSessionInfo(info) {
  state.sessionId = info.session_id; // may be null until first send
  const { learner, backend_kind, main_model, locale } = info;
  dom.sessionInfo.dataset.state = "ready";
  dom.sessionInfo.innerHTML = `
    <span class="pill" title="learner">
      ${escapeHtml(learner.name)} · age ${learner.age}
    </span>
    <span class="pill" title="backend / model">
      ${escapeHtml(backend_kind)}${
        main_model && main_model !== backend_kind ? " · " + escapeHtml(main_model) : ""
      }
    </span>
    <span class="pill" title="locale">${escapeHtml(locale)}</span>
  `;
}

function enableComposer() {
  dom.input.disabled = false;
  dom.send.disabled = false;
  dom.input.focus();
}

function disableComposer() {
  dom.input.disabled = true;
  dom.send.disabled = true;
}

function setupComposer() {
  dom.composer.addEventListener("submit", onSubmit);
  // Enter sends, Shift+Enter inserts a newline (standard chat affordance).
  dom.input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      onSubmit(e);
    }
  });
}

function setupAutogrow() {
  // Grow the textarea as the user types, capped by max-height in CSS.
  dom.input.addEventListener("input", () => {
    dom.input.style.height = "auto";
    dom.input.style.height = `${dom.input.scrollHeight}px`;
  });
}

async function onSubmit(e) {
  e.preventDefault();
  const text = dom.input.value.trim();
  if (!text || dom.send.disabled) return;

  hideError();
  hideEmptyState();
  appendChildBubble(text);
  state.streamingPrimerEl = appendStreamingPrimerBubble();
  dom.input.value = "";
  dom.input.style.height = "auto";
  disableComposer();

  try {
    await invoke("send_message", { input: text });
    // The turn_complete event listener will finalise the bubble and
    // re-enable the composer; nothing to do here on success.
  } catch (err) {
    finaliseStreamingBubble({ aborted: true });
    showError(formatErr(err));
    enableComposer();
  }
}

// Both listeners are wired once at startup and never torn down — the
// app has no flow today that closes the session UI without exiting
// the process. `listen` returns a Promise<UnlistenFn>; we discard the
// handle deliberately. If a future step adds in-app session teardown
// (e.g. settings-modal reload, picker-driven session switch), capture
// the resolved unlisten fns here and call them on teardown to avoid
// double-emission.
function setupChunkListener() {
  listen("primer://chunk", (event) => {
    if (!state.streamingPrimerEl) return;
    state.streamingPrimerEl.textContent += event.payload.text;
    scrollToBottom();
  });
}

function setupTurnCompleteListener() {
  listen("primer://turn_complete", (event) => {
    state.sessionId = event.payload.session_id;
    finaliseStreamingBubble({ aborted: false });
    enableComposer();
    // Fire-and-forget — sidebar updates are non-critical; a failure
    // here shouldn't deny the user the chat surface.
    refreshSignals();
  });
}

function setupSidebarToggle() {
  dom.sidebarToggle.addEventListener("click", () => {
    const collapsed = dom.body.classList.toggle("sidebar-collapsed");
    dom.sidebarToggle.setAttribute("aria-pressed", String(!collapsed));
    // Flip the label too: a screen reader user gets the state from
    // aria-pressed, but sighted users only see the button text. Keep
    // both surfaces in sync.
    dom.sidebarToggle.textContent = collapsed ? "Show Sidebar" : "Hide Sidebar";
  });
}

async function refreshSignals() {
  try {
    const signals = await invoke("get_turn_signals");
    renderSignals(signals);
  } catch (err) {
    // Sidebar errors are non-critical — log but don't surface.
    console.warn("get_turn_signals failed:", err);
  }
}

function renderSignals(signals) {
  if (!signals) {
    showSignalsEmpty();
    return;
  }
  const s = dom.signals;

  // The sidebar has SOMETHING to render once any field is populated.
  // Even on turn 1 we get intent + identifier strings; the lag-hint
  // appears once the engagement/concepts/comprehension cards do.
  const anyLagged =
    signals.engagement ||
    (signals.concepts && (signals.concepts.child?.length || signals.concepts.primer?.length)) ||
    (signals.comprehension && signals.comprehension.length > 0);
  s.empty.hidden = !!(signals.intent || anyLagged);
  s.lagHint.hidden = !anyLagged;

  // Intent
  if (signals.intent) {
    s.intentCard.hidden = false;
    s.intentBadge.textContent = signals.intent;
  } else {
    s.intentCard.hidden = true;
  }

  // Engagement
  if (signals.engagement) {
    s.engagementCard.hidden = false;
    s.engagementState.textContent = signals.engagement.state;
    const pct = Math.round((signals.engagement.confidence ?? 0) * 100);
    s.engagementFill.style.width = `${pct}%`;
    s.engagementValue.textContent = `${pct}%`;
    if (signals.engagement.reasoning) {
      s.engagementReasoning.hidden = false;
      s.engagementReasoning.textContent = `“${signals.engagement.reasoning}”`;
    } else {
      s.engagementReasoning.hidden = true;
      s.engagementReasoning.textContent = "";
    }
    s.engagementModel.textContent = signals.classifier_identifier
      ? `via ${signals.classifier_identifier}`
      : "";
  } else {
    s.engagementCard.hidden = true;
  }

  // Concepts
  const child = signals.concepts?.child ?? [];
  const primer = signals.concepts?.primer ?? [];
  if (child.length || primer.length) {
    s.conceptsCard.hidden = false;
    renderChips(s.conceptsChild, child);
    renderChips(s.conceptsPrimer, primer);
    s.extractorModel.textContent = signals.extractor_identifier
      ? `via ${signals.extractor_identifier}`
      : "";
  } else {
    s.conceptsCard.hidden = true;
  }

  // Comprehension
  const assessments = signals.comprehension ?? [];
  if (assessments.length) {
    s.comprehensionCard.hidden = false;
    s.comprehensionList.replaceChildren(
      ...assessments.map(renderComprehensionItem),
    );
    s.comprehensionModel.textContent = signals.comprehension_identifier
      ? `via ${signals.comprehension_identifier}`
      : "";
  } else {
    s.comprehensionCard.hidden = true;
  }
}

function showSignalsEmpty() {
  const s = dom.signals;
  s.empty.hidden = false;
  s.lagHint.hidden = true;
  s.intentCard.hidden = true;
  s.engagementCard.hidden = true;
  s.conceptsCard.hidden = true;
  s.comprehensionCard.hidden = true;
}

function renderChips(ulEl, items) {
  ulEl.replaceChildren();
  if (!items.length) {
    const li = document.createElement("li");
    li.className = "placeholder";
    li.textContent = "—";
    ulEl.appendChild(li);
    return;
  }
  for (const item of items) {
    const li = document.createElement("li");
    li.textContent = item;
    ulEl.appendChild(li);
  }
}

function renderComprehensionItem(a) {
  const li = document.createElement("li");
  const concept = document.createElement("span");
  concept.textContent = a.concept;
  const pill = document.createElement("span");
  pill.className = "depth-pill";
  // a.depth is always a canonical `UnderstandingDepth::name()` from
  // the Rust side (Unknown / Aware / Recall / Comprehension /
  // Application / Analysis). Lowercase it to match the CSS
  // `data-depth=` selectors in styles.css.
  pill.dataset.depth = a.depth.toLowerCase();
  pill.textContent = a.depth;
  const pct = Math.round((a.confidence ?? 0) * 100);
  const conf = document.createElement("span");
  conf.className = "muted";
  conf.textContent = `${pct}%`;
  conf.title = a.evidence ? `“${a.evidence}”` : "";
  li.appendChild(concept);
  li.appendChild(pill);
  li.appendChild(conf);
  return li;
}

function appendChildBubble(text) {
  const row = document.createElement("div");
  row.className = "bubble-row is-child";
  const bubble = document.createElement("div");
  bubble.className = "bubble";
  bubble.textContent = text;
  row.appendChild(bubble);
  dom.chatScroll.appendChild(row);
  scrollToBottom();
}

function appendStreamingPrimerBubble() {
  const row = document.createElement("div");
  row.className = "bubble-row is-primer";
  const bubble = document.createElement("div");
  bubble.className = "bubble is-streaming";
  bubble.textContent = "";
  row.appendChild(bubble);
  dom.chatScroll.appendChild(row);
  scrollToBottom();
  return bubble;
}

function finaliseStreamingBubble({ aborted }) {
  const el = state.streamingPrimerEl;
  if (!el) return;
  el.classList.remove("is-streaming");
  if (aborted && el.textContent.trim() === "") {
    // Empty-aborted: drop the placeholder rather than leaving a blank
    // Primer bubble. Matches DM's "partial Primer turn dropped on
    // mid-stream error" semantic.
    el.parentElement?.remove();
  }
  state.streamingPrimerEl = null;
}

function hideEmptyState() {
  if (dom.emptyState && !dom.emptyState.hidden) {
    dom.emptyState.hidden = true;
  }
}

function scrollToBottom() {
  dom.chatScroll.scrollTop = dom.chatScroll.scrollHeight;
}

function showError(msg) {
  dom.errorBanner.textContent = msg;
  dom.errorBanner.hidden = false;
}

function hideError() {
  dom.errorBanner.hidden = true;
  dom.errorBanner.textContent = "";
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

// Minimal HTML escaper for header pill content. The bubble text path
// uses textContent so it doesn't need escaping.
function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
