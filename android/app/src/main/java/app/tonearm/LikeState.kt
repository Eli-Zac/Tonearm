package app.tonearm

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.tonearm_android.Tonearm as Core

/**
 * Whether the song playing is liked, in one place. The Now Playing heart and
 * the notification's, the car's and the watch's heart all read and change it,
 * so pressing any of them moves the rest. Compose state, so the screen
 * redraws when a media button changes it; the media buttons follow it on
 * their half-second tick.
 *
 * `load` and `flip` call YouTube Music: run them off the main thread.
 */
object LikeState {
    /** The song `liked` is about. */
    var id by mutableStateOf<String?>(null)
        private set

    /** Null until YouTube Music says. */
    var liked by mutableStateOf<Boolean?>(null)
        private set

    /** Whether `songId` is liked, as far as is known. */
    fun of(songId: String?): Boolean? = if (songId != null && songId == id) liked else null

    /** Finds out whether the song is liked, unless that is known already. */
    fun load(core: Core, songId: String) {
        if (id == songId && liked != null) return
        id = songId
        liked = null
        val known = runCatching { core.isLiked(songId) }.getOrNull()
        if (id == songId) liked = known
    }

    /** Notes that a song was liked or unliked some other way (the song menu). */
    fun note(songId: String, now: Boolean) {
        if (id == songId) liked = now
    }

    /** Flips the like at once, and back if YouTube Music says no. Returns why it failed. */
    fun flip(core: Core, songId: String): Throwable? {
        val want = of(songId) != true
        id = songId
        liked = want
        val failure = runCatching { core.setLiked(songId, want) }.exceptionOrNull()
        if (failure != null && id == songId) liked = !want
        return failure
    }
}
