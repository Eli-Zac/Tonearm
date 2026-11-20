package app.tonearm

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import coil3.compose.AsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.Tonearm as Core

/**
 * The song playing, full screen: its cover large over the same cover blown
 * up and blurred, as the desktop's player. The lyrics button swaps the
 * cover for the song's lyrics. Swipe down or press back to leave it.
 */
@Composable
fun NowPlayingScreen(
    now: NowPlaying,
    controller: MediaController,
    core: Core,
    panel: Panel,
    onPanel: (Panel) -> Unit,
    /** A song's menu, from a long press in the queue. */
    onMenu: (Item) -> Unit,
    /** Whether the song is liked; null while unknown or signed out. */
    liked: Boolean?,
    onLike: () -> Unit,
    onClose: () -> Unit,
) {
    // A panel's button shows it, and pressed again goes back to the cover.
    val toggle = { p: Panel -> onPanel(if (panel == p) Panel.Cover else p) }
    val cover = now.meta?.artworkUri?.toString()
    // Each song's lyrics are looked for as it comes on, so they are ready
    // when the button is pressed. The core keeps those found.
    val title = now.meta?.title?.toString() ?: ""
    val artist = now.meta?.artist?.toString() ?: ""
    val lyrics by produceState<LyricsState>(LyricsState.Loading, now.id) {
        value = LyricsState.Loading
        val id = now.id
        val found = if (id == null) null else withContext(Dispatchers.IO) {
            runCatching { core.lyrics(id, title, artist, 0f) }.getOrNull()
        }
        value = found?.let { LyricsState.Found(it) } ?: LyricsState.None
    }
    // The time into the song, every frame, for the lyrics.
    val time = rememberSongTime(controller, now.playing)
    var dragged by remember { mutableFloatStateOf(0f) }
    Box(
        Modifier
            .fillMaxSize()
            .background(Tonearm.bg)
            .pointerInput(Unit) {
                detectVerticalDragGestures(
                    onDragEnd = { if (dragged > 200f) onClose(); dragged = 0f },
                    onDragCancel = { dragged = 0f },
                ) { _, amount -> dragged = (dragged + amount).coerceAtLeast(0f) }
            },
    ) {
        // The desktop's expanded Now playing background, flowing.
        FlowingBackdrop(cover, Modifier.fillMaxSize())
        Column(
            Modifier.fillMaxSize().systemBarsPadding().padding(horizontal = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(Modifier.fillMaxWidth().height(56.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.size(40.dp).clip(CircleShape).clickable(onClick = onClose),
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.down, size = 24.sp) }
                Spacer(Modifier.weight(1f))
                Dim("Now playing", 13.sp)
                Spacer(Modifier.weight(1f))
                Spacer(Modifier.size(40.dp))
            }
            if (panel == Panel.Lyrics) {
                LyricsPane(lyrics, controller, time, Modifier.weight(1f).fillMaxWidth())
                Spacer(Modifier.height(16.dp))
            } else if (panel == Panel.Queue) {
                QueuePane(core, { onMenu(it.asItem()) }, Modifier.weight(1f).fillMaxWidth().padding(top = 8.dp))
                Spacer(Modifier.height(16.dp))
            } else {
                // The cover and the lyrics under it, centred in the room
                // left. The cover is as wide as everything else, and only
                // gives up size on a screen too short to fit it.
                Column(
                    Modifier.weight(1f).fillMaxWidth(),
                    verticalArrangement = Arrangement.Center,
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    AsyncImage(
                        model = cover,
                        contentDescription = null,
                        contentScale = ContentScale.Crop,
                        modifier = Modifier.weight(1f, fill = false).aspectRatio(1f)
                            .clip(RoundedCornerShape(Tonearm.radius)).background(Tonearm.raised),
                    )
                    // A few lines from where the song is, under the cover,
                    // as on the desktop. Tap them for the lyrics.
                    // The room is kept from the start, with bars standing
                    // in while they load, so the cover never shifts.
                    Spacer(Modifier.height(20.dp))
                    when (val state = lyrics) {
                        is LyricsState.Found ->
                            LyricsPreview(state.lyrics, time, height = 60.dp, onOpen = { onPanel(Panel.Lyrics) })
                        LyricsState.Loading -> LyricsSkeleton(60.dp)
                        else -> Spacer(Modifier.height(60.dp))
                    }
                }
                Spacer(Modifier.height(16.dp))
            }
            // The name, with the heart beside it as on the desktop: filled
            // in the accent once liked.
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Title(now.meta?.title?.toString() ?: "", size = 24.sp, weight = FontWeight.Bold, lines = 2)
                    Spacer(Modifier.height(4.dp))
                    Title(
                        if (now.loading) "Loading…" else now.meta?.artist?.toString() ?: "",
                        size = 17.sp, weight = FontWeight.Medium, color = Tonearm.dim,
                    )
                }
                if (liked != null) {
                    LikeButton(liked, onLike)
                }
            }
            Spacer(Modifier.height(22.dp))
            SeekBar(now, controller)
            Spacer(Modifier.height(18.dp))
            Controls(now, controller, core)
            Spacer(Modifier.height(10.dp))
            // The panels: lyrics on the left, the queue on the right. Lit
            // while shown; lyrics dim when the song has none.
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                PanelButton(
                    Icons.lyrics,
                    when {
                        panel == Panel.Lyrics -> Tonearm.accent
                        lyrics == LyricsState.None -> Tonearm.faint
                        else -> Tonearm.dim
                    },
                ) { toggle(Panel.Lyrics) }
                Spacer(Modifier.weight(1f))
                PanelButton(Icons.queue, if (panel == Panel.Queue) Tonearm.accent else Tonearm.dim) { toggle(Panel.Queue) }
            }
            Spacer(Modifier.height(12.dp))
        }
    }
}

