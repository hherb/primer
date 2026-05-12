// Settings modal controller.
//
// Loaded before app.js so it can expose `window.PrimerSettings` for
// the chat shell to wire its "Settings" button to. The modal:
//   • Populates from `get_settings` (returns a redacted GuiConfigView).
//   • Gathers form state into a GuiConfigUpdate on submit.
//   • Validation runs server-side via `update_settings`; any error
//     comes back as a string and is rendered in the modal banner.
//   • "Save & start new session" close_session + start_session on
//     success; "Save (next session only)" just persists.
//
// The inline API key never round-trips: the view says only "is a key
// set", and the update path defaults to `ApiKeyUpdate::Keep` unless
// the user explicitly typed a new key or switched to env.

const { invoke } = window.__TAURI__.core;

// Locale list must stay in sync with `primer_core::i18n::Locale::ALL`.
// The validation step (`validate_locale`) is the authoritative check;
// this list is just for the dropdown UI.
const LOCALE_CHOICES = [
  { id: "en", label: "English" },
  { id: "de", label: "Deutsch" },
];

const SUBSYSTEMS = ["classifier", "extractor", "comprehension"];

const dom = {
  backdrop: document.getElementById("settings-backdrop"),
  modal: document.getElementById("settings-modal"),
  close: document.getElementById("settings-close"),
  cancel: document.getElementById("settings-cancel"),
  saveNext: document.getElementById("settings-save-next"),
  saveRestart: document.getElementById("settings-save-restart"),
  banner: document.getElementById("settings-banner"),
  form: document.getElementById("settings-form"),
  activeHint: document.getElementById("settings-active-hint"),
  toast: document.getElementById("toast"),
  fields: {
    learnerName: document.getElementById("f-learner-name"),
    learnerAge: document.getElementById("f-learner-age"),
    learnerLocale: document.getElementById("f-learner-locale"),
    backendKind: document.getElementById("f-backend-kind"),
    backendModel: document.getElementById("f-backend-model"),
    backendOllamaUrl: document.getElementById("f-backend-ollama-url"),
    backendOllamaUrlField: document.getElementById("f-backend-ollama-url-field"),
    apiKeyFieldset: document.getElementById("f-api-key-fieldset"),
    apiKeyEnv: document.getElementById("f-api-key-env"),
    apiKeyInline: document.getElementById("f-api-key-inline"),
    apiKeyInputField: document.getElementById("f-api-key-input-field"),
    apiKeyInputLabel: document.getElementById("f-api-key-input-label"),
    apiKeyInput: document.getElementById("f-api-key-input"),
    apiKeyHint: document.getElementById("f-api-key-hint"),
    embedderKind: document.getElementById("f-embedder-kind"),
    embedderModel: document.getElementById("f-embedder-model"),
    embedderOllamaUrl: document.getElementById("f-embedder-ollama-url"),
    embedderOllamaUrlField: document.getElementById("f-embedder-ollama-url-field"),
    vocabMax: document.getElementById("f-vocab-max"),
    breaksAfterMins: document.getElementById("f-breaks-after-mins"),
    sessionDb: document.getElementById("f-persistence-session-db"),
    knowledgeDb: document.getElementById("f-persistence-knowledge-db"),
    noPersist: document.getElementById("f-persistence-no-persist"),
  },
};

const state = {
  /// `has_key` flag we last saw from the view. Used to decide whether
  /// the "Save" path should send `Keep` (preserve existing inline) or
  /// reject an empty-string inline-key (no key on disk + empty field
  /// means the user picked Inline but never typed — clearly an error).
  hasInlineKey: false,
  /// Set when an open() call is in-flight so a second click while we
  /// wait on `get_settings` doesn't double-open the modal.
  isOpening: false,
  /// Resolves the in-flight save's promise; set while either save
  /// button is mid-flight so we can disable both for the duration.
  isSaving: false,
  /// Callback fired after a successful "Save & start new session" so
  /// the chat shell can refresh its session info / sidebar / bubbles.
  onSessionRestarted: null,
  /// Snapshot of the `ui` section from the most recent `get_settings`.
  /// The modal doesn't expose UI fields (sidebar_open / last_section
  /// are owned by other surfaces), so we round-trip the persisted
  /// values verbatim on save rather than substituting defaults — which
  /// would clobber the user's last-active sidebar section.
  lastUi: null,
};

