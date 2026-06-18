package org.theprimer.gui

import android.content.Context
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Looper-bound Android speech work, called from Rust over JNI. */
object PrimerSpeech {
    // Cached by init() so JNI calls on attached threads can resolve a real
    // Context (the system classloader on an attached thread cannot see app
    // classes — the canonical JNI-on-Android gotcha; the cached app Context
    // gives queryCapabilities what it needs).
    @Volatile @JvmStatic var appContext: Context? = null

    @JvmStatic
    fun init(ctx: Context) { appContext = ctx.applicationContext }

    /** Returns the SpeechCapabilities JSON the Rust side parses with serde. */
    @JvmStatic
    fun queryCapabilities(): String {
        val ctx = appContext ?: return """{"on_device_recognition_available":false,"recognition_locales":[],"tts_voices":[]}"""
        val obj = JSONObject()
        obj.put("on_device_recognition_available",
            SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx))
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
        latch.await(5, TimeUnit.SECONDS)
        tts?.shutdown()
        obj.put("tts_voices", voicesJson)
        return obj.toString()
    }
}
