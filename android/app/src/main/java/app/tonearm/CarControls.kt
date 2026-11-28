package app.tonearm

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
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
    /** Whether the song playing is liked, shared with the phone's own heart. */
    private var likedFor: String? = null
    private val liked get() = LikeState.of(likedFor)
    private var shown: List<CommandButton> = emptyList()
    private var error = ""

    val commands = listOf(SHUFFLE, REPEAT, LIKE, PARTY).map { SessionCommand(it, Bundle.EMPTY) }

    fun start(session: MediaSession) {
        this.session = session
        main.post(tick)
    }

    fun stop() {
        main.removeCallbacks(tick)
        session = null
    }

    /**
     * The buttons beside the transport. `full` is for the phone's
     * notification and Android Auto, which place extra buttons where they
     * fit; everything else (a Galaxy Watch) gets skip back and forward
     * alone, as its UI puts the first extra buttons where skip goes,
     * whatever slot they ask for.
     */
    fun buttons(full: Boolean = true): List<CommandButton> {
        // Skip back and forward named in their own slots, for the watch.
        val previous = CommandButton.Builder(CommandButton.ICON_PREVIOUS)
            .setDisplayName("Previous")
            .setPlayerCommand(Player.COMMAND_SEEK_TO_PREVIOUS)
            .setSlots(CommandButton.SLOT_BACK)
            .build()
        val next = CommandButton.Builder(CommandButton.ICON_NEXT)
            .setDisplayName("Next")
            .setPlayerCommand(Player.COMMAND_SEEK_TO_NEXT)
            .setSlots(CommandButton.SLOT_FORWARD)
            .build()
        if (!full) return listOf(previous, next)
        // The app's own shuffle and repeat icons: plain when off, with a
        // dot under them when on. Media3's own "off" icons are greyed out,
        // which reads as disabled.
        val shuffleButton = CommandButton.Builder(CommandButton.ICON_UNDEFINED)
            .setCustomIconResId(if (shuffle) R.drawable.ic_button_shuffle_on else R.drawable.ic_button_shuffle)
            .setDisplayName(if (shuffle) "Shuffle on" else "Shuffle off")
            .setSessionCommand(commands[0])
            .setSlots(CommandButton.SLOT_OVERFLOW)
            .build()
        val repeatButton = CommandButton.Builder(CommandButton.ICON_UNDEFINED)
            .setCustomIconResId(
                when (repeat) {
                    RepeatMode.OFF -> R.drawable.ic_button_repeat
                    RepeatMode.ALL -> R.drawable.ic_button_repeat_on
                    RepeatMode.ONE -> R.drawable.ic_button_repeat_one
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
        // No skip buttons here: the notification and car draw their own, and
        // these would show again beside them as extra buttons.
        // Start a Party, or, while one runs, View Party. Its tiles sit at
        // the top of the car's Home.
        val party = CommandButton.Builder(CommandButton.ICON_UNDEFINED)
            .setCustomIconResId(if (CarParty.active) R.drawable.ic_button_party_on else R.drawable.ic_button_party)
            .setDisplayName(if (CarParty.active) "View Party" else "Start a Party")
            .setSessionCommand(commands[3])
            .setSlots(CommandButton.SLOT_OVERFLOW)
            .build()
        return listOfNotNull(heart, shuffleButton, repeatButton, party)
    }

    /** Whether a controller gets the extra buttons: the notification (which
     *  the phone's media panel and Android Auto read) and the car itself. */
    fun wantsFull(session: MediaSession, controller: MediaSession.ControllerInfo) =
        session.isMediaNotificationController(controller) ||
            session.isAutoCompanionController(controller) || session.isAutomotiveController(controller)

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
                // Fills at once, and goes back if YouTube Music says no.
                work.execute { LikeState.flip(core, id); main.post { update() } }
            }
            // Starting one when there is none; with one running, the
            // button only names it (the car can't be sent to a page).
            PARTY -> {
                // The car can't be sent to Home, so it is told where to look.
                if (CarParty.active) {
                    tellCarOnce("Your Party is at the top of Home.")
                } else {
                    tellCarOnce("Starting a Party…")
                    work.execute {
                        val failed = CarParty.start(core)
                        main.post {
                            tellCarOnce(failed ?: "Party started. It's at the top of Home.")
                            update()
                        }
                    }
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
            // The Party, started or ended from the phone, the car or a guest.
            work.execute { CarParty.poll(core) }
            val id = core.nowPlaying()?.videoId
            likedFor = id
            if (id != null && core.signedIn() && LikeState.of(id) == null) {
                work.execute { LikeState.load(core, id); main.post { update() } }
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
        if (now.map { it.iconResId to it.displayName } != shown.map { it.iconResId to it.displayName }) {
            shown = now
            // Only those that place extra buttons well get them; the rest
            // keep skip back and forward, given as they connect.
            session.connectedControllers.filter { wantsFull(session, it) }
                .forEach { session.setMediaButtonPreferences(it, now) }
        }
    }

    /** A short note to the car's screen that isn't a failure: playback
     *  carries on, and Android Auto shows it briefly. */
    private fun tellCarOnce(message: String) {
        val session = session ?: return
        session.connectedControllers
            .filter { session.isAutoCompanionController(it) || session.isAutomotiveController(it) }
            .forEach { session.sendError(it, androidx.media3.session.SessionError(androidx.media3.session.SessionError.INFO_CANCELLED, message)) }
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
        const val PARTY = "app.tonearm.PARTY"
    }
}
