package org.theprimer.gui

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognitionSupport
import android.speech.RecognitionSupportCallback
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import java.util.concurrent.Executor
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * Looper-bound Android speech work, called from Rust over JNI (Plan 2).
 *
 * Threading: `SpeechRecognizer` and `TextToSpeech` must be created and
 * driven on the main Looper, but JNI calls arrive on a native attached
 * thread. Every method that touches a recognizer/synthesizer therefore
 * `post`s to a main-Looper [Handler] (blocking on a [CountDownLatch] when a
 * result is needed back on the JNI thread). The recognizer callbacks fire on
 * the main Looper and only ever push JSON onto the thread-safe [eventQueue],
 * which the JNI thread drains via [pollSpeechEvent]. This is the D4 poll
 * model — there are no Kotlin→Rust upcalls.
 *
 * Event-ordering contract (load-bearing — see the Rust `android::stt`
 * consumer and `ChannelStt`): the transcript-bearing `final` event IS the
 * end-of-utterance signal. Android fires `onEndOfSpeech` *before*
 * `onResults`, so emitting a standalone `end_of_speech` would drive the
 * derived VAD's `SpeechEnd` before the transcript is queued (an empty
 * utterance) and double the start/end cycle. We therefore DO NOT enqueue an
 * `end_of_speech` event; `onResults` → `{"kind":"final",...}` is the single
 * terminal edge.
 */
object PrimerSpeech {
    // How long queryCapabilities waits for the async TextToSpeech engine init
    // before giving up and returning whatever voices it has (empty on timeout).
    // A diagnostic-only bound; the real voice loop (Plan 2) does not block.
    private const val TTS_INIT_TIMEOUT_SECONDS = 5L

    // How long speak() blocks waiting for the engine's onDone/onError before
    // giving up and returning a tts_error. Generous so a slow long utterance
    // is not cut off, bounded so a (rare) engine that never fires onDone
    // cannot wedge the voice loop forever (Plan 2 risk register).
    private const val TTS_SPEAK_TIMEOUT_SECONDS = 60L

    // How long the recognizer/synthesizer construction post to the main
    // Looper is allowed to take before the JNI caller gives up. Construction
    // is near-instant; this only guards against a wedged main thread.
    private const val MAIN_POST_TIMEOUT_SECONDS = 5L

    // Utterance id handed to TextToSpeech.speak; the UtteranceProgressListener
    // matches on it to release the per-speak latch.
    private const val UTTERANCE_ID = "primer-tts"

    // Cached by init() so JNI calls on attached threads can resolve a real
    // Context (the system classloader on an attached thread cannot see app
    // classes — the canonical JNI-on-Android gotcha; the cached app Context
    // gives queryCapabilities what it needs).
    @Volatile @JvmStatic var appContext: Context? = null

    // Main-Looper handler for posting recognizer/synthesizer work.
    private val mainHandler = Handler(Looper.getMainLooper())

    // Recognizer callbacks push event JSON here; pollSpeechEvent drains it on
    // the JNI thread. LinkedBlockingQueue is thread-safe and gives us the
    // timed poll() pollSpeechEvent needs.
    private val eventQueue = LinkedBlockingQueue<String>()

    // Persistent recognizer + synthesizer, created lazily on the main Looper.
    @Volatile private var recognizer: SpeechRecognizer? = null
    @Volatile private var tts: TextToSpeech? = null
    @Volatile private var ttsReady = false

    // Latch released by the UtteranceProgressListener's onDone/onError so the
    // blocking speak() returns when playback finishes.
    @Volatile private var speakLatch: CountDownLatch? = null

    @JvmStatic
    fun init(ctx: Context) { appContext = ctx.applicationContext }

    /**
     * Implemented in Rust (primer-speech, android-native) as
     * `Java_org_theprimer_gui_PrimerSpeech_nativeInit`. Caches the JavaVM
     * so the JNI speech bridge can resolve it without `ndk_context`. Called
     * once from `MainActivity.onCreate` after the Rust library has loaded.
     */
    @JvmStatic external fun nativeInit()

