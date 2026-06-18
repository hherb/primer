package org.theprimer.gui

import android.content.Context
import android.os.Build
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Looper-bound Android speech work, called from Rust over JNI. */
object PrimerSpeech {
    // How long queryCapabilities waits for the async TextToSpeech engine init
    // before giving up and returning whatever voices it has (empty on timeout).
    // A diagnostic-only bound; the real voice loop (Plan 2) does not block.
    private const val TTS_INIT_TIMEOUT_SECONDS = 5L

    // Cached by init() so JNI calls on attached threads can resolve a real
    // Context (the system classloader on an attached thread cannot see app
    // classes — the canonical JNI-on-Android gotcha; the cached app Context
    // gives queryCapabilities what it needs).
    @Volatile @JvmStatic var appContext: Context? = null

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
        var tts: TextToSpeech? = null
        tts = TextToSpeech(ctx) { status ->
            if (status == TextToSpeech.SUCCESS) {
                runCatching {
                    for (vo in tts?.voices ?: emptySet()) {
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
        tts?.shutdown()
        obj.put("tts_voices", voicesJson)
        return obj.toString()
    }
}
