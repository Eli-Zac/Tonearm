package app.tonearm

import java.util.concurrent.CopyOnWriteArrayList
import uniffi.tonearm_android.PartyView
import uniffi.tonearm_android.Tonearm as Core

/**
 * The Party as the car sees it: the core's Party (the same one the phone's
 * queue shows), read again when it changes, with a way to tell the car's
 * pages and button. `poll` runs from CarControls' tick.
 */
object CarParty {
    @Volatile
    var view: PartyView? = null
        private set

    val active get() = view != null

    @Volatile
    private var seen: ULong? = null

    /** Told after every change, for the car's button and pages. */
    private val watchers = CopyOnWriteArrayList<() -> Unit>()
    fun watch(change: () -> Unit) { watchers += change }
    fun unwatch(change: () -> Unit) { watchers -= change }

    /** Reads the Party again if it changed. Off the main thread: it's quick,
     *  but the core's lock can be held by a song starting. */
    fun poll(core: Core) {
        val version = core.partyVersion()
        if (version == seen) return
        seen = version
        view = core.party()
        watchers.forEach { it() }
    }

    /** Starts a Party, as this phone's YouTube Music first name. Blocks on
     *  the network; gives why it failed, or null. */
    fun start(core: Core): String? {
        if (active) return null
        val name = runCatching { core.account()?.name }.getOrNull()?.trim()?.split(" ")?.firstOrNull()?.takeIf { it.isNotBlank() }
            ?: android.os.Build.MODEL
        return runCatching { core.partyStart(name) }.exceptionOrNull()?.let { it.message ?: "Couldn't start a Party" }
            .also { poll(core) }
    }
}
