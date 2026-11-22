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
        // A release build gets both from the tag (see .github/workflows/android.yml):
        // -PversionName=0.13.0 -PversionCode=1300.
        versionCode = (project.findProperty("versionCode") as String?)?.toInt() ?: 1
        versionName = (project.findProperty("versionName") as String?) ?: "0.13.0"
        // The core is built for these two. Discord's SDK also carries 32-bit
        // libraries, which would otherwise ship without a core beside them.
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    // The release key lives in the environment, never in the repository:
    // the release workflow sets these from its secrets.
    val releaseKey = System.getenv("TONEARM_KEYSTORE")?.takeIf { it.isNotBlank() }
    if (releaseKey != null) {
        signingConfigs {
            create("release") {
                storeFile = file(releaseKey)
                storePassword = System.getenv("TONEARM_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("TONEARM_KEY_ALIAS")
                keyPassword = System.getenv("TONEARM_KEY_PASSWORD")
            }
        }
    }

    buildFeatures {
        compose = true
    }

    // Lint's check before a release build trips on the activity-result
    // permission request in MainActivity, which has no fragments to upgrade.
    lint {
        checkReleaseBuilds = false
    }

    buildTypes {
        // The build handed out: not debuggable, signed with the release key
        // when the environment has one. Not shrunk, for the same reason as
        // `profile`.
        getByName("release") {
            isMinifyEnabled = false
            if (releaseKey != null) signingConfig = signingConfigs.getByName("release")
        }
        // As fast as a release (not debuggable, so ART compiles Compose
        // fully), but signed with the debug key so it installs over the
        // debug build and keeps its data. For judging smoothness on a
        // phone: `gradlew installProfile`. Not shrunk: JNA and UniFFI's
        // bindings are found by reflection.
        create("profile") {
            initWith(getByName("debug"))
            isDebuggable = false
            isJniDebuggable = false
            matchingFallbacks += listOf("release")
        }
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
    // Plays the moving album cover, muted and looping, over the still one.
    implementation("androidx.media3:media3-exoplayer:1.11.1")
    implementation("androidx.media3:media3-ui-compose:1.11.1")
    // UniFFI's Kotlin calls into Rust through JNA.
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    implementation("io.coil-kt.coil3:coil-compose:3.6.3")
    implementation("io.coil-kt.coil3:coil-network-okhttp:3.6.3")
    // Scans a pairing QR code in Google's own scanner screen, so Tonearm
    // needs no camera permission.
    implementation("com.google.android.gms:play-services-code-scanner:16.1.0")
    // Discord's Social SDK, for Rich Presence through the Discord app. Its
    // notices are in libs/DISCORD-SDK-NOTICES.txt.
    implementation(files("libs/discord_partner_sdk.aar"))
}
