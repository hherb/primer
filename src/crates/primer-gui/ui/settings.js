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
//
// IIFE wrap: see picker.js header — top-level `const invoke` collides
// across classic scripts otherwise.
(() => {
const { invoke } = window.__TAURI__.core;

// Locale choices come from the `list_locales` Tauri command, which
// reads `primer_core::i18n::Locale::ALL`. Sourcing the list from Rust
// means a new locale pack is a Rust-only edit — preview locales are
// excluded automatically. `state.localeChoices` is populated by the
// first `open()` call.

const SUBSYSTEMS = ["classifier", "extractor", "comprehension"];

const dom = {
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
    backendOpenaiCompatUrl: document.getElementById("f-backend-openai-compat-url"),
    backendOpenaiCompatUrlField: document.getElementById("f-backend-openai-compat-url-field"),
    backendQnnBundleDir: document.getElementById("f-backend-qnn-bundle-dir"),
    backendQnnBundleDirField: document.getElementById("f-backend-qnn-bundle-dir-field"),
    backendQnnQairtLibDir: document.getElementById("f-backend-qnn-qairt-lib-dir"),
    backendQnnQairtLibDirField: document.getElementById("f-backend-qnn-qairt-lib-dir-field"),
    backendGgufPath: document.getElementById("f-backend-gguf-path"),
    backendGgufPathField: document.getElementById("f-backend-gguf-path-field"),
    backendLlamacppGpuLayers: document.getElementById("f-backend-llamacpp-gpu-layers"),
    backendLlamacppGpuLayersField: document.getElementById(
      "f-backend-llamacpp-gpu-layers-field",
    ),
    backendLlamacppNCtx: document.getElementById("f-backend-llamacpp-n-ctx"),
    backendLlamacppNCtxField: document.getElementById("f-backend-llamacpp-n-ctx-field"),
    backendReasoningMarkers: document.getElementById("f-backend-reasoning-markers"),
    backendReasoningMarkersField: document.getElementById(
      "f-backend-reasoning-markers-field",
    ),
    backendFallbackBackend: document.getElementById("f-backend-fallback-backend"),
    backendFallbackModel: document.getElementById("f-backend-fallback-model"),
    backendFallbackModelField: document.getElementById(
      "f-backend-fallback-model-field",
    ),
    backendRouterMode: document.getElementById("f-backend-router-mode"),
    backendTtftBudget: document.getElementById("f-backend-ttft-budget"),
    backendTtftBudgetField: document.getElementById("f-backend-ttft-budget-field"),
    apiKeyFieldset: document.getElementById("f-api-key-fieldset"),
    apiKeyEnv: document.getElementById("f-api-key-env"),
    apiKeyInline: document.getElementById("f-api-key-inline"),
    apiKeyInputField: document.getElementById("f-api-key-input-field"),
    apiKeyInputLabel: document.getElementById("f-api-key-input-label"),
    apiKeyInput: document.getElementById("f-api-key-input"),
    apiKeyHint: document.getElementById("f-api-key-hint"),
    ocApiKeyFieldset: document.getElementById("f-openai-compat-api-key-fieldset"),
    ocApiKeyEnv: document.getElementById("f-openai-compat-api-key-env"),
    ocApiKeyInline: document.getElementById("f-openai-compat-api-key-inline"),
    ocApiKeyInputField: document.getElementById("f-openai-compat-api-key-input-field"),
    ocApiKeyInputLabel: document.getElementById("f-openai-compat-api-key-input-label"),
    ocApiKeyInput: document.getElementById("f-openai-compat-api-key-input"),
    ocApiKeyHint: document.getElementById("f-openai-compat-api-key-hint"),
    embedderKind: document.getElementById("f-embedder-kind"),
    embedderModel: document.getElementById("f-embedder-model"),
    embedderOllamaUrl: document.getElementById("f-embedder-ollama-url"),
    embedderOllamaUrlField: document.getElementById("f-embedder-ollama-url-field"),
    embedderOpenaiCompatUrl: document.getElementById("f-embedder-openai-compat-url"),
    embedderOpenaiCompatUrlField: document.getElementById("f-embedder-openai-compat-url-field"),
    vocabMax: document.getElementById("f-vocab-max"),
    breaksAfterMins: document.getElementById("f-breaks-after-mins"),
    sessionDb: document.getElementById("f-persistence-session-db"),
    knowledgeDb: document.getElementById("f-persistence-knowledge-db"),
    noPersist: document.getElementById("f-persistence-no-persist"),
    speechMicSilenceMs: document.getElementById("f-speech-mic-silence-ms"),
    speechDisableAutoDownload: document.getElementById("f-speech-disable-auto-download"),
    speechOverrides: document.getElementById("f-speech-overrides"),
    speechSttBackend: document.getElementById("f-speech-stt-backend"),
    speechTtsBackend: document.getElementById("f-speech-tts-backend"),
    speechMacosNativeUnavailableHint: document.getElementById(
      "f-speech-macos-native-unavailable-hint",
    ),
    speechSupertonicUnavailableHint: document.getElementById(
      "f-speech-supertonic-unavailable-hint",
    ),
  },
};

const state = {
  /// `has_key` flag we last saw from the view. Used to decide whether
  /// the "Save" path should send `Keep` (preserve existing inline) or
  /// reject an empty-string inline-key (no key on disk + empty field
  /// means the user picked Inline but never typed — clearly an error).
  hasInlineKey: false,
  /// Same `has_key` semantics as `hasInlineKey`, but for the
  /// openai-compat server key — tracked separately so the cloud and
  /// openai-compat secrets never cross-contaminate on save.
  hasOcInlineKey: false,
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
  /// Snapshot of `voice_mode_enabled` from the most recent `get_settings`.
  /// The modal doesn't expose a toggle for this (it will be a header
  /// button in PR 3+), so we round-trip it verbatim — never reset it
  /// to false when the user saves the speech settings form.
  lastVoiceModeEnabled: false,
  /// Snapshot of `speech.download_timeout_secs` from the most recent
  /// `get_settings`. The modal doesn't expose a visible input for it, so
  /// gather() round-trips this verbatim — never reset it to the serde
  /// default when the user saves the speech form.
  lastDownloadTimeoutSecs: null,
  /// Whether this binary was compiled with a macOS-native speech stack,
  /// from the `macos_native_speech_available` command. Drives whether the
  /// "macOS Native" backend option is selectable. Re-fetched on each open
  /// (it's a compile-time constant, so the IPC cost is trivial).
  macosNativeAvailable: false,
  /// Whether this binary was compiled with the Supertonic TTS stack, from
  /// the `supertonic_tts_available` command. Drives whether the "Supertonic"
  /// TTS option is selectable. Re-fetched on each open (compile-time const).
  supertonicAvailable: false,
  /// `[{id, label}]` returned by the `list_locales` Tauri command. Cached
  /// across `open()` calls so we don't re-invoke on every modal open.
  /// `null` until the first successful fetch; the fetch shares the same
  /// `Promise.all` as `get_settings`, so an IPC failure aborts the whole
  /// modal load and surfaces an error banner — there is no
  /// partially-populated state to worry about downstream.
  localeChoices: null,
};

// Initial DOM wiring — runs at script-load time. The modal stays
// hidden via the `hidden` attribute until `open()` is called. The
// locale dropdown is populated lazily inside `open()` once the
// `list_locales` IPC returns.
wireDismiss();
wireBackendKindReveal();
wireFallbackReveal();
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
    // `list_locales` is cached after the first successful fetch; on a
    // re-open we skip the IPC and reuse the stored choices.
    const localesPromise =
      state.localeChoices === null
        ? invoke("list_locales")
        : Promise.resolve(state.localeChoices);
    const [view, sessionInfo, locales, macosNativeAvailable, supertonicAvailable] =
      await Promise.all([
        invoke("get_settings"),
        invoke("current_session_info").catch(() => null),
        localesPromise,
        invoke("macos_native_speech_available").catch(() => false),
        invoke("supertonic_tts_available").catch(() => false),
      ]);
    state.localeChoices = locales;
    state.macosNativeAvailable = macosNativeAvailable === true;
    state.supertonicAvailable = supertonicAvailable === true;
    populateLocaleChoices();
    populate(view);
    dom.activeHint.hidden = sessionInfo === null;
    showModal();
    // Focus the first input so keyboard users can start editing. The
    // browser's focus-trap on the dialog ensures Tab cycles between
    // focusable descendants rather than escaping onto the chat shell.
    dom.fields.learnerName.focus();
  } catch (err) {
    showBanner(`Couldn't load settings: ${formatErr(err)}`);
    showModal();
  } finally {
    state.isOpening = false;
  }
}

