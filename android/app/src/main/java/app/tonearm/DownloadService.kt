package app.tonearm

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper

/**
 * Keeps the app alive while songs download, so leaving the app or turning
 * the screen off doesn't cut an album off half way. The downloading itself
 * is the core's; this only holds the process in the foreground, with a
 * notification of how far it has got, and stops once nothing is queued.
 */
class DownloadService : Service() {
    private val main = Handler(Looper.getMainLooper())
    private var done = 0
    private var total = 0
    private var seen = 0uL

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = notification(0, 0f, "Getting ready")
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(ID, notification)
        }
        main.removeCallbacks(tick)
        main.post(tick)
        return START_NOT_STICKY
    }

    /** Checks the queue twice a second: the notification follows it, and
     *  the service ends once it is empty. */
    private val tick = object : Runnable {
        override fun run() {
            val core = (application as TonearmApp).core
            val version = core.downloadsVersion()
            if (version != seen) {
                seen = version
                val marks = core.downloadMarks()
                if (marks.busy.isEmpty()) {
                    stopForeground(STOP_FOREGROUND_REMOVE)
                    stopSelf()
                    return
                }
                // A batch grows as more is asked for; it counts what has
                // landed since this service started.
                val left = marks.busy.size
                if (left + done > total) total = left + done
                done = total - left
                val first = marks.busy.first()
                getSystemService(NotificationManager::class.java)
                    .notify(ID, notification(left, first.progress, core.downloadTitle(first.videoId)))
            }
            main.postDelayed(this, 500)
        }
    }

    private fun notification(left: Int, progress: Float, title: String): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW),
        )
        val open = PendingIntent.getActivity(
            this, 1,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        // The whole batch's progress, the song on its way counted in part.
        val whole = if (total > 0) ((done + progress) / total * 1000).toInt() else 0
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(if (left <= 1) "Downloading" else "Downloading $left songs")
            .setContentText(title)
            .setProgress(1000, whole, total == 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
            .build()
    }

    override fun onDestroy() {
        main.removeCallbacks(tick)
        super.onDestroy()
    }

    companion object {
        private const val ID = 2
        private const val CHANNEL = "downloads"

        /** Starts holding the app up while downloads run. Called with the
         *  app on screen, which Android requires for a foreground start. */
        fun start(context: Context) {
            runCatching { context.startForegroundService(Intent(context, DownloadService::class.java)) }
        }
    }
}
