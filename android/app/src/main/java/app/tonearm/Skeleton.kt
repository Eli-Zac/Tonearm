package app.tonearm

import android.os.SystemClock
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clipToBounds
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

/** What a page looks like while it loads, as the desktop's `SkeletonPage`
 *  styles and `SkeletonDetail`, at the phone's own sizes. */
enum class PageSkeleton {
    /** Home: a row of chips over shelves of cards. */
    Chips,
    /** Shelves of cards: Library and the other tabs. */
    Shelves,
    /** A list of rows: search results. */
    Rows,
    /** An album or playlist: its cover, title and buttons over rows. */
    Detail,
    /** An artist: the same with a round picture. */
    Artist,
}

/** The skeleton for a page, filling the room it is given. It doesn't
 *  scroll: it is gone as soon as the page is there. */
@Composable
fun PageSkeletonView(kind: PageSkeleton, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxSize().clipToBounds()) {
        when (kind) {
            PageSkeleton.Chips -> {
                ChipsSkeleton()
                repeat(3) { ShelfSkeleton(it * 3) }
            }
            PageSkeleton.Shelves -> repeat(3) { ShelfSkeleton(it * 3) }
            PageSkeleton.Rows -> {
                Bone(
                    Modifier.padding(start = 16.dp, top = 22.dp, bottom = 10.dp).width(130.dp).height(20.dp),
                    RoundedCornerShape(7.dp),
                )
                repeat(10) { RowSkeleton(it + 2) }
            }
            PageSkeleton.Detail, PageSkeleton.Artist -> {
                // As PageHeader: the cover centred under the status bar,
                // the title, the byline and the buttons, then the songs.
                Box(Modifier.fillMaxWidth().statusBarsPadding().padding(top = 48.dp), contentAlignment = Alignment.Center) {
                    Bone(
                        Modifier.size(240.dp),
                        if (kind == PageSkeleton.Artist) CircleShape else RoundedCornerShape(Tonearm.radius),
                    )
                }
                Bone(Modifier.padding(start = 16.dp, top = 18.dp).fillMaxWidth(0.62f).height(30.dp), RoundedCornerShape(8.dp))
                Bone(Modifier.padding(start = 16.dp, top = 12.dp).width(110.dp).height(14.dp))
                Bone(Modifier.padding(start = 16.dp, top = 8.dp).width(170.dp).height(12.dp))
                Row(Modifier.padding(horizontal = 16.dp, vertical = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Bone(Modifier.size(48.dp), CircleShape)
                    Bone(Modifier.width(110.dp).height(48.dp), RoundedCornerShape(24.dp))
                }
                repeat(8) { RowSkeleton(it + 2) }
            }
        }
    }
}

/** Home's chips, while there are none yet to show. */
@Composable
fun ChipsSkeleton() {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState(), enabled = false).padding(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        repeat(6) { i -> Bone(Modifier.width((78 + 16 * (i * 2 % 3)).dp).height(34.dp), RoundedCornerShape(17.dp)) }
    }
}

/** A shelf: its title and a row of cards running off the edge, as the
 *  desktop's `SkeletonShelf`, at the phone's card size. */
@Composable
private fun ShelfSkeleton(seed: Int) {
    Column {
        Bone(
            Modifier.padding(start = 16.dp, top = 24.dp, bottom = 12.dp).width((130 + seed * 37 % 4 * 24).dp).height(20.dp),
            RoundedCornerShape(7.dp),
        )
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState(), enabled = false).padding(horizontal = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            repeat(4) { i -> CardSkeleton(seed + i) }
        }
    }
}

/** A card, as ItemCard: the cover, its title and a line under it. */
@Composable
private fun CardSkeleton(seed: Int) {
    Column(Modifier.width(150.dp)) {
        Bone(Modifier.size(150.dp), RoundedCornerShape(8.dp))
        Bone(Modifier.padding(top = 10.dp).fillMaxWidth(0.8f - (seed * 5 % 4) * 0.08f).height(11.dp))
        Bone(Modifier.padding(top = 8.dp).fillMaxWidth(0.5f - (seed * 3 % 3) * 0.07f).height(9.dp))
    }
}

/** A row, as ItemRow: the cover and two lines, their lengths varied by
 *  `seed` so a stack doesn't look stamped out. */
@Composable
fun RowSkeleton(seed: Int) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 7.dp), verticalAlignment = Alignment.CenterVertically) {
        Bone(Modifier.size(50.dp), RoundedCornerShape(8.dp))
        Column(Modifier.weight(1f).padding(start = 14.dp), verticalArrangement = Arrangement.spacedBy(9.dp)) {
            Bone(Modifier.fillMaxWidth(0.62f - (seed * 7 % 5) * 0.055f).height(12.dp))
            Bone(Modifier.fillMaxWidth(0.4f - (seed * 3 % 4) * 0.04f).height(10.dp))
        }
    }
}

// The desktop's #ffffff0e block and #ffffff16 band.
private val BASE = Color(0x0EFFFFFF)
private val LIGHT = Color(0x16FFFFFF)
