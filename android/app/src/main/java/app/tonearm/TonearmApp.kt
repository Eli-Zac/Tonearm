package app.tonearm

import android.app.Application
import uniffi.tonearm_android.Tonearm

/** Starts the Rust core once for the whole app. The playback service and
 *  the screens share it: the service can start without the screen, from a
 *  headphone button or the lock screen. */
class TonearmApp : Application() {
    val core: Tonearm by lazy {
        TonearmNative.init(this)
        Tonearm(filesDir.absolutePath)
    }
}