/** Two faint lines, gently pulsing, where the lyrics preview will be. The
 *  pulse runs in the draw phase only, so it costs no recomposition. */
@Composable
private fun LyricsSkeleton(height: androidx.compose.ui.unit.Dp) {
    val pulse = rememberInfiniteTransition(label = "skeleton")
    val alpha by pulse.animateFloat(
        initialValue = 0.06f,
        targetValue = 0.14f,
        animationSpec = infiniteRepeatable(tween(900, easing = FastOutSlowInEasing), RepeatMode.Reverse),
        label = "alpha",
    )
    Column(Modifier.fillMaxWidth().height(height), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        listOf(0.72f, 0.46f).forEach { width ->
            Box(
                Modifier.fillMaxWidth(width).height(18.dp).clip(RoundedCornerShape(6.dp))
                    .drawBehind { drawRect(Color.White.copy(alpha = alpha)) },
            )
        }
    }
}

/** The heart. A like gives a small pop, as the press lands. */
@Composable
private fun LikeButton(liked: Boolean, onClick: () -> Unit) {
    val scale = remember { Animatable(1f) }
    var pressed by remember { mutableIntStateOf(0) }
    LaunchedEffect(pressed) {
        if (pressed > 0 && liked) {
            scale.snapTo(0.8f)
            scale.animateTo(1f, spring(dampingRatio = 0.4f, stiffness = 600f))
        }
    }
    Box(
        Modifier.padding(start = 12.dp).size(48.dp).clip(CircleShape).clickable {
            pressed++
            onClick()
        },
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            Icons.heart,
            modifier = Modifier.graphicsLayer { scaleX = scale.value; scaleY = scale.value },
            size = 26.sp,
            filled = liked,
            color = if (liked) Tonearm.accent else Tonearm.text,
        )
    }
}

/** What Now Playing shows above the song's name. */
enum class Panel { Cover, Lyrics, Queue }

