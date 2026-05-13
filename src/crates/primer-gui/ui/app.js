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
//
// IIFE wrap: see picker.js header — top-level `const invoke` collides
// across classic scripts otherwise.
(() => {
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
  settingsOpen: document.getElementById("settings-open"),
  switchSession: document.getElementById("switch-session"),
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
  learner: {
    profileCard: document.getElementById("learner-profile-card"),
    name: document.getElementById("learner-name"),
    ageLocale: document.getElementById("learner-age-locale"),
    uuid: document.getElementById("learner-uuid"),
    uuidCopy: document.getElementById("learner-uuid-copy"),
    vocabCard: document.getElementById("learner-vocab-card"),
    vocabCount: document.getElementById("learner-vocab-count"),
    vocabList: document.getElementById("learner-vocab-list"),
    distributionCard: document.getElementById("learner-distribution-card"),
    conceptCount: document.getElementById("learner-concept-count"),
    depthBar: document.getElementById("learner-depth-bar"),
    depthLegend: document.getElementById("learner-depth-legend"),
    engagementCard: document.getElementById("learner-engagement-card"),
    engagementStrip: document.getElementById("learner-engagement-strip"),
  },
  session: {
    empty: document.getElementById("session-empty"),
    hint: document.getElementById("session-list-hint"),
    list: document.getElementById("session-turn-list"),
  },
};

/// Maximum filled box dots — matches MAX_BOX_LEVEL in primer_core::vocab.
const MAX_BOX_LEVEL = 4;

/// Sentinel returned by `send_message` when the user clicked the
/// Cancel button mid-stream. Matches the `CANCELLED_MESSAGE` const in
/// `commands/session.rs` — a `cancelled_message_is_stable_machine_token`
/// unit test on the Rust side pins the value, so a one-sided change
/// here or there breaks CI immediately. A machine-readable token
/// (rather than a sentence) means a missed match surfaces as obvious
/// machine output, not a localised-looking string.
const CANCEL_SENTINEL = "primer:turn_cancelled";

// Live state — `streamingPrimerEl` points at the currently-streaming
// Primer bubble (if any) so chunk events can append to its text node
// without re-querying the DOM.
const state = {
  sessionId: null,
  streamingPrimerEl: null,
  /// `true` while a `send_message` invocation is in flight. Drives the
  /// Send button's Send⇄Cancel label swap and lets the form submit
  /// handler route a click to `cancel_response` instead of starting a
  /// second turn while the first is still streaming.
  isStreaming: false,
  /// Next zero-based turn index in the session timeline. Grows by 2
  /// per successful exchange (child + primer) and rolls back by 1 on a
  /// mid-stream error (the dropped Primer turn — see CLAUDE.md
  /// "the partial Primer turn is not recorded into the session"). Used
  /// to tag bubble DOM elements with `data-turn-index` so the Session
  /// sidebar's click-to-scroll can address them in O(1).
  nextTurnIndex: 0,
  /// setTimeout id for the spotlight-clear on click-to-scroll. Tracked
  /// so a second click within the highlight window cancels the prior
  /// timeout instead of letting it strip the new target's highlight.
  spotlightTimer: null,
};

main();

async function main() {
  setupChunkListener();
  setupTurnCompleteListener();
  setupComposer();
  setupAutogrow();
  setupSidebarToggle();
  setupUuidCopy();
  setupSettingsButton();
  setupSwitchSessionButton();
  showPicker();
}

/// Open (or re-open) the launch-screen picker. The picker manages its
/// own lifecycle — it hides itself on resolution and the callbacks
/// drive the chat shell into the right state.
function showPicker() {
  window.PrimerPicker.show({
    onResumed: (info) => handleSessionReady({ info, shouldReplay: true }),
    onStarted: () => handleSessionReady({ info: null, shouldReplay: false }),
  });
}

function setupSwitchSessionButton() {
  dom.switchSession.addEventListener("click", onSwitchSession);
}

/// Tear down the active session and return to the picker. Used by the
/// header's "Sessions" button. We close the session server-side first
/// so its background tasks drain cleanly, then wipe the chat surface
/// before showing the picker — leaving stale bubbles up while the
/// picker opens would feel like the previous session was still live.
///
/// `close_session` is idempotent and silently succeeds with no active
/// session, so a stray click is harmless.
async function onSwitchSession() {
  try {
    await invoke("close_session");
  } catch (err) {
    console.warn("close_session failed:", err);
  }
  resetChatSurface();
  resetSessionHeader();
  setComposerState("disabled");
  showPicker();
}

function resetSessionHeader() {
  dom.sessionInfo.dataset.state = "loading";
  dom.sessionInfo.innerHTML = `<span class="muted">No session</span>`;
}

function setupSettingsButton() {
  dom.settingsOpen.addEventListener("click", () => {
    // settings.js is loaded by index.html before app.js, so the
    // global is guaranteed to exist by the time the button is
    // clickable. Save & start new session re-enters the chat surface
    // via the same handler the picker uses, so the post-start UI
    // state is consistent regardless of how the new session got
    // started.
    window.PrimerSettings.open({
      onSessionRestarted: () =>
        handleSessionReady({ info: null, shouldReplay: false }),
    });
  });
}

/// Bring up the chat surface for the session that just became active.
///
/// Called from three places — picker resume, picker start-new (via
/// settings modal), and the header settings modal's "Save & start
/// new session" — so all three paths land in the same UI state.
///
/// `info` is supplied directly when the caller already has it (picker
/// resume); a `null` triggers a `current_session_info` fetch. When
/// `shouldReplay` is true, the loaded session's turns are fetched in
/// full and replayed as bubbles so the user sees the conversation
/// history — that's the picker-resume path. Fresh sessions skip
/// replay (there's nothing to render) and leave the empty-state in
/// place until the first message lands.
async function handleSessionReady({ info, shouldReplay }) {
  resetChatSurface();

  const sessionInfo =
    info ?? (await invoke("current_session_info").catch(() => null));
  if (sessionInfo) {
    renderSessionInfo(sessionInfo);
  }

  if (shouldReplay) {
    try {
      const turns = await invoke("get_full_session_turns");
      if (turns && turns.length > 0) replayTurns(turns);
    } catch (err) {
      console.warn("get_full_session_turns failed:", err);
    }
  }

  setComposerState("idle");
  refreshSidebar();
}

/// Wipe the chat surface back to its empty-state shell — used before
/// rendering either a freshly-started session or replaying a resumed
/// one. Mirror of what fresh-launch shows.
function resetChatSurface() {
  state.streamingPrimerEl = null;
  state.nextTurnIndex = 0;
  state.sessionId = null;
  if (state.spotlightTimer !== null) {
    clearTimeout(state.spotlightTimer);
    state.spotlightTimer = null;
  }
  hideError();
  for (const row of Array.from(dom.chatScroll.querySelectorAll(".bubble-row"))) {
    row.remove();
  }
  if (dom.emptyState) {
    dom.emptyState.hidden = false;
    if (!dom.chatScroll.contains(dom.emptyState)) {
      dom.chatScroll.appendChild(dom.emptyState);
    }
  }
}

/// Render every turn of a resumed session as a chat bubble. `turns`
/// is the SessionFullTurn[] payload from `get_full_session_turns` —
/// each carries the FULL text (not the truncated sidebar preview)
/// because chat bubbles need to display the original content. The
/// next user-typed message lands at `state.nextTurnIndex = turns.length`
/// so the click-to-scroll indices stay aligned with the backend's
/// `session.turns` indices.
function replayTurns(turns) {
  hideEmptyState();
  for (const t of turns) {
    const row = document.createElement("div");
    row.className =
      t.speaker === "child" ? "bubble-row is-child" : "bubble-row is-primer";
    row.dataset.turnIndex = String(t.index);
    const bubble = document.createElement("div");
    bubble.className = "bubble";
    bubble.textContent = t.text;
    row.appendChild(bubble);
    dom.chatScroll.appendChild(row);
  }
  state.nextTurnIndex = turns.length;
  scrollToBottom();
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

/// Drive the composer's three-state UI from a single function so the
/// Send/Cancel toggle, input enabled-ness, and the "Sessions" header
/// button all stay in lockstep.
///
/// - `disabled`: pre-session — both input and send button are inert,
///   and the Sessions button is hidden behind a disabled state since
///   there's nothing to switch away from. Used between launch and the
///   first session, and after `onSwitchSession` returns to the picker.
/// - `idle`: a session is ready, no turn in flight — input is
///   editable, Send button posts the typed message, Sessions button
///   is enabled (the picker is reachable).
/// - `streaming`: a turn is in flight — input is locked (can't queue
///   a second turn), Send button shows "Cancel" and dispatches
///   `cancel_response` on click, Sessions button is disabled so a
///   stray click can't kick off a `close_session` that blocks on the
///   in-flight turn's DM lock.
///
/// Also flips `state.isStreaming` so the form submit handler can
/// route between submit-new and cancel-current paths.
function setComposerState(mode) {
  switch (mode) {
    case "disabled":
      dom.input.disabled = true;
      dom.send.disabled = true;
      dom.send.textContent = "Send";
      dom.send.classList.remove("is-cancel");
      dom.switchSession.disabled = true;
      state.isStreaming = false;
      break;
    case "idle":
      dom.input.disabled = false;
      dom.send.disabled = false;
      dom.send.textContent = "Send";
      dom.send.classList.remove("is-cancel");
      dom.switchSession.disabled = false;
      state.isStreaming = false;
      dom.input.focus();
      break;
    case "streaming":
      dom.input.disabled = true;
      // Send stays clickable so the user can cancel.
      dom.send.disabled = false;
      dom.send.textContent = "Cancel";
      dom.send.classList.add("is-cancel");
      dom.switchSession.disabled = true;
      state.isStreaming = true;
      break;
    default:
      console.warn("setComposerState: unknown mode", mode);
  }
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
  // The Send button doubles as Cancel during a streaming turn. Route
  // the click here rather than wiring a second listener so the form's
  // Enter-key submit (if a future change re-enables the textarea
  // mid-stream) still hits the same dispatch.
  if (state.isStreaming) {
    onCancel();
    return;
  }

  const text = dom.input.value.trim();
  if (!text || dom.send.disabled) return;

  hideError();
  hideEmptyState();
  appendChildBubble(text);
  state.streamingPrimerEl = appendStreamingPrimerBubble();
  dom.input.value = "";
  dom.input.style.height = "auto";
  setComposerState("streaming");

  try {
    await invoke("send_message", { input: text });
    // The turn_complete event listener will finalise the bubble and
    // re-enable the composer; nothing to do here on success.
  } catch (err) {
    finaliseStreamingBubble({ aborted: true });
    // User-initiated cancellation isn't an error the user needs to
    // see acknowledged — they clicked the button, the partial bubble
    // already disappeared as confirmation. A toast would be noise.
    const msg = formatErr(err);
    if (msg !== CANCEL_SENTINEL) {
      showError(msg);
    }
    setComposerState("idle");
    // Backend kept the child turn even on mid-stream failure; refresh
    // the sidebar so the Session list reflects it. turn_complete never
    // fired, so the standard refresh path didn't run.
    refreshSidebar();
  }
}

/// Best-effort cancel of the in-flight turn. The actual UI cleanup
/// (drop the streaming bubble, re-enable input) happens in the
/// `onSubmit` catch branch when `send_message` returns the
/// [`CANCEL_SENTINEL`] error — keeping the cleanup in one place
/// avoids racing the backend's abort path. The button briefly disables
/// here so a rapid double-click doesn't fire two cancel_response
/// commands (the second would be a no-op but the visual flash looks
/// glitchy).
async function onCancel() {
  dom.send.disabled = true;
  try {
    await invoke("cancel_response");
  } catch (err) {
    console.warn("cancel_response failed:", err);
  } finally {
    // Re-enable so the user can click again if the backend didn't
    // actually abort (shouldn't happen, but the button shouldn't
    // become permanently stuck if it does).
    dom.send.disabled = false;
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
    setComposerState("idle");
    // Fire-and-forget — sidebar updates are non-critical; a failure
    // here shouldn't deny the user the chat surface.
    refreshSidebar();
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

/// Refresh every sidebar section in parallel. One IPC round-trip per
/// section keeps the DM-lock duration small (each is a brief read,
/// not a stream) and lets the sections fail independently.
async function refreshSidebar() {
  await Promise.all([refreshSignals(), refreshLearner(), refreshTurnList()]);
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

async function refreshLearner() {
  try {
    const snap = await invoke("get_learner_state");
    renderLearner(snap);
  } catch (err) {
    console.warn("get_learner_state failed:", err);
  }
}

async function refreshTurnList() {
  try {
    const list = await invoke("list_session_turns");
    renderTurnList(list);
  } catch (err) {
    console.warn("list_session_turns failed:", err);
  }
}

function renderTurnList(list) {
  const s = dom.session;
  const turns = list ?? [];
  if (turns.length === 0) {
    s.list.hidden = true;
    s.list.replaceChildren();
    s.empty.hidden = false;
    s.hint.hidden = true;
    return;
  }
  s.empty.hidden = true;
  s.list.hidden = false;
  s.hint.hidden = false;
  s.list.replaceChildren(...turns.map(renderTurnRow));
}

function renderTurnRow(turn) {
  // Use a <button> inside <li> so the row is keyboard-focusable and
  // announced as an interactive element rather than plain text.
  const li = document.createElement("li");
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "turn-row";
  btn.dataset.turnIndex = String(turn.index);
  // Backend serialises `Speaker` as lowercase `"child"` / `"primer"`
  // via `speaker_name` in commands/session.rs — used as a `[data-speaker=…]`
  // selector hook in styles.css.
  btn.dataset.speaker = turn.speaker;
  btn.setAttribute(
    "aria-label",
    `Turn ${turn.index + 1}, ${turn.speaker}: ${turn.text_preview}`,
  );

  const idxEl = document.createElement("span");
  idxEl.className = "turn-index";
  idxEl.textContent = `T${turn.index + 1}`;

  const speakerEl = document.createElement("span");
  speakerEl.className = "turn-speaker";
  speakerEl.textContent = turn.speaker;

  const previewEl = document.createElement("span");
  previewEl.className = "turn-preview";

  const textEl = document.createElement("span");
  textEl.className = "turn-text";
  textEl.textContent = turn.text_preview;
  if (turn.truncated) {
    textEl.title = `${turn.text_preview} (truncated)`;
  }
  previewEl.appendChild(textEl);

  const intent = turn.intent;
  const conceptCount = turn.concepts.length;
  if (intent || conceptCount > 0) {
    const meta = document.createElement("span");
    meta.className = "turn-meta";
    if (intent) {
      const intentBadge = document.createElement("span");
      intentBadge.className = "turn-intent";
      intentBadge.textContent = intent;
      meta.appendChild(intentBadge);
    }
    if (conceptCount > 0) {
      const conceptEl = document.createElement("span");
      conceptEl.className = "turn-concept-count";
      conceptEl.textContent =
        conceptCount === 1 ? "1 concept" : `${conceptCount} concepts`;
      meta.appendChild(conceptEl);
    }
    previewEl.appendChild(meta);
  }

  btn.appendChild(idxEl);
  btn.appendChild(speakerEl);
  btn.appendChild(previewEl);
  btn.addEventListener("click", () => scrollChatToTurn(turn.index));
  li.appendChild(btn);
  return li;
}

/// Scroll the main chat scroll area to the bubble matching the given
/// turn index, then briefly outline it so the user can find it. The
/// bubbles carry `data-turn-index` from append-time (with a roll-back
/// path on mid-stream error so indices stay aligned with the backend's
/// session turns); this is a direct DOM query, no per-row
/// event-listener bookkeeping needed.
function scrollChatToTurn(index) {
  const row = dom.chatScroll.querySelector(
    `.bubble-row[data-turn-index="${index}"]`,
  );
  if (!row) return;
  row.scrollIntoView({ behavior: "smooth", block: "center" });
  // Cancel any in-flight spotlight before scheduling this one so a
  // rapid click sequence within the 1.6 s highlight window doesn't
  // let an earlier timeout strip the new target's highlight.
  if (state.spotlightTimer !== null) {
    clearTimeout(state.spotlightTimer);
    dom.chatScroll
      .querySelectorAll(".bubble-row.is-spotlight")
      .forEach((r) => r.classList.remove("is-spotlight"));
  }
  row.classList.add("is-spotlight");
  state.spotlightTimer = setTimeout(() => {
    row.classList.remove("is-spotlight");
    state.spotlightTimer = null;
  }, 1600);
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

function renderLearner(snap) {
  const l = dom.learner;
  if (!snap) {
    l.profileCard.hidden = true;
    l.vocabCard.hidden = true;
    l.distributionCard.hidden = true;
    l.engagementCard.hidden = true;
    return;
  }

  // Profile
  l.profileCard.hidden = false;
  l.name.textContent = snap.profile.name;
  l.ageLocale.textContent = `age ${snap.profile.age} · ${snap.profile.locale}`;
  l.uuid.textContent = snap.profile.id;
  l.uuid.dataset.uuid = snap.profile.id;

  // Vocabulary
  const due = snap.vocab_due ?? [];
  if (snap.concept_count > 0) {
    l.vocabCard.hidden = false;
    // Suppress the "0 due" pill when the list is empty — the inline
    // empty-state message already says it more clearly.
    l.vocabCount.textContent = due.length > 0 ? `${due.length} due` : "";
    l.vocabList.replaceChildren();
    if (due.length === 0) {
      const div = document.createElement("div");
      div.className = "vocab-empty";
      div.textContent = "Nothing's due — all concepts are within their review intervals.";
      l.vocabList.appendChild(div);
    } else {
      for (const concept of due) {
        l.vocabList.appendChild(renderVocabItem(concept));
      }
    }
  } else {
    l.vocabCard.hidden = true;
  }

  // Depth distribution
  if (snap.concept_count > 0) {
    l.distributionCard.hidden = false;
    l.conceptCount.textContent = `${snap.concept_count} total`;
    renderDepthBar(l.depthBar, snap.depth_distribution);
    renderDepthLegend(l.depthLegend, snap.depth_distribution);
  } else {
    l.distributionCard.hidden = true;
  }

  // Recent engagement strip
  const recent = snap.recent_engagement ?? [];
  if (recent.length > 0) {
    l.engagementCard.hidden = false;
    l.engagementStrip.replaceChildren();
    for (const state of recent) {
      const dot = document.createElement("span");
      dot.className = "dot";
      dot.dataset.state = state.toLowerCase();
      dot.title = state;
      l.engagementStrip.appendChild(dot);
    }
  } else {
    l.engagementCard.hidden = true;
  }
}

function renderVocabItem(c) {
  const li = document.createElement("li");
  const concept = document.createElement("span");
  concept.className = "concept";
  concept.textContent = c.concept_id;
  concept.title = `${c.concept_id} (${c.depth})`;
  const dots = document.createElement("span");
  dots.className = "box-dots";
  dots.setAttribute("aria-label", `box level ${c.box_level} of ${MAX_BOX_LEVEL}`);
  dots.textContent = renderBoxDots(c.box_level);
  const when = document.createElement("span");
  when.className = "due-when";
  when.textContent = formatDueWhen(c.days_until_due);
  when.dataset.overdue = String(c.days_until_due < 0);
  li.appendChild(concept);
  li.appendChild(dots);
  li.appendChild(when);
  return li;
}

function renderBoxDots(level) {
  const filled = Math.max(0, Math.min(MAX_BOX_LEVEL, level));
  return "●".repeat(filled) + "○".repeat(MAX_BOX_LEVEL - filled);
}

function formatDueWhen(days) {
  if (days < 0) {
    const n = -days;
    return n === 1 ? "1 day late" : `${n} days late`;
  }
  if (days === 0) return "due now";
  if (days === 1) return "due tmrw";
  return `due ${days}d`;
}

function renderDepthBar(barEl, counts) {
  barEl.replaceChildren();
  const total = counts.reduce((s, r) => s + r.count, 0);
  for (const row of counts) {
    if (row.count === 0) continue;
    const seg = document.createElement("span");
    seg.className = "seg";
    seg.dataset.depth = row.depth.toLowerCase();
    seg.style.flexGrow = String(row.count);
    seg.title = `${row.depth}: ${row.count} of ${total}`;
    barEl.appendChild(seg);
  }
}

function renderDepthLegend(legendEl, counts) {
  legendEl.replaceChildren();
  for (const row of counts) {
    const li = document.createElement("li");
    const swatch = document.createElement("span");
    swatch.className = "swatch";
    swatch.style.background = `var(--depth-${row.depth.toLowerCase()})`;
    const label = document.createElement("span");
    label.textContent = `${row.depth} · ${row.count}`;
    li.appendChild(swatch);
    li.appendChild(label);
    legendEl.appendChild(li);
  }
}

function setupUuidCopy() {
  dom.learner.uuidCopy.addEventListener("click", async () => {
    const uuid = dom.learner.uuid.dataset.uuid || dom.learner.uuid.textContent || "";
    if (!uuid || uuid === "—") return;
    try {
      await navigator.clipboard.writeText(uuid);
      dom.learner.uuidCopy.dataset.state = "copied";
      dom.learner.uuidCopy.textContent = "Copied";
      setTimeout(() => {
        dom.learner.uuidCopy.dataset.state = "";
        dom.learner.uuidCopy.textContent = "Copy";
      }, 1400);
    } catch (err) {
      // Tauri's WebView clipboard is generally available; falling
      // through is just a no-op for the rare case the API isn't.
      console.warn("clipboard.writeText failed:", err);
    }
  });
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
  row.dataset.turnIndex = String(state.nextTurnIndex);
  state.nextTurnIndex += 1;
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
  row.dataset.turnIndex = String(state.nextTurnIndex);
  state.nextTurnIndex += 1;
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
  if (aborted) {
    // The partial Primer turn is never persisted (CLAUDE.md: "On a
    // mid-stream error, the partial Primer turn is not recorded into
    // the session"). Roll back the index we provisionally claimed in
    // `appendStreamingPrimerBubble` so the next exchange's bubbles
    // realign with backend `session.turns` indices; otherwise the
    // Session-sidebar click-to-scroll would target the wrong bubble.
    state.nextTurnIndex -= 1;
    if (el.textContent.trim() === "") {
      // Empty-aborted: drop the placeholder entirely.
      el.parentElement?.remove();
    } else {
      // Non-empty partial: keep the visible text (the child saw it)
      // but strip its data-turn-index — there's no backend turn for
      // this bubble to be addressed by, and leaving the attribute on
      // would collide with the next exchange's primer bubble.
      el.parentElement?.removeAttribute("data-turn-index");
    }
  }
  state.streamingPrimerEl = null;
}

/// Append a text chunk to the streaming Primer bubble for `turnIndex`.
///
/// Called by voice.js for `primer://voice/response_chunk` events. On the
/// first chunk for a given index, a new streaming bubble is created; on
/// subsequent chunks the existing bubble's text is extended.  When the
/// response_complete event fires, voice.js calls primerRefreshSidebar and
/// the bubble remains on screen in its final (non-streaming) state.
function appendPrimerChunk(turnIndex, text) {
  // If a streaming bubble for this turn index is already alive, append to it.
  if (
    state.streamingPrimerEl &&
    state.streamingPrimerEl.parentElement &&
    state.streamingPrimerEl.parentElement.dataset.turnIndex === String(turnIndex)
  ) {
    state.streamingPrimerEl.textContent += text;
    scrollToBottom();
    return;
  }
  // Finalise any pre-existing streaming bubble from a prior turn (guard
  // against a missed response_complete in the text path).
  if (state.streamingPrimerEl) {
    state.streamingPrimerEl.classList.remove("is-streaming");
    state.streamingPrimerEl = null;
  }
  // Start a new streaming bubble with the given turn index.
  hideEmptyState();
  const row = document.createElement("div");
  row.className = "bubble-row is-primer";
  row.dataset.turnIndex = String(turnIndex);
  const bubble = document.createElement("div");
  bubble.className = "bubble is-streaming";
  bubble.textContent = text;
  row.appendChild(bubble);
  dom.chatScroll.appendChild(row);
  state.streamingPrimerEl = bubble;
  scrollToBottom();
}

/// Simple toast for voice.js (settings.js has its own local showToast).
let _appToastTimer = null;
function showToast(msg) {
  const el = document.getElementById("toast");
  if (!el) return;
  el.textContent = msg;
  el.hidden = false;
  if (_appToastTimer !== null) clearTimeout(_appToastTimer);
  _appToastTimer = setTimeout(() => {
    el.hidden = true;
    _appToastTimer = null;
  }, 2500);
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

// ─── Window-level exports for voice.js ──────────────────────────────
// voice.js is an IIFE (like this file) and loaded after app.js, so it
// reads these from `window` rather than sharing a module scope.
window.primerAppendChildBubble = appendChildBubble;
window.primerAppendPrimerChunk  = appendPrimerChunk;
window.primerRefreshSidebar     = refreshSidebar;
window.primerShowError          = showError;
window.primerShowToast          = showToast;

})();