// Initial DOM wiring — runs at script-load time. The modal stays
// hidden via the `hidden` attribute until `open()` is called.
populateLocaleChoices();
wireDismiss();
wireBackendKindReveal();
wireEmbedderKindReveal();
wireApiKeyRadios();
wireSubsystemMatchMain();
wireNoPersistToggle();
wireSaveButtons();

window.PrimerSettings = { open, close: closeModal };

async function open({ onSessionRestarted } = {}) {
  if (state.isOpening) return;
  state.isOpening = true;
  state.onSessionRestarted = onSessionRestarted ?? null;
  hideBanner();
  try {
    const [view, sessionInfo] = await Promise.all([
      invoke("get_settings"),
      invoke("current_session_info").catch(() => null),
    ]);
    populate(view);
    dom.activeHint.hidden = sessionInfo === null;
    dom.backdrop.hidden = false;
    dom.backdrop.setAttribute("aria-hidden", "false");
    document.addEventListener("keydown", onEscape);
    // Focus the first input so keyboard users can start editing.
    dom.fields.learnerName.focus();
  } catch (err) {
    showBanner(`Couldn't load settings: ${formatErr(err)}`);
    dom.backdrop.hidden = false;
    dom.backdrop.setAttribute("aria-hidden", "false");
  } finally {
    state.isOpening = false;
  }
}

function closeModal() {
  dom.backdrop.hidden = true;
  dom.backdrop.setAttribute("aria-hidden", "true");
  document.removeEventListener("keydown", onEscape);
  state.onSessionRestarted = null;
}

function onEscape(e) {
  if (e.key === "Escape" && !state.isSaving) {
    e.preventDefault();
    closeModal();
  }
}

function wireDismiss() {
  dom.close.addEventListener("click", closeModal);
  dom.cancel.addEventListener("click", closeModal);
  // Click on backdrop (but not on the modal card) closes.
  dom.backdrop.addEventListener("click", (e) => {
    if (e.target === dom.backdrop && !state.isSaving) closeModal();
  });
}

function populateLocaleChoices() {
  const sel = dom.fields.learnerLocale;
  sel.replaceChildren();
  for (const { id, label } of LOCALE_CHOICES) {
    const opt = document.createElement("option");
    opt.value = id;
    opt.textContent = `${label} (${id})`;
    sel.appendChild(opt);
  }
}

function populate(view) {
  const f = dom.fields;
  state.lastUi = view.ui ?? null;
  // Learner
  f.learnerName.value = view.learner.name;
  f.learnerAge.value = view.learner.age;
  // If the persisted locale isn't in LOCALE_CHOICES (e.g. a future
  // pack), still show it so the user isn't silently switched.
  if (!LOCALE_CHOICES.some((l) => l.id === view.learner.locale)) {
    const opt = document.createElement("option");
    opt.value = view.learner.locale;
    opt.textContent = `${view.learner.locale} (unknown pack)`;
    f.learnerLocale.appendChild(opt);
  }
  f.learnerLocale.value = view.learner.locale;

  // Backend
  f.backendKind.value = view.backend.kind;
  f.backendModel.value = view.backend.model ?? "";
  f.backendOllamaUrl.value = view.backend.ollama_url;
  applyBackendKindReveal(view.backend.kind);

  // API key
  state.hasInlineKey =
    view.backend.api_key_source.kind === "inline" &&
    view.backend.api_key_source.has_key === true;
  if (view.backend.api_key_source.kind === "inline") {
    f.apiKeyInline.checked = true;
  } else {
    f.apiKeyEnv.checked = true;
  }
  applyApiKeyReveal();
  // Reset the password field on every open — never show the previous
  // session's typed key when re-opening.
  f.apiKeyInput.value = "";

  // Subsystems
  for (const name of SUBSYSTEMS) {
    populateSubsystem(name, view[name]);
  }

  // Embedder
  f.embedderKind.value = view.embedder.kind;
  f.embedderModel.value = view.embedder.model ?? "";
  f.embedderOllamaUrl.value = view.embedder.ollama_url ?? "";
  applyEmbedderKindReveal(view.embedder.kind);

  // Vocab & breaks
  f.vocabMax.value =
    view.vocab.max_per_prompt === null || view.vocab.max_per_prompt === undefined
      ? ""
      : view.vocab.max_per_prompt;
  f.breaksAfterMins.value = view.breaks.after_mins;

  // Persistence
  f.sessionDb.value = view.persistence.session_db ?? "";
  f.knowledgeDb.value = view.persistence.knowledge_db ?? "";
  f.noPersist.checked = view.persistence.no_persist === true;
  applyNoPersistReveal();
}

