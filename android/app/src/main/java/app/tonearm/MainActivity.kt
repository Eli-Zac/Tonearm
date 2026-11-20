package app.tonearm

import android.content.ComponentName
import android.graphics.Color as AndroidColor
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import coil3.compose.AsyncImage
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Song

@UnstableApi
class MainActivity : ComponentActivity() {
    private var controllerFuture: ListenableFuture<MediaController>? = null
    /** The media session, as the screen sees it. Null until connected. */
    private var controller by mutableStateOf<MediaController?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val core = (application as TonearmApp).core
        // Light status bar icons on the dark background.
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
        )
        setContent {
            TonearmTheme {
                Box(Modifier.fillMaxSize().background(Tonearm.bg)) {
                    Screen(
                        search = { query -> withContext(Dispatchers.IO) { core.search(query) } },
                        controller = controller,
                    )
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        val token = SessionToken(this, ComponentName(this, PlaybackService::class.java))
        val future = MediaController.Builder(this, token).buildAsync()
        controllerFuture = future
        future.addListener({ controller = future.get() }, MoreExecutors.directExecutor())
    }

    override fun onStop() {
        controller = null
        controllerFuture?.let(MediaController::releaseFuture)
        controllerFuture = null
        super.onStop()
    }
}

/** What the screen shows of the player, kept up to date from the session. */
class NowPlaying(
    val meta: MediaMetadata?,
    val playing: Boolean,
    val loading: Boolean,
    val position: Long,
    val duration: Long,
    val error: String?,
    val index: Int,
)

@Composable
fun rememberNowPlaying(controller: MediaController?): NowPlaying? {
    var now by remember { mutableStateOf<NowPlaying?>(null) }
    LaunchedEffect(controller) {
        val c = controller ?: run { now = null; return@LaunchedEffect }
        fun read() {
            now = if (c.mediaItemCount == 0) null else NowPlaying(
                meta = c.mediaMetadata,
                playing = c.playWhenReady && c.playbackState != Player.STATE_ENDED,
                loading = c.playbackState == Player.STATE_BUFFERING,
                position = c.currentPosition,
                duration = c.duration,
                error = c.playerError?.message,
                index = c.currentMediaItemIndex,
            )
        }
        val listener = object : Player.Listener {
            override fun onEvents(player: Player, events: Player.Events) = read()
        }
        c.addListener(listener)
        try {
            while (true) {
                read()
                delay(250)
            }
        } finally {
            c.removeListener(listener)
        }
    }
    return now
}

@Composable
fun Screen(search: suspend (String) -> List<Song>, controller: MediaController?) {
    val scope = rememberCoroutineScope()
    val focus = LocalFocusManager.current
    var query by remember { mutableStateOf("") }
    var songs by remember { mutableStateOf<List<Song>>(emptyList()) }
    var searching by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    val now = rememberNowPlaying(controller)

    fun runSearch() {
        val words = query.trim()
        if (words.isEmpty()) return
        focus.clearFocus()
        searching = true
        message = null
        scope.launch {
            try {
                songs = search(words)
                if (songs.isEmpty()) message = "No songs found"
            } catch (e: Exception) {
                message = e.message ?: "Search failed"
            } finally {
                searching = false
            }
        }
    }

    fun play(index: Int) {
        val c = controller ?: return
        c.setMediaItems(songs.map(TonearmPlayer::mediaItem), index, 0)
        c.play()
    }

    Column(Modifier.fillMaxSize().systemBarsPadding()) {
        SearchBox(query, { query = it }, ::runSearch)
        if (searching) {
            LinearProgressIndicator(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp).height(2.dp),
                color = Tonearm.accent,
                trackColor = Tonearm.line,
            )
        }
        val shown = message ?: now?.error
        shown?.let {
            Text(it, color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = 14.sp,
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
        }
        val playingId = now?.meta?.let { controller?.currentMediaItem?.mediaId }
        LazyColumn(Modifier.weight(1f), contentPadding = PaddingValues(vertical = 4.dp)) {
            itemsIndexed(songs, key = { i, s -> s.videoId + i }) { i, song ->
                SongRow(song, playing = song.videoId == playingId) { play(i) }
            }
        }
        if (now != null && controller != null) MiniPlayer(now, controller)
    }
}

@Composable
fun SearchBox(query: String, onChange: (String) -> Unit, onSearch: () -> Unit) {
    Row(
        Modifier
            .padding(horizontal = 16.dp, vertical = 12.dp)
            .fillMaxWidth()
            .height(48.dp)
            .clip(RoundedCornerShape(Tonearm.radius))
            .background(Tonearm.island)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.search, color = Tonearm.dim, size = 20.sp)
        Spacer(Modifier.width(12.dp))
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) {
                Text("Search songs", color = Tonearm.faint, fontFamily = Tonearm.font, fontSize = 16.sp)
            }
            BasicTextField(
                value = query,
                onValueChange = onChange,
                singleLine = true,
                textStyle = TextStyle(color = Tonearm.text, fontFamily = Tonearm.font, fontSize = 16.sp),
                cursorBrush = SolidColor(Tonearm.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { onSearch() }),
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
fun SongRow(song: Song, playing: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AsyncImage(
            model = song.cover,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.size(50.dp).clip(RoundedCornerShape(8.dp)).background(Tonearm.raised),
        )
        Column(Modifier.weight(1f).padding(start = 14.dp)) {
            Text(
                song.title,
                color = if (playing) Tonearm.accent else Tonearm.text,
                fontFamily = Tonearm.font,
                fontWeight = FontWeight.SemiBold,
                fontSize = 16.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                listOf(song.artist, song.album).filter { it.isNotEmpty() }.joinToString(" • "),
                color = Tonearm.dim,
                fontFamily = Tonearm.font,
                fontSize = 14.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (song.duration.isNotEmpty()) {
            Text(song.duration, color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = 13.sp,
                modifier = Modifier.padding(start = 8.dp))
        }
    }
}

/** The song playing, in an island along the bottom, as on the desktop. */
@Composable
fun MiniPlayer(now: NowPlaying, controller: MediaController) {
    val progress = if (now.duration > 0) (now.position.toFloat() / now.duration).coerceIn(0f, 1f) else 0f
    Column(
        Modifier
            .padding(horizontal = 8.dp, vertical = 6.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(Tonearm.radius))
            .background(Tonearm.island),
    ) {
        Row(Modifier.padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            AsyncImage(
                model = now.meta?.artworkUri,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.size(44.dp).clip(RoundedCornerShape(8.dp)).background(Tonearm.raised),
            )
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(
                    now.meta?.title?.toString() ?: "",
                    color = Tonearm.text, fontFamily = Tonearm.font, fontWeight = FontWeight.SemiBold,
                    fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                Text(
                    if (now.loading) "Loading…" else now.meta?.artist?.toString() ?: "",
                    color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = 13.sp,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            IconButton(Icons.prev) { controller.seekToPrevious() }
            IconButton(if (now.playing) Icons.pause else Icons.play, size = 30) {
                if (now.playing) controller.pause() else controller.play()
            }
            IconButton(Icons.next) { controller.seekToNext() }
        }
        // The progress line along the island's foot.
        Box(Modifier.fillMaxWidth().height(2.dp).background(Tonearm.line)) {
            Box(Modifier.fillMaxWidth(progress).fillMaxHeight().background(Tonearm.text))
        }
    }
}

@Composable
fun IconButton(glyph: String, size: Int = 22, onClick: () -> Unit) {
    Box(
        Modifier.size(44.dp).clip(RoundedCornerShape(22.dp)).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(glyph, size = size.sp, filled = true)
    }
}
