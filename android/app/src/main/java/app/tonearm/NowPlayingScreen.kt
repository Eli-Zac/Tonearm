package app.tonearm

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

/**
 * The song playing, full screen: its cover large over the same cover blown
 * up and blurred, as the desktop's player. Swipe down or press back to
 * leave it.
 */
@Composable
fun NowPlayingScreen(now: NowPlaying, controller: MediaController, actions: Actions, onClose: () -> Unit) {
    val cover = now.meta?.artworkUri?.toString()
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
        // The backdrop: the cover, blurred and darkened.
        AsyncImage(
            model = cover,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize().blur(80.dp),
            alpha = 0.55f,
        )
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(0f to Color(0x6608080A), 0.6f to Color(0xB308080A), 1f to Tonearm.bg),
            ),
        )
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
            Spacer(Modifier.weight(1f))
            AsyncImage(
                model = cover,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxWidth().aspectRatio(1f)
                    .clip(RoundedCornerShape(Tonearm.radius)).background(Tonearm.raised),
            )
            Spacer(Modifier.weight(1f))
            Column(Modifier.fillMaxWidth()) {
                Title(now.meta?.title?.toString() ?: "", size = 24.sp, weight = FontWeight.Bold, lines = 2)
                Spacer(Modifier.height(4.dp))
                Title(
                    if (now.loading) "Loading…" else now.meta?.artist?.toString() ?: "",
                    size = 17.sp, weight = FontWeight.Medium, color = Tonearm.dim,
                )
            }
            Spacer(Modifier.height(22.dp))
            SeekBar(now, controller)
            Spacer(Modifier.height(18.dp))
            Controls(now, controller)
            Spacer(Modifier.height(36.dp))
        }
    }
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
fun Controls(now: NowPlaying, controller: MediaController) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        val repeat = controller.repeatMode
        Box(
            Modifier.size(48.dp).clip(CircleShape).clickable {
                controller.repeatMode = when (repeat) {
                    Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
                    Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
                    else -> Player.REPEAT_MODE_OFF
                }
            },
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                if (repeat == Player.REPEAT_MODE_ONE) Icons.repeatOne else Icons.repeat,
                size = 22.sp,
                color = if (repeat == Player.REPEAT_MODE_OFF) Tonearm.dim else Tonearm.accent,
            )
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
        // Room for the queue and lyrics buttons to come.
        Spacer(Modifier.size(48.dp))
    }
}

fun clock(ms: Long): String {
    val s = ms.coerceAtLeast(0) / 1000
    return "%d:%02d".format(s / 60, s % 60)
}

private fun Modifier.onSizeChangedWidth(set: (Int) -> Unit) =
    this.onSizeChanged { set(it.width.coerceAtLeast(1)) }
