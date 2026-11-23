package app.tonearm

import androidx.compose.animation.core.AnimationSpec
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.Stroke
import kotlin.math.PI
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.media3.session.MediaController
import coil3.compose.AsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Tonearm as Core

/** A song's cover, as the carousel holds it. */
data class CoverSong(val id: String, val cover: String?)

/**
 * Now Playing's cover. When the song changes it slides: the new cover in
 * from the right and the old out to the left going forward, the other way
 * going back. It can be dragged sideways too, with the next or previous
 * song's cover coming in beside it; let go far or fast enough and that
 * song plays, the slide carrying on from where the finger left it.
 *
 * Positions are read only while drawing, so a slide or a drag redraws
 * without recomposing. Each cover is keyed by its song, so the one that
 * slides in keeps its image (and moving cover) as it becomes the current.
 */
@Composable
fun CoverCarousel(
    song: CoverSong?,
    index: Int,
    core: Core,
    controller: MediaController,
    modifier: Modifier = Modifier,
) {
    val scope = rememberCoroutineScope()
    val gap = with(LocalDensity.current) { 32.dp.toPx() }
    var width by remember { mutableIntStateOf(0) }
    // How far one cover sits from the next: off the screen's edge.
    val span = width + gap
    // Where the current cover is, sideways. The others sit a span away.
    // A plain value the finger writes straight into, so the cover keeps up
    // with it frame for frame; slides animate it through `glide`.
    var x by remember { mutableFloatStateOf(0f) }
    // Its speed as the last slide left it, so a swipe's momentum carries on
    // into the next song's slide.
    var speed by remember { mutableFloatStateOf(0f) }
    var slide by remember { mutableStateOf<Job?>(null) }
    fun glide(to: Float, from: Float, spec: AnimationSpec<Float>): Job {
        slide?.cancel()
        return scope.launch {
            animate(x, to, from, spec) { value, v -> x = value; speed = v }
            speed = 0f
        }.also { slide = it }
    }
    // A swipe's slide: a spring, so the finger's speed carries through.
    val swipeSpring = spring<Float>(dampingRatio = 1f, stiffness = 700f)
    var current by remember { mutableStateOf(song) }
    var currentIndex by remember { mutableIntStateOf(index) }
    var leaving by remember { mutableStateOf<CoverSong?>(null) }
    var leavingDir by remember { mutableIntStateOf(1) }
    // While dragging: the songs either side, from the queue.
    var before by remember { mutableStateOf<CoverSong?>(null) }
    var after by remember { mutableStateOf<CoverSong?>(null) }
    // A swipe's direction, waiting for its song to come on.
    var pending by remember { mutableIntStateOf(0) }
    var giveUp by remember { mutableStateOf<Job?>(null) }
    // A mix under way: the song going out, how long the mix runs and how
    // far in it is, in milliseconds. Elapsed is read only while drawing.
    var mixFrom by remember { mutableStateOf<CoverSong?>(null) }
    var mixLength by remember { mutableFloatStateOf(0f) }
    var mixElapsed by remember { mutableFloatStateOf(0f) }

    // A new song: the old cover goes, the new one comes in a span beyond
    // it, from wherever the old one was.
    LaunchedEffect(song?.id) {
        val old = current
        if (song?.id == old?.id) return@LaunchedEffect
        // Where the song is in the queue, read from the core now: the index
        // passed in can lag the song by a moment.
        val now = placeOf(core, song?.id)
        val swiped = pending != 0
        val dir = when {
            swiped -> pending
            now in 0 until currentIndex -> -1
            else -> 1
        }
        pending = 0
        giveUp?.cancel()
        // Moved on by a mix long enough to show it: the pair animation, as
        // the desktop's, instead of the slide.
        val mix = if (old == null || swiped || width == 0) null
        else withContext(Dispatchers.IO) { runCatching { core.mixShown() }.getOrNull() }
            ?.takeIf { it.lengthMs >= 2000uL }
        if (mix != null && old != null) {
            slide?.cancel()
            x = 0f
            current = song
            currentIndex = now
            before = null
            after = null
            mixFrom = old
            mixLength = mix.lengthMs.toFloat()
            try {
                val start = withFrameMillis { it } - mix.elapsedMs.toLong()
                while (mixFrom != null) {
                    val t = (withFrameMillis { it } - start).toFloat()
                    mixElapsed = t
                    if (t >= mixLength) break
                }
            } finally {
                mixFrom = null
            }
            return@LaunchedEffect
        }
        leaving = old
        leavingDir = dir
        current = song
        currentIndex = now
        before = null
        after = null
        try {
            if (old == null || width == 0) {
                slide?.cancel()
                x = 0f
            } else {
                val v = speed
                slide?.cancel()
                x += dir * span
                // A swipe carries on at the finger's speed; a skip from the
                // buttons slides in from rest.
                (if (swiped) glide(0f, v, swipeSpring)
                else glide(0f, 0f, tween(420, easing = FastOutSlowInEasing))).join()
            }
        } finally {
            // Also when a drag cuts the slide short: the old cover must go,
            // or it would stay beside the new one and block every swipe.
            leaving = null
        }
    }
    // The same song's cover can arrive a moment after the song does.
    val live = if (song != null && song.id == current?.id) song else current

    // The songs either side, looked up off the main thread once each song
    // has settled in, so their covers are loaded (off the screen's edges)
    // before a swipe ever starts. Looked up again as the queue moves on.
    LaunchedEffect(current?.id, index, leaving == null, pending) {
        if (leaving != null || pending != 0) return@LaunchedEffect
        val id = current?.id ?: return@LaunchedEffect
        val found = withContext(Dispatchers.IO) {
            runCatching {
                val queue = core.queue()
                val i = placeOf(core, id, queue)
                Triple(
                    i,
                    if (i > 0) queue.getOrNull(i - 1)?.let { CoverSong(it.videoId, it.cover) } else null,
                    if (i >= 0) queue.getOrNull(i + 1)?.let { CoverSong(it.videoId, it.cover) } else null,
                )
            }.getOrNull()
        } ?: return@LaunchedEffect
        if (current?.id != id || pending != 0) return@LaunchedEffect
        currentIndex = found.first
        before = found.second
        after = found.third
    }

    fun settle(from: Float = 0f) {
        glide(0f, from, spring(dampingRatio = 0.85f, stiffness = Spring.StiffnessMediumLow))
    }

    val velocity = remember { VelocityTracker() }
    Box(
        modifier
            .onSizeChanged { width = it.width }
            .pointerInput(core, controller) {
                detectHorizontalDragGestures(
                    onDragStart = {
                        velocity.resetTracking()
                        // A drag during a slide takes over from it, and
                        // ends a mix's animation.
                        slide?.cancel()
                        leaving = null
                        mixFrom = null
                    },
                    onDragEnd = {
                        val fling = velocity.calculateVelocity().x
                        val far = width * 0.3f
                        when {
                            pending != 0 -> settle(fling)
                            after != null && (x < -far || (fling < -900f && x < 0f)) -> {
                                pending = 1
                                val target = currentIndex + 1
                                scope.launch(Dispatchers.IO) { runCatching { core.skipTo(target.toUInt()) } }
                                glide(-span, fling, swipeSpring)
                            }
                            before != null && (x > far || (fling > 900f && x > 0f)) -> {
                                pending = -1
                                val target = currentIndex - 1
                                // The song before, even well into this one,
                                // where the previous button would restart it.
                                scope.launch(Dispatchers.IO) { runCatching { core.skipTo(target.toUInt()) } }
                                glide(span, fling, swipeSpring)
                            }
                            else -> settle(fling)
                        }
                        if (pending != 0) {
                            // Should the song not change, the cover comes back.
                            giveUp = scope.launch {
                                delay(4000)
                                if (pending != 0) {
                                    pending = 0
                                    settle()
                                }
                            }
                        }
                    },
                    onDragCancel = { settle() },
                ) { change, amount ->
                    change.consume()
                    velocity.addPosition(change.uptimeMillis, change.position)
                    // Against the end of the queue it gives, but only a little.
                    val stiff = (x + amount > 0 && before == null) || (x + amount < 0 && after == null)
                    x += if (stiff) amount * 0.25f else amount
                }
            },
    ) {
        val from = mixFrom
        if (from != null && live != null) {
            MixPair(from, live, core, { mixElapsed }, mixLength, Modifier.matchParentSize())
            return@Box
        }
        // The covers on show, each where it sits; one per song.
        val shown = buildList {
            live?.let { add(Triple(it, 0, true)) }
            leaving?.let { add(Triple(it, -leavingDir, true)) }
            before?.let { add(Triple(it, -1, false)) }
            after?.let { add(Triple(it, 1, false)) }
        }.distinctBy { it.first.id }
        shown.forEach { (s, place, moving) ->
            key(s.id) {
                CoverFace(
                    s, core, moving,
                    Modifier.matchParentSize().graphicsLayer { translationX = x + place * span },
                )
            }
        }
    }
}

