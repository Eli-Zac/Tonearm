package app.tonearm

import android.net.Uri
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.compose.PlayerSurface
import androidx.media3.ui.compose.SURFACE_TYPE_TEXTURE_VIEW
import java.io.File

/**
 * The album's moving cover from Apple Music, over the still one, as the
 * desktop's Now playing shows it: a short clip looping, silent. It fades in
 * over 450 ms once its first frame is drawn (the desktop's `motion-t`), so
 * the still cover never flashes away to nothing.
 *
 * Its own small player, apart from the music's: audio is switched off, so
 * it never decodes sound or asks for audio focus. It is let go when the
 * cover leaves the screen and pauses while the app is out of sight.
 */
@Composable
fun MotionCover(path: String, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val player = remember(path) {
        ExoPlayer.Builder(context).build().apply {
            trackSelectionParameters = trackSelectionParameters.buildUpon()
                .setTrackTypeDisabled(C.TRACK_TYPE_AUDIO, true)
                .build()
            volume = 0f
            repeatMode = Player.REPEAT_MODE_ONE
            setMediaItem(MediaItem.fromUri(Uri.fromFile(File(path))))
            prepare()
            playWhenReady = true
        }
    }
    var shown by remember(player) { mutableStateOf(false) }
    DisposableEffect(player) {
        val listener = object : Player.Listener {
            override fun onRenderedFirstFrame() {
                shown = true
            }
        }
        player.addListener(listener)
        onDispose {
            player.removeListener(listener)
            player.release()
        }
    }
    // Out of sight, the clip waits.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle, player) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_STOP -> player.pause()
                Lifecycle.Event.ON_START -> player.play()
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    val alpha by animateFloatAsState(
        if (shown) 1f else 0f,
        tween(450, easing = FastOutSlowInEasing),
        label = "motion",
    )
    // A texture view, so it clips to the cover's rounded corners and fades.
    PlayerSurface(
        player,
        modifier = modifier.graphicsLayer { this.alpha = alpha },
        surfaceType = SURFACE_TYPE_TEXTURE_VIEW,
    )
}
