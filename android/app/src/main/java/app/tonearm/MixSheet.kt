package app.tonearm

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.MixEditorChoice
import uniffi.tonearm_android.MixEditorData
import uniffi.tonearm_android.MixSettings
import uniffi.tonearm_android.Tonearm as Core

/**
 * The desktop's transition editor, as a bottom sheet on a phone. Its
 * picture and choices are plain data from tonearm-core.
 */
@Composable
fun MixSheet(
    from: Item,
    to: Item,
    core: Core,
    onClose: () -> Unit,
    onError: (String) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val mutex = remember { Mutex() }
    var data by remember(from.videoId, to.videoId) { mutableStateOf<MixEditorData?>(null) }
    var settings by remember(from.videoId, to.videoId) { mutableStateOf<MixSettings?>(null) }
    var previewing by remember(from.videoId, to.videoId) { mutableStateOf(false) }
    var previewAt by remember(from.videoId, to.videoId) { mutableFloatStateOf(-1f) }

    LaunchedEffect(from.videoId, to.videoId) {
        try {
            val initial = withContext(Dispatchers.IO) { core.mixEditor(from, to) }
            settings = initial.settings
            data = initial
        } catch (e: Exception) {
            onError(e.message ?: "Couldn't open the transition")
        }
    }

    LaunchedEffect(data?.from?.analyzing, data?.to?.analyzing) {
        while (data?.from?.analyzing == true || data?.to?.analyzing == true) {
            delay(750)
            try {
                val current = settings ?: break
                data = withContext(Dispatchers.IO) { core.mixEditorView(from, to, current) }
            } catch (e: Exception) {
                onError(e.message ?: "Couldn't load song analysis")
                break
            }
        }
    }

    LaunchedEffect(previewing) {
        while (previewing) {
            val state = try {
                withContext(Dispatchers.IO) { core.mixPreviewActive() to core.mixPreviewPosition() }
            } catch (e: Exception) {
                onError(e.message ?: "Couldn't check the preview")
                null
            }
            if (state?.first != true) {
                previewing = false
                previewAt = -1f
            } else {
                previewAt = state.second ?: -1f
            }
            delay(60)
        }
        previewAt = -1f
    }

    fun update(change: suspend (MixSettings) -> MixSettings) {
        scope.launch {
            mutex.withLock {
                val old = settings ?: return@withLock
                try {
                    val next = withContext(Dispatchers.IO) { change(old) }
                    settings = next
                    data = withContext(Dispatchers.IO) { core.mixEditorView(from, to, next) }
                    if (previewing) {
                        withContext(Dispatchers.IO) { core.previewTransition(from, to, next) }
                    }
                } catch (e: Exception) {
                    onError(e.message ?: "Couldn't update the transition")
                }
            }
        }
    }

    Column(
        Modifier
            .fillMaxWidth()
            .heightIn(max = 760.dp)
            .clip(RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp))
            .background(Tonearm.raised)
            .navigationBarsPadding()
            .padding(top = 10.dp),
    ) {
        Box(
            Modifier.align(Alignment.CenterHorizontally).size(width = 36.dp, height = 4.dp)
                .clip(RoundedCornerShape(2.dp)).background(Tonearm.line),
        )
        Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp)) {
            Row(
                Modifier.fillMaxWidth().height(46.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Title("Edit transition", size = 18.sp, weight = FontWeight.Bold)
                Spacer(Modifier.weight(1f))
                PillButton("Save", primary = data?.changed == true) {
                    val current = settings ?: return@PillButton
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { core.setTransition(from, to, current) }
                            onClose()
                        } catch (e: Exception) {
                            onError(e.message ?: "Couldn't save the transition")
                        }
                    }
                }
                Box(
                    Modifier.size(36.dp).clip(CircleShape).clickable(onClick = onClose),
                    contentAlignment = Alignment.Center,
                ) {
                    Title("×", size = 25.sp, color = Tonearm.dim, weight = FontWeight.Normal)
                }
            }

            SongLine(
                from, data?.from?.bpm, data?.from?.camelot, data?.from?.keyColor,
                data == null || data?.from?.analyzing == true, data?.whyUnmatched,
            )
            Spacer(Modifier.height(12.dp))

            val picture = data?.picture
            if (picture == null) {
                Bone(Modifier.fillMaxWidth().aspectRatio(340f / 230f), RoundedCornerShape(12.dp))
            } else {
                MixPictureView(
                    picture = picture,
                    playhead = previewAt,
                    onDrag = { side, delta, exact ->
                        update { current -> core.mixEditorDrag(from, to, current, side.toUInt(), delta, exact) }
                    },
                )
            }

            Spacer(Modifier.height(12.dp))
            SongLine(
                to, data?.to?.bpm, data?.to?.camelot, data?.to?.keyColor,
                data == null || data?.to?.analyzing == true, data?.whyUnmatched,
            )
            Box(Modifier.fillMaxWidth().height(18.dp), contentAlignment = Alignment.CenterStart) {
                if (!data?.whyUnmatched.isNullOrEmpty()) {
                    Title(data!!.whyUnmatched, size = 11.sp, color = Color(0xFFF5A524), weight = FontWeight.Medium)
                }
            }
            Spacer(Modifier.height(10.dp))
            Dim("Presets", 12.sp, modifier = Modifier.fillMaxWidth(), lines = 1)
            Spacer(Modifier.height(6.dp))
            if (data == null) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    repeat(4) { Bone(Modifier.width(68.dp).height(36.dp), RoundedCornerShape(10.dp)) }
                }
            } else {
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    data!!.presets.forEachIndexed { index, name ->
                        val selected = data!!.presetSelected == index
                        Row(
                            Modifier.height(36.dp).clip(RoundedCornerShape(10.dp))
                                .background(if (selected) Color(0xFFF0F0F3) else Color(0x14FFFFFF))
                                .clickable {
                                    update { current -> core.mixEditorPreset(from, to, current, index.toUInt()) }
                                }
                                .padding(horizontal = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Title(
                                name, size = 14.sp, color = if (selected) Color(0xFF08080A) else Tonearm.text,
                                weight = FontWeight.SemiBold,
                            )
                        }
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            if (data == null) {
                repeat(8) { Bone(Modifier.fillMaxWidth().height(52.dp), RoundedCornerShape(12.dp)); Spacer(Modifier.height(8.dp)) }
                Spacer(Modifier.height(6.dp))
            } else {
                data!!.choices.forEachIndexed { index, choice ->
                    ChoiceRow(choice) { picked ->
                        update { current -> core.mixEditorChoice(current, index.toUInt(), picked.toUInt()) }
                    }
                    Spacer(Modifier.height(8.dp))
                }
                ToggleRow("Beatmatching", data!!.beatmatch, data!!.canBeatmatch) {
                    update { current -> core.mixEditorBeatmatch(current, data!!.canBeatmatch) }
                }
                Spacer(Modifier.height(8.dp))
                ToggleRow("Key lock", data!!.keyLock, data!!.canBeatmatch && data!!.beatmatch) {
                    update { current -> core.mixEditorKeyLock(current, data!!.canBeatmatch) }
                }
                Spacer(Modifier.height(14.dp))
            }
        }
        Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 10.dp)) {
            PillButton(if (previewing) "Stop preview" else "Preview", onClick = {
                val current = settings ?: return@PillButton
                scope.launch {
                    try {
                        if (previewing) {
                            withContext(Dispatchers.IO) { core.pause() }
                            previewing = false
                        } else {
                            withContext(Dispatchers.IO) { core.previewTransition(from, to, current) }
                            previewing = true
                        }
                    } catch (e: Exception) {
                        onError(e.message ?: "Couldn't preview the transition")
                    }
                }
            })
        }
    }
}

