package app.tonearm

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.PlayState
import uniffi.tonearm_android.Playback
import uniffi.tonearm_android.Song
import uniffi.tonearm_android.Tonearm

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        TonearmNative.init(applicationContext)
        val core = Core.get(filesDir.absolutePath)
        enableEdgeToEdge()
        setContent {
            MaterialTheme(colorScheme = darkColorScheme(background = Ground, surface = Ground)) {
                Surface(Modifier.fillMaxSize(), color = Ground) {
                    Player(core)
                }
            }
        }
    }
}

/** The core, started once for the app's life. */
object Core {
    private var tonearm: Tonearm? = null

    fun get(home: String): Tonearm = tonearm ?: Tonearm(home).also { tonearm = it }
}

private val Ground = Color(0xFF0B0B0F)
private val Soft = Color(0xB8FFFFFF)

@Composable
fun Player(core: Tonearm) {
    val scope = rememberCoroutineScope()
    val focus = LocalFocusManager.current
    var query by remember { mutableStateOf("") }
    var songs by remember { mutableStateOf<List<Song>>(emptyList()) }
    var searching by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var current by remember { mutableStateOf<Song?>(null) }
    var playback by remember { mutableStateOf<Playback?>(null) }

    // The engine keeps its own state. Read it a few times a second.
    LaunchedEffect(core) {
        while (true) {
            playback = withContext(Dispatchers.Default) { core.playback() }
            delay(250)
        }
    }

    fun search() {
        val words = query.trim()
        if (words.isEmpty()) return
        focus.clearFocus()
        searching = true
        message = null
        scope.launch {
            try {
                songs = withContext(Dispatchers.IO) { core.search(words) }
                if (songs.isEmpty()) message = "No songs found"
            } catch (e: Exception) {
                message = e.message ?: "Search failed"
            } finally {
                searching = false
            }
        }
    }

    fun play(song: Song) {
        current = song
        message = null
        scope.launch {
            try {
                withContext(Dispatchers.IO) { core.play(song.videoId) }
            } catch (e: Exception) {
                message = e.message ?: "Couldn't play ${song.title}"
            }
        }
    }

    Column(Modifier.fillMaxSize().systemBarsPadding()) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            modifier = Modifier.fillMaxWidth().padding(16.dp),
            placeholder = { Text("Search songs") },
            leadingIcon = { Icon(Icons.Rounded.Search, contentDescription = null) },
            singleLine = true,
            shape = RoundedCornerShape(28.dp),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { search() }),
        )
        if (searching) LinearProgressIndicator(Modifier.fillMaxWidth().padding(horizontal = 16.dp))
        message?.let {
            Text(it, color = Soft, modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
        }
        LazyColumn(Modifier.weight(1f)) {
            items(songs, key = { it.videoId }) { song ->
                SongRow(song, playing = song.videoId == current?.videoId) { play(song) }
            }
        }
        current?.let { NowPlaying(it, playback, core) }
    }
}

@Composable
fun SongRow(song: Song, playing: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AsyncImage(
            model = song.cover,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.size(52.dp).clip(RoundedCornerShape(6.dp)).background(Color(0xFF222222)),
        )
        Column(Modifier.weight(1f).padding(start = 14.dp)) {
            Text(
                song.title,
                fontWeight = FontWeight.SemiBold,
                color = if (playing) MaterialTheme.colorScheme.primary else Color.White,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                listOf(song.artist, song.album).filter { it.isNotEmpty() }.joinToString(" • "),
                color = Soft,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (song.duration.isNotEmpty()) {
            Text(song.duration, color = Soft, style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
fun NowPlaying(song: Song, playback: Playback?, core: Tonearm) {
    val state = playback?.state ?: PlayState.LOADING
    val duration = playback?.durationMs?.toLong() ?: 0L
    val position = playback?.positionMs?.toLong() ?: 0L
    Column(Modifier.fillMaxWidth().background(Color(0xFF16161C))) {
        LinearProgressIndicator(
            progress = { if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f },
            modifier = Modifier.fillMaxWidth().height(2.dp),
            drawStopIndicator = {},
        )
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            AsyncImage(
                model = song.cover,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.size(44.dp).clip(RoundedCornerShape(6.dp)),
            )
            Column(Modifier.weight(1f).padding(start = 12.dp)) {
                Text(song.title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    when (state) {
                        PlayState.LOADING -> "Loading…"
                        PlayState.FAILED -> playback?.error?.ifEmpty { null } ?: "Couldn't play"
                        else -> "${clock(position)} / ${clock(duration)}"
                    },
                    color = Soft,
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            val playing = state == PlayState.PLAYING
            IconButton(onClick = { if (playing) core.pause() else core.resume() }) {
                Icon(
                    if (playing) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                    contentDescription = if (playing) "Pause" else "Play",
                    tint = Color.White,
                )
            }
        }
    }
}

private fun clock(ms: Long): String {
    val s = ms / 1000
    return "%d:%02d".format(s / 60, s % 60)
}
