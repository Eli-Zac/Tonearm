package app.tonearm

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.zIndex
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Song
import uniffi.tonearm_android.Tonearm as Core
import kotlin.math.roundToInt

// The desktop's queue panel (QueueRow and the queue in ui/app.slint): the
// song playing, then what plays next, which can be dragged into a new
// order. Keep the two alike.

private val ROW = 60.dp
/** How the rows passed by a dragged one slide aside, as on desktop. */
private val slideEasing = CubicBezierEasing(0.22f, 1f, 0.36f, 1f)

/**
 * The queue in place of the cover: the song playing, then the songs up
 * next. Tap one to play it; drag one by its handle to a new place, and the
 * rows it passes step aside to open the gap it will drop into.
 */
@Composable
fun QueuePane(core: Core, modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()
    var songs by remember { mutableStateOf<List<Song>>(emptyList()) }
    var index by remember { mutableIntStateOf(-1) }
    // The row picked up, among those up next, and how far it has moved.
    var from by remember { mutableIntStateOf(-1) }
    var offset by remember { mutableFloatStateOf(0f) }

    // The queue as the core has it, read again whenever it changes. Not
    // while a row is being dragged, which would pull the list from under it.
    LaunchedEffect(core) {
        var seen: ULong? = null
        while (true) {
            if (from < 0) {
                val read = withContext(Dispatchers.IO) {
                    val playback = core.playback()
                    val list = if (playback.version != seen) core.queue() else null
                    Triple(playback.version, playback.index.toInt(), list)
                }
                if (from < 0) {
                    read.third?.let { songs = it; seen = read.first }
                    index = read.second
                }
            }
            delay(400)
        }
    }

    if (index !in songs.indices) {
        Box(modifier, contentAlignment = Alignment.Center) { Dim("Play something to start a queue", 15.sp) }
        return
    }
    val upNext = songs.subList(index + 1, songs.size)
    val rowPx = with(LocalDensity.current) { ROW.toPx() }
    // Where the dragged row would drop, among those up next.
    val to = if (from < 0) -1 else (from + (offset / rowPx).roundToInt()).coerceIn(0, upNext.size - 1)

    Column(modifier) {
        SectionLabel("Now playing")
        QueueRow(songs[index], current = true)
        if (upNext.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            SectionLabel("Next up")
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), userScrollEnabled = from < 0) {
            itemsIndexed(upNext) { i, song ->
                val lifted = i == from
                // Rows between the dragged one and where it is going step
                // up or down by a row. When the drag ends they snap back as
                // the list takes its new order underneath, so nothing jumps.
                val target = when {
                    from < 0 || lifted -> 0f
                    to > from && i in (from + 1)..to -> -rowPx
                    to < from && i in to until from -> rowPx
                    else -> 0f
                }
                val shift by animateFloatAsState(
                    target,
                    if (from < 0) snap() else tween(180, easing = slideEasing),
                )
                QueueRow(
                    song,
                    current = false,
                    lifted = lifted,
                    modifier = Modifier
                        .zIndex(if (lifted) 1f else 0f)
                        .graphicsLayer { translationY = if (lifted) offset else shift },
                    onClick = {
                        scope.launch(Dispatchers.IO) { runCatching { core.skipTo((index + 1 + i).toUInt()) } }
                    },
                    handle = Modifier.pointerInput(i, upNext.size) {
                        detectDragGestures(
                            onDragStart = {
                                from = i
                                offset = 0f
                            },
                            onDragEnd = {
                                val a = from
                                val b = (a + (offset / rowPx).roundToInt()).coerceIn(0, upNext.size - 1)
                                if (a >= 0 && a != b) {
                                    // The new order shows at once; the core
                                    // catches up off the main thread.
                                    val moved = songs.toMutableList()
                                    moved.add(index + 1 + b, moved.removeAt(index + 1 + a))
                                    songs = moved
                                    val start = index + 1
                                    scope.launch(Dispatchers.IO) { core.moveInQueue((start + a).toUInt(), (start + b).toUInt()) }
                                }
                                from = -1
                                offset = 0f
                            },
                            onDragCancel = {
                                from = -1
                                offset = 0f
                            },
                        ) { change, amount ->
                            change.consume()
                            // Kept within the list's rows.
                            offset = (offset + amount.y).coerceIn(-i * rowPx, (upNext.size - 1 - i) * rowPx)
                        }
                    },
                )
            }
        }
    }
}

@Composable
private fun SectionLabel(text: String) {
    Text(
        text,
        style = TextStyle(fontFamily = Tonearm.font, fontWeight = FontWeight.Bold, fontSize = 16.sp, color = Tonearm.text),
        modifier = Modifier.padding(start = 4.dp, bottom = 4.dp).height(24.dp).wrapContentHeight(Alignment.CenterVertically),
    )
}

/** A song in the queue: its art, title and artist, and a handle to drag it
 *  by. The song playing has its title in the accent colour. The row being
 *  dragged is raised above the rest. */
@Composable
private fun QueueRow(
    song: Song,
    current: Boolean,
    lifted: Boolean = false,
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    handle: Modifier? = null,
) {
    val shape = RoundedCornerShape(8.dp)
    Row(
        modifier
            .fillMaxWidth()
            .height(ROW)
            .graphicsLayer {
                if (lifted) {
                    shadowElevation = 18.dp.toPx()
                    this.shape = shape
                    clip = true
                }
            }
            .clip(shape)
            .background(if (lifted) Tonearm.raised else Color.Transparent)
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(start = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Art(song.cover, 44.dp, RoundedCornerShape(6.dp))
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Title(song.title, size = 16.sp, weight = FontWeight.Medium, color = if (current) Tonearm.accent else Tonearm.text)
            if (song.artist.isNotEmpty()) {
                Spacer(Modifier.height(3.dp))
                Dim(song.artist, 14.sp)
            }
        }
        if (handle != null) {
            // Six dots: hold and drag to move the song.
            Box(handle.size(48.dp), contentAlignment = Alignment.Center) {
                Canvas(Modifier.size(width = 12.dp, height = 18.dp)) {
                    val r = 1.6.dp.toPx()
                    for (col in 0 until 2) for (row in 0 until 3) {
                        drawCircle(
                            Tonearm.dim,
                            radius = r,
                            center = Offset(r + col * (size.width - 2 * r), r + row * (size.height - 2 * r) / 2),
                        )
                    }
                }
            }
        }
    }
}
