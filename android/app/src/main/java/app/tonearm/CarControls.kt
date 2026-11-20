package app.tonearm

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import androidx.media3.common.PlaybackException
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.CommandButton
import androidx.media3.session.MediaSession
import androidx.media3.session.SessionCommand
import com.google.common.util.concurrent.ListeningExecutorService
import uniffi.tonearm_android.PlayState
import uniffi.tonearm_android.RepeatMode
import uniffi.tonearm_android.Tonearm as Core

/**
 * The buttons Android Auto shows beside play, pause and skip: shuffle,
 * repeat and like, as the phone's Now Playing has them, kept in step with
 * the core. They sit in the overflow slots, so skip keeps its place on the
 * car, the notification and the watch.
 *
 * Also tells the car when a song can't play. The phone shows that itself
 * (handed to Android, it popped up a system message the user didn't want),
 * so only the car's controllers hear it.
 */
@UnstableApi
class CarControls(private val core: Core, private val work: ListeningExecutorService) {
    private val main = Handler(Looper.getMainLooper())
    private var session: MediaSession? = null

    private var shuffle = false
    private var repeat = RepeatMode.OFF
    /** Whether the song playing is liked; null until YouTube Music says. */
    private var liked: Boolean? = null
    private var likedFor: String? = null
    private var shown: List<CommandButton> = emptyList()
    private var error = ""

    val commands = listOf(SHUFFLE, REPEAT, LIKE).map { SessionCommand(it, Bundle.EMPTY) }

    fun start(session: MediaSession) {
        this.session = session
        main.post(tick)
    }

    fun stop() {
        main.removeCallbacks(tick)
        session = null
    }

    fun buttons(): List<CommandButton> {
        val shuffleButton = CommandButton.Builder(if (shuffle) CommandButton.ICON_SHUFFLE_ON else CommandButton.ICON_SHUFFLE_OFF)
            .setDisplayName(if (shuffle) "Shuffle on" else "Shuffle off")
            .setSessionCommand(commands[0])
            .setSlots(CommandButton.SLOT_OVERFLOW)
            .build()
        val repeatButton = CommandButton.Builder(
            when (repeat) {
                RepeatMode.OFF -> CommandButton.ICON_REPEAT_OFF
                RepeatMode.ALL -> CommandButton.ICON_REPEAT_ALL
                RepeatMode.ONE -> CommandButton.ICON_REPEAT_ONE
            },
        )
            .setDisplayName(
                when (repeat) {
                    RepeatMode.OFF -> "Repeat off"
                    RepeatMode.ALL -> "Repeat all"
                    RepeatMode.ONE -> "Repeat one"
                },
            )
            .setSessionCommand(commands[1])
            .setSlots(CommandButton.SLOT_OVERFLOW)
            .build()
        // Liking needs an account; signed out there is no heart.
        val heart = if (core.signedIn() && likedFor != null) {
            CommandButton.Builder(if (liked == true) CommandButton.ICON_HEART_FILLED else CommandButton.ICON_HEART_UNFILLED)
                .setDisplayName(if (liked == true) "Liked" else "Like")
                .setSessionCommand(commands[2])
                .setSlots(CommandButton.SLOT_OVERFLOW)
                .build()
        } else {
            null
        }
        return listOfNotNull(heart, shuffleButton, repeatButton)
    }

    /** A button pressed in the car (or the notification). */
    fun press(action: String) {
        when (action) {
            SHUFFLE -> { core.setShuffle(!shuffle); shuffle = !shuffle }
            REPEAT -> {
                repeat = when (repeat) {
                    RepeatMode.OFF -> RepeatMode.ALL
                    RepeatMode.ALL -> RepeatMode.ONE
                    RepeatMode.ONE -> RepeatMode.OFF
                }
                core.setRepeat(repeat)
            }
            LIKE -> {
                val id = likedFor ?: return
                val want = liked != true
                liked = want
                work.execute {
                    // Back as it was if YouTube Music says no.
                    if (runCatching { core.setLiked(id, want) }.isFailure) main.post { if (likedFor == id) liked = !want; update() }
                }
            }
        }
        update()
    }

    /** Follows the core twice a second: the buttons, the song's like, and
     *  any failure to tell the car about. */
    private val tick = object : Runnable {
        override fun run() {
            val p = core.playback()
            shuffle = p.shuffle
            repeat = p.repeat
            val id = core.nowPlaying()?.videoId
            if (id != likedFor) {
                likedFor = id
                liked = null
                if (id != null && core.signedIn()) {
                    work.execute {
                        val known = runCatching { core.isLiked(id) }.getOrNull()
                        main.post { if (likedFor == id) { liked = known; update() } }
                    }
                }
            }
            update()
            val failed = if (p.state == PlayState.FAILED) p.error else ""
            if (failed != error) {
                error = failed
                tellCar(failed)
            }
            main.postDelayed(this, 500)
        }
    }

    private fun update() {
        val session = session ?: return
        val now = buttons()
        // Compared by what they show; a new list each time would redraw them.
        if (now.map { it.icon to it.displayName } != shown.map { it.icon to it.displayName }) {
            shown = now
            session.setMediaButtonPreferences(now)
        }
    }

    private fun tellCar(message: String) {
        val session = session ?: return
        val exception = if (message.isEmpty()) null else
            PlaybackException(message, null, PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED)
        session.connectedControllers
            .filter { session.isAutoCompanionController(it) || session.isAutomotiveController(it) }
            .forEach { session.setPlaybackException(it, exception) }
    }

    companion object {
        const val SHUFFLE = "app.tonearm.SHUFFLE"
        const val REPEAT = "app.tonearm.REPEAT"
        const val LIKE = "app.tonearm.LIKE"
    }
}
