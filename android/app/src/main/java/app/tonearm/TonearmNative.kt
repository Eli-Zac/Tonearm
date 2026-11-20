package app.tonearm

import android.content.Context

/** The one call into the core that UniFFI doesn't make: handing Rust the
 *  app's context, which Android's audio needs before anything plays. */
object TonearmNative {
    init {
        System.loadLibrary("tonearm_android")
    }

    @JvmStatic
    external fun init(context: Context)
}
