package app.tonearm

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.PartyView
import uniffi.tonearm_android.QrMatrix
import uniffi.tonearm_android.Tonearm as Core

// The Party (docs/party.md) at the top of the queue, as the desktop's party
// box in ui/app.slint has it. Keep the two alike.

/** Avatar colours, picked by name: the same on the desktop and in the car. */
private val PARTY_COLOURS = longArrayOf(0xFFE0565B, 0xFF4C8DF6, 0xFF3FB27F, 0xFFE39B3B, 0xFF9B6BE0, 0xFF2BB3C0)

fun partyColour(name: String) = Color(PARTY_COLOURS[Math.floorMod(name.hashCode(), PARTY_COLOURS.size)])

/** A member's round letter avatar. */
@Composable
fun PartyAvatar(name: String, size: Dp, ring: Color? = null) {
    Box(
        Modifier.size(size).clip(CircleShape).background(partyColour(name))
            .then(if (ring != null) Modifier.border(2.dp, ring, CircleShape) else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            name.firstOrNull()?.uppercase() ?: "?",
            color = Color.White,
            fontFamily = Tonearm.font,
            fontWeight = FontWeight.Bold,
            fontSize = (size.value * 0.44f).sp,
        )
    }
}

/** How this phone shows in a Party: the YouTube Music profile's first name,
 *  else the phone's own name. Blocks. */
private fun myName(core: Core): String =
    runCatching { core.account()?.name }.getOrNull()?.trim()?.split(" ")?.firstOrNull()?.takeIf { it.isNotBlank() }
        ?: android.os.Build.MODEL

/**
 * The Party above the queue: Start a Party and Join a Party (a code, a link
 * or a QR scan); in one, its name, the first few members, Invite, Members
 * and End (Leave for a guest), the invite card with Copy link and the QR
 * code, and for the host "Let others change what's playing".
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun PartyHeader(core: Core, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val scope = rememberCoroutineScope()
    var party by remember { mutableStateOf<PartyView?>(null) }
    var joining by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    var invite by remember { mutableStateOf(false) }
    var members by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf("") }

    LaunchedEffect(core) {
        var seen: ULong? = null
        while (true) {
            val version = withContext(Dispatchers.IO) { core.partyVersion() }
            if (version != seen) {
                val now = withContext(Dispatchers.IO) { core.party() }
                // The host sees the invite as soon as the Party starts.
                if (party == null && now != null && now.host) invite = true
                if (now == null) { invite = false; members = false }
                party = now
                seen = version
            }
            delay(400)
        }
    }

    fun join(text: String) {
        if (busy) return
        busy = true
        error = ""
        scope.launch {
            val result = withContext(Dispatchers.IO) { runCatching { core.partyJoin(text, myName(core)) } }
            busy = false
            result.exceptionOrNull()?.let { error = it.message ?: "Couldn't join that Party" } ?: run { joining = false; code = "" }
        }
    }

    Column(modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, bottom = 14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        val p = party
        if (p == null) {
            if (!joining) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PillButton(if (busy) "Starting…" else "Start a Party") {
                        if (busy) return@PillButton
                        busy = true
                        error = ""
                        scope.launch {
                            val result = withContext(Dispatchers.IO) { runCatching { core.partyStart(myName(core)) } }
                            busy = false
                            result.exceptionOrNull()?.let { error = it.message ?: "Couldn't start a Party" }
                        }
                    }
                    PillButton("Join a Party") { joining = true; error = "" }
                }
            } else {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    CodeBox(code, { code = it }, Modifier.weight(1f)) { join(code) }
                    PillButton(if (busy) "Joining…" else "Join", primary = true) { join(code) }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PillButton("Scan a QR code") {
                        // Google's scanner screen reads it; no camera permission here.
                        val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
                        GmsBarcodeScanning.getClient(context, options).startScan()
                            .addOnSuccessListener { barcode ->
                                val found = core.partyCodeFrom(barcode.rawValue.orEmpty())
                                if (found == null) error = "That QR code isn't a Tonearm Party" else join(found)
                            }
                    }
                    PillButton("Cancel") { joining = false; error = "" }
                }
            }
            if (error.isNotEmpty()) Text(error, color = Color(0xFFFF8095), fontFamily = Tonearm.font, fontSize = 13.sp)
            return@Column
        }

        Title(p.name, size = 22.sp, weight = FontWeight.ExtraBold)
        if (!p.host && p.playing.isNotEmpty()) Dim(p.playing, 14.sp, modifier = Modifier.offset(y = (-6).dp))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            // The first few members, overlapping; a tap opens the list.
            val shown = p.members.take(4)
            Box(
                Modifier.height(36.dp).width((shown.size * 22 + 12).dp).clip(RoundedCornerShape(18.dp))
                    .clickable { members = !members },
                contentAlignment = Alignment.CenterStart,
            ) {
                shown.forEachIndexed { i, m ->
                    Box(Modifier.offset(x = (i * 22).dp)) { PartyAvatar(m.name, 32.dp, ring = Tonearm.bg) }
                }
            }
            PillButton("Invite") { invite = !invite }
            PillButton("Members") { members = !members }
            PillButton(if (p.host) "End" else "Leave") { scope.launch(Dispatchers.IO) { core.partyEnd() } }
        }
        if (p.note.isNotEmpty()) Dim(p.note, 13.sp)
        if (invite) {
            InviteCard(
                p,
                qr = remember(p.code) { core.partyQr(p.code) },
                onCopy = { clipboard.setText(AnnotatedString(p.link)) },
                onClose = { invite = false },
            )
        }
        if (p.host) {
            PartyToggle("Let others change what's playing", p.othersControl) {
                scope.launch(Dispatchers.IO) { core.partySetOthers(!p.othersControl) }
            }
        }
        if (members) {
            Column {
                Dim("Members", 13.sp, modifier = Modifier.padding(bottom = 4.dp))
                p.members.forEach { m ->
                    Row(Modifier.fillMaxWidth().height(52.dp), verticalAlignment = Alignment.CenterVertically) {
                        PartyAvatar(m.name, 36.dp)
                        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                            Title(m.name, size = 15.sp)
                            Dim(m.line, 13.sp)
                        }
                        if (p.host && !m.host) {
                            PillButton("Remove") { scope.launch(Dispatchers.IO) { core.partyRemove(m.id) } }
                        }
                    }
                }
            }
        }
    }
}

/** "Invite friends to your Party": Copy link and the code on the left,
 *  the QR code on the right, as Spotify's Jam card has them. */
