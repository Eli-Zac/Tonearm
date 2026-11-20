package app.tonearm

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.BlurredEdgeTreatment
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.onPlaced
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.session.MediaController
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.tonearm_android.LyricLine
import uniffi.tonearm_android.Lyrics
import uniffi.tonearm_android.Syllable
import kotlin.math.pow

// The motion here is the desktop's (the lyrics panel in ui/app.slint):
// keep the two the same.

/** Where a song's lyrics stand. */
sealed interface LyricsState {
    data object Loading : LyricsState
    data object None : LyricsState
    data class Found(val lyrics: Lyrics) : LyricsState
}

/** A row down the lyrics: a line, or the three dots of a long wait before
 *  the line `next`. */
private sealed interface LyricRow {
    /** The line number the ripple counts from. */
    val line: Int

    data class Line(val number: Int, val text: LyricLine) : LyricRow {
        override val line get() = number
    }

    data class Gap(val start: Long, val end: Long, val next: Int) : LyricRow {
        override val line get() = next - 1
    }
}

/** A wait this long between lines gets the interlude dots. */
private const val GAP_MS = 5000

private fun rowsOf(lyrics: Lyrics): List<LyricRow> {
    val rows = mutableListOf<LyricRow>()
    var previousEnd = 0L
    lyrics.lines.forEachIndexed { number, line ->
        if (lyrics.synced && line.start.toLong() > previousEnd + GAP_MS) {
            rows += LyricRow.Gap(previousEnd, line.start.toLong(), number)
        }
        rows += LyricRow.Line(number, line)
        previousEnd = line.end.toLong()
    }
    return rows
}

/** The line being sung at `t`: the last one started, or -1 before the
 *  first. A sung line stays lit through the quiet after it. */
private fun lineAt(lyrics: Lyrics, t: Long): Int {
    if (!lyrics.synced) return -1
    var found = -1
    for ((i, line) in lyrics.lines.withIndex()) {
        if (line.start.toLong() > t) break
        found = i
    }
    return found
}

/**
 * The time into the song, for every frame. The session's position comes a
 * few times a second, so between readings the time runs on by the clock
 * while the song plays. Small steps back as a reading lands are smoothed
 * over; a seek jumps.
 */
@Composable
fun rememberSongTime(controller: MediaController, playing: Boolean, song: String): LongState {
    val time = remember { mutableLongStateOf(controller.currentPosition) }
    // A new song starts from the top. For a moment its readings can still
    // be the last song's place, so those are passed over until one from
    // near the start comes, or a couple of seconds go by (a song resumed
    // part way through).
    var fresh by remember { mutableStateOf<Pair<String, Long>?>(null) }
    var seen by remember { mutableStateOf(song) }
    if (seen != song) {
        seen = song
        fresh = song to System.nanoTime()
        time.longValue = 0
    }
    LaunchedEffect(controller, playing) {
        var raw = controller.currentPosition
        var stamp = -1L
        while (true) {
            withFrameNanos { frame ->
                var reading = controller.currentPosition
                fresh?.let { (_, since) ->
                    val waited = (System.nanoTime() - since) / 1_000_000
                    if (reading < 1500 || waited > 2500) fresh = null else reading = 0
                }
                if (reading != raw || stamp < 0) {
                    raw = reading
                    stamp = frame
                }
                val ran = if (playing) ((frame - stamp) / 1_000_000).coerceAtMost(1500) else 0
                val guess = raw + ran
                val shown = time.longValue
                time.longValue = if (guess < shown && shown - guess < 400) shown else guess
            }
        }
    }
    return time
}

/**
 * The song's lyrics in place of its cover: the line being sung bright and
 * a little larger, a fifth of the way down, the rest faint and out of
 * focus by how far they are from it. Words fill syllable by syllable where
 * the source times them. Tap a line to play from it; scroll by hand and
 * the lyrics stay put for a few seconds.
 */
