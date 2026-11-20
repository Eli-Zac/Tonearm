package app.tonearm

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
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
    val offset = remember { Animatable(0f) }
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
        val dir = when {
            pending != 0 -> pending
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
                offset.snapTo(0f)
            } else {
                offset.snapTo(offset.value + dir * span)
                offset.animateTo(0f, tween(420, easing = FastOutSlowInEasing))
            }
        } finally {
            // Also when a drag cuts the slide short: the old cover must go,
            // or it would stay beside the new one and block every swipe.
            leaving = null
        }
    }
    // The same song's cover can arrive a moment after the song does.
    val live = if (song != null && song.id == current?.id) song else current

    fun settle() {
        scope.launch { offset.animateTo(0f, spring(dampingRatio = 0.8f, stiffness = Spring.StiffnessMediumLow)) }
    }

    val velocity = remember { VelocityTracker() }
    // The finger's position, kept apart from the animation so quick moves
    // add up even before each lands.
    var dragX by remember { mutableFloatStateOf(0f) }
    Box(
        modifier
            .onSizeChanged { width = it.width }
            .pointerInput(core, controller) {
                detectHorizontalDragGestures(
                    onDragStart = {
                        velocity.resetTracking()
                        dragX = offset.value
                        // A drag during a slide takes over from it.
                        leaving = null
                        if (pending == 0) {
                            val queue = runCatching { core.queue() }.getOrDefault(emptyList())
                            val i = placeOf(core, current?.id, queue)
                            currentIndex = i
                            before = if (i > 0) queue.getOrNull(i - 1)?.let { CoverSong(it.videoId, it.cover) } else null
                            after = if (i >= 0) queue.getOrNull(i + 1)?.let { CoverSong(it.videoId, it.cover) } else null
                        }
                    },
                    onDragEnd = {
                        val speed = velocity.calculateVelocity().x
                        val far = width * 0.3f
                        val x = offset.value
                        when {
                            pending != 0 -> settle()
                            after != null && (x < -far || (speed < -900f && x < 0f)) -> {
                                pending = 1
                                controller.seekToNext()
                                scope.launch { offset.animateTo(-(width + gap), tween(260, easing = FastOutSlowInEasing)) }
                            }
                            before != null && (x > far || (speed > 900f && x > 0f)) -> {
                                pending = -1
                                val target = currentIndex - 1
                                // The song before, even well into this one,
                                // where the previous button would restart it.
                                scope.launch(Dispatchers.IO) { runCatching { core.skipTo(target.toUInt()) } }
                                scope.launch { offset.animateTo(width + gap, tween(260, easing = FastOutSlowInEasing)) }
                            }
                            else -> settle()
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
                    val x = dragX
                    // Against the end of the queue it gives, but only a little.
                    val stiff = (x + amount > 0 && before == null) || (x + amount < 0 && after == null)
                    dragX = x + if (stiff) amount * 0.25f else amount
                    val to = dragX
                    scope.launch { offset.snapTo(to) }
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
                    Modifier.matchParentSize().graphicsLayer { translationX = offset.value + place * span },
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

