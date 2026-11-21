package app.tonearm

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.tonearm_android.Tonearm as Core
import java.io.File

/**
 * The app's own updates, from the GitHub releases page like the desktop's.
 * The core finds and downloads the latest release's APK (checked against
 * GitHub's SHA-256); Android's installer puts it in place. The first time,
 * Android asks to let Tonearm install apps and to confirm; the app then
 * restarts on the new version.
 *
 * `check` and `install` block on the network: run them off the main thread.
 */
object Updater {
    sealed interface State {
        data object Idle : State
        data object Checking : State
        data object Downloading : State
        data object Installing : State
        data class Failed(val message: String) : State
    }

    var state by mutableStateOf<State>(State.Idle)
        private set

    /** The newer version found, or null when this one is the latest (or unchecked). */
    var available by mutableStateOf<String?>(null)
        private set

    /** Set when a check finds a version the person hasn't been told about. */
    var announce: String? = null

    /** This app's version name. */
    fun current(context: Context): String =
        runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull() ?: ""

    /** Looks for a newer release. Quiet when it can't reach GitHub. */
    fun check(core: Core, context: Context, quiet: Boolean = true) {
        if (state is State.Checking || state is State.Downloading || state is State.Installing) return
        state = State.Checking
        try {
            val version = core.updateAvailable(current(context))
            if (version.isNotEmpty()) {
                if (available != version) announce = version
                available = version
            } else {
                available = null
            }
            state = State.Idle
        } catch (e: Exception) {
            state = if (quiet) State.Idle else State.Failed(e.message ?: "Couldn't check for updates")
        }
    }

    /** Downloads the update and hands it to Android's installer. */
    fun install(core: Core, context: Context) {
        if (state is State.Downloading || state is State.Installing) return
        state = State.Downloading
        try {
            val file = File(context.cacheDir, "update.apk")
            core.downloadUpdate(file.absolutePath)
            state = State.Installing
            val installer = context.packageManager.packageInstaller
            val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
            // An update of an app this installer put there needs no tap
            // after the first time, where Android allows it.
            if (Build.VERSION.SDK_INT >= 31) {
                params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            }
            val id = installer.createSession(params)
            installer.openSession(id).use { session ->
                file.inputStream().use { input ->
                    session.openWrite("tonearm", 0, file.length()).use { output ->
                        input.copyTo(output)
                        session.fsync(output)
                    }
                }
                val result = PendingIntent.getBroadcast(
                    context,
                    id,
                    Intent(context, InstallResult::class.java).setPackage(context.packageName),
                    PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
                session.commit(result.intentSender)
            }
        } catch (e: Exception) {
            state = State.Failed(e.message ?: "Couldn't install the update")
        }
    }

    internal fun failed(message: String) { state = State.Failed(message) }
    internal fun idle() { state = State.Idle }
}

/** Android's answer to an install: it may need the person's say first. */
class InstallResult : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                @Suppress("DEPRECATION")
                val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
                if (confirm != null) context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            // The app is replaced and restarted on success; nothing to show.
            PackageInstaller.STATUS_SUCCESS -> Updater.idle()
            PackageInstaller.STATUS_FAILURE_ABORTED -> Updater.idle()
            else -> Updater.failed(
                intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "Couldn't install the update",
            )
        }
    }
}