@Composable
fun LyricsPane(state: LyricsState, controller: MediaController, time: LongState, modifier: Modifier = Modifier) {
    when (state) {
        LyricsState.Loading -> LyricsSkeletonLines(modifier)
        LyricsState.None -> Box(modifier, contentAlignment = Alignment.Center) {
            Dim("No lyrics for this track", 16.sp)
        }
        is LyricsState.Found ->
            if (state.lyrics.synced) SyncedLyrics(state.lyrics, controller, time, modifier)
            else PlainLyrics(state.lyrics, modifier)
    }
}

/** While the lyrics are looked for: faint pulsing bars where the lines
 *  will be, starting where the sung line sits, the size of a line each. */
@Composable
private fun LyricsSkeletonLines(modifier: Modifier) {
    BoxWithConstraints(modifier.fadeEdges()) {
        Column(Modifier.fillMaxWidth().padding(top = maxHeight * 0.2f)) {
            listOf(0.82f, 0.64f, 0.9f, 0.56f, 0.74f, 0.86f, 0.5f, 0.7f).forEach { width ->
                Box(Modifier.padding(vertical = 9.dp + 4.dp)) {
                    TextSkeleton(width, 26.dp)
                }
            }
        }
    }
}

@Composable
private fun PlainLyrics(lyrics: Lyrics, modifier: Modifier) {
    LazyColumn(modifier.fadeEdges(), contentPadding = PaddingValues(vertical = 24.dp)) {
        itemsIndexed(lyrics.lines) { _, line ->
            Text(
                line.text.ifBlank { " " },
                style = TextStyle(fontFamily = Tonearm.font, fontSize = 16.sp, lineHeight = 24.sp, color = Tonearm.text),
                modifier = Modifier.fillMaxWidth(),
            )
        }
        item { Credit(lyrics.source) }
    }
}

@Composable
private fun Credit(source: String) {
    Dim("Lyrics from $source", 12.sp, modifier = Modifier.padding(top = 28.dp))
}

/** One move of the lyrics to a new sung line. Every row starts `delta`
 *  pixels from where it now sits and rides back in, each on its own beat. */
private class Move(val delta: Float, val at: Long, val sung: Int)

/** Apple Music's ripple, as AMLL measures it, and the desktop draws it: a
 *  spring fitted by this curve over 800 ms. */
private val rippleEasing = CubicBezierEasing(0.228f, 0.481f, 0.069f, 1.05f)
private const val RIPPLE_MS = 800f

/** The line above the sung one moves first; each line under it 50 ms after
 *  the last, the step shrinking by 5% a line from the sung one down. */
private fun rippleDelay(rel: Int): Float =
    if (rel < 0) 0f else 50f + 1050f * (1f - (1f / 1.05f).pow(rel))

/** How far a row still sits from its place, from the moves under way. */
private fun shiftOf(line: Int, moves: List<Move>, now: Long): Float {
    var shift = 0f
    for (move in moves) {
        val ms = (now - move.at) / 1_000_000f - rippleDelay(line - move.sung)
        val progress = (ms / RIPPLE_MS).coerceIn(0f, 1f)
        shift += move.delta * (1f - rippleEasing.transform(progress))
    }
    return shift
}

