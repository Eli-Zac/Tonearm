package app.tonearm

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.net.Uri
import androidx.core.content.ContextCompat
import java.io.File
import java.io.FileNotFoundException
import java.security.MessageDigest

/**
 * The pictures of a Party in the car, drawn as PNG files for
 * [ArtProvider] to hand over: the QR code to join by, a member's letter
 * avatar, and the tiles for Members and End Party.
 */
object PartyArt {
    fun file(context: Context, uri: Uri): File {
        val folder = File(context.cacheDir, "party").apply { mkdirs() }
        val key = MessageDigest.getInstance("SHA-1").digest(uri.toString().toByteArray())
            .joinToString("") { "%02x".format(it) }
        val file = File(folder, "$key.png")
        if (file.isFile) return file
        val bitmap = when (uri.path) {
            "/party-qr" -> qr(context, uri.getQueryParameter("c") ?: throw FileNotFoundException("no code"))
            "/avatar" -> avatar(uri.getQueryParameter("n") ?: "?")
            "/tile" -> tile(context, uri.getQueryParameter("i") ?: throw FileNotFoundException("no icon"))
            else -> throw FileNotFoundException("not party art")
        }
        val part = File(folder, "$key.part")
        part.outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        part.renameTo(file)
        return file
    }

    /** Black modules on white, with the four-module quiet border a camera
     *  needs to find the code, in sun or shade. */
    private fun qr(context: Context, code: String): Bitmap {
        val core = (context.applicationContext as TonearmApp).core
        val qr = core.partyQr(code) ?: throw FileNotFoundException("no qr")
        val size = qr.size.toInt()
        val module = 16
        val border = 4
        val side = (size + border * 2) * module
        val bitmap = Bitmap.createBitmap(side, side, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        canvas.drawColor(Color.WHITE)
        val dark = Paint().apply { color = Color.BLACK }
        for (y in 0 until size) for (x in 0 until size) {
            if (qr.dark[y * size + x]) {
                val left = ((x + border) * module).toFloat()
                val top = ((y + border) * module).toFloat()
                canvas.drawRect(left, top, left + module, top + module, dark)
            }
        }
        return bitmap
    }

    private val COLOURS = intArrayOf(0xFFE0565B.toInt(), 0xFF4C8DF6.toInt(), 0xFF3FB27F.toInt(), 0xFFE39B3B.toInt(), 0xFF9B6BE0.toInt(), 0xFF2BB3C0.toInt())

    private fun avatar(name: String): Bitmap {
        val side = 256
        val bitmap = Bitmap.createBitmap(side, side, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = COLOURS[Math.floorMod(name.hashCode(), COLOURS.size)] }
        canvas.drawCircle(side / 2f, side / 2f, side / 2f, fill)
        val letter = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.WHITE
            textSize = side * 0.45f
            typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
            textAlign = Paint.Align.CENTER
        }
        val text = name.firstOrNull()?.uppercase() ?: "?"
        val y = side / 2f - (letter.descent() + letter.ascent()) / 2
        canvas.drawText(text, side / 2f, y, letter)
        return bitmap
    }

    private fun tile(context: Context, icon: String): Bitmap {
        val res = when (icon) {
            "members" -> R.drawable.ic_party_members
            "end" -> R.drawable.ic_party_end
            else -> throw FileNotFoundException("no such icon")
        }
        val side = 512
        val bitmap = Bitmap.createBitmap(side, side, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        val back = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = 0xFF2C2C2C.toInt() }
        canvas.drawRoundRect(RectF(0f, 0f, side.toFloat(), side.toFloat()), 48f, 48f, back)
        val drawable = ContextCompat.getDrawable(context, res) ?: throw FileNotFoundException("no drawable")
        val inset = side / 4
        drawable.setBounds(inset, inset, side - inset, side - inset)
        drawable.draw(canvas)
        return bitmap
    }
}