@Composable
private fun InviteCard(p: PartyView, qr: QrMatrix?, onCopy: () -> Unit, onClose: () -> Unit) {
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color(0x0DFFFFFF))) {
        Row(Modifier.padding(start = 16.dp, top = 14.dp, end = 4.dp, bottom = 12.dp)) {
            Column(Modifier.weight(1f)) {
                Title("Invite friends to your Party", size = 15.sp, weight = FontWeight.Bold)
                Spacer(Modifier.height(4.dp))
                Dim("Copy and share the link, or ask them to scan the QR code with their phone's camera.", 13.sp, lines = 3)
            }
            Box(Modifier.size(36.dp).clip(CircleShape).clickable(onClick = onClose), contentAlignment = Alignment.Center) {
                Text("✕", color = Tonearm.dim, fontSize = 16.sp)
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Color(0x14FFFFFF)))
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
                PillButton("Copy link", primary = true, onClick = onCopy)
                Spacer(Modifier.height(8.dp))
                Text(p.code, color = Tonearm.dim, fontFamily = Tonearm.font, fontWeight = FontWeight.SemiBold, fontSize = 14.sp, letterSpacing = 1.sp)
            }
            // Held at its size while the QR is drawn, so nothing moves.
            Box(Modifier.size(128.dp).clip(RoundedCornerShape(10.dp)).background(Color.White)) {
                if (qr != null) PartyQr(qr, Modifier.fillMaxSize())
            }
        }
    }
}

/** A QR code, black on white with its blank border. */
@Composable
private fun PartyQr(qr: QrMatrix, modifier: Modifier) {
    val size = qr.size.toInt()
    Canvas(modifier) {
        val quiet = 2
        val module = this.size.width / (size + quiet * 2)
        for (y in 0 until size) for (x in 0 until size) {
            if (qr.dark[y * size + x]) {
                drawRect(Color.Black, topLeft = Offset((x + quiet) * module, (y + quiet) * module), size = Size(module + 0.5f, module + 0.5f))
            }
        }
    }
}

@Composable
private fun PartyToggle(title: String, on: Boolean, onToggle: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(52.dp).clip(RoundedCornerShape(12.dp))
            .background(Color(0x0AFFFFFF)).clickable(onClick = onToggle).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Title(title, size = 15.sp, modifier = Modifier.weight(1f))
        Box(
            Modifier.width(42.dp).height(24.dp).clip(RoundedCornerShape(12.dp))
                .background(if (on) Tonearm.accent else Color(0x30FFFFFF)),
            contentAlignment = if (on) Alignment.CenterEnd else Alignment.CenterStart,
        ) {
            Box(Modifier.padding(horizontal = 3.dp).size(18.dp).clip(CircleShape).background(Color.White))
        }
    }
}

/** The code box: a pill like the desktop's field. */
@Composable
private fun CodeBox(value: String, onChange: (String) -> Unit, modifier: Modifier, onDone: () -> Unit) {
    var focused by remember { mutableStateOf(false) }
    Box(
        modifier.height(40.dp).clip(RoundedCornerShape(20.dp))
            .background(if (focused) Color(0x1CFFFFFF) else Color(0x10FFFFFF))
            .border(1.dp, if (focused) Color(0x40FFFFFF) else Color.Transparent, RoundedCornerShape(20.dp))
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        if (value.isEmpty()) Dim("Party code or link", 14.sp)
        BasicTextField(
            value = value,
            onValueChange = onChange,
            singleLine = true,
            textStyle = TextStyle(color = Tonearm.text, fontFamily = Tonearm.font, fontSize = 14.sp),
            cursorBrush = SolidColor(Tonearm.accent),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { onDone() }),
            modifier = Modifier.fillMaxWidth().onFocusChanged { focused = it.isFocused },
        )
    }
}