@Composable
private fun SyncedLyrics(lyrics: Lyrics, controller: MediaController, time: LongState, modifier: Modifier) {
    val rows = remember(lyrics) { rowsOf(lyrics) }
    val rowOfLine = remember(rows) {
        IntArray(lyrics.lines.size).also { index ->
            rows.forEachIndexed { i, row -> if (row is LyricRow.Line) index[row.number] = i }
        }
    }
    val sung by remember(lyrics) { derivedStateOf { lineAt(lyrics, time.longValue) } }
    // The interlude dots, while a long wait runs.
    val gap by remember(rows) {
        derivedStateOf {
            val t = time.longValue
            rows.indexOfFirst { it is LyricRow.Gap && t >= it.start && t < it.end }
        }
    }
    // The row brought into place: the dots in a wait, else the sung line.
    val target = when {
        gap >= 0 -> gap
        sung >= 0 -> rowOfLine[sung]
        else -> -1
    }

    // A frame clock for the ripple, and the moves still rippling.
    val clock = remember { mutableLongStateOf(0L) }
    val moves = remember(lyrics) { mutableStateListOf<Move>() }
    LaunchedEffect(Unit) {
        while (true) {
            withFrameNanos { frame ->
                clock.longValue = frame
                // The last row starts at most 1.15 s late.
                if (moves.isNotEmpty() && frame - moves.first().at > 2_100_000_000L) moves.removeAt(0)
            }
        }
    }

    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    // Scrolled by hand: the lyrics stop following the song for a while,
    // the lines stay sharp and evenly faint.
    var free by remember { mutableStateOf(false) }
    LaunchedEffect(list) {
        var settle: Job? = null
        list.interactionSource.interactions.collect { interaction ->
            when (interaction) {
                is DragInteraction.Start -> {
                    settle?.cancel()
                    moves.clear()
                    free = true
                }
                is DragInteraction.Stop, is DragInteraction.Cancel -> {
                    settle?.cancel()
                    settle = scope.launch {
                        delay(3000)
                        free = false
                    }
                }
            }
        }
    }

    BoxWithConstraints(modifier) {
        val top = maxHeight * 0.2f
        // The first placing jumps, so the lyrics open on the sung line.
        var placed by remember(lyrics) { mutableStateOf(false) }
        LaunchedEffect(target, free, lyrics) {
            if (free || target < 0) return@LaunchedEffect
            val now = list.layoutInfo.visibleItemsInfo.firstOrNull { it.index == target }
            if (!placed || now == null) {
                // Too far to ripple to: go straight there.
                moves.clear()
                list.scrollToItem(target)
                placed = true
                return@LaunchedEffect
            }
            // The list jumps to its new place at once, and every row starts
            // from where it was, so the move is drawn by the rows' ripple.
            val delta = now.offset.toFloat()
            if (delta == 0f) return@LaunchedEffect
            val line = rows[target].line
            Snapshot.withMutableSnapshot {
                moves.add(Move(delta, clock.longValue, if (rows[target] is LyricRow.Gap) line + 1 else line))
                list.requestScrollToItem(target)
            }
        }
        LazyColumn(
            Modifier.fillMaxSize().fadeEdges(),
            state = list,
            contentPadding = PaddingValues(top = top, bottom = maxHeight - top),
        ) {
            itemsIndexed(rows) { index, row ->
                val ripple = Modifier.graphicsLayer {
                    translationY = shiftOf(row.line, moves, clock.longValue)
                }
                when (row) {
                    is LyricRow.Gap -> Interlude(row, index == gap, time, ripple)
                    is LyricRow.Line -> Line(
                        row.text,
                        active = row.number == sung,
                        distance = kotlin.math.abs(row.number - sung.coerceAtLeast(0)),
                        free = free,
                        time = time,
                        modifier = ripple,
                    ) { at ->
                        // A word tapped plays from that word, anywhere else
                        // on the line from the line's start.
                        controller.seekTo(at)
                        free = false
                    }
                }
            }
            item {
                Box(Modifier.graphicsLayer { translationY = shiftOf(lyrics.lines.size, moves, clock.longValue) }) {
                    Credit(lyrics.source)
                }
            }
        }
    }
}

private val lineFont = 26.sp
private const val GROW = 1.04f
private val growEasing = CubicBezierEasing(0.2f, 0.85f, 0.25f, 1f)

/** The desktop's two blur levels, light and heavy, as standard deviations
 *  of 1.2 and 3.2: as radii, with what's between drawn between. */
private fun blurOf(level: Float): Dp = when {
    level <= 0f -> 0.dp
    level <= 1f -> (1.2f * level).dp
    else -> (1.2f + 3.5f * (level - 1f).coerceAtMost(1f)).dp
}

