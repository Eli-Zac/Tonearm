package app.tonearm

import java.util.concurrent.ConcurrentHashMap

/**
 * How far behind the phone the sound is heard while Android Auto plays it.
 * The phone's position is where the song is being decoded. The car's
 * speakers sound it a little later (the projection link and the car's own
 * buffer), so the lyrics on the phone run that much early unless they wait.
 */
object CarLag {
    /** Android Auto's own packages, which connect to the session while the car is in use. */
    private val projection = setOf(
        "com.google.android.projection.gearhead",
        "com.google.android.projection.gearhead.pwa",
    )
    private val connected = ConcurrentHashMap.newKeySet<Int>()

    /** What Android Auto adds, in milliseconds. */
    private const val LAG_MS = 350L

    fun connected(uid: Int, packageName: String) {
        if (packageName in projection) connected.add(uid)
    }

    fun disconnected(uid: Int) {
        connected.remove(uid)
    }

    /** Whether the music plays on a Cast device. The car then plays none of
     *  it, and the core already takes the device's own delay off the time. */
    @Volatile
    var casting = false

    /** The delay to take off the song's time shown on the phone, 0 outside the car. */
    fun ms(): Long = if (connected.isEmpty() || casting) 0 else LAG_MS
}
