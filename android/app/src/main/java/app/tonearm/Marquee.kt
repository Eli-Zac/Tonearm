package app.tonearm

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

/** CSS's ease-in-out, which the desktop's slide uses. */
private val EaseInOut = CubicBezierEasing(0.42f, 0f, 0.58f, 1f)

/**
 * The desktop's `Marquee`: a line of text that scrolls when it is too long
 * to fit, instead of wrapping or being cut short. It rests at the start,
 * slides left at a steady 32dp a second until its end shows, rests there,
 * then slides back, and so on. An edge fades where the text runs past it:
 * at the start only the right, at the end only the left, and both in
 * between. Text that fits stays put; new text starts over at the start.
 *
 * The slide moves a layer in the draw phase only, so it costs no
 * recomposition.
 */
@Composable
fun Marquee(text: String, style: TextStyle, modifier: Modifier = Modifier) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    BoxWithConstraints(modifier.clipToBounds()) {
        val room = constraints.maxWidth.toFloat()
        val full = remember(text, style) {
            measurer.measure(text, style, maxLines = 1, softWrap = false).size.width.toFloat()
        }
        val travel = (full - room).coerceAtLeast(0f)
        val long = travel > 0.5f
        val fade = with(density) { 28.dp.toPx() }
        val x = remember(text) { Animatable(0f) }
        LaunchedEffect(text, travel) {
            x.snapTo(0f)
            if (!long) return@LaunchedEffect
            // A steady 32dp a second, however long the text. It sets off
            // soon after the song starts, then rests 2.5 s at each end.
            val glide = (travel / density.density / 32f * 1000f).toInt().coerceAtLeast(1)
            delay(1500)
            while (true) {
                x.animateTo(-travel, tween(glide, easing = EaseInOut))
                delay(2500)
                x.animateTo(0f, tween(glide, easing = EaseInOut))
                delay(2500)
            }
        }
        Text(
            text,
            style = style,
            maxLines = 1,
            softWrap = false,
            modifier = Modifier
                .fillMaxWidth()
                // Its own layer, so the fade can cut into the text alone.
                .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
                .drawWithContent {
                    drawContent()
                    if (long) {
                        // How much of each fade shows: none with the text's
                        // own end at that edge, all of it once a fade's width
                        // of text is past it.
                        val left = (-x.value / fade).coerceIn(0f, 1f)
                        val right = ((full + x.value - room) / fade).coerceIn(0f, 1f)
                        val band = (fade / size.width).coerceAtMost(0.5f)
                        drawRect(
                            Brush.horizontalGradient(
                                0f to Color.Black.copy(alpha = 1f - left),
                                band to Color.Black,
                                1f - band to Color.Black,
                                1f to Color.Black.copy(alpha = 1f - right),
                            ),
                            blendMode = BlendMode.DstIn,
                        )
                    }
                }
                .wrapContentWidth(Alignment.Start, unbounded = true)
                .graphicsLayer { translationX = x.value },
        )
    }
}
