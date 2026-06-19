package org.theprimer.gui

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  companion object {
    // Request code for the RECORD_AUDIO runtime permission prompt. The
    // recognizer (Plan 2) needs it; we request once at startup so the
    // first start_voice_mode_android has the grant in hand. Result is not
    // consumed (the recognizer surfaces a denial as an stt_error).
    private const val RECORD_AUDIO_REQUEST_CODE = 1001
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    PrimerSpeech.init(this)
    // Cache the JavaVM for the JNI speech bridge (Plan 2 Task 1). Must run
    // after super.onCreate — TauriActivity loads the Rust shared library
    // there, and nativeInit's symbol lives in that library. Caching here
    // replaces the ndk_context path the Tauri-mobile runtime never
    // populated (Plan 1 gate finding).
    PrimerSpeech.nativeInit()
    // Request the mic permission the on-device recognizer needs (the manifest
    // permission was declared in Plan 1). Runtime grant is required on API 23+.
    requestRecordAudioIfNeeded()
  }

  private fun requestRecordAudioIfNeeded() {
    val granted = ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
      PackageManager.PERMISSION_GRANTED
    if (!granted) {
      ActivityCompat.requestPermissions(
        this,
        arrayOf(Manifest.permission.RECORD_AUDIO),
        RECORD_AUDIO_REQUEST_CODE,
      )
    }
  }
}