function wireNoPersistToggle() {
  dom.fields.noPersist.addEventListener("change", applyNoPersistReveal);
}

function applyNoPersistReveal() {
  // When in-memory-only is on, both path inputs are ignored by the
  // wiring code — disable them so the form reflects that the values
  // won't be honoured.
  const disabled = dom.fields.noPersist.checked;
  dom.fields.sessionDb.disabled = disabled;
  dom.fields.knowledgeDb.disabled = disabled;
}

function populateSubsystem(name, cfg) {
  const root = dom.form.querySelector(`[data-subsystem="${name}"]`);
  const matchEl = root.querySelector('[data-field="match-main"]');
  const kindEl = root.querySelector('[data-field="kind"]');
  const modelEl = root.querySelector('[data-field="model"]');
  const timeoutEl = root.querySelector('[data-field="timeout-ms"]');
  matchEl.checked = cfg.match_main === true;
  kindEl.value = cfg.kind ?? "";
  modelEl.value = cfg.model ?? "";
  timeoutEl.value = cfg.timeout_ms;
  applySubsystemOverride(root, matchEl.checked);
}

function applySubsystemOverride(root, matchMain) {
  root.dataset.overridden = String(matchMain);
  for (const el of root.querySelectorAll(
    '[data-field="kind"], [data-field="model"]',
  )) {
    el.disabled = matchMain;
  }
  // Timeout always stays editable — it applies whether the subsystem
  // matches the main backend or runs its own. Disabling it would
  // surprise users who want to tune classifier latency without
  // forking the backend.
}

function wireSubsystemMatchMain() {
  for (const name of SUBSYSTEMS) {
    const root = dom.form.querySelector(`[data-subsystem="${name}"]`);
    const matchEl = root.querySelector('[data-field="match-main"]');
    matchEl.addEventListener("change", () =>
      applySubsystemOverride(root, matchEl.checked),
    );
  }
}

function wireBackendKindReveal() {
  dom.fields.backendKind.addEventListener("change", () => {
    applyBackendKindReveal(dom.fields.backendKind.value);
  });
}

function applyBackendKindReveal(kind) {
  // Ollama URL only relevant for the ollama backend.
  dom.fields.backendOllamaUrlField.hidden = kind !== "ollama";
  // API key fieldset only relevant for cloud — fade it for stub/ollama
  // so the user isn't tempted to put a key where it'd be ignored.
  const cloud = kind === "cloud";
  if (cloud) {
    dom.fields.apiKeyFieldset.removeAttribute("disabled");
  } else {
    dom.fields.apiKeyFieldset.setAttribute("disabled", "");
  }
}

function wireEmbedderKindReveal() {
  dom.fields.embedderKind.addEventListener("change", () => {
    applyEmbedderKindReveal(dom.fields.embedderKind.value);
  });
}

function applyEmbedderKindReveal(kind) {
  dom.fields.embedderOllamaUrlField.hidden = kind !== "ollama";
}

