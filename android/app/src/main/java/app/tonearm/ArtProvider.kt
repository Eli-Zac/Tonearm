package app.tonearm

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import java.io.File
import java.io.FileNotFoundException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * Covers for Android Auto. The car's screen reads art only through a
 * content URI, not from the web, so this fetches a cover once, keeps it in
 * the cache folder, and hands the car the file. It serves only covers:
 * YouTube Music's image hosts, and the covers saved with downloads.
 */
class ArtProvider : ContentProvider() {
    override fun onCreate() = true

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        val context = context ?: throw FileNotFoundException("not ready")
        // A Party's pictures, drawn here rather than fetched.
        if (uri.path in PARTY_PATHS) {
            return ParcelFileDescriptor.open(PartyArt.file(context, uri), ParcelFileDescriptor.MODE_READ_ONLY)
        }
        val source = uri.getQueryParameter("u") ?: throw FileNotFoundException("no cover")
        val file = if (source.startsWith("/")) {
            // A download's cover: only from the app's own downloads folder.
            File(source).canonicalFile.takeIf {
                it.path.startsWith(File(context.filesDir, "").canonicalPath) && it.extension == "jpg" && it.isFile
            } ?: throw FileNotFoundException("not a cover")
        } else {
            fetch(source, File(context.cacheDir, "art"))
        }
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    /** The cover's file, fetched the first time it is asked for. */
    private fun fetch(link: String, folder: File): File {
        val url = runCatching { URL(link) }.getOrNull() ?: throw FileNotFoundException("bad link")
        if (url.protocol != "https" || HOSTS.none { url.host == it || url.host.endsWith(".$it") }) {
            android.util.Log.w("Tonearm", "art: not a cover host: ${url.host}")
            throw FileNotFoundException("not a cover host")
        }
        val name = MessageDigest.getInstance("SHA-1").digest(link.toByteArray()).joinToString("") { "%02x".format(it) }
        val file = File(folder, "$name.jpg")
        if (file.isFile) return file
        folder.mkdirs()
        val connection = url.openConnection() as HttpURLConnection
        connection.connectTimeout = 10_000
        connection.readTimeout = 15_000
        try {
            if (connection.responseCode != 200) throw FileNotFoundException("cover: ${connection.responseCode}")
            val part = File(folder, "$name.part")
            connection.inputStream.use { input ->
                part.outputStream().use { output ->
                    // Covers are small; nothing bigger is kept.
                    val buffer = ByteArray(16 * 1024)
                    var total = 0
                    while (true) {
                        val n = input.read(buffer)
                        if (n < 0) break
                        total += n
                        if (total > 4 * 1024 * 1024) throw FileNotFoundException("cover too big")
                        output.write(buffer, 0, n)
                    }
                }
            }
            part.renameTo(file)
        } finally {
            connection.disconnect()
        }
        trim(folder)
        return file
    }

    /** Keeps the cache of covers to a few hundred, oldest gone first. */
    private fun trim(folder: File) {
        val files = folder.listFiles { f -> f.extension == "jpg" } ?: return
        if (files.size <= 600) return
        files.sortedBy { it.lastModified() }.take(files.size - 500).forEach { it.delete() }
    }

    override fun getType(uri: Uri) = if (uri.path in PARTY_PATHS) "image/png" else "image/jpeg"
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?): Cursor? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?) = 0

    companion object {
        private const val AUTHORITY = "app.tonearm.art"
        private const val VERSION = "2"
        private val HOSTS = listOf("googleusercontent.com", "ggpht.com", "ytimg.com", "music.youtube.com", "gstatic.com", "scdn.co", "spotifycdn.com")

        private val PARTY_PATHS = setOf("/party-qr", "/avatar", "/tile")

        /** A Party's QR code, for the car's join tile. */
        fun partyQr(code: String): Uri = party("party-qr", "c", code)

        /** A round picture with a name's first letter, for a member. */
        fun avatar(name: String): Uri = party("avatar", "n", name)

        /** One of the app's icons on a tile, for the car: "members" or "end". */
        fun tile(icon: String): Uri = party("tile", "i", icon)

        private fun party(path: String, key: String, value: String): Uri =
            Uri.Builder().scheme("content").authority(AUTHORITY).path(path)
                .appendQueryParameter("v", VERSION).appendQueryParameter(key, value).build()

        /** The car's way to a cover: a link or a download's file path. */
        fun uri(art: String?): Uri? {
            if (art.isNullOrEmpty()) return null
            // The version is part of the address: Android Auto remembers a cover
            // that failed by its address, so a fix to how covers are served
            // bumps it.
            return Uri.Builder().scheme("content").authority(AUTHORITY).path("art")
                .appendQueryParameter("v", VERSION).appendQueryParameter("u", art).build()
        }
    }
}
