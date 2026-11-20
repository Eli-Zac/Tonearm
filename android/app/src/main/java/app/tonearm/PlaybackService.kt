package app.tonearm

import android.app.PendingIntent
import android.content.Intent
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.DefaultMediaNotificationProvider
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import uniffi.tonearm_android.Tonearm

/**
 * Keeps the music going with the app closed or the screen off, and puts it
 * where Android shows media: the notification, the lock screen, Bluetooth
 * and headphone buttons, Android Auto and watches. Media3 starts it in the
 * foreground while something plays.
 */
@UnstableApi
class PlaybackService : MediaSessionService() {
    private var session: MediaSession? = null

    override fun onCreate() {
        super.onCreate()
        val core = (application as TonearmApp).core
        // Tapping the notification opens the app.
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        session = MediaSession.Builder(this, TonearmPlayer(this, core))
            .setSessionActivity(open)
            .setCallback(Callback(core))
            .build()
        // Tonearm's own icon in the status bar, not the generic one.
        setMediaNotificationProvider(
            DefaultMediaNotificationProvider.Builder(this).build().apply {
                setSmallIcon(R.drawable.ic_notification)
            },
        )
    }

    private class Callback(private val core: Tonearm) : MediaSession.Callback {
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

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    /** Swiped away from recents: the music stops only if it wasn't playing. */
    override fun onTaskRemoved(rootIntent: Intent?) {
        val player = session?.player
        if (player == null || !player.playWhenReady || player.mediaItemCount == 0) {
            stopSelf()
        }
    }

    override fun onDestroy() {
        session?.run {
            player.release()
            release()
        }
        session = null
        super.onDestroy()
    }
}
