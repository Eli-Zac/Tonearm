package app.tonearm

import android.app.Activity
import android.app.Application
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import com.discord.socialsdk.DiscordSocialSdkInit

/**
 * What Discord's Social SDK is given where it asks for an activity.
 *
 * The SDK binds the Discord app's service through that activity, and Android
 * drops such a binding when the activity is destroyed, so presence would stop
 * as soon as the screen was closed while the music played on. This stands in
 * for the activity and binds through the application instead, which lives as
 * long as the playback does. It is never shown, and only the calls the SDK
 * makes for Rich Presence are forwarded.
 */
class DiscordHost private constructor(private val app: Application) : Activity() {
    override fun getApplicationContext(): Context = app
    override fun getPackageManager(): PackageManager = app.packageManager
    override fun getPackageName(): String = app.packageName
    override fun bindService(service: Intent, conn: ServiceConnection, flags: Int): Boolean =
        app.bindService(service, conn, flags)
    override fun unbindService(conn: ServiceConnection) = app.unbindService(conn)

    companion object {
        /** Loads the SDK's native library and hands it the stand-in. Once. */
        fun install(app: Application) {
            DiscordSocialSdkInit.setEngineActivity(DiscordHost(app))
        }
    }
}
