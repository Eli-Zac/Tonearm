package app.tonearm

import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.tonearm_android.CastKind
import uniffi.tonearm_android.CastState

// The desktop's cast icons (`CastIcon` and `CastLine` in ui/app.slint), in a
// 24 unit box. The icon font has none of them.
private const val CAST_SHAPE =
    "M 2 8 L 2 4 L 22 4 L 22 20 L 14 20 L 14 18.5 L 20.5 18.5 L 20.5 5.5 L 3.5 5.5 L 3.5 8 Z M 2 17.8 A 2.2 2.2 0 0 1 4.2 20 L 2 20 Z M 2 13.5 A 6.5 6.5 0 0 1 8.5 20 L 7 20 A 5 5 0 0 0 2 15 Z M 2 9.5 A 10.5 10.5 0 0 1 12.5 20 L 11 20 A 9 9 0 0 0 2 11 Z"
private const val SPEAKER =
    "M 8 3 L 16 3 A 2 2 0 0 1 18 5 L 18 19 A 2 2 0 0 1 16 21 L 8 21 A 2 2 0 0 1 6 19 L 6 5 A 2 2 0 0 1 8 3 Z M 9 14.5 A 3 3 0 1 0 15 14.5 A 3 3 0 1 0 9 14.5 M 11 8 A 1 1 0 1 0 13 8 A 1 1 0 1 0 11 8"
private const val TV =
    "M 4 4 L 20 4 A 2 2 0 0 1 22 6 L 22 15 A 2 2 0 0 1 20 17 L 4 17 A 2 2 0 0 1 2 15 L 2 6 A 2 2 0 0 1 4 4 Z M 8 21 L 16 21 M 12 17 L 12 21"
private const val GROUP =
    "M 4 4 L 9 4 A 1.5 1.5 0 0 1 10.5 5.5 L 10.5 18.5 A 1.5 1.5 0 0 1 9 20 L 4 20 A 1.5 1.5 0 0 1 2.5 18.5 L 2.5 5.5 A 1.5 1.5 0 0 1 4 4 Z M 15 4 L 20 4 A 1.5 1.5 0 0 1 21.5 5.5 L 21.5 18.5 A 1.5 1.5 0 0 1 20 20 L 15 20 A 1.5 1.5 0 0 1 13.5 18.5 L 13.5 5.5 A 1.5 1.5 0 0 1 15 4 Z M 4.5 14 A 2 2 0 1 0 8.5 14 A 2 2 0 1 0 4.5 14 M 15.5 14 A 2 2 0 1 0 19.5 14 A 2 2 0 1 0 15.5 14"
// This phone, drawn like the others.
private const val PHONE =
    "M 9 3 L 15 3 A 2 2 0 0 1 17 5 L 17 19 A 2 2 0 0 1 15 21 L 9 21 A 2 2 0 0 1 7 19 L 7 5 A 2 2 0 0 1 9 3 Z M 10.5 18 L 13.5 18"

/** The cast button's mark: a screen with the corner waves. */
@Composable
fun CastMark(color: Color, modifier: Modifier = Modifier) {
    val path = remember { PathParser().parsePathString(CAST_SHAPE).toPath() }
    Canvas(modifier.size(22.dp)) {
        scale(size.width / 24f, pivot = Offset.Zero) { drawPath(path, color) }
    }
}

