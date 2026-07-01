import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    compileSdk = 36
    namespace = "org.theprimer.gui"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "org.theprimer.gui"
        minSdk = 24
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }

    val keystorePropsFile = rootProject.file("keystore.properties")
    val keystoreProps = Properties().apply {
        if (keystorePropsFile.exists()) {
            keystorePropsFile.inputStream().use { load(it) }
        }
    }

    signingConfigs {
        create("release") {
            if (keystorePropsFile.exists()) {
                storeFile = file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                // Extract native libs to the app's real nativeLibraryDir at install
                // time (extractNativeLibs=true). The Hexagon DSP loads the bundled
                // QAIRT skel (libQnnHtpV81Skel.so) over FastRPC, which needs a real
                // on-disk file reachable via ADSP_LIBRARY_PATH — it cannot push a
                // skel that lives only inside the APK (base.apk!/lib/...). Modern AGP
                // defaults this to false (libs mmap'd from the APK), which left the
                // real lib dir empty and failed DSP bring-up with `Failed to load
                // skel, error: 1002` after `First connection to QNN stub established`
                // (read from the on-device genie.log). The manifest extractNativeLibs
                // attribute is overridden by AGP, so this gradle knob is authoritative.
                //
                // NB: this lives in the `debug` build type only — the QNN APK is
                // built `--debug` today. A future `release` QNN build must set the
                // same `jniLibs.useLegacyPackaging = true` or DSP bring-up will
                // regress to `Failed to load skel, error: 1002`.
                jniLibs.useLegacyPackaging = true
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            // Minify stays OFF for the test APK: R8 would risk stripping or
            // renaming org.theprimer.gui.PrimerSpeech, which the Rust side
            // invokes reflectively over JNI (nativeInit caches it as a
            // GlobalRef). A future minified production build must add explicit
            // proguard-keep rules for that class + its native methods.
            isMinifyEnabled = false
            signingConfig = if (keystorePropsFile.exists()) {
                signingConfigs.getByName("release")
            } else {
                null
            }
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")