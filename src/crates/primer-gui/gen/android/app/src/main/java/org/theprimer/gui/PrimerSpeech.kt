package org.theprimer.gui

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
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

    /**
     * Arm the on-device recognizer for one utterance in `bcp47`. The
     * recognizer is one-shot per startListening; the Rust consumer re-arms
     * after each terminal event. Strict offline-first: built via
     * `createOnDeviceSpeechRecognizer` (API 31+) with `EXTRA_PREFER_OFFLINE`.
     */
    @JvmStatic
    fun startListening(bcp47: String) {
        val ctx = appContext ?: run { enqueueSttError(SpeechRecognizer.ERROR_CLIENT); return }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
            !SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx)) {
            // No offline recognizer — never fall back to the network factory
            // ([[project_strict_offline_first]]). Surface as an STT error.
            enqueueSttError(SpeechRecognizer.ERROR_RECOGNIZER_BUSY)
            return
        }
        runOnMainBlocking {
            if (recognizer == null) {
                recognizer = SpeechRecognizer.createOnDeviceSpeechRecognizer(ctx).apply {
                    setRecognitionListener(listener)
                }
            }
            val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                    RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                putExtra(RecognizerIntent.EXTRA_LANGUAGE, bcp47)
                putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
                putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
            }
            recognizer?.startListening(intent)
        }
    }

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
        override fun onReadyForSpeech(params: Bundle?) {}
        override fun onBeginningOfSpeech() {}
        override fun onRmsChanged(rmsdB: Float) {}
        override fun onBufferReceived(buffer: ByteArray?) {}

        // Deliberately NOT enqueued: onEndOfSpeech fires before onResults, so
        // an end_of_speech event would drive SpeechEnd before the transcript
        // is queued. onResults (final) is the single terminal edge.
        override fun onEndOfSpeech() {}

        override fun onPartialResults(partialResults: Bundle?) {
            firstResult(partialResults)?.let { enqueuePartial(it) }
        }

        override fun onResults(results: Bundle?) {
            // Always enqueue a final (even empty) so the consumer re-arms.
            enqueueFinal(firstResult(results) ?: "")
        }

        override fun onError(error: Int) {
            enqueueSttError(error)
        }

        override fun onEvent(eventType: Int, params: Bundle?) {}
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
