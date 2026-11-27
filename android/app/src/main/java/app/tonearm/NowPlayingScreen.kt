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
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.animation.core.animateFloatAsState
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
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withLink
import androidx.compose.material3.Text
import uniffi.tonearm_android.Link
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import coil3.compose.AsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.CastState
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
    /** A song's menu, from a long press in the queue or the dots at the top. */
    onMenu: (Item) -> Unit,
    /** The Cast devices, and the button that opens the Devices sheet. */
    cast: CastState? = null,
    onCast: () -> Unit = {},
    /** Whether a sleep timer runs, and the button that opens its choices. */
    sleeping: Boolean = false,
    onSleep: () -> Unit = {},
    /** Whether the song is liked; null while unknown or signed out. */
    liked: Boolean?,
    onLike: () -> Unit,
    /** An artist's name tapped: their page. */
    onOpenArtist: (id: String, name: String) -> Unit,
    /** Swiped off the bottom: already gone, so no closing slide after. */
    onSwipedAway: () -> Unit = {},
    /** Whether it is meant to be showing: opened again, it comes back
     *  from wherever the last swipe left it. */
    open: Boolean = true,
    onClose: () -> Unit,
) {
    // A panel's button shows it, and pressed again goes back to the cover.
    val toggle = { p: Panel -> onPanel(if (panel == p) Panel.Cover else p) }
    // The song from the core's queue, by id: its cover and linked artists.
    // The session's details can still be the last song's for a moment.
    val playing = remember(now.id) { core.nowPlaying()?.takeIf { it.videoId == now.id } }
    val cover = playing?.cover?.ifEmpty { null } ?: now.meta?.artworkUri?.toString()
    // Each song's lyrics are looked for as it comes on, so they are ready
    // when the button is pressed. The core keeps those found.
    val title = now.meta?.title?.toString() ?: ""
    val artist = now.meta?.artist?.toString() ?: ""
    var shownLyricsFor by remember { mutableStateOf<String?>(null) }
    // Looked up again when Settings change the translation. The lyrics'
    // own translation comes with them; Google Translate's for the rest of
    // the lines follows when it has come, as on the desktop.
    val lyrics by produceState<LyricsState>(LyricsState.Loading, now.id, Prefs.version) {
        val id = now.id
        val shown = value
        // Same song, a new translation: the lines stay up meanwhile.
        if (shown !is LyricsState.Found || shownLyricsFor != id) value = LyricsState.Loading
        val found = if (id == null) null else withContext(Dispatchers.IO) {
            runCatching { core.lyrics(id, title, artist, 0f) }.getOrNull()
        }
        shownLyricsFor = id
        value = found?.let { LyricsState.Found(it) } ?: LyricsState.None
        if (found != null && id != null) {
            withContext(Dispatchers.IO) { runCatching { core.translatedLyrics(id) }.getOrNull() }
                ?.let { value = LyricsState.Found(it) }
        }
    }
    // The time into the song, every frame, for the lyrics.
    val time = rememberSongTime(controller, now.playing, now.id ?: "")
    // Swiped down, the screen follows the finger. Let go past a quarter of
    // the way, or with a downward flick, it slides the rest of the way off;
    // otherwise it springs back.
    val pulled = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    var height by remember { mutableIntStateOf(0) }
    val flick = with(LocalDensity.current) { 1000.dp.toPx() }
    // Once it is going, the swipe's catcher goes too: it stays the size
    // of the screen while the page slides off, and would take the taps
    // meant for the mini-player under it.
    var closing by remember { mutableStateOf(false) }
    LaunchedEffect(open) {
        if (open) {
            closing = false
            pulled.snapTo(0f)
        }
    }
    Box(
        Modifier
            .fillMaxSize()
            .onSizeChanged { height = it.height }
            // Before the layer moves, so the finger is measured on the
            // screen, not on the sliding page.
            .then(if (closing) Modifier else Modifier.pointerInput(Unit) {
                val tracker = androidx.compose.ui.input.pointer.util.VelocityTracker()
                val settle = {
                    val speed = tracker.calculateVelocity().y
                    tracker.resetTracking()
                    scope.launch {
                        if (pulled.value > height / 4f || speed > flick) {
                            closing = true
                            pulled.animateTo(height.toFloat(), tween(220, easing = FastOutSlowInEasing), speed)
                            onSwipedAway()
                            onClose()
                        } else {
                            pulled.animateTo(0f, spring(stiffness = 600f), speed)
                        }
                    }
                }
                detectVerticalDragGestures(
                    onDragEnd = { settle() },
                    onDragCancel = { settle() },
                ) { change, amount ->
                    tracker.addPosition(change.uptimeMillis, change.position)
                    scope.launch { pulled.snapTo((pulled.value + amount).coerceAtLeast(0f)) }
                }
            })
            .graphicsLayer { translationY = pulled.value }
            .background(Tonearm.bg),
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
                // Opposite the arrow: on the cover, the song's menu, as a long
                // press in the queue opens it; with the lyrics or queue up, an
                // X that goes back to the cover. One fades into the other in
                // the same room, so the title never shifts.
                val showX by animateFloatAsState(if (panel != Panel.Cover) 1f else 0f, tween(220), label = "x")
                Box(
                    Modifier.size(40.dp).clip(CircleShape).clickable {
                        if (panel != Panel.Cover) onPanel(Panel.Cover)
                        else playing?.let { onMenu(it.asItem()) }
                    },
                    contentAlignment = Alignment.Center,
                ) {
                    Box(Modifier.graphicsLayer { alpha = showX }) { CloseMark() }
                    Box(Modifier.graphicsLayer { alpha = 1f - showX }) {
                        Icon(Icons.dots, size = 24.sp, filled = true)
                    }
                }
            }
            // The cover stays put under the lyrics and queue, only hidden,
            // so its moving cover keeps playing rather than loading again
            // each time it comes back.
            // Each view fades in and rises into place as the desktop's panel
            // views do: 220 ms, coming up 16 dp.
            val coverT by animateFloatAsState(if (panel == Panel.Cover) 1f else 0f, tween(220, easing = panelEasing))
            val lyricsT by animateFloatAsState(if (panel == Panel.Lyrics) 1f else 0f, tween(220, easing = panelEasing))
            val queueT by animateFloatAsState(if (panel == Panel.Queue) 1f else 0f, tween(220, easing = panelEasing))
            val rise = with(LocalDensity.current) { 16.dp.toPx() }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                // The cover and the lyrics under it, centred in the room
                // left. The cover is as wide as everything else, and only
                // gives up size on a screen too short to fit it.
                Column(
                    Modifier.fillMaxSize().graphicsLayer { alpha = coverT; translationY = (1f - coverT) * rise },
                    verticalArrangement = Arrangement.Center,
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    // Slides as songs change, swipes to skip, and plays the
                    // album's moving cover when it has one.
                    CoverCarousel(
                        now.id?.let { CoverSong(it, cover) },
                        now.index,
                        now.queueVersion,
                        core,
                        controller,
                        Modifier.weight(1f, fill = false).aspectRatio(1f),
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
            // Over the hidden cover: taking every touch, so a swipe
            // there never skips the song.
                if (panel != Panel.Cover) {
                    Box(Modifier.fillMaxSize().pointerInput(Unit) { awaitPointerEventScope { while (true) awaitPointerEvent() } })
                }
                // Kept while fading out, so they leave as smoothly as they came.
                if (lyricsT > 0f) {
                    LyricsPane(lyrics, controller, time, Modifier.fillMaxSize().graphicsLayer {
                        alpha = lyricsT
                        translationY = (1f - lyricsT) * rise
                    })
                }
                if (queueT > 0f) {
                    QueuePane(core, { onMenu(it.asItem()) }, Modifier.fillMaxSize().padding(top = 8.dp).graphicsLayer {
                        alpha = queueT
                        translationY = (1f - queueT) * rise
                    })
                }
            }
            Spacer(Modifier.height(16.dp))
            // The name, with the heart beside it as on the desktop: filled
            // in the accent once liked.
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    // From the core's queue, known the moment the song
                    // changes, and kept through loading and seeking. A bar
                    // stands in only while neither knows it yet.
                    val songTitle = playing?.title ?: now.meta?.title?.toString().orEmpty()
                    val songArtist = playing?.artist ?: now.meta?.artist?.toString().orEmpty()
                    // Each line keeps one height, loaded or not, so nothing
                    // above or below moves when it fills in.
                    // One line, scrolling when too long, as the desktop's.
                    Box(Modifier.fillMaxWidth().height(30.dp), contentAlignment = Alignment.CenterStart) {
                        if (songTitle.isEmpty()) {
                            TextSkeleton(width = 0.6f, height = 24.dp)
                        } else {
                            Marquee(
                                songTitle,
                                TextStyle(
                                    color = Tonearm.text, fontFamily = Tonearm.font,
                                    fontWeight = FontWeight.Bold, fontSize = 24.sp, lineHeight = 30.sp,
                                ),
                            )
                        }
                    }
                    Spacer(Modifier.height(4.dp))
                    Box(Modifier.fillMaxWidth().height(24.dp), contentAlignment = Alignment.CenterStart) {
                        if (songArtist.isEmpty()) {
                            TextSkeleton(width = 0.35f, height = 18.dp)
                        } else {
                            ArtistLine(playing?.artists.orEmpty(), songArtist, onOpenArtist)
                        }
                    }
                }
                // The heart's room is kept while it loads, so the title
                // never wraps differently when it appears.
                Box(Modifier.padding(start = 12.dp).size(48.dp)) {
                    if (liked != null) LikeButton(liked, onLike)
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
                // The sleep timer: lit, with a dot, while one runs, as the desktop's.
                Box(
                    Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onSleep),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(Icons.moon, size = 22.sp, color = if (sleeping) Tonearm.accent else Tonearm.dim)
                    if (sleeping) {
                        Box(
                            Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp).size(5.dp)
                                .clip(CircleShape).background(Tonearm.accent),
                        )
                    }
                }
                Spacer(Modifier.weight(1f))
                // Lit while the music plays on a device, or is reaching one.
                Box(
                    Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onCast),
                    contentAlignment = Alignment.Center,
                ) {
                    CastMark(if (cast != null && (cast.active >= 0 || cast.connecting)) Tonearm.accent else Tonearm.dim)
                }
                Spacer(Modifier.weight(1f))
                PanelButton(Icons.queue, if (panel == Panel.Queue) Tonearm.accent else Tonearm.dim) { toggle(Panel.Queue) }
            }
            Spacer(Modifier.height(12.dp))
        }
    }
}