@Composable
private fun PanelButton(glyph: String, color: Color, onClick: () -> Unit) {
    Box(
        Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Icon(glyph, size = 22.sp, color = color) }
}

/** A thin bar to drag or tap, with the times under it. */
@Composable
fun SeekBar(now: NowPlaying, controller: MediaController) {
    var dragging by remember { mutableStateOf<Float?>(null) }
    var width by remember { mutableIntStateOf(1) }
    val duration = now.duration.coerceAtLeast(0)
    val shown = dragging ?: if (duration > 0) now.position.toFloat() / duration else 0f
    Column(Modifier.fillMaxWidth()) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(28.dp)
                .onSizeChangedWidth { width = it }
                .pointerInput(duration) {
                    detectTapGestures { offset ->
                        if (duration > 0) controller.seekTo((offset.x / width * duration).toLong())
                    }
                }
                .pointerInput(duration) {
                    detectHorizontalDragGestures(
                        onDragStart = { dragging = it.x / width },
                        onDragEnd = {
                            dragging?.let { if (duration > 0) controller.seekTo((it * duration).toLong()) }
                            dragging = null
                        },
                        onDragCancel = { dragging = null },
                    ) { change, _ -> dragging = (change.position.x / width).coerceIn(0f, 1f) }
                },
            contentAlignment = Alignment.CenterStart,
        ) {
            Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(Color(0x33FFFFFF))) {
                Box(Modifier.fillMaxWidth(shown.coerceIn(0f, 1f)).fillMaxHeight().background(Tonearm.text))
            }
        }
        Row(Modifier.fillMaxWidth()) {
            val at = dragging?.let { (it * duration).toLong() } ?: now.position
            Dim(clock(at), 12.sp)
            Spacer(Modifier.weight(1f))
            Dim(if (duration > 0) clock(duration) else "", 12.sp)
        }
    }
}

@Composable
fun Controls(now: NowPlaying, controller: MediaController, core: Core) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        // Shuffle on the left and repeat on the right, as on the desktop,
        // each with a dot under it while on. The core mixes the queue
        // itself, so shuffle goes to it rather than through Media3.
        var shuffle by remember(core) { mutableStateOf(core.playback().shuffle) }
        ModeButton(Icons.shuffle, shuffle) {
            shuffle = !shuffle
            core.setShuffle(shuffle)
        }
        Spacer(Modifier.weight(1f))
        IconButton(Icons.prev, size = 30) { controller.seekToPrevious() }
        Spacer(Modifier.width(18.dp))
        RoundButton(if (now.playing) Icons.pause else Icons.play, size = 72.dp) {
            if (now.playing) controller.pause() else controller.play()
        }
        Spacer(Modifier.width(18.dp))
        IconButton(Icons.next, size = 30) { controller.seekToNext() }
        Spacer(Modifier.weight(1f))
        val repeat = controller.repeatMode
        ModeButton(if (repeat == Player.REPEAT_MODE_ONE) Icons.repeatOne else Icons.repeat, repeat != Player.REPEAT_MODE_OFF) {
            controller.repeatMode = when (repeat) {
                Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
                Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
                else -> Player.REPEAT_MODE_OFF
            }
        }
    }
}

/** Shuffle or repeat: the accent and a dot under it while on. */
@Composable
private fun ModeButton(glyph: String, on: Boolean, onClick: () -> Unit) {
    Box(Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onClick), contentAlignment = Alignment.Center) {
        Icon(glyph, size = 22.sp, color = if (on) Tonearm.accent else Tonearm.dim)
        if (on) {
            Box(
                Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp).size(5.dp)
                    .clip(CircleShape).background(Tonearm.accent),
            )
        }
    }
}

fun clock(ms: Long): String {
    val s = ms.coerceAtLeast(0) / 1000
    return "%d:%02d".format(s / 60, s % 60)
}

private fun Modifier.onSizeChangedWidth(set: (Int) -> Unit) =
    this.onSizeChanged { set(it.width.coerceAtLeast(1)) }