@Composable
private fun Line(
    line: LyricLine,
    active: Boolean,
    distance: Int,
    free: Boolean,
    time: LongState,
    modifier: Modifier,
    onTap: (Long) -> Unit,
) {
    // Lines fade with distance from the one being sung.
    val fade by animateFloatAsState(
        when {
            active -> 1f
            free -> 0.6f
            else -> (0.42f - 0.03f * distance).coerceIn(0.28f, 0.42f)
        },
        tween(350, easing = EaseOut),
    )
    // And go out of focus: 0 sharp, 1 a little, 2 a lot.
    val blur by animateFloatAsState(
        if (!free && distance > 1) minOf(2f, 0.6f * (distance - 1)) else 0f,
        tween(400, easing = EaseOut),
    )
    // The sung line grows.
    val grow by animateFloatAsState(if (active) GROW else 1f, tween(350, easing = growEasing))
    val align = line.align.toInt()
    val horizontal = when (align) {
        1 -> Alignment.End
        2 -> Alignment.CenterHorizontally
        else -> Alignment.Start
    }
    val origin = when (align) {
        1 -> TransformOrigin(1f, 0.5f)
        2 -> TransformOrigin(0.5f, 0.5f)
        else -> TransformOrigin(0f, 0.5f)
    }
    Column(
        modifier
            .fillMaxWidth()
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null) { onTap(line.start.toLong()) }
            .padding(vertical = 9.dp),
        horizontalAlignment = horizontal,
    ) {
        // Laid out narrower by the growth, so the sung line, drawn larger,
        // still fits the width. As the desktop wraps for the grown size.
        Box(
            Modifier
                .fillMaxWidth(1f / GROW)
                .graphicsLayer {
                    alpha = fade
                    scaleX = grow
                    scaleY = grow
                    transformOrigin = origin
                }
                .blur(blurOf(blur), BlurredEdgeTreatment.Unbounded),
        ) {
            if (line.syllables.isNotEmpty()) {
                Words(line.syllables, align, lineFont, active, time, onTap)
            } else {
                Text(line.text, style = lyricStyle(lineFont, align), modifier = Modifier.fillMaxWidth())
            }
        }
        // Backing vocals come out from under their line while it is sung,
        // and tuck away after.
        if (line.background.isNotEmpty() || line.backgroundText.isNotBlank()) {
            val tenPx = with(LocalDensity.current) { 10.dp.roundToPx() }
            AnimatedVisibility(
                visible = active,
                enter = expandVertically(tween(300, easing = EaseOut)) +
                    fadeIn(tween(350, easing = EaseOut)) +
                    slideInVertically(tween(350, easing = EaseOut)) { -tenPx },
                exit = shrinkVertically(tween(300, easing = EaseOut)) +
                    fadeOut(tween(350, easing = EaseOut)) +
                    slideOutVertically(tween(350, easing = EaseOut)) { -tenPx },
            ) {
                Box(Modifier.padding(top = 4.dp).graphicsLayer { alpha = 0.85f }) {
                    if (line.background.isNotEmpty()) {
                        Words(line.background, align, lineFont * 0.6f, active, time, onTap)
                    } else {
                        Text(line.backgroundText, style = lyricStyle(lineFont * 0.6f, align), modifier = Modifier.fillMaxWidth())
                    }
                }
            }
        }
        if (line.translation.isNotBlank()) {
            Text(
                line.translation,
                style = lyricStyle(lineFont * 0.6f, align).copy(fontWeight = FontWeight.Medium),
                modifier = Modifier.fillMaxWidth().padding(top = 4.dp).graphicsLayer { alpha = fade * 0.7f },
            )
        }
    }
}

private fun lyricStyle(size: TextUnit, align: Int, color: Color = Color.White) = TextStyle(
    fontFamily = Tonearm.font,
    fontWeight = FontWeight.Bold,
    fontSize = size,
    lineHeight = size * 1.3f,
    color = color,
    textAlign = when (align) {
        1 -> TextAlign.End
        2 -> TextAlign.Center
        else -> TextAlign.Start
    },
)