/** Two faint lines, gently pulsing, where the lyrics preview will be. */
@Composable
private fun LyricsSkeleton(height: androidx.compose.ui.unit.Dp) {
    Column(Modifier.fillMaxWidth().height(height), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        TextSkeleton(0.72f, 18.dp)
        TextSkeleton(0.46f, 18.dp)
    }
}

/** An X drawn as two strokes, the weight of the icon font's down arrow
 *  (the font is cut down to the glyphs the app uses, and has none). */
@Composable
private fun CloseMark() {
    androidx.compose.foundation.Canvas(Modifier.size(16.dp)) {
        val w = 2.dp.toPx()
        drawLine(Tonearm.text, androidx.compose.ui.geometry.Offset(0f, 0f), androidx.compose.ui.geometry.Offset(size.width, size.height), w, androidx.compose.ui.graphics.StrokeCap.Round)
        drawLine(Tonearm.text, androidx.compose.ui.geometry.Offset(size.width, 0f), androidx.compose.ui.geometry.Offset(0f, size.height), w, androidx.compose.ui.graphics.StrokeCap.Round)
    }
}

/** A bone, `width` of the room across, where text will be. */
@Composable
internal fun TextSkeleton(width: Float, height: androidx.compose.ui.unit.Dp) {
    Bone(Modifier.fillMaxWidth(width).height(height))
}