function wireApiKeyRadios() {
  for (const radio of [dom.fields.apiKeyEnv, dom.fields.apiKeyInline]) {
    radio.addEventListener("change", applyApiKeyReveal);
  }
}

function applyApiKeyReveal() {
  const inline = dom.fields.apiKeyInline.checked;
  dom.fields.apiKeyInputField.hidden = !inline;
  if (inline) {
    if (state.hasInlineKey) {
      dom.fields.apiKeyInputLabel.textContent = "Replace inline key (or leave blank to keep existing)";
      dom.fields.apiKeyInput.placeholder = "(leave blank to keep the existing key)";
      dom.fields.apiKeyHint.textContent =
        "An inline key is already stored. Leave blank to keep it; fill to overwrite.";
    } else {
      dom.fields.apiKeyInputLabel.textContent = "Anthropic API key";
      dom.fields.apiKeyInput.placeholder = "sk-ant-…";
      dom.fields.apiKeyHint.textContent =
        "Will be saved to ~/.primer/gui-config.json (file mode 0600).";
    }
  }
}

function wireSaveButtons() {
  dom.saveNext.addEventListener("click", () => onSave({ restart: false }));
  dom.saveRestart.addEventListener("click", () => onSave({ restart: true }));
}

async function onSave({ restart }) {
  if (state.isSaving) return;
  hideBanner();

  let update;
  try {
    update = gather();
  } catch (err) {
    showBanner(err.message ?? String(err));
    return;
  }

  state.isSaving = true;
  setButtonsBusy(true, { restarting: restart });
  try {
    await invoke("update_settings", { config: update });
    if (restart) {
      // Close any active session then start fresh with the new config.
      // close_session is a no-op when no session is active, so we can
      // call it unconditionally.
      await invoke("close_session").catch(() => {});
      await invoke("start_session");
      // Snapshot the callback before closeModal() clears it.
      const cb = state.onSessionRestarted;
      closeModal();
      if (cb) {
        try {
          await cb();
        } catch (cbErr) {
          // Callback failures shouldn't deny the user a closed modal —
          // the settings were saved and the new session is live. We
          // surface the failure via console + toast rather than
          // silently swallowing it (covers async rejections too, since
          // we now `await` the callback).
          console.warn("onSessionRestarted callback failed:", cbErr);
        }
      }
      showToast("Settings saved — new session started.");
    } else {
      closeModal();
      showToast("Settings saved — changes take effect at the next session start.");
    }
  } catch (err) {
    showBanner(formatErr(err));
  } finally {
    state.isSaving = false;
    setButtonsBusy(false, {});
  }
}

const RESTART_LABEL_IDLE = "Save & start new session";
const RESTART_LABEL_BUSY = "Restarting session…";

function setButtonsBusy(busy, { restarting = false } = {}) {
  dom.saveNext.disabled = busy;
  dom.saveRestart.disabled = busy;
  dom.cancel.disabled = busy;
  dom.close.disabled = busy;
  // Restart can take seconds (close_session drains background analysis
  // tasks, start_session re-wires backends + may construct an
  // embedder). Swap the button label so the user has a visible cue
  // that work is in flight instead of looking at a silent disabled UI.
  if (busy && restarting) {
    dom.saveRestart.textContent = RESTART_LABEL_BUSY;
    dom.saveRestart.setAttribute("aria-busy", "true");
  } else {
    dom.saveRestart.textContent = RESTART_LABEL_IDLE;
    dom.saveRestart.removeAttribute("aria-busy");
  }
}

