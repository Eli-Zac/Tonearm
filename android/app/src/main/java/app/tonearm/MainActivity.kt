package app.tonearm

import android.content.ComponentName
import android.graphics.Color as AndroidColor
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
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
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

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
        finder = CastFinder(this, core)
        setContent {
            TonearmTheme {
                App(core, controller)
            }
        }
    }

    /** Looks for Cast devices while the app is on screen. */
    private var finder: CastFinder? = null

    /** Android 17 apps need the user's say to reach devices on the
     *  network, which Cast discovery does. The finder starts once it is
     *  answered, or at once where nothing is asked. */
    private val localNetwork = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        finder?.start()
    }

    override fun onStart() {
        super.onStart()
        if (Build.VERSION.SDK_INT >= 37 &&
            ContextCompat.checkSelfPermission(this, LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED
        ) {
            localNetwork.launch(LOCAL_NETWORK)
        } else {
            finder?.start()
        }
        val token = SessionToken(this, ComponentName(this, PlaybackService::class.java))
        val future = MediaController.Builder(this, token).buildAsync()
        controllerFuture = future
        future.addListener({ controller = future.get() }, MoreExecutors.directExecutor())
    }

    override fun onStop() {
        finder?.stop()
        controller = null
        controllerFuture?.let(MediaController::releaseFuture)
        controllerFuture = null
        super.onStop()
    }
}

private const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"

/** What the screen shows of the player, kept up to date from the session. */
class NowPlaying(
    /** The song's video id. */
    val id: String?,
    val meta: MediaMetadata?,
    val playing: Boolean,
    val loading: Boolean,
    val position: Long,
    val duration: Long,
    val error: String?,
    val index: Int,
)

@Composable
fun rememberNowPlaying(controller: MediaController?, core: uniffi.tonearm_android.Tonearm): NowPlaying? {
    var now by remember { mutableStateOf<NowPlaying?>(null) }
    LaunchedEffect(controller) {
        val c = controller ?: run { now = null; return@LaunchedEffect }
        fun read() {
            now = if (c.mediaItemCount == 0) null else NowPlaying(
                id = c.currentMediaItem?.mediaId,
                meta = c.mediaMetadata,
                playing = c.playWhenReady && c.playbackState != Player.STATE_ENDED,
                loading = c.playbackState == Player.STATE_BUFFERING,
                position = c.currentPosition,
                duration = c.duration,
                // From the core: the player keeps errors from Media3.
                error = core.playback().takeIf { it.state == uniffi.tonearm_android.PlayState.FAILED }
                    ?.error?.ifEmpty { "Couldn't play this song" },
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
 *  Tapping it opens Now Playing. Swiping it left skips to the next song and
 *  right goes back to the one before, as the buttons do. */
@Composable
fun MiniPlayer(now: NowPlaying, controller: MediaController, onOpen: () -> Unit) {
    val progress = if (now.duration > 0) (now.position.toFloat() / now.duration).coerceIn(0f, 1f) else 0f
    // How far the island has been dragged. It follows the finger, then
    // springs back; a long enough drag also skips.
    val drag = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val reach = with(LocalDensity.current) { 56.dp.toPx() }
    Column(
        Modifier
            .padding(horizontal = 8.dp, vertical = 6.dp)
            .offset { IntOffset(drag.value.roundToInt(), 0) }
            .fillMaxWidth()
            .clip(RoundedCornerShape(Tonearm.radius))
            .background(Tonearm.island)
            .pointerInput(controller) {
                detectHorizontalDragGestures(
                    onDragStart = { scope.launch { drag.stop() } },
                    onDragEnd = {
                        val moved = drag.value
                        if (moved <= -reach) controller.seekToNext()
                        else if (moved >= reach) controller.seekToPrevious()
                        scope.launch { drag.animateTo(0f) }
                    },
                    onDragCancel = { scope.launch { drag.animateTo(0f) } },
                    onHorizontalDrag = { change, amount ->
                        change.consume()
                        // Resists a little: it is a nudge, not a slide away.
                        scope.launch { drag.snapTo(drag.value + amount * 0.6f) }
                    },
                )
            }
            .clickable(onClick = onOpen),
    ) {
        Row(Modifier.padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Art(now.meta?.artworkUri?.toString(), 44.dp)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Title(now.meta?.title?.toString() ?: "", size = 15.sp)
                Dim(
                    now.error ?: now.meta?.artist?.toString() ?: "",
                    13.sp,
                )
            }
            IconButton(Icons.rewind) { controller.seekToPrevious() }
            IconButton(if (now.playing) Icons.pause else Icons.play, size = 26) {
                if (now.playing) controller.pause() else controller.play()
            }
            IconButton(Icons.fastForward) { controller.seekToNext() }
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
        Modifier.size((size * 1.5f).coerceAtLeast(44f).dp).clip(CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(glyph, size = size.sp, filled = true)
    }
}
