package app.tonearm

import android.os.SystemClock
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.Shape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.unit.dp

/** One sweep across the screen and out the other side, as the desktop's. */
private const val SWEEP_MS = 1600L

/**
 * The desktop's `Bone`, which every skeleton is built from: a faint block
 * with a band of light sweeping left to right across it. The band's place
 * is worked out across the whole window from one shared clock, so a single
 * wave passes over every bone on the screen together, as on the desktop.
 *
 * The desktop's band is 800px wide and travels 3200px every 1.6 s over a
 * window about 1400px across; here the same proportions of the phone's
 * width. It moves in the draw phase only, so it costs no recomposition.
 */
@Composable
fun Bone(modifier: Modifier, shape: Shape = RoundedCornerShape(6.dp)) {
    // Only to redraw every frame; the place comes from the clock, so bones
    // that appeared at different times still share one wave.
    val tick by rememberInfiniteTransition(label = "bone").animateFloat(
        0f, 1f, infiniteRepeatable(tween(SWEEP_MS.toInt(), easing = LinearEasing)), label = "tick",
    )
    val screen = LocalWindowInfo.current.containerSize.width.toFloat()
    var left by remember { mutableFloatStateOf(0f) }
    Box(
        modifier
            .clip(shape)
            .onGloballyPositioned { left = it.positionInWindow().x }
            .drawBehind {
                @Suppress("UNUSED_EXPRESSION") tick
                val phase = (SystemClock.uptimeMillis() % SWEEP_MS) / SWEEP_MS.toFloat()
                val band = screen * 0.57f
                val start = phase * screen * 2.3f - band - left
                drawRect(BASE)
                drawRect(
                    Brush.horizontalGradient(
                        0f to Color.Transparent, 0.5f to LIGHT, 1f to Color.Transparent,
                        startX = start, endX = start + band,
                    ),
                )
            },
    )
}

// The desktop's #ffffff0e block and #ffffff16 band.
private val BASE = Color(0x0EFFFFFF)
private val LIGHT = Color(0x16FFFFFF)
