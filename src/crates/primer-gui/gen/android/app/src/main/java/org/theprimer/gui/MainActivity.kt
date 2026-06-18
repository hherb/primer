package org.theprimer.gui

import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
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
  }
}