/** An ease from 0 to 1, as the desktop's: it starts from rest and settles
 *  to rest, with no overshoot (smootherstep). */
private fun spring01(x: Float): Float = when {
    x <= 0f -> 0f
    x >= 1f -> 1f
    else -> x * x * x * (x * (x * 6f - 15f) + 10f)
}

/**
 * The mix animation, as the desktop's and Spotify's mid-mix Now Playing:
 * the cover going out shrinks to the left while the new one comes in from
 * the right, the two side by side through the mix with rays of light
 * sweeping beneath them and light running round their borders, then the old one is
 * pushed off and the new one grows to fill again. `elapsed` is read only
 * while drawing, so the animation never recomposes.
 */
@Composable
private fun MixPair(
    from: CoverSong,
    to: CoverSong,
    core: Core,
    elapsed: () -> Float,
    length: Float,
    modifier: Modifier,
) {
    val lag = 140f
    val step = minOf(1800f, (length - 2 * lag) / 2.4f)
    val leaveAt = length - step - lag
    val pad = with(LocalDensity.current) { 12.dp.toPx() }
    // Where everything is at `t`, for a box `w` by `h`.
    class Frame(t: Float, w: Float, h: Float) {
        val oe = spring01(t / step)
        val ie = spring01((t - lag) / step)
        val ol = spring01((t - leaveAt) / step)
        val il = spring01((t - leaveAt - lag) / step)
        val hold = min(oe, ie).coerceIn(0f, 1f) * (1f - max(ol, il).coerceIn(0f, 1f))
        val m = min(w, h)
        val c = m * 0.45f
        val g = m * 0.05f
        val cy = (h - c) / 2
        val lx = w / 2 - c - g / 2
        val rx = w / 2 + g / 2
        val secs = t / 1000f
    }
    Box(modifier) {
        // Rays of light falling from beneath the covers, as stage lights:
        // two from each, one leaning out and one more upright, each
        // sweeping slowly from side to side at its own pace and shimmering.
        // Two soft layers each, the inner narrower, so the edges blur.
        Canvas(Modifier.matchParentSize()) {
            val f = Frame(elapsed(), size.width, size.height)
            if (f.hold <= 0.01f) return@Canvas
            val turn = 2 * PI.toFloat()
            val top = f.cy + f.c - 2f
            for (side in listOf(-1, 1)) {
                val x0 = if (side < 0) f.lx else f.rx
                for (r in 0..1) {
                    val tc = x0 + f.c * (0.5f + side * (if (r == 0) 0.2f else -0.18f))
                    val phase = r * 0.37f + (if (side > 0) 0.5f else 0f)
                    val lean = side * f.c * (if (r == 0) 0.55f else 0.05f) +
                        f.c * 0.35f * sin((f.secs / (if (r == 0) 4.2f else 5.6f) + phase) * turn)
                    val shimmer = 0.75f + 0.25f * sin((f.secs * 0.9f + r * 0.4f + side * 0.2f) * turn)
                    for (layer in 0..1) {
                        val th = f.c * (0.16f - 0.06f * layer)
                        val len = f.c * (1.9f - 0.2f * layer)
                        val bh = f.c * (0.42f - 0.12f * layer)
                        val path = Path().apply {
                            moveTo(tc - th, top)
                            lineTo(tc + th, top)
                            lineTo(tc + lean + bh, top + len)
                            lineTo(tc + lean - bh, top + len)
                            close()
                        }
                        drawPath(
                            path,
                            Brush.verticalGradient(
                                0f to Color.White.copy(alpha = 0.1f),
                                0.4f to Color.White.copy(alpha = 0.04f),
                                1f to Color.Transparent,
                                startY = top, endY = top + len,
                            ),
                            alpha = f.hold * shimmer,
                        )
                    }
                }
            }
        }
        // Going out: full size to the left slot, then off.
        key(from.id) {
            CoverFace(
                from, core, false,
                Modifier.matchParentSize().graphicsLayer {
                    val f = Frame(elapsed(), size.width, size.height)
                    transformOrigin = TransformOrigin(0f, 0f)
                    translationX = f.lx * f.oe + (-f.c - pad - f.lx) * f.ol
                    translationY = f.cy * min(1f, f.oe)
                    scaleX = (size.width + (f.c - size.width) * f.oe) / size.width
                    scaleY = (size.height + (f.c - size.height) * f.oe) / size.height
                    alpha = 1f - f.ol.coerceIn(0f, 1f)
                    shadowElevation = 16f * f.hold
                    shape = RoundedCornerShape(Tonearm.radius)
                    clip = true
                },
            )
        }
        // Coming in: from the right to the right slot, then to the middle
        // and full size.
        key(to.id) {
            CoverFace(
                to, core, false,
                Modifier.matchParentSize().graphicsLayer {
                    val f = Frame(elapsed(), size.width, size.height)
                    val il = min(1f, f.il)
                    transformOrigin = TransformOrigin(0f, 0f)
                    translationX = (size.width + pad) + (f.rx - size.width - pad) * f.ie - f.rx * f.il
                    translationY = f.cy - f.cy * il
                    scaleX = (f.c + (size.width - f.c) * il) / size.width
                    scaleY = (f.c + (size.height - f.c) * il) / size.height
                    shadowElevation = 16f * f.hold
                    shape = RoundedCornerShape(Tonearm.radius)
                    clip = true
                },
            )
        }
        // A thin rim of light on each, with light running round it.
        Canvas(Modifier.matchParentSize()) {
            val f = Frame(elapsed(), size.width, size.height)
            if (f.hold <= 0.01f) return@Canvas
            val rim = Color.White.copy(alpha = 0.14f * f.hold)
            val stroke = Stroke(1.dp.toPx())
            val radius = Tonearm.radius.toPx()
            // Where each cover is drawn and how big, as the layers above
            // have them, so the outline stays on its edges while the covers
            // scale into and out of place. A cover's corners scale with it.
            val oy = f.cy * min(1f, f.oe)
            val ox = f.lx * f.oe + (-f.c - pad - f.lx) * f.ol
            val ow = size.width + (f.c - size.width) * f.oe
            val oh = size.height + (f.c - size.height) * f.oe
            val il = min(1f, f.il)
            val ix = (size.width + pad) + (f.rx - size.width - pad) * f.ie - f.rx * f.il
            val iy = f.cy - f.cy * il
            val iw = f.c + (size.width - f.c) * il
            val ih = f.c + (size.height - f.c) * il
            val covers = listOf(
                listOf(ox, oy, ow, oh),
                listOf(ix, iy, iw, ih),
            )
            for ((x, y, w, h) in covers) {
                drawRoundRect(
                    rim, Offset(x, y), Size(w, h),
                    CornerRadius(radius * w / size.width, radius * h / size.height), stroke,
                )
            }
            // Light running round each cover's border, a lap every 2.6 s,
            // half a lap apart: a bright head with a glow, and a tail of
            // light fading behind it.
            fun around(x0: Float, y0: Float, w: Float, h: Float, u: Float): Offset {
                val d = (((u % 1f) + 1f) % 1f) * 4f
                val s = d.toInt()
                val k = d - s
                return when (s) {
                    0 -> Offset(x0 + w * k, y0)
                    1 -> Offset(x0 + w, y0 + h * k)
                    2 -> Offset(x0 + w * (1 - k), y0 + h)
                    else -> Offset(x0, y0 + h * (1 - k))
                }
            }
            val width = 2.5.dp.toPx()
            val glow = f.c * 0.08f
            for ((n, rect) in covers.withIndex()) {
                val (x, y, w, h) = rect
                val head = f.secs / 2.6f + n * 0.5f
                val steps = 40
                for (i in 0 until steps) {
                    val fade = 1f - i / steps.toFloat()
                    drawLine(
                        Color.White.copy(alpha = 0.9f * fade * fade * f.hold),
                        around(x, y, w, h, head - i * 0.003f),
                        around(x, y, w, h, head - (i + 1) * 0.003f),
                        strokeWidth = width * (0.5f + 0.5f * fade),
                        cap = StrokeCap.Butt,
                    )
                }
                val p = around(x, y, w, h, head)
                drawCircle(
                    Brush.radialGradient(
                        0f to Color.White.copy(alpha = 0.8f),
                        0.4f to Color.White.copy(alpha = 0.25f),
                        1f to Color.Transparent,
                        center = p, radius = glow,
                    ),
                    radius = glow, center = p, alpha = f.hold,
                )
            }
        }
    }
}

