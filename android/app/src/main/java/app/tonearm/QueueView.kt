package app.tonearm

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.shape.CircleShape
import androidx.media3.session.MediaController
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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
 * The queue as a full-screen sheet over Now playing, as Spotify's phone
 * queue: it rises from the queue button and goes with a swipe down or the
 * X. One list holds the Party, the song playing and what plays next, so
 * the Party scrolls away and the songs get the screen. A slim player along
 * the bottom keeps play and pause at hand; tapping it goes back.
 */
@Composable
fun QueueSheet(
    core: Core,
    party: PartyUi,
    now: NowPlaying,
    controller: MediaController,
    onMenu: (Song) -> Unit,
    onClose: () -> Unit,
) {
    // Now playing's own flowing backdrop under the queue, the same as on
    // Now playing, so the sheet reads as part of it. The cover is the
    // core's, as Now playing picks it.
    val playing = remember(now.id, now.queueVersion) { core.nowPlaying()?.takeIf { it.videoId == now.id } }
    val cover = playing?.cover?.ifEmpty { null } ?: now.meta?.artworkUri?.toString()
    Box(Modifier.fillMaxSize().background(Tonearm.bg)) {
        FlowingBackdrop(cover, Modifier.fillMaxSize())
        QueueSheetBody(core, party, now, controller, onMenu, onClose)
    }
}

@Composable
private fun QueueSheetBody(
    core: Core,
    party: PartyUi,
    now: NowPlaying,
    controller: MediaController,
    onMenu: (Song) -> Unit,
    onClose: () -> Unit,
) {
    Column(Modifier.fillMaxSize().systemBarsPadding()) {
        Box(
            Modifier.align(Alignment.CenterHorizontally).padding(top = 8.dp).size(width = 36.dp, height = 4.dp)
                .clip(RoundedCornerShape(2.dp)).background(Tonearm.line),
        )
        Row(Modifier.fillMaxWidth().height(52.dp).padding(start = 20.dp, end = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            Title("Queue", size = 22.sp, weight = FontWeight.Bold, modifier = Modifier.weight(1f))
            Box(Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onClose), contentAlignment = Alignment.Center) {
                CloseMark()
            }
        }
        QueuePane(core, party, onMenu, Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp))
        PlayerStrip(now, controller, onClose)
    }
}

/** The song playing, slim, along the sheet's foot: its cover, name and
 *  play or pause, with the progress line under, as the mini-player. */
@Composable
private fun PlayerStrip(now: NowPlaying, controller: MediaController, onOpen: () -> Unit) {
    val progress = if (now.duration > 0) (now.position.toFloat() / now.duration).coerceIn(0f, 1f) else 0f
    Column(
        Modifier.padding(horizontal = 8.dp, vertical = 6.dp).fillMaxWidth()
            .clip(RoundedCornerShape(Tonearm.radius)).background(Tonearm.island).clickable(onClick = onOpen),
    ) {
        Row(Modifier.padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Art(now.meta?.artworkUri?.toString(), 44.dp)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Title(now.meta?.title?.toString() ?: "", size = 15.sp)
                PlayerLine(now)
            }
            IconButton(if (now.playing) Icons.pause else Icons.play, size = 26) {
                if (now.playing) controller.pause() else controller.play()
            }
        }
        Box(Modifier.fillMaxWidth().height(2.dp).background(Tonearm.line)) {
            Box(Modifier.fillMaxWidth(progress).fillMaxHeight().background(Tonearm.text))
        }
    }
}

/** Items in the list before the first song up next: the Party, "Now
 *  playing", the song playing and "Next up". */
private const val LEAD = 4

/**
 * The Party, the song playing, then the songs up next, all in one list.
 * Tap a song to play it, long press it for its menu. Drag one by its
 * handle to a new place: a raised copy follows the finger, the rows it
 * passes step aside to open the gap it will drop into, and near the top or
 * bottom the list scrolls on.
 */
