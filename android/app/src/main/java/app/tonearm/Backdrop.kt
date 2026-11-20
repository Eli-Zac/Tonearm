package app.tonearm

import android.graphics.Bitmap
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.nio.ByteBuffer
import uniffi.tonearm_android.backdrop
import uniffi.tonearm_android.backdropWave

/** The flow's frames, a side: `tonearm_core::backdrop::FRAME`. */
private const val FRAME = 64

/**
 * The desktop's expanded Now playing background: the cover worked down to
 * a soft wash of its colours, kept dark enough for white words, flowing
 * slowly like ink in water, under a light shade.
 *
 * Cheap to run: the core makes each 64 pixel frame, about 30 a second, into
 * one bitmap that is reused, and the GPU stretches it smoothly over the
 * screen. Only drawing reruns, never composition, and the frames follow
 * the display's clock, so nothing runs while the screen is off or hidden.
 */
@Composable
fun FlowingBackdrop(cover: String?, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    // The still backdrop, from a small copy of the cover. The last one
    // stays up until the next is ready.
    val base by produceState<ByteArray?>(null, cover) {
        if (cover == null) return@produceState
        val made = withContext(Dispatchers.IO) {
            runCatching {
                val request = ImageRequest.Builder(context).data(cover).size(96).allowHardware(false).build()
                val result = SingletonImageLoader.get(context).execute(request) as? SuccessResult
                val bitmap = result?.image?.toBitmap() ?: return@runCatching null
                val (w, h) = bitmap.width to bitmap.height
                val argb = IntArray(w * h)
                bitmap.getPixels(argb, 0, w, 0, 0, w, h)
                val rgba = ByteArray(w * h * 4)
                for (i in argb.indices) {
                    val p = argb[i]
                    rgba[i * 4] = (p shr 16).toByte()
                    rgba[i * 4 + 1] = (p shr 8).toByte()
                    rgba[i * 4 + 2] = p.toByte()
                    rgba[i * 4 + 3] = -1
                }
                backdrop(rgba, w.toUInt(), h.toUInt())
            }.onFailure { android.util.Log.w("Tonearm", "No backdrop for the cover", it) }.getOrNull()
        }
        if (made != null) value = made
    }

    val frame = remember { Bitmap.createBitmap(FRAME, FRAME, Bitmap.Config.ARGB_8888) }
    val image = remember(frame) { frame.asImageBitmap() }
    // The last song's background, held still while the next fades in over
    // it, so a new song never cuts.
    val from = remember { Bitmap.createBitmap(FRAME, FRAME, Bitmap.Config.ARGB_8888) }
    val fromImage = remember(from) { from.asImageBitmap() }
    val fade = remember { Animatable(1f) }
    // Bumped as each frame lands, so the canvas redraws.
    var drawn by remember { mutableIntStateOf(0) }
    LaunchedEffect(base) {
        val still = base ?: return@LaunchedEffect
        if (drawn > 0) {
            android.graphics.Canvas(from).drawBitmap(frame, 0f, 0f, null)
            fade.snapTo(0f)
            launch { fade.animateTo(1f, tween(700, easing = FastOutSlowInEasing)) }
        }
        val start = withFrameNanos { it }
        var last = -1L
        while (true) {
            val now = withFrameNanos { it }
            if (last >= 0 && now - last < 33_000_000L) continue
            last = now
            val t = (now - start) / 1_000_000_000f
            val pixels = withContext(Dispatchers.Default) { backdropWave(still, t) }
            frame.copyPixelsFromBuffer(ByteBuffer.wrap(pixels))
            drawn++
        }
    }

    Canvas(modifier) {
        if (drawn == 0 || base == null) {
            drawRect(Tonearm.island)
        } else {
            // Fills the screen as the desktop's `image-fit: cover` does:
            // the frame's middle, cut to the screen's shape.
            val aspect = size.width / size.height
            val src = if (aspect < 1f) {
                val w = (FRAME * aspect).toInt().coerceAtLeast(1)
                IntOffset((FRAME - w) / 2, 0) to IntSize(w, FRAME)
            } else {
                val h = (FRAME / aspect).toInt().coerceAtLeast(1)
                IntOffset(0, (FRAME - h) / 2) to IntSize(FRAME, h)
            }
            val dst = IntSize(size.width.toInt(), size.height.toInt())
            // Read here, in drawing, so the fade only redraws.
            val t = fade.value
            if (t < 1f) {
                drawImage(fromImage, srcOffset = src.first, srcSize = src.second, dstSize = dst, filterQuality = FilterQuality.Low)
            }
            drawImage(
                image,
                srcOffset = src.first,
                srcSize = src.second,
                dstSize = dst,
                alpha = t,
                filterQuality = FilterQuality.Low,
            )
        }
        // Darkened a little, so the cover and words stand out on it.
        drawRect(Brush.verticalGradient(0f to Color(0x38000000), 1f to Color(0x70000000)))
    }
}