/** A line's syllables, grouped into words so a word never breaks across
 *  rows, each filling left to right as it is sung. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Words(
    syllables: List<Syllable>,
    align: Int,
    size: TextUnit,
    active: Boolean,
    time: LongState,
    onWord: ((Long) -> Unit)? = null,
) {
    val words = remember(syllables) {
        val words = mutableListOf<MutableList<Syllable>>(mutableListOf())
        for (syllable in syllables) {
            words.last().add(syllable)
            if (syllable.text.endsWith(" ")) words.add(mutableListOf())
        }
        words.filter { it.isNotEmpty() }
    }
    val space = with(LocalDensity.current) { (size * 0.3f).toDp() }
    FlowRow(
        Modifier.fillMaxWidth(),
        horizontalArrangement = when (align) {
            1 -> Arrangement.spacedBy(space, Alignment.End)
            2 -> Arrangement.spacedBy(space, Alignment.CenterHorizontally)
            else -> Arrangement.spacedBy(space)
        },
    ) {
        for (word in words) {
            // A tap on a word plays from where it is sung.
            val tap = onWord?.let { seek ->
                Modifier.clickable(interactionSource = remember { MutableInteractionSource() }, indication = null) {
                    seek(word.first().start.toLong())
                }
            } ?: Modifier
            Row(tap) {
                for (syllable in word) SyllableText(syllable, size, active, time)
            }
        }
    }
}

/** One syllable: faint until sung, then white from the left as far as the
 *  song has got through it. Sung syllables rise a little, the way Apple
 *  Music lifts them. Only the sung line's syllables fill; the rest are
 *  drawn whole. */
@Composable
private fun SyllableText(syllable: Syllable, fontSize: TextUnit, active: Boolean, time: LongState) {
    val start = syllable.start.toLong()
    val end = syllable.end.toLong()
    val text = syllable.text.trimEnd()
    val started by remember(syllable) { derivedStateOf { time.longValue >= start } }
    val lift by animateFloatAsState(if (active && started) -2f else 0f, tween(350, easing = EaseOut))
    if (!active && lift == 0f) {
        Text(text, style = lyricStyle(fontSize, 0))
        return
    }
    Box(Modifier.graphicsLayer { translationY = lift * density }) {
        Text(text, style = lyricStyle(fontSize, 0, if (active) Color(0x66FFFFFF) else Color.White))
        if (active) Text(
            text,
            style = lyricStyle(fontSize, 0),
            modifier = Modifier.drawWithContent {
                val t = time.longValue
                val progress = when {
                    t <= start -> 0f
                    t >= end -> 1f
                    else -> (t - start).toFloat() / (end - start).coerceAtLeast(1)
                }
                if (progress > 0f) clipRect(right = size.width * progress) { this@drawWithContent.drawContent() }
            },
        )
    }
}

/** Three dots that fill one after another through a long wait between
 *  lines. They take no room until the wait begins. */