/// Open the native <dialog>. Guarded against double-showModal() which
/// the spec defines as a no-op only when called with the same args, but
/// some platforms (Tauri webview WebKitGTK 2.40) have reported throws.
function showModal() {
  if (!dom.modal.open) dom.modal.showModal();
}

/// Teardown shared by every dismissal path (explicit button click,
/// backdrop click, Escape via the `cancel` event). Anything that has to
/// run on close-by-any-means lives here so the cancel listener and
/// closeModal() can never drift apart.
function clearTransientState() {
  state.onSessionRestarted = null;
}

function closeModal() {
  if (dom.modal.open) dom.modal.close();
  clearTransientState();
}

function wireDismiss() {
  dom.close.addEventListener("click", closeModal);
  dom.cancel.addEventListener("click", closeModal);
  // Backdrop click closes. Native <dialog> doesn't natively dismiss on
  // backdrop click, but the click bubbles up with event.target equal to
  // the dialog element itself (rather than any descendant) when the
  // user clicks the ::backdrop area. Gating on `!state.isSaving` mirrors
  // the cancel-button behaviour: in-flight saves shouldn't drop the
  // modal mid-operation.
  dom.modal.addEventListener("click", (e) => {
    if (e.target === dom.modal && !state.isSaving) closeModal();
  });
  // The browser fires a `cancel` event on Escape. Prevent it while a
  // save is in flight so the user can't drop the modal mid-operation
  // (which previously would have stranded the in-flight invoke).
  // Otherwise the UA auto-closes the dialog; we mirror the explicit-
  // button path by running the same teardown helper.
  dom.modal.addEventListener("cancel", (e) => {
    if (state.isSaving) {
      e.preventDefault();
      return;
    }
    clearTransientState();
  });
}