/** Where the song `id` is in the queue: the core's own place when that is
 *  the song, else its first place. -1 when it isn't queued. */
private fun placeOf(core: Core, id: String?, queue: List<uniffi.tonearm_android.Song>? = null): Int {
    if (id == null) return -1
    val songs = queue ?: runCatching { core.queue() }.getOrDefault(emptyList())
    val at = runCatching { core.playback().index.toInt() }.getOrDefault(-1)
    return if (songs.getOrNull(at)?.videoId == id) at else songs.indexOfFirst { it.videoId == id }
}

/** One cover: the still image, with the album's moving cover over it once
 *  found, for the song playing and the one sliding away. */
@Composable
private fun CoverFace(song: CoverSong, core: Core, moving: Boolean, modifier: Modifier) {
    Box(modifier.clip(RoundedCornerShape(Tonearm.radius)).background(Tonearm.raised)) {
        AsyncImage(
            model = song.cover,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize(),
        )
        // Settings > Customization can keep the still cover.
        if (moving) {
            val clip by produceState<String?>(null, song.id, Prefs.version) {
                value = withContext(Dispatchers.IO) {
                    if (!core.animatedCovers()) null
                    else runCatching { core.animatedCover(song.id) }.getOrNull()
                }
            }
            clip?.let { MotionCover(it, Modifier.fillMaxSize()) }
        }
    }
}

