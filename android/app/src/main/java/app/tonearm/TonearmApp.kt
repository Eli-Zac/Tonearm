package app.tonearm

import android.app.Application
import android.os.Build
import android.provider.Settings
import uniffi.tonearm_android.Tonearm

/** Starts the Rust core once for the whole app. The playback service and
 *  the screens share it: the service can start without the screen, from a
 *  headphone button or the lock screen. */
class TonearmApp : Application() {
    val core: Tonearm by lazy {
        TonearmNative.init(this)
        // Before the core starts: it finds Discord's library already loaded.
        DiscordHost.install(this)
        Tonearm(filesDir.absolutePath).also {
            // How the account's device list names this phone: the name the
            // user gave it in Settings, and its make and model.
            val name = runCatching { Settings.Global.getString(contentResolver, "device_name") }.getOrNull().orEmpty()
            val maker = Build.MANUFACTURER.replaceFirstChar { c -> c.uppercase() }
            it.accountSetDevice(name, if (Build.MODEL.startsWith(maker, ignoreCase = true)) Build.MODEL else "$maker ${Build.MODEL}")
        }
    }
}
