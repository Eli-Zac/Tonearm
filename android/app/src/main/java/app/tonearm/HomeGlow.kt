package app.tonearm

import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color

/**
 * Home's soft glows of colour at the top of the page, behind the shelves,
 * scrolling away with them, as the desktop's Home has them: a pink one in
 * the accent to the left, three quarters of the page wide, and a violet one
 * to the right, nearly the page's width. Each fades out well inside its
 * square, so no edge shows. Drawn behind the page's top, from its top edge.
 */
fun Modifier.homeGlow(top: Float = 0f) = drawBehind {
    val w = size.width
    // A square of `side` at (x, y), with a circle fading out at half the
    // distance to its corners, as Slint's `radial-gradient(circle, … 50%)`.
    fun glow(color: Color, alpha: Float, side: Float, x: Float, y: Float) {
        val centre = Offset(x + side / 2, y + side / 2)
        val reach = side * 0.7071f
        drawCircle(
            Brush.radialGradient(
                0f to color.copy(alpha = alpha),
                0.5f to color.copy(alpha = 0f),
                center = centre,
                radius = reach,
            ),
            radius = reach * 0.5f,
            center = centre,
        )
    }
    val pink = w * 0.75f
    val violet = w * 0.95f
    glow(Tonearm.accent, 0.3f, pink, -pink * 0.3f, top - pink * 0.5f)
    glow(Color(0xFF7A5CFF), 0.26f, violet, w - violet * 0.65f, top - violet * 0.55f)
}