@Composable
private fun QueuePane(core: Core, party: PartyUi, onMenu: (Song) -> Unit, modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()
    // Read at once, so the sheet opens on the queue as it is rather than
    // on "Play something" for a moment. Both are only locks in the core.
    var songs by remember { mutableStateOf(core.queue()) }
    var index by remember { mutableIntStateOf(core.playback().index.toInt()) }
    // Who added each song in a Party, by video id. Only songs a member
    // added are here, never ones the radio or autoplay put in.
    var adders by remember {
        mutableStateOf(songs.mapNotNull { s -> core.partyAdder(s.videoId)?.let { s.videoId to it } }.toMap())
    }
    remember { party.refresh(core) }
    LaunchedEffect(core) { party.follow(core) }
    // The row picked up, among those up next; -1 when none.
    var from by remember { mutableIntStateOf(-1) }
    // A guest whose host only lets others add songs: the list is to look at.
    var locked by remember { mutableStateOf(core.party()?.let { !it.canControl } ?: false) }
    // Where the finger is down the list's view, and where on the row it
    // took hold.
    var fingerY by remember { mutableFloatStateOf(0f) }
    var grab by remember { mutableFloatStateOf(0f) }
    var viewHeight by remember { mutableFloatStateOf(0f) }
    val list = rememberLazyListState()

    // The queue as the core has it, read again whenever it changes. Not
    // while a row is being dragged, which would pull the list from under it.
    LaunchedEffect(core) {
        // Read once more off the main thread, in case it moved meanwhile.
        var seen: ULong? = null
        var seenParty: ULong? = null
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
                // Adders follow both the queue and the Party.
                val party = withContext(Dispatchers.IO) { core.partyVersion() }
                if (read.third != null || party != seenParty) {
                    seenParty = party
                    locked = withContext(Dispatchers.IO) { core.party()?.let { !it.canControl } ?: false }
                    val list = songs
                    adders = withContext(Dispatchers.IO) {
                        list.mapNotNull { s -> core.partyAdder(s.videoId)?.let { s.videoId to it } }.toMap()
                    }
                }
            }
            delay(400)
        }
    }

    if (index !in songs.indices) {
        Column(modifier) {
            PartyHeader(core, party)
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { Dim("Play something to start a queue", 15.sp) }
        }
        return
    }
    val upNext = songs.subList(index + 1, songs.size)
    val density = LocalDensity.current
    val rowPx = with(density) { ROW.toPx() }
    val handlePx = with(density) { 56.dp.toPx() }
    val edgePx = with(density) { 72.dp.toPx() }
    // Where the first song up next would sit in the view, scrolled or not;
    // kept from the last time one of them was in sight.
    // Not state: it is worked out while composing, from the list's layout.
    val rowsTop = remember { floatArrayOf(0f) }

    // Where the dragged row would drop: the slot nearest the raised copy.
    // The Party above the songs is any height, so the rows are measured
    // from one of them in sight rather than from the top of the list.
    fun target(): Int {
        list.layoutInfo.visibleItemsInfo.firstOrNull { it.index >= LEAD }?.let {
            rowsTop[0] = it.offset - (it.index - LEAD) * rowPx
        }
        return ((fingerY - grab - rowsTop[0]) / rowPx).roundToInt().coerceIn(0, upNext.size - 1)
    }
    val to = if (from < 0) -1 else target()

    // Near the list's top or bottom edge, the list scrolls under the row
    // being dragged, faster the closer it gets.
    LaunchedEffect(from >= 0) {
        if (from < 0) return@LaunchedEffect
        while (true) {
            withFrameNanos { }
            val speed = when {
                fingerY < edgePx -> -(edgePx - fingerY) / edgePx * 22f
                fingerY > viewHeight - edgePx -> (fingerY - (viewHeight - edgePx)) / edgePx * 22f
                else -> 0f
            }
            if (speed != 0f) list.scrollBy(speed)
        }
    }

    Box(modifier) {
            LazyColumn(
                Modifier
                    .fillMaxSize()
                    .onSizeChanged { viewHeight = it.height.toFloat() }
                    // A press on a row's handle picks the row up. Caught here,
                    // ahead of the list, so the list doesn't scroll instead,
                    // and so the drag lives on when the row scrolls away.
                    .pointerInput(index, upNext.size, locked) {
                        if (locked) return@pointerInput
                        awaitEachGesture {
                            val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                            if (down.position.x < size.width - handlePx) return@awaitEachGesture
                            // Only a song up next has a handle; the Party's
                            // buttons and the song playing are left alone.
                            val row = list.layoutInfo.visibleItemsInfo.firstOrNull {
                                down.position.y >= it.offset && down.position.y < it.offset + it.size
                            }?.takeIf { it.index - LEAD in upNext.indices } ?: return@awaitEachGesture
                            down.consume()
                            grab = down.position.y - row.offset
                            fingerY = down.position.y
                            from = row.index - LEAD
                            try {
                                while (true) {
                                    val event = awaitPointerEvent(PointerEventPass.Initial)
                                    val change = event.changes.firstOrNull { it.id == down.id } ?: break
                                    change.consume()
                                    if (!change.pressed) break
                                    fingerY = change.position.y.coerceIn(0f, viewHeight)
                                }
                                val a = from
                                val b = target()
                                if (a >= 0 && a != b) {
                                    // The new order shows at once; the core
                                    // catches up off the main thread.
                                    val start = index + 1
                                    val moved = songs.toMutableList()
                                    moved.add(start + b, moved.removeAt(start + a))
                                    songs = moved
                                    scope.launch(Dispatchers.IO) { core.moveInQueue((start + a).toUInt(), (start + b).toUInt()) }
                                }
                            } finally {
                                from = -1
                            }
                        }
                    },
                state = list,
                userScrollEnabled = from < 0,
            ) {
                // LEAD counts these four.
                item(key = "party") { PartyHeader(core, party) }
                item(key = "now-label") { SectionLabel("Now playing") }
                item(key = "now") {
                    QueueRow(songs[index], current = true, adder = adders[songs[index].videoId], onLongClick = { onMenu(songs[index]) })
                }
                item(key = "next-label") {
                    // Kept, empty, with nothing up next, so LEAD holds.
                    if (upNext.isNotEmpty()) {
                        Box(Modifier.padding(top = 12.dp)) { SectionLabel("Next up") }
                    }
                }
                itemsIndexed(upNext) { i, song ->
                    val lifted = i == from
                    // Rows between the dragged one and where it is going step
                    // up or down by a row. When the drag ends they snap back
                    // as the list takes its new order underneath.
                    val shift by animateFloatAsState(
                        when {
                            from < 0 || lifted -> 0f
                            to > from && i in (from + 1)..to -> -rowPx
                            to < from && i in to until from -> rowPx
                            else -> 0f
                        },
                        if (from < 0) snap() else tween(180, easing = slideEasing),
                    )
                    QueueRow(
                        song,
                        current = false,
                        handle = !locked,
                        adder = adders[song.videoId],
                        // The row picked up leaves a gap; its raised copy
                        // is drawn over the list.
                        modifier = Modifier.graphicsLayer {
                            // Off at once when the drag ends: the animated
                            // value lags a frame behind, which drew the
                            // rows a row out of place over the new order.
                            translationY = if (from < 0) 0f else shift
                            alpha = if (lifted) 0f else 1f
                        },
                        onClick = if (locked) null else {
                            { scope.launch(Dispatchers.IO) { runCatching { core.skipTo((index + 1 + i).toUInt()) } } }
                        },
                        onLongClick = { onMenu(song) },
                    )
                }
            }
            // The raised copy, riding on the finger, kept within the view.
            upNext.getOrNull(from)?.let { song ->
                QueueRow(
                    song,
                    current = false,
                    lifted = true,
                    handle = true,
                    adder = adders[song.videoId],
                    modifier = Modifier.offset {
                        IntOffset(0, (fingerY - grab).coerceIn(0f, (viewHeight - rowPx).coerceAtLeast(0f)).roundToInt())
                    },
                )
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
    handle: Boolean = false,
    /** Who in the Party added it; null for the radio's and the phone's own. */
    adder: String? = null,
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    onLongClick: (() -> Unit)? = null,
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
            .then(
                if (onClick != null || onLongClick != null) {
                    Modifier.combinedClickable(onClick = { onClick?.invoke() }, onLongClick = onLongClick)
                } else Modifier,
            )
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
        // Who added it, as Spotify's Jam marks a guest's song.
        if (adder != null) {
            Box(Modifier.padding(end = if (handle) 0.dp else 8.dp)) { PartyAvatar(adder, 22.dp) }
        }
        if (handle) {
            // Six dots: hold and drag to move the song.
            Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
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
