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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import kotlinx.coroutines.delay

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
                App(core, controller)
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

/** The song playing, in an island above the tabs, as on the desktop.
 *  Tapping it opens Now Playing. */
@Composable
fun MiniPlayer(now: NowPlaying, controller: MediaController, onOpen: () -> Unit) {
    val progress = if (now.duration > 0) (now.position.toFloat() / now.duration).coerceIn(0f, 1f) else 0f
    Column(
        Modifier
            .padding(horizontal = 8.dp, vertical = 6.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(Tonearm.radius))
            .background(Tonearm.island)
            .clickable(onClick = onOpen),
    ) {
        Row(Modifier.padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Art(now.meta?.artworkUri?.toString(), 44.dp)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Title(now.meta?.title?.toString() ?: "", size = 15.sp)
                Dim(
                    if (now.loading) "Loading…" else now.error ?: now.meta?.artist?.toString() ?: "",
                    13.sp,
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
