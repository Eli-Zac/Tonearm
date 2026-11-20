package app.tonearm

import androidx.compose.animation.core.AnimationSpec
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
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
                        // A drag during a slide takes over from it.
                        slide?.cancel()
                        leaving = null
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