@Composable
private fun Interlude(gap: LyricRow.Gap, active: Boolean, time: LongState, modifier: Modifier) {
    AnimatedVisibility(
        visible = active,
        modifier = modifier,
        enter = expandVertically(tween(300, easing = EaseOut)) + fadeIn(tween(300, easing = EaseOut)),
        exit = shrinkVertically(tween(300, easing = EaseOut)) + fadeOut(tween(300, easing = EaseOut)),
    ) {
        Row(
            Modifier.padding(start = 2.dp, top = 9.dp, bottom = 9.dp).height(34.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for (n in 0 until 3) {
                Box(
                    Modifier
                        .size(10.dp)
                        .graphicsLayer {
                            val progress = ((time.longValue - gap.start).toFloat() / (gap.end - gap.start).coerceAtLeast(1))
                                .coerceIn(0f, 1f)
                            alpha = 0.25f + 0.75f * (progress * 3 - n).coerceIn(0f, 1f)
                        }
                        .clip(CircleShape)
                        .background(Color.White),
                )
            }
        }
    }
}

/**
 * The lyrics in a small window under the cover, as on the desktop's Now
 * playing: every line, wrapped, scrolled so the line being sung sits at
 * the top, the lines under it fading out. The lines ripple up into place
 * and the sung one fills syllable by syllable, as in the full lyrics. Tap
 * it to open the lyrics.
 */
@Composable
fun LyricsPreview(lyrics: Lyrics, time: LongState, height: Dp, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    val current by remember(lyrics) { derivedStateOf { lineAt(lyrics, time.longValue).coerceAtLeast(0) } }
    val tops = remember(lyrics) { mutableStateMapOf<Int, Int>() }
    val heights = remember(lyrics) { mutableStateMapOf<Int, Int>() }
    // The same ripple as the full lyrics, as the window moves line to line.
    val clock = remember { mutableLongStateOf(0L) }
    val moves = remember(lyrics) { mutableStateListOf<Move>() }
    LaunchedEffect(Unit) {
        while (true) {
            withFrameNanos { frame ->
                clock.longValue = frame
                if (moves.isNotEmpty() && frame - moves.first().at > 2_100_000_000L) moves.removeAt(0)
            }
        }
    }
    var shown by remember(lyrics) { mutableIntStateOf(current) }
    if (shown != current) {
        // The window jumps to the new line; the lines start from where
        // they were and ride up.
        val delta = ((tops[current] ?: 0) - (tops[shown] ?: 0)).toFloat()
        if (delta != 0f && kotlin.math.abs(current - shown) <= 3) moves.add(Move(delta, clock.longValue, current))
        else moves.clear()
        shown = current
    }
    val density = LocalDensity.current
    val fadeFrom = with(density) { (height - 20.dp).toPx() }
    val fadeOver = with(density) { 36.dp.toPx() }
    Box(
        modifier
            .fillMaxWidth()
            .height(height)
            .clipToBounds()
            // The line under the sung one fades into the cut, and a line
            // rippling down from above comes in soft.
            .fadeEdges(top = 0.05f, bottom = 0.55f)
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onOpen),
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .wrapContentHeight(Alignment.Top, unbounded = true)
                .offset { IntOffset(0, -(tops[current] ?: 0)) },
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            lyrics.lines.forEachIndexed { i, line ->
                val sung = lyrics.synced && i == current
                val lit = !lyrics.synced || sung
                val color by animateColorAsState(if (lit) Tonearm.text else Tonearm.text.copy(alpha = 0.5f), tween(300))
                Box(
                    Modifier
                        .fillMaxWidth()
                        .onPlaced {
                            tops[i] = it.positionInParent().y.toInt()
                            heights[i] = it.size.height
                        }
                        .graphicsLayer {
                            translationY = shiftOf(i, moves, clock.longValue)
                            // Lines fade out as they reach the bottom; the
                            // sung one stays whole.
                            val bottom = (tops[i] ?: 0) - (tops[current] ?: 0) + (heights[i] ?: 0)
                            alpha = if (i == current) 1f
                            else (1f - (bottom - fadeFrom) / fadeOver).coerceIn(0.15f, 1f)
                        },
                ) {
                    if (sung && line.syllables.isNotEmpty()) {
                        Words(line.syllables, 0, 18.sp, active = true, time = time)
                    } else {
                        Text(
                            line.text,
                            style = TextStyle(fontFamily = Tonearm.font, fontWeight = FontWeight.Bold, fontSize = 18.sp, lineHeight = 23.sp),
                            color = color,
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
            }
        }
    }
}

/** Fades the lyrics out at the top and bottom edges rather than cutting
 *  them off there: whole from `top` to `bottom`, as shares of the height. */
private fun Modifier.fadeEdges(top: Float = 0.06f, bottom: Float = 0.88f) = this
    .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
    .drawWithContent {
        drawContent()
        drawRect(
            Brush.verticalGradient(
                0f to Color.Transparent,
                top to Color.Black,
                bottom to Color.Black,
                1f to Color.Transparent,
            ),
            blendMode = BlendMode.DstIn,
        )
    }
