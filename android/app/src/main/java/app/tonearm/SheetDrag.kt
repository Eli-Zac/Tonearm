package app.tonearm

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Velocity
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * A sheet that rises from the bottom (a song's menu, the sleep timer,
 * Devices) and can be pulled down to close. It follows the finger down;
 * let go past a quarter of its height, or with a downward flick, and it
 * slides off and closes, otherwise it springs back. Pulling up does
 * nothing. A list inside scrolls first, and the sheet moves once it is at
 * its top.
 *
 * It also takes every touch that lands on it. Without that, a drag on an
 * empty part of the sheet fell through to the dimmed screen behind, whose
 * tap closed the sheet when the finger lifted, up or down.
 */
@Composable
fun DragToClose(onClose: () -> Unit, content: @Composable () -> Unit) {
    val scope = rememberCoroutineScope()
    // How far down the sheet has been pulled, in pixels.
    var y by remember { mutableFloatStateOf(0f) }
    var height by remember { mutableIntStateOf(0) }
    var settling by remember { mutableStateOf<Job?>(null) }
    val flick = with(LocalDensity.current) { 1000.dp.toPx() }

    fun pull(dy: Float) {
        settling?.cancel()
        y = (y + dy).coerceAtLeast(0f)
    }

    fun settle(speed: Float) {
        settling?.cancel()
        settling = scope.launch {
            if (y > 0f && (y > height / 4f || speed > flick)) {
                animate(y, height.toFloat(), speed, tween(180, easing = FastOutSlowInEasing)) { v, _ -> y = v }
                onClose()
            } else if (y > 0f) {
                animate(y, 0f, speed, spring(stiffness = 600f)) { v, _ -> y = v }
            }
        }
    }

    // A list inside the sheet scrolls first. Pulled down at its top, the
    // rest of the drag moves the sheet; pushed up with the sheet pulled
    // down, the sheet goes back up before the list scrolls.
    val connection = remember {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (available.y < 0f && y > 0f) {
                    val take = maxOf(available.y, -y)
                    pull(take)
                    return Offset(0f, take)
                }
                return Offset.Zero
            }

            override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset {
                if (available.y > 0f && source == NestedScrollSource.UserInput) {
                    pull(available.y)
                    return Offset(0f, available.y)
                }
                return Offset.Zero
            }

            override suspend fun onPreFling(available: Velocity): Velocity {
                if (y > 0f) {
                    settle(available.y)
                    return available
                }
                return Velocity.Zero
            }
        }
    }

    Box(
        Modifier
            .onSizeChanged { height = it.height }
            .graphicsLayer { translationY = y }
            .nestedScroll(connection)
            .draggable(
                rememberDraggableState { pull(it) },
                Orientation.Vertical,
                onDragStopped = { speed -> settle(speed) },
            )
            // Taps on the sheet itself stay on it.
            .pointerInput(Unit) { detectTapGestures { } },
    ) {
        content()
    }
}
