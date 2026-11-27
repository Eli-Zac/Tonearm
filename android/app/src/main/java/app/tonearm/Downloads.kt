package app.tonearm

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.Tonearm as Core

/** The Downloads page's route id, in a tab's stack like any page. */
const val DOWNLOADS_ROUTE = "tonearm:downloads"

/** Which songs are on the phone, and which are on their way and how far. */
data class DownloadState(
    val saved: Set<String> = emptySet(),
    val busy: Map<String, Float> = emptyMap(),
    /** The core's download version these were read at. */
    val version: ULong = 0u,
) {
    fun isSaved(id: String?) = id != null && id in saved
}

/** The download marks, for any row or header that shows them. */
val LocalDownloads = compositionLocalOf { DownloadState() }

/**
 * Keeps the download marks current: the core bumps a counter whenever a
 * download starts, moves on, ends or is removed, and this reads the marks
 * again when it moves, as Now Playing polls playback. What finished or
 * failed goes to `onNote`, as the desktop's toasts.
 */
@Composable
fun rememberDownloads(core: Core, onNote: (String) -> Unit): DownloadState {
    var state by remember { mutableStateOf(DownloadState()) }
    val note by rememberUpdatedState(onNote)
    val context = androidx.compose.ui.platform.LocalContext.current
    // Downloads under way hold the app up, so they finish with it closed.
    val downloading = state.busy.isNotEmpty()
    LaunchedEffect(downloading) { if (downloading) DownloadService.start(context) }
    LaunchedEffect(core) {
        var seen = 0uL
        while (true) {
            if (core.downloadsVersion() != seen) {
                val (marks, notes) = withContext(Dispatchers.IO) { core.downloadMarks() to core.takeDownloadNotes() }
                seen = marks.version
                state = DownloadState(marks.saved.toSet(), marks.busy.associate { it.videoId to it.progress }, marks.version)
                // A batch says one thing when it ends; the latest is enough.
                notes.lastOrNull()?.let { note(it) }
            }
            delay(300)
        }
    }
    return state
}

/**
 * A ring that fills clockwise as a song downloads, as the desktop's
 * `DownloadRing`: a faint track, the accent over the part done.
 */
@Composable
fun DownloadRing(progress: Float, size: Dp, stroke: Dp = 2.dp) {
    Canvas(Modifier.size(size)) {
        val width = stroke.toPx()
        val inset = width / 2
        val arc = Size(this.size.width - width, this.size.height - width)
        drawArc(Color(0x33FFFFFF), 0f, 360f, false, Offset(inset, inset), arc, style = Stroke(width))
        drawArc(
            Tonearm.accent, -90f, 360f * progress.coerceIn(0f, 1f), false, Offset(inset, inset), arc,
            style = Stroke(width, cap = StrokeCap.Round),
        )
    }
}

/**
 * A song row's download slot, beside its length: the filled download icon
 * once saved, a ring while it downloads, empty otherwise. Always the same
 * width, so a mark coming or going moves nothing.
 */
@Composable
fun DownloadMark(videoId: String) {
    val downloads = LocalDownloads.current
    Box(Modifier.padding(start = 8.dp).width(18.dp), contentAlignment = Alignment.Center) {
        val busy = downloads.busy[videoId]
        when {
            downloads.isSaved(videoId) -> Icon(Icons.download, size = 16.sp, filled = true, color = Tonearm.accent)
            busy != null -> DownloadRing(busy, 15.dp)
        }
    }
}

/**
 * A page's download button, in the row under its title beside the heart:
 * the plain icon to save every song, a ring of how many are saved while
 * they download, the filled accent icon once all are, as the desktop's
 * hero button.
 */
@Composable
fun HeaderDownload(songs: List<Item>, onDownload: () -> Unit) {
    val downloads = LocalDownloads.current
    val ids = songs.mapNotNull { it.videoId }
    if (ids.isEmpty()) return
    val saved = ids.count { it in downloads.saved }
    val busy = ids.any { it in downloads.busy }
    Box(
        Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onDownload),
        contentAlignment = Alignment.Center,
    ) {
        when {
            saved == ids.size -> Icon(Icons.download, size = 26.sp, filled = true, color = Tonearm.accent)
            busy -> {
                // Counts the song on its way as part done, so the ring moves
                // while each one downloads, not only as each lands.
                val partial = ids.sumOf { (downloads.busy[it] ?: 0f).toDouble() }.toFloat()
                DownloadRing((saved + partial) / ids.size, 24.dp, 2.5.dp)
            }
            else -> Icon(Icons.download, size = 26.sp, color = Color(0xB3FFFFFF))
        }
    }
}

