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
    // True on an `android-native` build: the OS owns the recognizer + TTS,
    // so we invoke the `*_voice_mode_android` command variants. Set at
    // launch from `android_voice_available`.
    android: false,
    currentVoiceState: null,    // "listen" | "latent_think" | "speak" | null
    // Guard against concurrent auto-start attempts (e.g. rapid back-to-back
    // session switches firing `primerRestoreVoiceMode` before the first
    // call's `start_voice_mode` has resolved and set `state.active = true`).
    // Without this, a second call would race past the `state.active` check
    // and invoke `start_voice_mode` a second time, whose step-2 teardown
    // briefly flips the sticky toggle to `false` on disk before step-8
    // restores it — a tiny window where a crash leaves the persisted
    // toggle stale.
    starting: false,
  };

  // Per-state label/hint copy. Populated from get_voice_state_copy on init;
  // these English defaults are the fallback if the command fails.
  const STATE_COPY = {
    listen:       { label: "Listening…",  hint: "take your time" },
    latent_think: { label: "Thinking…",   hint: "the Primer is working on a reply" },
    speak:        { label: "Speaking…",   hint: "let the Primer finish" },
  };

  function $(id) { return document.getElementById(id); }

  // Map a base voice command to its Android variant on android-native
  // builds (the OS owns the mic/speaker, so a distinct command path builds
  // the cpal-free backends). Desktop builds use the base names unchanged.
  function vcmd(base) {
    return state.android ? `${base}_android` : base;
  }

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
      await invoke(vcmd("stop_voice_mode")).catch((e) => showError(`stop voice: ${e}`));
      setActive(false);
      return;
    }
    try {
      await invoke(vcmd("start_voice_mode"));
      hidePermissionDenied();
      setActive(true);
    } catch (err) {
      if (err && err.kind === "asset_missing") {
        await showConsentModal(err.entries);
      } else if (err && err.kind === "auto_download_disabled") {
        const names = (err.entries || []).map((e) => e.kind).join(", ");
        showError(
          "Voice models aren't downloaded and automatic download is off. " +
            "Add the model paths in Settings → Speech, or re-enable " +
            "automatic download." + (names ? " Missing: " + names : ""),
        );
      } else if (err && err.kind === "permission_denied") {
        showPermissionDenied();
      } else if (err && err.kind === "not_built") {
        showError("Voice mode is not built into this binary.");
      } else {
        showError(`start voice mode: ${(err && (err.message || JSON.stringify(err))) || String(err)}`);
      }
    }
  }

  async function showConsentModal(entries) {
    const dialog = $("voice-consent-modal");
    if (!dialog) return;
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
    if (!dialog.open) dialog.showModal();

    // No backdrop-click dismissal is wired here — deliberate asymmetry
    // vs the settings modal. The consent decision has side effects
    // (start a ~530 MB download OR persist voice_mode_enabled=false via
    // stop_voice_mode), so we require an explicit Cancel/Download click
    // rather than letting a stray off-target click drop the modal.
    // Escape still routes through the `cancel` event handler below so
    // keyboard dismissal stays available.

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
      // Listener handle for the dialog's `cancel` event (fired on
      // Escape). Tracked here so cleanup() can remove it — without
      // removal the listener leaks across re-opens and the
      // stop_voice_mode call would fire twice on the second close.
      let cancelListener = null;

      const cleanup = () => {
        if (unlistenProgress) { unlistenProgress(); unlistenProgress = null; }
        if ($("voice-consent-cancel")) $("voice-consent-cancel").onclick = null;
        if ($("voice-consent-close"))  $("voice-consent-close").onclick  = null;
        if ($("voice-consent-download")) $("voice-consent-download").onclick = null;
        if (cancelListener) {
          dialog.removeEventListener("cancel", cancelListener);
          cancelListener = null;
        }
      };

      const closeDialog = () => {
        if (dialog.open) dialog.close();
      };

      const onCancel = async () => {
        closeDialog();
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
          closeDialog();
          cleanup();
          // Retry start_voice_mode now that assets are local.
          try {
            await invoke(vcmd("start_voice_mode"));
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

      // Native <dialog>.showModal() fires a `cancel` event on Escape
      // before auto-closing. Block the auto-close and route through
      // onCancel so stop_voice_mode is called and the promise resolves —
      // the same path the explicit Cancel button takes. Without this,
      // Escape would close the dialog without persisting
      // voice_mode_enabled=false and the modal would re-appear on the
      // next launch with no way to dismiss it durably.
      cancelListener = (e) => {
        e.preventDefault();
        onCancel();
      };
      dialog.addEventListener("cancel", cancelListener);
    });
  }

  function showError(msg) {
    if (window.primerShowError) window.primerShowError(msg);
    else console.error("[voice]", msg);
  }

  // Show / hide the mic-permission-denied banner (Android voice mode,
  // issue #253). The banner carries an "Open settings" button wired in
  // `init` to the `open_app_settings` command.
  function showPermissionDenied(msg) {
    const banner = $("voice-permission-banner");
    if (!banner) {
      // Fallback for any host without the banner markup.
      showError(msg || "Microphone permission is needed for voice mode.");
      return;
    }
    const messageEl = $("voice-permission-message");
    if (messageEl && msg) messageEl.textContent = msg;
    banner.hidden = false;
  }

  function hidePermissionDenied() {
    const banner = $("voice-permission-banner");
    if (banner) banner.hidden = true;
  }

  // Stop button + Esc both route to cancel_voice_response.
  async function onStop() {
    if (!state.active) return;
    await invoke(vcmd("cancel_voice_response")).catch(() => {});
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

  // === Sticky-toggle restoration ===

  /// Auto-start voice mode if the sticky toggle is on.
  ///
  /// Shared by `restoreOnLaunch` (initial GUI start) and
  /// `restoreAfterSessionChange` (post-`start_session` /
  /// `resume_session` re-entry — backend preserves the sticky toggle
  /// through the teardown so voice mode flows seamlessly into the new
  /// session under its new locale; closes #102 polished follow-up).
  ///
  /// Caller controls the error-message prefix via `contextLabel` so
  /// the user can tell launch-time auto-resume apart from
  /// session-switch auto-resume in the error banner.
  async function tryAutoStartVoiceMode(contextLabel) {
    if (!state.available || state.active || state.starting) return;
    const cfg = await invoke("get_settings").catch(() => null);
    if (!cfg || !cfg.speech || cfg.speech.voice_mode_enabled !== true) return;
    state.starting = true;
    try {
      await invoke(vcmd("start_voice_mode"));
      hidePermissionDenied();
      setActive(true);
    } catch (err) {
      if (err && err.kind === "asset_missing") {
        await showConsentModal(err.entries);
      } else if (err && err.kind === "auto_download_disabled") {
        const names = (err.entries || []).map((e) => e.kind).join(", ");
        showError(
          `${contextLabel}: Voice models aren't downloaded and automatic ` +
            "download is off. Add the model paths in Settings → Speech, or " +
            "re-enable automatic download." +
            (names ? " Missing: " + names : ""),
        );
      } else if (err && err.kind === "permission_denied") {
        showPermissionDenied();
      } else if (err && err.kind === "not_built") {
        // Silent — toggle is already disabled.
      } else {
        showError(`${contextLabel}: ${(err && (err.message || JSON.stringify(err))) || String(err)}`);
      }
    } finally {
      state.starting = false;
    }
  }

  async function restoreOnLaunch() {
    // Read the speech-feature flag from the dedicated capability command,
    // not from `current_session_info`. The session-info command returns
    // null when no session is active (e.g. on the session picker at
    // launch), which previously left the voice toggle permanently
    // disabled and showed the misleading "Voice mode is not built into
    // this binary" tooltip even on speech-enabled binaries.
    const available = await invoke("voice_mode_available").catch(() => false);
    // `android-native` is independent of the cpal `speech` feature, so an
    // Android build reports voice_mode_available=false but still has the
    // OS-backed voice loop. Treat either as available and route to the
    // `*_android` commands via `vcmd`.
    const androidAvailable = await invoke("android_voice_available").catch(() => false);
    state.android = androidAvailable === true;
    state.available = available === true || state.android;
    const toggle = $("voice-mode-toggle");
    if (!toggle) return;
    toggle.disabled = !state.available;
    if (!state.available) {
      toggle.title = "Voice mode is not built into this binary";
      return;
    }
    await tryAutoStartVoiceMode("auto-resume voice mode");
  }

  /// Re-enter voice mode after a session switch when the user had it
  /// active before the switch. Mirrors `restoreOnLaunch`'s auto-start
  /// branch — the backend's `prepare_for_session_change` preserves
  /// `speech.voice_mode_enabled` across the transient teardown so this
  /// helper sees the still-`true` flag and rebuilds voice mode against
  /// the new locale's Whisper + Piper backends.
  async function restoreAfterSessionChange() {
    if (!state.available) return;
    await tryAutoStartVoiceMode("restart voice mode after session switch");
  }

  // Exposed so `app.js` / `picker.js` / `settings.js` can call it after
  // a successful `start_session` / `resume_session`. Idempotent: a
  // sticky toggle of `false` (user explicitly stopped voice mode
  // earlier) makes this a no-op.
  window.primerRestoreVoiceMode = restoreAfterSessionChange;

  // === DOM wiring ===

  function init() {
    const toggle = $("voice-mode-toggle");
    if (toggle) toggle.addEventListener("click", onToggleClick);
    const stopBtn = $("voice-stop");
    if (stopBtn) stopBtn.addEventListener("click", onStop);
    const permSettingsBtn = $("voice-permission-settings");
    if (permSettingsBtn) {
      permSettingsBtn.addEventListener("click", () => {
        invoke("open_app_settings").catch((e) =>
          console.warn("[voice] open_app_settings:", e),
        );
      });
    }
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