// Reads `state.localeChoices`, which is guaranteed non-null here because
// `open()` only calls this after the `list_locales` IPC resolves
// successfully (a rejected IPC short-circuits into the catch branch).
function populateLocaleChoices() {
  const sel = dom.fields.learnerLocale;
  sel.replaceChildren();
  for (const { id, label } of state.localeChoices) {
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
  // If the persisted locale isn't in the choices returned by Rust (e.g.
  // a preview pack reached the user via --language, or a pack id was
  // retired), still show it so the user isn't silently switched.
  if (!state.localeChoices.some((l) => l.id === view.learner.locale)) {
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
  f.backendOpenaiCompatUrl.value = view.backend.openai_compat_url ?? "";
  f.backendQnnBundleDir.value = view.backend.qnn_bundle_dir ?? "";
  f.backendQnnQairtLibDir.value = view.backend.qnn_qairt_lib_dir ?? "";
  f.backendGgufPath.value = view.backend.gguf_path ?? "";
  f.backendLlamacppGpuLayers.value = view.backend.llamacpp_gpu_layers ?? "";
  f.backendLlamacppNCtx.value = view.backend.llamacpp_n_ctx ?? "";
  f.backendReasoningMarkers.value = view.backend.reasoning_markers ?? "";
  f.backendFallbackBackend.value = view.backend.fallback_backend ?? "";
  f.backendFallbackModel.value = view.backend.fallback_model ?? "";
  f.backendRouterMode.value = view.backend.router_mode ?? "local-only";
  f.backendTtftBudget.value = view.backend.primary_ttft_budget_ms ?? "";
  applyRouterModeReveal(f.backendRouterMode.value);
  applyBackendKindReveal(view.backend.kind);
  applyFallbackReveal(f.backendFallbackBackend.value);

  // API key (cloud)
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

  // API key (openai-compat server)
  const ocSrc = view.backend.openai_compat_api_key_source ?? { kind: "env" };
  state.hasOcInlineKey = ocSrc.kind === "inline" && ocSrc.has_key === true;
  if (ocSrc.kind === "inline") {
    f.ocApiKeyInline.checked = true;
  } else {
    f.ocApiKeyEnv.checked = true;
  }
  applyOcApiKeyReveal();
  f.ocApiKeyInput.value = "";

  // Subsystems
  for (const name of SUBSYSTEMS) {
    populateSubsystem(name, view[name]);
  }

  // Embedder
  f.embedderKind.value = view.embedder.kind;
  f.embedderModel.value = view.embedder.model ?? "";
  f.embedderOllamaUrl.value = view.embedder.ollama_url ?? "";
  f.embedderOpenaiCompatUrl.value = view.embedder.openai_compat_url ?? "";
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

  // Speech
  state.lastVoiceModeEnabled = view.speech?.voice_mode_enabled === true;
  state.lastDownloadTimeoutSecs = view.speech?.download_timeout_secs ?? null;
  f.speechMicSilenceMs.value = view.speech?.mic_silence_ms ?? 600;
  f.speechDisableAutoDownload.checked = view.speech?.disable_auto_download === true;
  // STT/TTS are decoupled. Set values first; an option may be disabled
  // just below but the browser keeps a disabled option as the selected
  // value (show-but-disable for a persisted choice the build can't run).
  f.speechSttBackend.value = view.speech?.stt_backend ?? "whisper";
  f.speechTtsBackend.value = view.speech?.tts_backend ?? "piper";
  // Gate macOS-native (STT + TTS options) behind the compiled feature.
  const sttMacosOption = f.speechSttBackend.querySelector('option[value="macos-native"]');
  if (sttMacosOption) sttMacosOption.disabled = !state.macosNativeAvailable;
  const ttsMacosOption = f.speechTtsBackend.querySelector('option[value="macos-native"]');
  if (ttsMacosOption) ttsMacosOption.disabled = !state.macosNativeAvailable;
  f.speechMacosNativeUnavailableHint.hidden = state.macosNativeAvailable;
  // Gate Supertonic behind its feature.
  const supertonicOption = f.speechTtsBackend.querySelector('option[value="supertonic"]');
  if (supertonicOption) supertonicOption.disabled = !state.supertonicAvailable;
  f.speechSupertonicUnavailableHint.hidden = state.supertonicAvailable;
  populateSpeechOverrides(view.speech?.overrides ?? {});

  // Voice-mode status badge — read-only hint so the user understands
  // the header toggle owns the voice_mode_enabled flip, not this form.
  const speechBlock = document.getElementById("speech-settings-fields");
  if (speechBlock) {
    // Remove any previous badge before re-inserting (re-open modal path).
    const existing = speechBlock.querySelector(".voice-mode-status-badge");
    if (existing) existing.remove();
    const status = document.createElement("p");
    status.className = "hint muted voice-mode-status-badge";
    status.textContent = state.lastVoiceModeEnabled
      ? "Voice mode is ON — toggle it off via the header button"
      : "Voice mode is off — toggle it on via the header button";
    speechBlock.insertBefore(status, speechBlock.firstChild);
  }
}

function populateSpeechOverrides(overrides) {
  const container = dom.fields.speechOverrides;
  container.replaceChildren();
  // Speech-override cards mirror the locale dropdown — same source of
  // truth (`list_locales`) so a preview locale doesn't accidentally
  // show up here while excluded from the picker. `populate()` only
  // runs after `state.localeChoices` has been resolved, so it's
  // guaranteed non-null here.
  for (const { id: locale } of state.localeChoices) {
    const ov = overrides[locale] ?? {};
    const card = document.createElement("div");
    card.className = "settings-grid";
    card.dataset.locale = locale;
    card.innerHTML = `
      <h4>${locale.toUpperCase()}</h4>
      <label class="field field-full">
        <span>Piper voice id</span>
        <input type="text" data-field="voice_id" placeholder="(locale default)" value="${ov.voice_id ?? ""}" />
      </label>
      <label class="field field-full">
        <span>Piper .onnx path</span>
        <input type="text" data-field="piper_onnx_path" placeholder="(default cache location)" value="${ov.piper_onnx_path ?? ""}" />
      </label>
      <label class="field field-full">
        <span>Piper .onnx.json path</span>
        <input type="text" data-field="piper_config_path" placeholder="(default cache location)" value="${ov.piper_config_path ?? ""}" />
      </label>
      <label class="field field-full">
        <span>Whisper model path</span>
        <input type="text" data-field="whisper_model_path" placeholder="(default cache location)" value="${ov.whisper_model_path ?? ""}" />
      </label>
      <label class="field field-full">
        <span>Supertonic onnx/ dir</span>
        <input type="text" data-field="supertonic_onnx_dir" placeholder="(set for Supertonic TTS)" value="${ov.supertonic_onnx_dir ?? ""}" />
      </label>
      <label class="field field-full">
        <span>Supertonic voice-style .json</span>
        <input type="text" data-field="supertonic_voice_style_path" placeholder="(set for Supertonic TTS)" value="${ov.supertonic_voice_style_path ?? ""}" />
      </label>
    `;
    container.appendChild(card);
  }
}

function gatherSpeechOverrides() {
  const overrides = {};
  const cards = dom.fields.speechOverrides.querySelectorAll("[data-locale]");
  for (const card of cards) {
    const locale = card.dataset.locale;
    const voiceId = card.querySelector('[data-field="voice_id"]').value.trim();
    const piperOnnx = card.querySelector('[data-field="piper_onnx_path"]').value.trim();
    const piperConfig = card.querySelector('[data-field="piper_config_path"]').value.trim();
    const whisper = card.querySelector('[data-field="whisper_model_path"]').value.trim();
    const supertonicDir = card.querySelector('[data-field="supertonic_onnx_dir"]').value.trim();
    const supertonicStyle = card.querySelector('[data-field="supertonic_voice_style_path"]').value.trim();
    // Only include locale in overrides if at least one field is non-empty.
    if (voiceId || piperOnnx || piperConfig || whisper || supertonicDir || supertonicStyle) {
      overrides[locale] = {
        voice_id: voiceId || null,
        piper_onnx_path: piperOnnx || null,
        piper_config_path: piperConfig || null,
        whisper_model_path: whisper || null,
        supertonic_onnx_dir: supertonicDir || null,
        supertonic_voice_style_path: supertonicStyle || null,
      };
    }
  }
  return overrides;
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

function wireFallbackReveal() {
  dom.fields.backendFallbackBackend.addEventListener("change", () => {
    applyFallbackReveal(dom.fields.backendFallbackBackend.value);
  });
  dom.fields.backendRouterMode.addEventListener("change", () => {
    applyRouterModeReveal(dom.fields.backendRouterMode.value);
  });
}

// The fallback model field only matters once a fallback backend is chosen —
// hide it for "(no fallback)" so the form isn't cluttered. The picker itself
// is always visible: any primary backend may opt into a fallback. Choosing
// cloud as the fallback also enables the cloud API-key fieldset so a local
// primary can supply the key the cloud secondary needs (issue #205 follow-up).
function applyFallbackReveal(fallbackKind) {
  dom.fields.backendFallbackModelField.hidden = !fallbackKind;
  applyCloudKeyEnable();
}

function applyRouterModeReveal(mode) {
  dom.fields.backendTtftBudgetField.hidden = mode !== "hybrid";
}

function applyBackendKindReveal(kind) {
  // Ollama URL only relevant for the ollama backend.
  dom.fields.backendOllamaUrlField.hidden = kind !== "ollama";
  // OpenAI-compat server URL only relevant for that backend.
  dom.fields.backendOpenaiCompatUrlField.hidden = kind !== "openai-compat";
  // QNN bundle / QAIRT-lib paths only relevant for the qnn backend.
  const qnn = kind === "qnn";
  dom.fields.backendQnnBundleDirField.hidden = !qnn;
  dom.fields.backendQnnQairtLibDirField.hidden = !qnn;
  // GGUF path / gpu-layers / n_ctx only relevant for the llamacpp backend.
  const llamacpp = kind === "llamacpp";
  dom.fields.backendGgufPathField.hidden = !llamacpp;
  dom.fields.backendLlamacppGpuLayersField.hidden = !llamacpp;
  dom.fields.backendLlamacppNCtxField.hidden = !llamacpp;
  // Reasoning markers only apply to the ollama / openai-compat backends
  // (stub/cloud/qnn ignore them).
  dom.fields.backendReasoningMarkersField.hidden =
    kind !== "ollama" && kind !== "openai-compat";
  // Each API-key fieldset is only relevant for its own backend — fade
  // the others so the user isn't tempted to put a key where it'd be
  // ignored. `hidden` removes the openai-compat fieldset entirely for
  // non-openai-compat backends (it would otherwise clutter the cloud /
  // stub / ollama forms); the cloud fieldset keeps its long-standing
  // disabled-fade behaviour.
  applyCloudKeyEnable();
  dom.fields.ocApiKeyFieldset.hidden = kind !== "openai-compat";
}

// Enable the cloud API-key fieldset whenever the cloud backend is reachable —
// as the primary OR as the opt-in fallback secondary (issue #205 follow-up).
// A local-primary → cloud-fallback setup needs a way to supply the cloud key;
// without this the fieldset stays disabled and the inline-key path is blocked,
// leaving env-mode the only option (which itself only worked for a cloud
// primary before the wiring fix). Driven by both the backend-kind picker and
// the fallback picker.
function applyCloudKeyEnable() {
  const cloudInUse =
    dom.fields.backendKind.value === "cloud" ||
    dom.fields.backendFallbackBackend.value === "cloud";
  if (cloudInUse) {
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
  dom.fields.embedderOpenaiCompatUrlField.hidden = kind !== "openai-compat";
}

function wireApiKeyRadios() {
  for (const radio of [dom.fields.apiKeyEnv, dom.fields.apiKeyInline]) {
    radio.addEventListener("change", applyApiKeyReveal);
  }
  for (const radio of [dom.fields.ocApiKeyEnv, dom.fields.ocApiKeyInline]) {
    radio.addEventListener("change", applyOcApiKeyReveal);
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

/// Mirror of [`applyApiKeyReveal`] for the openai-compat server key.
/// Kept as a sibling rather than parameterising the original so the
/// long-standing cloud path stays byte-identical and bisects cleanly.
function applyOcApiKeyReveal() {
  const inline = dom.fields.ocApiKeyInline.checked;
  dom.fields.ocApiKeyInputField.hidden = !inline;
  if (inline) {
    if (state.hasOcInlineKey) {
      dom.fields.ocApiKeyInputLabel.textContent =
        "Replace server key (or leave blank to keep existing)";
      dom.fields.ocApiKeyInput.placeholder = "(leave blank to keep the existing key)";
      dom.fields.ocApiKeyHint.textContent =
        "An inline key is already stored. Leave blank to keep it; fill to overwrite.";
    } else {
      dom.fields.ocApiKeyInputLabel.textContent = "Server API key";
      dom.fields.ocApiKeyInput.placeholder = "(only needed for remote providers)";
      dom.fields.ocApiKeyHint.textContent =
        "Optional for local servers (oMLX/LM Studio/vLLM); required for remote providers (Together/Groq). Saved to ~/.primer/gui-config.json (file mode 0600).";
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
  // Only resolve the openai-compat key from the form when that backend is
  // active; the fieldset is hidden otherwise, so its (stale) radio state
  // must not drive the save. `Keep` carries the persisted source forward
  // untouched and side-steps the half-configured throw on, e.g., a save
  // made from the cloud backend after toggling away from openai-compat.
  const ocApiKeyUpdate =
    f.backendKind.value === "openai-compat"
      ? resolveOcApiKeyUpdate()
      : { kind: "keep" };

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
      // BackendConfigUpdate requires a non-null String here; fall back
      // to the CLI default when the field was cleared so we never send
      // an empty URL the wiring layer would choke on.
      openai_compat_url:
        f.backendOpenaiCompatUrl.value.trim() || "http://localhost:8000",
      api_key_source: apiKeyUpdate,
      openai_compat_api_key_source: ocApiKeyUpdate,
      // BackendConfigUpdate has no serde(default), so these two QNN path
      // fields are MANDATORY in the IPC payload — always send them (null
      // when blank). They're plain Option<PathBuf> (not secrets), so no
      // Keep/Env dance.
      qnn_bundle_dir: orNull(f.backendQnnBundleDir.value.trim()),
      qnn_qairt_lib_dir: orNull(f.backendQnnQairtLibDir.value.trim()),
      // llama.cpp GGUF path / gpu-layers / n_ctx — also mandatory (no
      // serde default). GGUF path is null when blank; the two numeric
      // overrides are null when empty, else integers.
      gguf_path: orNull(f.backendGgufPath.value.trim()),
      llamacpp_gpu_layers: orNullNumber(f.backendLlamacppGpuLayers.value),
      llamacpp_n_ctx: orNullNumber(f.backendLlamacppNCtx.value),
      // Raw textarea text — also mandatory (no serde default). Sent
      // verbatim (no trim) so the stored text round-trips exactly; the
      // Rust parser handles all whitespace. Empty string when blank.
      reasoning_markers: f.backendReasoningMarkers.value,
      // Opt-in local→cloud fallback (issue #205) — also mandatory (no serde
      // default). Both null when no fallback is chosen; the picker's empty
      // value and a blank model field map to null via orNull.
      fallback_backend: orNull(f.backendFallbackBackend.value.trim()),
      fallback_model: orNull(f.backendFallbackModel.value.trim()),
      // Phase 1.3 router mode — also mandatory (no serde default). Empty
      // selection falls back to "local-only" (today's no-routing behavior).
      router_mode: f.backendRouterMode.value || "local-only",
      // Latency-aware routing: optional TTFT budget in ms. Blank = null = off.
      primary_ttft_budget_ms: (() => {
        const v = f.backendTtftBudget.value.trim();
        if (!v) return null;
        const n = Number.parseInt(v, 10);
        return Number.isFinite(n) && n > 0 ? n : null;
      })(),
    },
    classifier: gatherSubsystem("classifier"),
    extractor: gatherSubsystem("extractor"),
    comprehension: gatherSubsystem("comprehension"),
    embedder: {
      kind: f.embedderKind.value,
      model: orNull(f.embedderModel.value.trim()),
      ollama_url: orNull(f.embedderOllamaUrl.value.trim()),
      openai_compat_url: orNull(f.embedderOpenaiCompatUrl.value.trim()),
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
    speech: {
      // voice_mode_enabled is owned by a header toggle (PR 3+), not this
      // form — round-trip the persisted value so saving speech settings
      // never silently switches voice mode off.
      voice_mode_enabled: state.lastVoiceModeEnabled,
      disable_auto_download: dom.fields.speechDisableAutoDownload.checked,
      // STT/TTS are decoupled (legacy `backend` is skip-serialized and the
      // new fields win), so we never send a `backend` key.
      stt_backend: dom.fields.speechSttBackend.value,
      tts_backend: dom.fields.speechTtsBackend.value,
      mic_silence_ms: parseIntOrZero(dom.fields.speechMicSilenceMs.value) || 600,
      // No visible input — round-trip the persisted value so saving never
      // resets it to the serde default. `?? undefined` lets serde apply its
      // default only on the (defensive) never-populated path.
      download_timeout_secs: state.lastDownloadTimeoutSecs ?? undefined,
      overrides: gatherSpeechOverrides(),
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

/// Sibling of [`resolveApiKeyUpdate`] for the openai-compat server key.
/// Same env / inline / keep resolution; the only behavioural difference
/// is the wording of the half-configured error.
function resolveOcApiKeyUpdate() {
  if (dom.fields.ocApiKeyEnv.checked) {
    return { kind: "env" };
  }
  const typed = dom.fields.ocApiKeyInput.value;
  if (typed.length > 0) {
    return { kind: "inline", key: typed };
  }
  if (state.hasOcInlineKey) {
    return { kind: "keep" };
  }
  throw new Error(
    "Inline server API key selected but no key entered. Type a key or pick 'Read from environment variable'.",
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

})();