/// Read every field into a `GuiConfigUpdate` shape matching what
/// `update_settings` deserialises. Throws a user-facing `Error` for
/// any client-side issue that the server-side validator can't catch
/// — today only the "Inline picked but no key available" case.
function gather() {
  const f = dom.fields;

  const name = f.learnerName.value.trim();
  if (name.length === 0) {
    throw new Error("Learner name is required.");
  }
  const age = parseIntOrNull(f.learnerAge.value);
  if (age === null || age < 1) {
    throw new Error("Learner age must be a whole number ≥ 1.");
  }

  const apiKeyUpdate = resolveApiKeyUpdate();

  return {
    learner: {
      name,
      age,
      locale: f.learnerLocale.value,
    },
    backend: {
      kind: f.backendKind.value,
      model: orNull(f.backendModel.value.trim()),
      ollama_url: f.backendOllamaUrl.value.trim(),
      api_key_source: apiKeyUpdate,
    },
    classifier: gatherSubsystem("classifier"),
    extractor: gatherSubsystem("extractor"),
    comprehension: gatherSubsystem("comprehension"),
    embedder: {
      kind: f.embedderKind.value,
      model: orNull(f.embedderModel.value.trim()),
      ollama_url: orNull(f.embedderOllamaUrl.value.trim()),
    },
    vocab: {
      max_per_prompt: orNullNumber(f.vocabMax.value),
    },
    breaks: {
      after_mins: parseIntOrZero(f.breaksAfterMins.value),
    },
    persistence: {
      session_db: orNull(f.sessionDb.value.trim()),
      knowledge_db: orNull(f.knowledgeDb.value.trim()),
      no_persist: f.noPersist.checked,
    },
    // UI section is owned by other surfaces (sidebar toggle, sidebar
    // section tabs) — the modal doesn't render any UI fields, so we
    // round-trip whatever `get_settings` returned. Falling back to
    // defaults only when we never saw a view (defensive — populate()
    // captures lastUi on every successful open).
    ui: state.lastUi ?? {
      sidebar_open: !document.body.classList.contains("sidebar-collapsed"),
      last_section: "current_turn",
    },
  };
}

function gatherSubsystem(name) {
  const root = dom.form.querySelector(`[data-subsystem="${name}"]`);
  const matchMain = root.querySelector('[data-field="match-main"]').checked;
  const kind = root.querySelector('[data-field="kind"]').value;
  const model = root.querySelector('[data-field="model"]').value.trim();
  const timeoutMs = parseIntOrZero(
    root.querySelector('[data-field="timeout-ms"]').value,
  );
  return {
    match_main: matchMain,
    kind: matchMain ? null : orNull(kind),
    model: matchMain ? null : orNull(model),
    timeout_ms: timeoutMs,
  };
}

function resolveApiKeyUpdate() {
  if (dom.fields.apiKeyEnv.checked) {
    return { kind: "env" };
  }
  // Inline path.
  const typed = dom.fields.apiKeyInput.value;
  if (typed.length > 0) {
    return { kind: "inline", key: typed };
  }
  if (state.hasInlineKey) {
    // User picked inline but didn't type — keep the existing secret.
    return { kind: "keep" };
  }
  // No existing key + nothing typed: refuse to save a half-configured
  // cloud backend. The server-side validator doesn't catch this
  // because `Keep` resolves to whatever was on disk (which here is
  // Env), so the saved config would silently switch back to env.
  throw new Error(
    "Inline API key selected but no key entered. Type a key or pick 'Read from environment variable'.",
  );
}

function parseIntOrZero(s) {
  const n = parseInt(s, 10);
  return Number.isFinite(n) ? n : 0;
}

function parseIntOrNull(s) {
  const trimmed = String(s).trim();
  if (trimmed.length === 0) return null;
  const n = parseInt(trimmed, 10);
  return Number.isFinite(n) ? n : null;
}

function orNull(s) {
  return s.length > 0 ? s : null;
}

function orNullNumber(s) {
  const trimmed = String(s).trim();
  if (trimmed.length === 0) return null;
  const n = parseInt(trimmed, 10);
  return Number.isFinite(n) ? n : null;
}

function showBanner(msg) {
  dom.banner.textContent = msg;
  dom.banner.hidden = false;
}

function hideBanner() {
  dom.banner.hidden = true;
  dom.banner.textContent = "";
}

let toastTimer = null;
function showToast(msg) {
  dom.toast.textContent = msg;
  dom.toast.hidden = false;
  if (toastTimer !== null) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    dom.toast.hidden = true;
    toastTimer = null;
  }, 2500);
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
