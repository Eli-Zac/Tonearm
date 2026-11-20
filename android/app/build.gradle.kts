plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "app.tonearm"
    // Compose 1.13 builds against 37.1 or later.
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }

    defaultConfig {
        applicationId = "app.tonearm"
        // AAudio, which the core's audio plays through, arrived in 8.0.
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "0.10.0"
    }

    buildFeatures {
        compose = true
    }

    // The core is built by `build-rust.ps1` into src/main/jniLibs, and its
    // Kotlin side written by UniFFI into src/main/java/uniffi.

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    val compose = platform("androidx.compose:compose-bom:2026.11.01")
    implementation(compose)
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.1")
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
    // The media session: notification, lock screen, headphone buttons.
    implementation("androidx.media3:media3-session:1.11.1")
    implementation("androidx.media3:media3-common:1.11.1")
    // UniFFI's Kotlin calls into Rust through JNA.
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    implementation("io.coil-kt.coil3:coil-compose:3.6.3")
    implementation("io.coil-kt.coil3:coil-network-okhttp:3.6.3")
}