@Composable
private fun SongLine(
    item: Item,
    bpm: String?,
    camelot: String?,
    keyColor: UInt?,
    analyzing: Boolean,
    warning: String?,
) {
    Row(Modifier.fillMaxWidth().height(32.dp), verticalAlignment = Alignment.CenterVertically) {
        Art(item.thumb, 32.dp, RoundedCornerShape(4.dp))
        Spacer(Modifier.width(10.dp))
        Title(
            item.title + if (item.artist.isNotEmpty()) "  •  ${item.artist}" else "",
            size = 14.sp, modifier = Modifier.weight(1f),
        )
        if (bpm.isNullOrEmpty()) {
            if (analyzing) Bone(Modifier.width(48.dp).height(12.dp)) else Spacer(Modifier.width(48.dp))
        } else {
            Title(
                bpm, size = 13.sp, color = if (!warning.isNullOrEmpty()) Color(0xFFF5A524) else Tonearm.dim,
                weight = if (!warning.isNullOrEmpty()) FontWeight.SemiBold else FontWeight.Normal,
            )
        }
        Spacer(Modifier.width(8.dp))
        if (camelot.isNullOrEmpty()) {
            if (analyzing) Bone(Modifier.width(24.dp).height(14.dp)) else Spacer(Modifier.width(24.dp))
        } else {
            Box(
                Modifier.clip(RoundedCornerShape(5.dp))
                    .background(Color((keyColor ?: 0xFF777777u).toInt()))
                    .padding(horizontal = 5.dp, vertical = 2.dp),
            ) {
                Title(camelot, size = 12.sp, color = Color(0xFF101014), weight = FontWeight.Bold)
            }
        }
    }
}

@Composable
private fun ChoiceRow(choice: MixEditorChoice, onPick: (Int) -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(52.dp).clip(RoundedCornerShape(12.dp))
            .background(Color(0x0AFFFFFF)).padding(start = 14.dp, end = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(3.dp).height(16.dp).clip(RoundedCornerShape(2.dp)).background(Color(choice.accent.toInt())))
        Title(choice.title, size = 15.sp, modifier = Modifier.weight(1f).padding(start = 9.dp))
        ChoiceDropdown(choice.title, choice.options, choice.selected.toInt(), onPick)
    }
}

