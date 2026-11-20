package app.tonearm

import android.app.PendingIntent
import android.content.Intent
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Handler
import android.os.Looper
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.DefaultMediaNotificationProvider
import android.os.Bundle
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaLibraryService.LibraryParams
import androidx.media3.session.MediaLibraryService.MediaLibrarySession
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaConstants
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionError
import androidx.media3.session.SessionResult
import com.google.common.util.concurrent.ListeningExecutorService
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import java.util.concurrent.Executors
import uniffi.tonearm_android.Tonearm

/**
 * Keeps the music going with the app closed or the screen off, and puts it
 * where Android shows media: the notification, the lock screen, Bluetooth
 * and headphone buttons, Android Auto and watches. Media3 starts it in the
 * foreground while something plays.
 */
@UnstableApi
class PlaybackService : MediaLibraryService() {
    private var session: MediaLibrarySession? = null
    private var devices: AudioDeviceCallback? = null
    private var controls: CarControls? = null

    override fun onCreate() {
        super.onCreate()
        val core = (application as TonearmApp).core
        // Tapping the notification opens the app.
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val player = TonearmPlayer(this, core)
        val work = MoreExecutors.listeningDecorator(Executors.newFixedThreadPool(2))
        val car = CarControls(core, work)
        controls = car
        session = MediaLibrarySession.Builder(this, player, Callback(core, player, AutoLibrary(core), car, work))
            .setSessionActivity(open)
            .setMediaButtonPreferences(car.buttons())
            // Skip keeps its place on every controller: a Galaxy Watch
            // otherwise puts the like and shuffle buttons where skip goes.
            .setSessionExtras(
                Bundle().apply {
                    putBoolean(MediaConstants.EXTRAS_KEY_SLOT_RESERVATION_SEEK_TO_PREV, true)
                    putBoolean(MediaConstants.EXTRAS_KEY_SLOT_RESERVATION_SEEK_TO_NEXT, true)
                },
            )
            .build()
            .also { car.start(it) }
        // A car, earbuds or headphones coming or going: the core's output
        // stays on the device it opened, which on Android may have left
        // (Android Auto's stream goes silent rather than following), so it
        // opens the default output again. Once at once, and once more after
        // Bluetooth has had time to take the route.
        val audio = getSystemService(AUDIO_SERVICE) as AudioManager
        val main = Handler(Looper.getMainLooper())
        val reopen = Runnable { core.audioDevicesChanged() }
        devices = object : AudioDeviceCallback() {
            // Registering reports every device there already; only later
            // calls are changes.
            private var first = true
            override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) {
                if (first) { first = false; return }
                changed(added)
            }
            override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = changed(removed)
            private fun changed(which: Array<out AudioDeviceInfo>) {
                if (which.none { it.isSink }) return
                main.removeCallbacks(reopen)
                core.audioDevicesChanged()
                main.postDelayed(reopen, 1500)
            }
        }.also { audio.registerAudioDeviceCallback(it, main) }
        // Tonearm's own icon in the status bar, not the generic one.
        setMediaNotificationProvider(
            DefaultMediaNotificationProvider.Builder(this).build().apply {
                setSmallIcon(R.drawable.ic_notification)
            },
        )
    }

    /**
     * The session's answers: to the phone's own screen, and to Android Auto,
     * which browses `library` and plays what is picked there.
     */
    private class Callback(
        private val core: Tonearm,
        private val player: TonearmPlayer,
        private val library: AutoLibrary,
        private val car: CarControls,
        /** The car's browsing reaches the network, so it runs off the main thread. */
        private val work: ListeningExecutorService,
    ) : MediaLibrarySession.Callback {
        /** Everyone gets the usual commands, plus shuffle, repeat and like. */
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            val commands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon()
                .apply { car.commands.forEach { add(it) } }
                .build()
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session, controller)
                .setAvailableSessionCommands(commands)
                .setMediaButtonPreferences(car.buttons())
                .build()
        }

        override fun onCustomCommand(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
            customCommand: SessionCommand,
            args: Bundle,
        ): ListenableFuture<SessionResult> {
            car.press(customCommand.customAction)
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }

        override fun onGetLibraryRoot(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<MediaItem>> {
            // Folders of albums show as a grid, songs as a list, unless a
            // folder says otherwise.
            val style = Bundle().apply {
                putBoolean(AutoLibrary.STYLE_SUPPORTED, true)
                putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_BROWSABLE, AutoLibrary.GRID)
                putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_PLAYABLE, AutoLibrary.LIST)
            }
            // Asked for what played last (Android Auto starting up): the
            // queue as it was left, if there is one.
            if (params?.isRecent == true) {
                if (core.queue().isEmpty()) return Futures.immediateFuture(LibraryResult.ofError(SessionError.ERROR_NOT_SUPPORTED))
                return Futures.immediateFuture(
                    LibraryResult.ofItem(library.recentRoot(), LibraryParams.Builder().setRecent(true).setExtras(style).build()),
                )
            }
            return Futures.immediateFuture(
                LibraryResult.ofItem(library.root(), LibraryParams.Builder().setExtras(style).build()),
            )
        }

        override fun onGetChildren(
            session: MediaLibrarySession,
            browser: MediaSession.ControllerInfo,
            parentId: String,
            page: Int,
            pageSize: Int,
            params: LibraryParams?,
        ): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> = work.submit<LibraryResult<ImmutableList<MediaItem>>> {
            try {
                val all = library.children(parentId)
                val from = (page.toLong() * pageSize).coerceAtMost(all.size.toLong()).toInt()
                val to = (from.toLong() + pageSize).coerceAtMost(all.size.toLong()).toInt()
                LibraryResult.ofItemList(ImmutableList.copyOf(all.subList(from, to)), params)
            } catch (e: Exception) {
                LibraryResult.ofError(SessionError.ERROR_IO)
            }
        }

        /**
         * Picked in the car: the queue the phone would play for the same tap,
         * handed to the player whole, so its songs keep their details. Anything
         * else (the phone's own screen, a resumed queue) passes as it is.
         */
        override fun onSetMediaItems(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            mediaItems: MutableList<MediaItem>,
            startIndex: Int,
            startPositionMs: Long,
        ): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val picked = mediaItems.singleOrNull()?.mediaId
            if (picked == null || !library.knows(picked)) {
                return super.onSetMediaItems(mediaSession, controller, mediaItems, startIndex, startPositionMs)
            }
            return work.submit<MediaSession.MediaItemsWithStartPosition> {
                val (songs, start) = library.queueFor(picked) ?: throw IllegalStateException("gone")
                if (songs.isEmpty()) throw IllegalStateException("Nothing to play")
                player.pending = songs
                MediaSession.MediaItemsWithStartPosition(songs.map(TonearmPlayer::mediaItem), start, C.TIME_UNSET)
            }
        }

        /** Songs come by their video id, with no link: the core finds the
         *  stream itself. Media3 otherwise turns away items without one. */
        override fun onAddMediaItems(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            mediaItems: MutableList<MediaItem>,
        ): ListenableFuture<MutableList<MediaItem>> = Futures.immediateFuture(mediaItems)

        /** Play pressed on headphones or the lock screen with nothing
         *  loaded: carry on the queue the core has. */
        override fun onPlaybackResumption(
            mediaSession: MediaSession,
            controller: MediaSession.ControllerInfo,
            isForPlayback: Boolean,
        ): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val queue = core.queue()
            if (queue.isEmpty()) {
                return Futures.immediateFailedFuture(UnsupportedOperationException("nothing to resume"))
            }
            val index = core.playback().index.toInt().coerceIn(0, queue.size - 1)
            return Futures.immediateFuture(
                MediaSession.MediaItemsWithStartPosition(
                    queue.map(TonearmPlayer::mediaItem), index, C.TIME_UNSET,
                ),
            )
        }
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession? = session

    /** Swiped away from recents: the music stops only if it wasn't playing. */
    override fun onTaskRemoved(rootIntent: Intent?) {
        val player = session?.player
        if (player == null || !player.playWhenReady || player.mediaItemCount == 0) {
            stopSelf()
        }
    }

    override fun onDestroy() {
        devices?.let { (getSystemService(AUDIO_SERVICE) as AudioManager).unregisterAudioDeviceCallback(it) }
        devices = null
        controls?.stop()
        controls = null
        session?.run {
            player.release()
            release()
        }
        session = null
        super.onDestroy()
    }
}