/** Downloads' art: the download icon on a tile of the accent, as the
 *  desktop's icon tile for the Downloads page. */
@Composable
fun DownloadsTile(size: Dp, shape: Shape = RoundedCornerShape(size * 0.16f)) {
    Box(
        Modifier.size(size).clip(shape).background(
            Brush.linearGradient(listOf(Tonearm.accent, Color(0xFF7A5CFF))),
        ),
        contentAlignment = Alignment.Center,
    ) {
        Icon(Icons.download, size = (size.value * 0.46f).sp, filled = true, color = Tonearm.text)
    }
}

/**
 * The Downloads page, as the desktop's: every song saved on the phone,
 * newest first, as a playlist. Read from the phone alone, so it opens and
 * plays with no connection. Shuffle and the round play button as on any
 * page; a long press offers "Remove download".
 */
@Composable
fun DownloadsScreen(core: Core, actions: Actions, onBack: () -> Unit) {
    val downloads = LocalDownloads.current
    // Read again as songs land or go; the list keeps its place meanwhile.
    var songs by remember { mutableStateOf<List<Item>?>(null) }
    var summary by remember { mutableStateOf("") }
    LaunchedEffect(downloads.saved) {
        val (page, line) = withContext(Dispatchers.IO) { core.downloadsPage() to core.downloadsSummary() }
        songs = page.shelves.flatMap { it.items }.filter { it.videoId != null }
        summary = line
    }
    Box(Modifier.fillMaxSize()) {
        val list = songs
        val listState = rememberLazyListState()
        LazyColumn(Modifier.fillMaxSize(), state = listState, contentPadding = PaddingValues(bottom = 24.dp)) {
            item(contentType = "header") {
                Column(Modifier.fillMaxWidth().statusBarsPadding().padding(top = 56.dp)) {
                    Row(
                        Modifier.padding(horizontal = 16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        DownloadsTile(112.dp)
                        Column(Modifier.weight(1f).padding(start = 16.dp)) {
                            Title("Downloads", size = 30.sp, weight = FontWeight.ExtraBold)
                            Dim(summary.ifEmpty { " " }, 14.sp, modifier = Modifier.padding(top = 4.dp))
                        }
                    }
                    // As every page's row: shuffle and the round play button
                    // at the right end. Nothing to play, nothing shown, but
                    // the row keeps its height.
                    Row(
                        Modifier.fillMaxWidth().height(72.dp).padding(start = 4.dp, end = 16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Spacer(Modifier.weight(1f))
                        if (!list.isNullOrEmpty()) {
                            if (list.size > 1) HeroIcon(Icons.shuffle) { actions.playList(list.shuffled(), 0) }
                            Spacer(Modifier.width(8.dp))
                            RoundButton(Icons.play, accent = true, size = 56.dp) { actions.playList(list, 0) }
                        }
                    }
                }
            }
            when {
                list == null -> items(6) { RowSkeleton(it) }
                list.isEmpty() -> item {
                    Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) {
                        Dim("Download songs, albums and playlists from their menus or page headers to play them here, even offline.",
                            15.sp, lines = 4)
                    }
                }
                else -> itemsIndexed(list, key = { _, it -> it.videoId!! }, contentType = { _, _ -> "row" }) { i, item ->
                    ItemRow(
                        item,
                        playing = item.videoId == actions.playing,
                        // The page plays as a list, so it keeps playing
                        // offline: no radio that would need the network.
                        onClick = { actions.playList(list, i) },
                        onPlay = { actions.menu(item) },
                    )
                }
            }
        }
        StatusBar(listState.canScrollBackward)
        Box(
            Modifier.statusBarsPadding().padding(8.dp).size(40.dp).clip(RoundedCornerShape(20.dp))
                .background(Color(0x9908080A)).clickable(onClick = onBack),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.back, size = 20.sp)
        }
    }
}
