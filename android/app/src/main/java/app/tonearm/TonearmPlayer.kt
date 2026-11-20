package app.tonearm

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.SimpleBasePlayer
import androidx.media3.common.util.UnstableApi
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import java.util.concurrent.Executors
import uniffi.tonearm_android.PlayState
import uniffi.tonearm_android.Playback
import uniffi.tonearm_android.RepeatMode
import uniffi.tonearm_android.Song
import uniffi.tonearm_android.Tonearm

/**
 * The Rust engine as a Media3 player. Android's media system (the
 * notification, the lock screen, headphone and Bluetooth buttons, Android
 * Auto, the output switcher) drives it like any other player, and every
 * button goes to the core, which decides what plays.
 *
 * The core keeps its own state. This reads it a few times a second and
 * tells Media3 when it changed. It also does what a phone player owes the
 * rest of the phone: it gives way to calls and other apps (audio focus) and
 * pauses when headphones are unplugged.
 */
@UnstableApi
class TonearmPlayer(private val context: Context, private val core: Tonearm) :
    SimpleBasePlayer(Looper.getMainLooper()) {

    /** Calls into the core that can reach the network run here, in order. */
    private val worker = MoreExecutors.listeningDecorator(Executors.newSingleThreadExecutor())
    private val handler = Handler(Looper.getMainLooper())
    private val audio = context.getSystemService(AudioManager::class.java)

    private var playback: Playback = core.playback()
    private var queue: List<Song> = core.queue()
    /** Held while we play, so other apps and calls know to ask. */
    private var hasFocus = false
    /** Paused for a call or another app's sound, to carry on after. */
    private var pausedForFocus = false

    private val poll = object : Runnable {
        override fun run() {
            refresh()
            handler.postDelayed(this, 300)
        }
    }

    init {
        handler.post(poll)
    }

    /** Reads the core, and tells Media3 only when something it shows moved. */
    private fun refresh() {
        val now = core.playback()
        val before = playback
        playback = now
        if (now.version != before.version) queue = core.queue()
        // Songs also start from the app's own screens, straight through
        // the core. Whoever started it, playing means holding focus.
        if (now.state == PlayState.PLAYING && !hasFocus && !focus()) {
            core.pause()
        }
        if (now.state != before.state || now.version != before.version ||
            now.durationMs != before.durationMs || now.repeat != before.repeat ||
            now.error != before.error
        ) {
            invalidateState()
        }
        noisy(now.state == PlayState.PLAYING)
    }

    override fun getState(): State {
        val p = playback
        val commands = Player.Commands.Builder()
            .addAll(
                COMMAND_PLAY_PAUSE, COMMAND_STOP,
                COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM, COMMAND_SEEK_TO_MEDIA_ITEM,
                COMMAND_SEEK_TO_NEXT, COMMAND_SEEK_TO_NEXT_MEDIA_ITEM,
                COMMAND_SEEK_TO_PREVIOUS, COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM,
                COMMAND_SET_MEDIA_ITEM, COMMAND_CHANGE_MEDIA_ITEMS,
                COMMAND_GET_CURRENT_MEDIA_ITEM, COMMAND_GET_TIMELINE, COMMAND_GET_METADATA,
                COMMAND_SET_REPEAT_MODE, COMMAND_RELEASE,
            )
            .build()
        val state = when (p.state) {
            PlayState.LOADING -> STATE_BUFFERING
            PlayState.PLAYING, PlayState.PAUSED -> STATE_READY
            PlayState.ENDED -> STATE_ENDED
            PlayState.FAILED -> STATE_IDLE
            PlayState.IDLE -> if (queue.isEmpty()) STATE_IDLE else STATE_ENDED
        }
        val playlist = queue.mapIndexed { i, song ->
            val item = MediaItemData.Builder(song.videoId + "#" + i).setMediaItem(mediaItem(song))
            if (i == p.index.toInt() && p.durationMs > 0u) item.setDurationUs(p.durationMs.toLong() * 1000)
            item.build()
        }
        val builder = State.Builder()
            .setAvailableCommands(commands)
            .setPlaylist(playlist)
            .setPlaybackState(state)
            .setPlayWhenReady(
                p.state == PlayState.PLAYING || p.state == PlayState.LOADING,
                PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST,
            )
            .setRepeatMode(
                when (p.repeat) {
                    RepeatMode.OFF -> REPEAT_MODE_OFF
                    RepeatMode.ALL -> REPEAT_MODE_ALL
                    RepeatMode.ONE -> REPEAT_MODE_ONE
                },
            )
            .setContentPositionMs { core.playback().positionMs.toLong() }
        if (queue.isNotEmpty()) builder.setCurrentMediaItemIndex(p.index.toInt().coerceIn(0, queue.size - 1))
        // A song that fails just stops here. Its error is shown inside the
        // app from the core: handed to Media3, Android pops it up as well.
        return builder.build()
    }

    override fun handleSetPlayWhenReady(playWhenReady: Boolean): ListenableFuture<*> {
        pausedForFocus = false
        if (playWhenReady) {
            if (focus()) core.resume()
        } else {
            core.pause()
        }
        return later { refresh() }
    }

    /** The queue Android Auto's pick was worked out to, set just before
     *  Media3 hands its items here. */
    @Volatile
    var pending: List<Song>? = null

    override fun handleSetMediaItems(
        mediaItems: MutableList<MediaItem>,
        startIndex: Int,
        startPositionMs: Long,
    ): ListenableFuture<*> {
        // A queue built for a pick in the car comes whole, with every
        // song's details; the media items carry only some of them.
        val built = pending?.takeIf { it.map(Song::videoId) == mediaItems.map { m -> m.mediaId } }
        pending = null
        val songs = built ?: mediaItems.map(::song)
        val start = if (startIndex == C.INDEX_UNSET) 0 else startIndex
        focus()
        return work { core.setQueue(songs, start.toUInt()) }
    }

    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int): ListenableFuture<*> {
        val current = playback.index.toInt()
        return when (seekCommand) {
            // The core keeps the desktop's rules for next and previous.
            COMMAND_SEEK_TO_NEXT, COMMAND_SEEK_TO_NEXT_MEDIA_ITEM -> work { core.next() }
            COMMAND_SEEK_TO_PREVIOUS, COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM -> work { core.previous() }
            else -> if (mediaItemIndex != current && mediaItemIndex != C.INDEX_UNSET) {
                work { core.skipTo(mediaItemIndex.toUInt()) }
            } else {
                core.seek(positionMs.coerceAtLeast(0) / 1000f)
                later { refresh() }
            }
        }
    }

    override fun handleSetRepeatMode(repeatMode: Int): ListenableFuture<*> {
        core.setRepeat(
            when (repeatMode) {
                REPEAT_MODE_ALL -> RepeatMode.ALL
                REPEAT_MODE_ONE -> RepeatMode.ONE
                else -> RepeatMode.OFF
            },
        )
        return later { refresh() }
    }

    override fun handleStop(): ListenableFuture<*> {
        core.pause()
        return later { refresh() }
    }

    override fun handleRelease(): ListenableFuture<*> {
        handler.removeCallbacks(poll)
        core.pause()
        noisy(false)
        abandonFocus()
        worker.shutdown()
        return Futures.immediateVoidFuture()
    }

    /** Runs a core call that can block on the worker, then reads the core. */
    private fun work(call: () -> Unit): ListenableFuture<*> {
        val future = worker.submit {
            try {
                call()
            } catch (e: Exception) {
                // The core keeps the reason, which shows as the player's error.
            }
        }
        future.addListener({ refresh() }, ContextCompat.getMainExecutor(context))
        return future
    }

    private fun later(call: () -> Unit): ListenableFuture<*> {
        call()
        return Futures.immediateVoidFuture()
    }

    // ---------------------------------------------------------- audio focus

    private val focusListener = AudioManager.OnAudioFocusChangeListener { change ->
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS -> {
                // Another app is playing now. Stay paused.
                pausedForFocus = false
                hasFocus = false
                core.pause()
            }
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                // A call, or a voice note. Carry on after.
                if (playback.state == PlayState.PLAYING) {
                    pausedForFocus = true
                    core.pause()
                }
            }
            AudioManager.AUDIOFOCUS_GAIN -> if (pausedForFocus) {
                pausedForFocus = false
                core.resume()
            }
            // Navigation prompts and the like: Android lowers the volume
            // itself while they speak.
        }
        refresh()
    }

    private val focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
        .setAudioAttributes(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build(),
        )
        .setOnAudioFocusChangeListener(focusListener, handler)
        .setWillPauseWhenDucked(false)
        .build()

    /** Asks to be the app playing. False during a phone call. */
    private fun focus(): Boolean {
        hasFocus = audio.requestAudioFocus(focusRequest) != AudioManager.AUDIOFOCUS_REQUEST_FAILED
        return hasFocus
    }

    private fun abandonFocus() {
        audio.abandonAudioFocusRequest(focusRequest)
        hasFocus = false
    }

    // ------------------------------------------------ headphones unplugged

    private val unplugged = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            // Headphones out or Bluetooth gone: never on to the speaker.
            core.pause()
            refresh()
        }
    }
    private var listening = false

    /** Listens for headphones going while something plays, and only then. */
    private fun noisy(on: Boolean) {
        if (on == listening) return
        listening = on
        if (on) {
            ContextCompat.registerReceiver(
                context, unplugged,
                IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY),
                ContextCompat.RECEIVER_NOT_EXPORTED,
            )
        } else {
            context.unregisterReceiver(unplugged)
        }
    }

    companion object {
        fun mediaItem(song: Song): MediaItem = MediaItem.Builder()
            .setMediaId(song.videoId)
            .setMediaMetadata(
                MediaMetadata.Builder()
                    .setTitle(song.title)
                    .setArtist(song.artist)
                    .setAlbumTitle(song.album.ifEmpty { null })
                    .setArtworkUri(song.cover.ifEmpty { null }?.let(Uri::parse))
                    .setIsPlayable(true)
                    .setIsBrowsable(false)
                    .build(),
            )
            .build()

        fun song(item: MediaItem): Song {
            val meta = item.mediaMetadata
            return Song(
                videoId = item.mediaId,
                title = meta.title?.toString() ?: "",
                artist = meta.artist?.toString() ?: "",
                album = meta.albumTitle?.toString() ?: "",
                cover = meta.artworkUri?.toString() ?: "",
                duration = "",
                artists = emptyList(),
                albumId = null,
            )
        }
    }
}