    /** Returns the SpeechCapabilities JSON the Rust side parses with serde. */
    @JvmStatic
    fun queryCapabilities(): String {
        val ctx = appContext ?: return """{"on_device_recognition_available":false,"recognition_locales":[],"tts_voices":[]}"""
        val obj = JSONObject()
        // isOnDeviceRecognitionAvailable is API 31+; minSdk is 24, so guard it
        // (the build-time NewApi lint would otherwise fail the APK build).
        val onDevice = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx)
        } else {
            false
        }
        obj.put("on_device_recognition_available", onDevice)
        obj.put("recognition_locales", JSONArray())

        val voicesJson = JSONArray()
        val latch = CountDownLatch(1)
        var probe: TextToSpeech? = null
        probe = TextToSpeech(ctx) { status ->
            if (status == TextToSpeech.SUCCESS) {
                runCatching {
                    for (vo in probe?.voices ?: emptySet()) {
                        voicesJson.put(JSONObject().apply {
                            put("name", vo.name)
                            put("locale", vo.locale.toLanguageTag())
                            put("network_required", vo.isNetworkConnectionRequired)
                            put("not_installed",
                                vo.features?.contains(
                                    TextToSpeech.Engine.KEY_FEATURE_NOT_INSTALLED) == true)
                        })
                    }
                }
            }
            latch.countDown()
        }
        latch.await(TTS_INIT_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        probe?.shutdown()
        obj.put("tts_voices", voicesJson)
        return obj.toString()
    }

    // ── Voice-loop bridge methods (Plan 2 Task 7) ──────────────────────

    // The BCP-47 the Rust loop last requested, and the effective installed
    // tag we actually arm with. The device may have only a same-language
    // *variant* installed on-device (device-found 2026-06-19: en-AU installed,
    // en-US merely supported → ERROR_LANGUAGE_NOT_SUPPORTED). We resolve the
    // requested tag to an installed variant once (via checkRecognitionSupport)
    // and cache it so every later arm goes straight to startListening.
    @Volatile private var requestedTag: String? = null
    @Volatile private var effectiveTag: String? = null

    /**
     * Arm the on-device recognizer for one utterance in `bcp47`. The
     * recognizer is one-shot per startListening; the Rust consumer re-arms
     * after each terminal event. Strict offline-first: built via
     * `createOnDeviceSpeechRecognizer` (API 31+) with `EXTRA_PREFER_OFFLINE`.
     *
     * If the exact `bcp47` isn't installed on-device but a same-language
     * variant is (e.g. requested en-US, installed en-AU), we arm with the
     * installed variant — it works fully offline now, rather than failing
     * with ERROR_LANGUAGE_NOT_SUPPORTED waiting on a download.
     */
    @JvmStatic
    fun startListening(bcp47: String) {
        val ctx = appContext ?: run { enqueueSttError(SpeechRecognizer.ERROR_CLIENT); return }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
            !SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx)) {
            // No offline recognizer — never fall back to the network factory
            // ([[project_strict_offline_first]]). Surface as an STT error.
            dbg("startListening: on-device recognizer unavailable")
            enqueueSttError(SpeechRecognizer.ERROR_RECOGNIZER_BUSY)
            return
        }
        dbg("startListening($bcp47)")
        runOnMainBlocking {
            if (recognizer == null) {
                recognizer = SpeechRecognizer.createOnDeviceSpeechRecognizer(ctx).apply {
                    setRecognitionListener(listener)
                }
                dbg("recognizer created")
            }
            val cached = if (bcp47 == requestedTag) effectiveTag else null
            if (cached != null) {
                arm(cached)
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                // Resolve the requested tag to an installed variant, then arm.
                resolveAndArm(bcp47)
            } else {
                // Pre-API-33: no support query; arm with the requested tag.
                arm(bcp47)
            }
        }
    }

    /** Build the recognition intent for `tag` and start listening. Must run on
     *  the main Looper (called from inside [runOnMainBlocking]). */
    private fun arm(tag: String) {
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, tag)
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        }
        dbg("arm($tag)")
        recognizer?.startListening(intent)
    }

    /** Query on-device recognition support, pick the best installed variant of
     *  `bcp47` (exact > same-language > requested), cache it, and arm. If no
     *  same-language variant is installed, trigger a background model download
     *  for the requested tag (works for next time) and arm with the request.
     *  API 33+. */
    @androidx.annotation.RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private fun resolveAndArm(bcp47: String) {
        val rec = recognizer ?: return
        val probeIntent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, bcp47)
        }
        val direct = Executor { it.run() }
        val ok = runCatching {
            rec.checkRecognitionSupport(probeIntent, direct, object : RecognitionSupportCallback {
                override fun onSupportResult(support: RecognitionSupport) {
                    val installed = support.installedOnDeviceLanguages
                    dbg("support installed=$installed pending=${support.pendingOnDeviceLanguages}")
                    val eff = pickInstalledVariant(bcp47, installed)
                    if (eff == null && installed.none { sameLanguage(it, bcp47) }) {
                        dbg("triggerModelDownload($bcp47)")
                        runCatching { rec.triggerModelDownload(probeIntent) }
                            .onFailure { dbg("triggerModelDownload threw: $it") }
                    }
                    val tag = eff ?: bcp47
                    requestedTag = bcp47
                    effectiveTag = tag
                    arm(tag)
                }
                override fun onError(error: Int) {
                    dbg("checkRecognitionSupport onError $error")
                    arm(bcp47) // fall back to the requested tag
                }
            })
        }.isSuccess
        if (!ok) {
            dbg("checkRecognitionSupport threw; arming requested tag")
            arm(bcp47)
        }
    }

    /** Pick the best installed on-device tag for `requested`: exact match
     *  wins, else any same-language variant (en-US → en-AU), else null. */
    private fun pickInstalledVariant(requested: String, installed: List<String>): String? {
        installed.firstOrNull { it.equals(requested, ignoreCase = true) }?.let { return it }
        return installed.firstOrNull { sameLanguage(it, requested) }
    }

    /** Whether two BCP-47 tags share a primary language subtag (before '-'). */
    private fun sameLanguage(a: String, b: String): Boolean =
        a.substringBefore('-').equals(b.substringBefore('-'), ignoreCase = true)

    /** Stop / cancel the recognizer (no terminal event is enqueued). */
    @JvmStatic
    fun stopListening() {
        runOnMainBlocking { recognizer?.cancel() }
    }

    /**
     * Drain the next queued speech event, waiting up to `timeoutMs`. Returns
     * the event JSON, or "" when nothing arrived within the timeout (the Rust
     * bridge maps "" → `Ok(None)`).
     */
    @JvmStatic
    fun pollSpeechEvent(timeoutMs: Int): String {
        val ev = eventQueue.poll(timeoutMs.toLong(), TimeUnit.MILLISECONDS)
        return ev ?: ""
    }

    /**
     * Speak `text` and BLOCK until the engine reports done (D3) — on Android
     * the synthesis *is* the playback. A per-utterance latch is released by
     * the UtteranceProgressListener's onDone/onError. A timeout returns
     * normally (a tts_error is enqueued) so a wedged engine cannot hang the
     * loop forever.
     */
    @JvmStatic
    fun speak(text: String) {
        val ctx = appContext ?: return
        ensureTts(ctx)
        if (!ttsReady) { enqueueTtsError("tts engine not ready"); return }
        val latch = CountDownLatch(1)
        speakLatch = latch
        val params = Bundle()
        val rc = tts?.speak(text, TextToSpeech.QUEUE_FLUSH, params, UTTERANCE_ID)
        if (rc != TextToSpeech.SUCCESS) {
            speakLatch = null
            enqueueTtsError("tts speak returned $rc")
            return
        }
        latch.await(TTS_SPEAK_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        speakLatch = null
    }

    /** Abort any in-progress speech (GUI Stop / Esc). */
    @JvmStatic
    fun cancelSpeech() {
        tts?.stop()
        speakLatch?.countDown()
        speakLatch = null
    }

    // ── Internals ──────────────────────────────────────────────────────

    /** Recognizer callbacks → event-queue JSON (see ordering contract above). */
    private val listener = object : RecognitionListener {
        override fun onReadyForSpeech(params: Bundle?) { dbg("onReadyForSpeech") }
        override fun onBeginningOfSpeech() { dbg("onBeginningOfSpeech") }
        override fun onRmsChanged(rmsdB: Float) {}
        override fun onBufferReceived(buffer: ByteArray?) {}

        // Deliberately NOT enqueued: onEndOfSpeech fires before onResults, so
        // an end_of_speech event would drive SpeechEnd before the transcript
        // is queued. onResults (final) is the single terminal edge.
        override fun onEndOfSpeech() { dbg("onEndOfSpeech") }

        override fun onPartialResults(partialResults: Bundle?) {
            val t = firstResult(partialResults)
            dbg("onPartialResults: ${t ?: "(none)"}")
            t?.let { enqueuePartial(it) }
        }

        override fun onResults(results: Bundle?) {
            val t = firstResult(results) ?: ""
            dbg("onResults: '$t'")
            // Always enqueue a final (even empty) so the consumer re-arms.
            enqueueFinal(t)
        }

        override fun onError(error: Int) {
            dbg("onError: $error")
            enqueueSttError(error)
        }

        override fun onEvent(eventType: Int, params: Bundle?) {}
    }

    // Device-diagnostic sink: logcat is dead on some ROMs, so recognizer
    // lifecycle is appended to <filesDir>/recognizer.log, readable via
    // `adb shell run-as <pkg> cat files/recognizer.log`. Cheap append; safe
    // to keep — it is the only window into on-device recognizer behaviour.
    private fun dbg(line: String) {
        val ctx = appContext ?: return
        runCatching {
            java.io.File(ctx.filesDir, "recognizer.log")
                .appendText("${System.currentTimeMillis()} $line\n")
        }
    }

    private fun firstResult(bundle: Bundle?): String? {
        val list = bundle?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
        return list?.firstOrNull()
    }

    private fun enqueuePartial(text: String) {
        eventQueue.offer(JSONObject().put("kind", "partial").put("text", text).toString())
    }

    private fun enqueueFinal(text: String) {
        eventQueue.offer(JSONObject().put("kind", "final").put("text", text).toString())
    }

    private fun enqueueSttError(code: Int) {
        eventQueue.offer(JSONObject().put("kind", "stt_error").put("code", code).toString())
    }

    private fun enqueueTtsError(message: String) {
        eventQueue.offer(JSONObject().put("kind", "tts_error").put("message", message).toString())
    }

    /** Construct the TextToSpeech engine on first use and wire its progress
     *  listener. Blocks (bounded) for the async init callback. */
    private fun ensureTts(ctx: Context) {
        if (tts != null) return
        val initLatch = CountDownLatch(1)
        val engine = TextToSpeech(ctx) { status ->
            ttsReady = status == TextToSpeech.SUCCESS
            initLatch.countDown()
        }
        engine.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String?) {}
            override fun onDone(utteranceId: String?) { speakLatch?.countDown() }
            @Deprecated("required override", ReplaceWith(""))
            override fun onError(utteranceId: String?) {
                enqueueTtsError("utterance error")
                speakLatch?.countDown()
            }
            override fun onError(utteranceId: String?, errorCode: Int) {
                enqueueTtsError("utterance error $errorCode")
                speakLatch?.countDown()
            }
        })
        tts = engine
        initLatch.await(TTS_INIT_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        // Best-effort: match the engine's default voice to the app locale.
        runCatching { engine.language = Locale.getDefault() }
    }

    /** Post `block` to the main Looper and block the calling (JNI) thread
     *  until it runs. Recognizer methods require the main thread. */
    private fun runOnMainBlocking(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) { block(); return }
        val latch = CountDownLatch(1)
        mainHandler.post {
            try { block() } finally { latch.countDown() }
        }
        latch.await(MAIN_POST_TIMEOUT_SECONDS, TimeUnit.SECONDS)
    }
}