/** What a device is drawn as; null is this phone. */
@Composable
private fun DeviceMark(kind: CastKind?, color: Color) {
    val path = remember(kind) {
        PathParser().parsePathString(
            when (kind) {
                CastKind.SPEAKER -> SPEAKER
                CastKind.SCREEN -> TV
                CastKind.GROUP -> GROUP
                null -> PHONE
            },
        ).toPath()
    }
    Canvas(Modifier.size(22.dp)) {
        scale(size.width / 24f, pivot = Offset.Zero) {
            drawPath(path, color, style = Stroke(1.6f, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}

/** What the phone is called to a speaker: the name set in Settings, else
 *  the model. */
fun phoneName(context: Context): String {
    val named = runCatching { Settings.Global.getString(context.contentResolver, "device_name") }.getOrNull()
    return named?.takeIf { it.isNotBlank() } ?: Build.MODEL
}

/**
 * The Devices sheet, as the desktop's: where the music plays now, the
 * volume while it plays on a device, and the other devices to move it to.
 * Rises from the bottom as an item's menu does.
 */
@Composable
fun CastSheet(
    cast: CastState?,
    onPick: (Int) -> Unit,
    onVolume: (Float) -> Unit,
) {
    val devices = cast?.devices.orEmpty()
    val active = cast?.active ?: -1
    val casting = active in devices.indices
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp))
            .background(Tonearm.raised)
            .navigationBarsPadding()
            .padding(top = 10.dp),
    ) {
        Box(
            Modifier.align(Alignment.CenterHorizontally).size(width = 36.dp, height = 4.dp)
                .clip(RoundedCornerShape(2.dp)).background(Tonearm.line),
        )
        Column(Modifier.heightIn(max = 560.dp).verticalScroll(rememberScrollState())) {
            Title("Devices", size = 20.sp, weight = FontWeight.Bold, modifier = Modifier.padding(horizontal = 20.dp, vertical = 14.dp))
            Label("Playing on")
            if (casting) {
                CastLine(
                    devices[active].name,
                    if (cast?.connecting == true) "Connecting…" else "Casting from this phone",
                    devices[active].kind, true,
                ) {}
                Volume(cast?.volume ?: 0f, onVolume)
            } else {
                CastLine("This phone", Build.MODEL, null, true) {}
            }
            Label("Other devices", top = 14.dp)
            val others = devices.indices.filter { it != active }
            if (!casting && others.isEmpty()) {
                Dim(
                    "Looking for speakers and TVs on your network…", 14.sp, lines = 2,
                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
                )
            }
            if (casting) CastLine("This phone", Build.MODEL, null, false) { onPick(-1) }
            others.forEach { i ->
                val device = devices[i]
                CastLine(
                    device.name,
                    if (device.kind == CastKind.GROUP) "Speaker group" else device.model,
                    device.kind, false,
                ) { onPick(i) }
            }
            Dim(
                "Devices on the same Wi-Fi as this phone.", 12.sp, lines = 2,
                modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 14.dp, bottom = 18.dp),
            )
        }
    }
}

@Composable
private fun Label(text: String, top: androidx.compose.ui.unit.Dp = 0.dp) {
    Dim(text, 12.sp, modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = top, bottom = 6.dp))
}

/** A device, or this phone: its mark, name and what it is, with a dot
 *  while it is the one playing. */
@Composable
private fun CastLine(title: String, subtitle: String, kind: CastKind?, active: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(8.dp))
            .then(if (active) Modifier else Modifier.clickable(onClick = onClick))
            .heightIn(min = 56.dp)
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        DeviceMark(kind, if (active) Tonearm.accent else Tonearm.dim)
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Title(
                title, size = 14.sp,
                color = if (active) Tonearm.accent else Tonearm.text,
                weight = if (active) FontWeight.SemiBold else FontWeight.Normal,
            )
            if (subtitle.isNotEmpty()) Dim(subtitle, 12.sp)
        }
        if (active) Box(Modifier.size(6.dp).clip(CircleShape).background(Tonearm.accent))
    }
}

/** The device's volume: a thin bar to drag or tap, in the seek bar's way.
 *  The level is sent as it moves, not faster than the device can take it. */
@Composable
private fun Volume(level: Float, onChange: (Float) -> Unit) {
    var dragging by remember { mutableStateOf<Float?>(null) }
    // Where the last drag left it, until the device's own level catches up.
    var set by remember { mutableStateOf<Float?>(null) }
    var width by remember { mutableIntStateOf(1) }
    val sent = remember { longArrayOf(0L) }
    LaunchedEffect(level) { if (dragging == null) set = null }
    val shown = dragging ?: set ?: level
    fun send(v: Float, force: Boolean) {
        val now = System.currentTimeMillis()
        if (force || now - sent[0] > 150) {
            sent[0] = now
            onChange(v)
        }
    }
    Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(Icons.speakerHigh, size = 20.sp, color = Tonearm.dim)
        Box(
            Modifier
                .weight(1f)
                .padding(start = 14.dp)
                .height(36.dp)
                .onSizeChanged { width = it.width.coerceAtLeast(1) }
                .pointerInput(Unit) {
                    detectTapGestures { offset ->
                        val v = (offset.x / width).coerceIn(0f, 1f)
                        set = v
                        send(v, true)
                    }
                }
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragStart = { dragging = (it.x / width).coerceIn(0f, 1f) },
                        onDragEnd = {
                            dragging?.let { v -> set = v; send(v, true) }
                            dragging = null
                        },
                        onDragCancel = { dragging = null },
                    ) { change, _ ->
                        val v = (change.position.x / width).coerceIn(0f, 1f)
                        dragging = v
                        send(v, false)
                    }
                },
            contentAlignment = Alignment.CenterStart,
        ) {
            Box(Modifier.fillMaxWidth().height(3.dp).clip(RoundedCornerShape(2.dp)).background(Tonearm.line)) {
                Box(Modifier.fillMaxWidth(shown.coerceIn(0f, 1f)).fillMaxHeight().background(Tonearm.text))
            }
        }
    }
}
