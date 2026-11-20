plugins {
    id("com.android.application") version "9.5.1" apply false
    // AGP 9 builds Kotlin itself. This brings Compose's compiler, and with
    // it the Kotlin version used.
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
}