/** The song's artists, each name opening their page where YouTube Music
 *  links one, as the desktop's artist line does. */
@Composable
private fun ArtistLine(artists: List<Link>, fallback: String, onOpen: (String, String) -> Unit) {
    val style = TextStyle(fontFamily = Tonearm.font, fontWeight = FontWeight.Medium, fontSize = 17.sp, color = Tonearm.dim)
    if (artists.none { it.id != null }) {
        Title(fallback, size = 17.sp, weight = FontWeight.Medium, color = Tonearm.dim)
        return
    }
    val text = buildAnnotatedString {
        artists.forEachIndexed { i, artist ->
            if (i > 0) append(", ")
            val id = artist.id
            if (id == null) {
                append(artist.name)
            } else {
                // Looks just like the rest of the line: no colour, no underline.
                val plain = TextLinkStyles(SpanStyle(color = Tonearm.dim, textDecoration = TextDecoration.None))
                withLink(LinkAnnotation.Clickable(id, plain) { onOpen(id, artist.name) }) { append(artist.name) }
            }
        }
    }
    Text(text, style = style, maxLines = 1, overflow = TextOverflow.Ellipsis)
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
        Modifier.size(48.dp).clip(CircleShape).clickable {
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

/** The desktop's panel views' curve, cubic-bezier(0.22, 1, 0.36, 1). */
private val panelEasing = androidx.compose.animation.core.CubicBezierEasing(0.22f, 1f, 0.36f, 1f)

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
    // Where a tap or drag sent the song. The position read back lags the
    // audio by up to a second, so the bar holds here until it catches up
    // rather than jumping back to where it was.
    var seekedTo by remember { mutableStateOf<Long?>(null) }
    val target = seekedTo
    if (target != null && kotlin.math.abs(now.position - target) < 1500) {
        SideEffect { seekedTo = null }
    }
    LaunchedEffect(target) {
        if (target != null) {
            delay(2000)
            seekedTo = null
        }
    }
    fun seek(ms: Long) {
        seekedTo = ms
        controller.seekTo(ms)
    }
    val position = target ?: now.position
    val shown = dragging ?: if (duration > 0) position.toFloat() / duration else 0f
    Column(Modifier.fillMaxWidth()) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(28.dp)
                .onSizeChangedWidth { width = it }
                .pointerInput(duration) {
                    detectTapGestures { offset ->
                        if (duration > 0) seek((offset.x / width * duration).toLong())
                    }
                }
                .pointerInput(duration) {
                    detectHorizontalDragGestures(
                        onDragStart = { dragging = it.x / width },
                        onDragEnd = {
                            dragging?.let { if (duration > 0) seek((it * duration).toLong()) }
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
            val at = dragging?.let { (it * duration).toLong() } ?: position
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
        // The notification's, the car's and the watch's shuffle change it
        // too, so it follows the core.
        // Pressed again with shuffle on, it turns to smart shuffle, which
        // mixes in songs like the queue (the sparkle), then off, as the
        // desktop's button does.
        var shuffle by remember(core) { mutableStateOf(core.playback().shuffle) }
        var smart by remember(core) { mutableStateOf(core.playback().smart) }
        LaunchedEffect(core) {
            while (true) {
                val p = core.playback()
                shuffle = p.shuffle
                smart = p.smart
                delay(500)
            }
        }
        ModeButton(if (smart) Icons.sparkle else Icons.shuffle, shuffle) {
            if (shuffle && !smart) {
                smart = true
            } else {
                shuffle = !shuffle
                smart = false
            }
            core.cycleShuffle()
        }
        Spacer(Modifier.weight(1f))
        IconButton(Icons.rewind, size = 36) { controller.seekToPrevious() }
        Spacer(Modifier.width(18.dp))
        // Plain white glyphs, no disc behind play, as on the desktop.
        IconButton(if (now.playing) Icons.pause else Icons.play, size = 52) {
            if (now.playing) controller.pause() else controller.play()
        }
        Spacer(Modifier.width(18.dp))
        IconButton(Icons.fastForward, size = 36) { controller.seekToNext() }
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
