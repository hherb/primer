// voice.js — voice-mode frontend controller.
//
// Coordinates: header toggle, composer swap, Tauri command invocation,
// event subscriptions for primer://voice/*, asset-consent modal,
// sticky-toggle restoration on launch.
//
// Relies on app.js exposing the following on `window`:
//   window.primerAppendChildBubble(text)
//   window.primerAppendPrimerChunk(primerTurnIndex, text)
//   window.primerRefreshSidebar()
//   window.primerShowError(msg)
//   window.primerShowToast(msg)

(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const state = {
    active: false,
    available: false,
    currentVoiceState: null,    // "listen" | "latent_think" | "speak" | null
  };

  // Per-state label/hint copy. Populated from get_voice_state_copy on init;
  // these English defaults are the fallback if the command fails.
  const STATE_COPY = {
    listen:       { label: "Listening…",  hint: "take your time" },
    latent_think: { label: "Thinking…",   hint: "the Primer is working on a reply" },
    speak:        { label: "Speaking…",   hint: "let the Primer finish" },
  };

  function $(id) { return document.getElementById(id); }

  function setActive(active) {
    state.active = active;
    const toggle = $("voice-mode-toggle");
    const label  = $("voice-toggle-label");
    const composerText  = $("composer");
    const composerVoice = $("composer-voice");
    toggle.setAttribute("aria-pressed", active ? "true" : "false");
    label.textContent = active ? "End voice mode" : "Voice mode";
    composerText.hidden  = active;
    composerVoice.hidden = !active;
    if (!active) {
      // Return to the default listen state visually when deactivated.
      setVoiceState("listen", null);
    }
  }

  function setVoiceState(s, hint) {
    state.currentVoiceState = s;
    const root = $("voice-state");
    if (!root) return;
    root.dataset.state = s;
    const copy = STATE_COPY[s] || { label: s, hint: "" };
    const labelEl = $("voice-state-label");
    const hintEl  = $("voice-state-hint");
    if (labelEl) labelEl.textContent = copy.label;
    if (hintEl)  hintEl.textContent  = hint || copy.hint;
  }

  async function onToggleClick() {
    if (!state.available) return;
    if (state.active) {
      await invoke("stop_voice_mode").catch((e) => showError(`stop voice: ${e}`));
      setActive(false);
      return;
    }
    try {
      await invoke("start_voice_mode");
      setActive(true);
    } catch (err) {
      if (err && err.kind === "asset_missing") {
        await showConsentModal(err.entries);
      } else if (err && err.kind === "not_built") {
        showError("Voice mode is not built into this binary.");
      } else {
        showError(`start voice mode: ${(err && (err.message || JSON.stringify(err))) || String(err)}`);
      }
    }
  }

  async function showConsentModal(entries) {
    const backdrop = $("voice-consent-backdrop");
    if (!backdrop) return;
    const tbody = $("voice-consent-assets");
    if (tbody) tbody.innerHTML = "";
    let totalMb = 0;
    for (const e of (entries || [])) {
      if (!tbody) break;
      const tr = document.createElement("tr");
      const sizeMb = e.approx_size_mb || 0;
      totalMb += sizeMb;
      tr.innerHTML = `
        <td>${e.kind}</td>
        <td>${sizeMb ? `≈ ${sizeMb} MB` : "—"}</td>
        <td><span class="source-url">${e.suggested_url || "(no URL)"}</span></td>
      `;
      tbody.appendChild(tr);
    }
    const totalEl = $("voice-consent-total-size");
    if (totalEl) totalEl.textContent = totalMb ? `~${totalMb} MB` : "(unknown)";
    backdrop.hidden = false;
    backdrop.setAttribute("aria-hidden", "false");

    // Subscribe to download_progress events for the progress bar.
    let unlistenProgress = null;
    const progressBar   = $("voice-consent-progress-bar");
    const progressLabel = $("voice-consent-progress-label");
    const progressSect  = $("voice-consent-progress");
    if (progressBar && progressLabel) {
      unlistenProgress = await listen("primer://voice/download_progress", (evt) => {
        const { asset_id, bytes_done, bytes_total } = evt.payload;
        if (progressSect) progressSect.hidden = false;
        if (bytes_total && bytes_total > 0) {
          const pct = Math.round((bytes_done / bytes_total) * 100);
          progressBar.value = pct;
          progressLabel.textContent = `${asset_id}: ${pct}%`;
        } else {
          progressLabel.textContent = `${asset_id}: ${(bytes_done / 1_048_576).toFixed(1)} MB`;
        }
      });
    }

    return new Promise((resolve) => {
      const cleanup = () => {
        if (unlistenProgress) { unlistenProgress(); unlistenProgress = null; }
        if ($("voice-consent-cancel")) $("voice-consent-cancel").onclick = null;
        if ($("voice-consent-close"))  $("voice-consent-close").onclick  = null;
        if ($("voice-consent-download")) $("voice-consent-download").onclick = null;
      };

      const onCancel = async () => {
        backdrop.hidden = true;
        cleanup();
        // Persist voice_mode_enabled=false so the next GUI launch doesn't
        // re-prompt the consent dialog. stop_voice_mode is idempotent and
        // flips the sticky flag off via gui-config.json. Without this, a
        // child or parent who chose "not now" would see the modal every
        // launch with no way to dismiss it durably.
        await invoke("stop_voice_mode").catch(() => {});
        resolve();
      };

      const downloadBtn = $("voice-consent-download");
      const onDownload = async () => {
        if (progressSect) progressSect.hidden = false;
        if (progressBar)  progressBar.value = 0;
        const errBanner = $("voice-consent-error");
        if (errBanner) errBanner.hidden = true;
        // Disable the Download button during the in-flight invoke. Without
        // this, a rapid double-click fires two concurrent
        // download_voice_assets invocations that race on the same
        // `<dest>.partial` files in voice/download.rs — undefined behaviour
        // in the worst case, redundant network traffic at best. Re-enabled
        // on error so the user can retry. On success the modal is hidden
        // and the button reference goes out of scope with this promise.
        if (downloadBtn) downloadBtn.disabled = true;
        try {
          // IPC trust boundary: echo only the asset `kind` strings; the
          // host re-resolves `path` and `suggested_url` server-side via
          // `resolve_requested_kinds`. A compromised webview cannot
          // direct the host to write outside `~/.cache/primer/models/`.
          await invoke("download_voice_assets", {
            kinds: entries.map((e) => e.kind),
          });
          backdrop.hidden = true;
          cleanup();
          // Retry start_voice_mode now that assets are local.
          try {
            await invoke("start_voice_mode");
            setActive(true);
          } catch (err) {
            showError(`start after download: ${(err && (err.message || JSON.stringify(err))) || String(err)}`);
          }
          resolve();
        } catch (err) {
          const msg = `download failed: ${(err && (err.message || JSON.stringify(err))) || String(err)}`;
          if (errBanner) {
            errBanner.textContent = msg;
            errBanner.hidden = false;
          } else {
            showError(msg);
          }
          // Reset the progress UI so a retry click starts from a clean
          // slate visually, instead of from the partial-bar of the failed
          // attempt. The progress listener subscription is intentionally
          // NOT torn down here — `unlistenProgress` was set up once at
          // modal show and the same subscription serves both the failed
          // attempt and any retry. It is unsubscribed by `cleanup()` via
          // the Cancel/Close paths or the success path above.
          if (progressBar)   progressBar.value = 0;
          if (progressLabel) progressLabel.textContent = "";
          if (progressSect)  progressSect.hidden = true;
          // Re-enable so the user can retry. Cancel/Close still work
          // because cleanup() didn't run — the modal stays interactive.
          if (downloadBtn) downloadBtn.disabled = false;
        }
      };

      if ($("voice-consent-cancel")) $("voice-consent-cancel").onclick = onCancel;
      if ($("voice-consent-close"))  $("voice-consent-close").onclick  = onCancel;
      if ($("voice-consent-download")) $("voice-consent-download").onclick = onDownload;
    });
  }

  function showError(msg) {
    if (window.primerShowError) window.primerShowError(msg);
    else console.error("[voice]", msg);
  }

  // Stop button + Esc both route to cancel_voice_response.
  async function onStop() {
    if (!state.active) return;
    await invoke("cancel_voice_response").catch(() => {});
  }

  function onKeyDown(e) {
    if (!state.active) return;
    if (e.key === "Escape") onStop();
  }

  // === Tauri event subscriptions ===

  async function subscribeEvents() {
    await listen("primer://voice/state_change", (evt) => {
      const { state: s, hint } = evt.payload;
      setVoiceState(s, hint || null);
    });

    await listen("primer://voice/transcript", (evt) => {
      if (window.primerAppendChildBubble) {
        window.primerAppendChildBubble(evt.payload.text);
      }
    });

    await listen("primer://voice/response_chunk", (evt) => {
      if (window.primerAppendPrimerChunk) {
        window.primerAppendPrimerChunk(evt.payload.primer_turn_index, evt.payload.text);
      }
    });

    await listen("primer://voice/response_complete", (_evt) => {
      if (window.primerRefreshSidebar) window.primerRefreshSidebar();
    });

    await listen("primer://voice/exit", (evt) => {
      setActive(false);
      const reason = (evt.payload && evt.payload.reason) || "unknown";
      if (window.primerShowToast) {
        window.primerShowToast(`Voice mode ended (${reason})`);
      }
    });

    await listen("primer://voice/inference_error", (evt) => {
      showError(`Voice inference error: ${evt.payload.message}`);
    });
  }

  // === Locale-aware copy ===

  async function loadStateCopy() {
    let c = null;
    try { c = await invoke("get_voice_state_copy"); } catch (_) { /* use defaults */ }
    if (!c) return;
    STATE_COPY.listen       = { label: c.listen_label,    hint: c.listen_hint };
    STATE_COPY.latent_think = { label: c.thinking_label,  hint: c.thinking_hint };
    STATE_COPY.speak        = { label: c.speak_label,     hint: c.speak_hint };
  }

  // === Sticky-toggle restoration on launch ===

  async function restoreOnLaunch() {
    // Read the speech-feature flag from the dedicated capability command,
    // not from `current_session_info`. The session-info command returns
    // null when no session is active (e.g. on the session picker at
    // launch), which previously left the voice toggle permanently
    // disabled and showed the misleading "Voice mode is not built into
    // this binary" tooltip even on speech-enabled binaries.
    const available = await invoke("voice_mode_available").catch(() => false);
    state.available = available === true;
    const toggle = $("voice-mode-toggle");
    if (!toggle) return;
    toggle.disabled = !state.available;
    if (!state.available) {
      toggle.title = "Voice mode is not built into this binary";
      return;
    }
    // Read persisted speech.voice_mode_enabled via get_settings.
    const cfg = await invoke("get_settings").catch(() => null);
    if (cfg && cfg.speech && cfg.speech.voice_mode_enabled === true) {
      try {
        await invoke("start_voice_mode");
        setActive(true);
      } catch (err) {
        if (err && err.kind === "asset_missing") {
          await showConsentModal(err.entries);
        } else if (err && err.kind === "not_built") {
          // Silent — toggle is already disabled.
        } else {
          showError(`auto-resume voice mode: ${(err && (err.message || JSON.stringify(err))) || String(err)}`);
        }
      }
    }
  }

  // === DOM wiring ===

  function init() {
    const toggle = $("voice-mode-toggle");
    if (toggle) toggle.addEventListener("click", onToggleClick);
    const stopBtn = $("voice-stop");
    if (stopBtn) stopBtn.addEventListener("click", onStop);
    document.addEventListener("keydown", onKeyDown);
    // Load locale copy first (fast, non-blocking), then subscribe events + restore.
    loadStateCopy().catch((e) => console.warn("voice loadStateCopy:", e));
    subscribeEvents().catch((e) => console.error("voice subscribeEvents:", e));
    restoreOnLaunch().catch((e) => console.error("voice restoreOnLaunch:", e));
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