@Composable
private fun ToggleRow(title: String, on: Boolean, enabled: Boolean, onToggle: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(52.dp).clip(RoundedCornerShape(12.dp))
            .background(Color(0x0AFFFFFF)).clickable(enabled = enabled, onClick = onToggle)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Title(title, size = 15.sp, color = if (enabled) Tonearm.text else Tonearm.dim, modifier = Modifier.weight(1f))
        Box(
            Modifier.width(42.dp).height(24.dp).clip(RoundedCornerShape(12.dp))
                .background(if (on && enabled) Tonearm.accent else Color(0x30FFFFFF)),
            contentAlignment = if (on && enabled) Alignment.CenterEnd else Alignment.CenterStart,
        ) {
            Box(Modifier.padding(horizontal = 3.dp).size(18.dp).clip(CircleShape).background(Color.White))
        }
    }
}

@Composable
private fun MixPictureView(
    picture: uniffi.tonearm_android.MixPicture,
    playhead: Float,
    onDrag: (Int, Double, Boolean) -> Unit,
) {
    var size by remember { mutableStateOf(IntSize.Zero) }
    var dragSide by remember { mutableIntStateOf(-1) }
    var dragPixels by remember { mutableFloatStateOf(0f) }
    val scaleFactor = size.width.coerceAtLeast(1).toFloat() / 340f
    val paths = remember(picture) {
        listOf(
            picture.bars, picture.beats, picture.outWave, picture.outLow, picture.inWave, picture.inLow,
            picture.effect, picture.filter, picture.bass, picture.volume,
        ).map { command -> PathParser().parsePathString(command).toPath() }
    }
    Canvas(
        Modifier.fillMaxWidth().aspectRatio(340f / 230f).onSizeChanged { size = it }
            .pointerInput(picture.span, scaleFactor) {
                detectDragGestures(
                    onDragStart = { point ->
                        dragSide = if (point.y < size.height / 2f) 0 else 1
                        dragPixels = 0f
                    },
                    onDragEnd = {
                        val side = dragSide
                        val seconds = -dragPixels / size.width.coerceAtLeast(1) * picture.span
                        dragSide = -1
                        dragPixels = 0f
                        if (side >= 0 && seconds != 0f) onDrag(side, seconds.toDouble(), false)
                    },
                    onDragCancel = { dragSide = -1; dragPixels = 0f },
                    onDrag = { change, amount ->
                        change.consume()
                        dragPixels += amount.x
                    },
                )
            },
    ) {
        if (size.width <= 0) return@Canvas
        val color = picture.palette
        scale(scaleFactor, scaleFactor, pivot = Offset.Zero) {
            drawRoundRect(
                Color(0x0FFFFFFF),
                topLeft = Offset(picture.cardFrom * 340f, 0f),
                size = androidx.compose.ui.geometry.Size((picture.cardTo - picture.cardFrom) * 340f, 230f),
                cornerRadius = androidx.compose.ui.geometry.CornerRadius(12f),
            )
            drawPath(paths[0], Color(0x0CFFFFFF), style = Stroke(1f))
            drawPath(paths[1], Color(0x22FFFFFF), style = Stroke(1f))
            val shift = if (dragSide >= 0) dragPixels / scaleFactor else 0f
            clipRect(0f, 0f, 340f, 115f) {
                translate(left = -340f + if (dragSide == 0) shift else 0f) {
                    drawPath(paths[2], Color(color.outWave.toInt()))
                    drawPath(paths[3], Color(color.low.toInt()))
                }
            }
            clipRect(0f, 115f, 340f, 230f) {
                translate(left = -340f + if (dragSide == 1) shift else 0f) {
                    drawPath(paths[4], Color(color.inWave.toInt()))
                    drawPath(paths[5], Color(color.low.toInt()))
                }
            }
            drawPath(paths[6], Color(color.effect.toInt()), style = Stroke(2f, cap = StrokeCap.Round, join = StrokeJoin.Round))
            drawPath(paths[7], Color(color.filter.toInt()), style = Stroke(2f, cap = StrokeCap.Round, join = StrokeJoin.Round))
            drawPath(paths[8], Color(color.bass.toInt()), style = Stroke(2f, cap = StrokeCap.Round, join = StrokeJoin.Round))
            drawPath(paths[9], Color(color.volume.toInt()), style = Stroke(2f, cap = StrokeCap.Round, join = StrokeJoin.Round))
            if (playhead in 0f..1f) {
                val x = playhead * 340f
                drawLine(Color(0xFF1ED760), Offset(x, 0f), Offset(x, 230f), 2f)
            }
        }
    }
}
