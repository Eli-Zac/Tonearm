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
import kotlinx.coroutines.async
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
 *  else the phone's own name as set in Settings ("Eli's S22 Ultra"), else
 *  its model. The profile can take a network fetch, so it gets 3 seconds,
 *  not the whole join's patience on a poor connection. */
private suspend fun myName(core: Core, context: android.content.Context): String {
    // Not a child of the caller: a blocked fetch can't be cancelled, and a
    // child would hold the timeout until it ended anyway.
    val fetch = kotlinx.coroutines.CoroutineScope(Dispatchers.IO).async { runCatching { core.account()?.name }.getOrNull() }
    val profile = kotlinx.coroutines.withTimeoutOrNull(3000) { fetch.await() }
    profile?.trim()?.split(" ")?.firstOrNull()?.takeIf { it.isNotBlank() }?.let { return it }
    val device = runCatching {
        android.provider.Settings.Global.getString(context.contentResolver, android.provider.Settings.Global.DEVICE_NAME)
    }.getOrNull()
    return device?.trim()?.takeIf { it.isNotBlank() } ?: android.os.Build.MODEL
}

/**
 * What the Party header shows, kept above the queue sheet: the header is an
 * item in the queue's list, which drops it when it scrolls away, and the
 * sheet comes and goes. Kept here, the invite card shows once when a Party
 * starts, not each time the queue opens, and a half-typed code survives.
 */
class PartyUi {
    /** Where starting, joining and the rest run: as long-lived as this, not
     *  the header's own scope, which ends when the header scrolls away or
     *  the sheet closes. That left "Joining…" stuck for good, and every
     *  later press ignored, when the queue closed mid join. */
    val scope = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.SupervisorJob() + Dispatchers.Main)
    var party by mutableStateOf<PartyView?>(null)
    var joining by mutableStateOf(false)
    var busy by mutableStateOf(false)
    var error by mutableStateOf("")
    var invite by mutableStateOf(false)
    var members by mutableStateOf(false)
    var menu by mutableStateOf(false)
    var code by mutableStateOf("")
    private var seen: ULong? = null
    /** The Party whose invite card has shown by itself already. */
    private var invited: String? = null

    private fun show(now: PartyView?, version: ULong) {
        // The host sees the invite once, as soon as the Party starts.
        if (now != null && now.host && invited != now.code) invite = true
        if (now != null) invited = now.code
        if (now == null) { invite = false; members = false; menu = false }
        party = now
        seen = version
    }

    /** Reads the Party at once. Only a lock in the core, so fine on the
     *  main thread: the header opens as it is, never Start a Party first. */
    fun refresh(core: Core) {
        val version = core.partyVersion()
        if (version != seen) show(core.party(), version)
    }

    /** Keeps following the Party while the queue shows. */
    suspend fun follow(core: Core) {
        while (true) {
            val read = withContext(Dispatchers.IO) {
                val version = core.partyVersion()
                version to (if (version != seen) core.party() else null)
            }
            if (read.first != seen) show(read.second, read.first)
            delay(400)
        }
    }
}

/**
 * The Party at the top of the queue. Not in one: Start a Party and Join a
 * Party (a code, a link or a QR scan). In one, a single row: the members'
 * avatars and the Party's name (both open the members), Invite, and ⋯ for
 * Members and End (Leave for a guest). Under it the invite card when shown,
 * and for the host a compact "Let others change what's playing".
 */
@Composable
fun PartyHeader(core: Core, ui: PartyUi, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val scope = ui.scope

    fun join(text: String) {
        if (ui.busy) return
        ui.busy = true
        ui.error = ""
        scope.launch {
            try {
                val name = myName(core, context)
                val result = withContext(Dispatchers.IO) { runCatching { core.partyJoin(text, name) } }
                result.exceptionOrNull()?.let { ui.error = it.message ?: "Couldn't join that Party" } ?: run { ui.joining = false; ui.code = "" }
            } finally {
                ui.busy = false
            }
        }
    }

    Column(modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        val p = ui.party
        if (p == null) {
            if (!ui.joining) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PillButton(if (ui.busy) "Starting…" else "Start a Party") {
                        if (ui.busy) return@PillButton
                        ui.busy = true
                        ui.error = ""
                        scope.launch {
                            try {
                                val name = myName(core, context)
                                val result = withContext(Dispatchers.IO) { runCatching { core.partyStart(name) } }
                                result.exceptionOrNull()?.let { ui.error = it.message ?: "Couldn't start a Party" }
                            } finally {
                                ui.busy = false
                            }
                        }
                    }
                    PillButton("Join a Party") { ui.joining = true; ui.error = "" }
                }
            } else {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    CodeBox(ui.code, { ui.code = it }, Modifier.weight(1f)) { join(ui.code) }
                    PillButton(if (ui.busy) "Joining…" else "Join", primary = true) { join(ui.code) }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PillButton("Scan a QR code") {
                        // Google's scanner screen reads it; no camera permission here.
                        val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
                        GmsBarcodeScanning.getClient(context, options).startScan()
                            .addOnSuccessListener { barcode ->
                                val found = core.partyCodeFrom(barcode.rawValue.orEmpty())
                                if (found == null) ui.error = "That QR code isn't a Tonearm Party" else join(found)
                            }
                    }
                    PillButton("Cancel") { ui.joining = false; ui.error = "" }
                }
            }
            if (ui.error.isNotEmpty()) Text(ui.error, color = Color(0xFFFF8095), fontFamily = Tonearm.font, fontSize = 13.sp)
            return@Column
        }

        Row(Modifier.fillMaxWidth().height(52.dp), verticalAlignment = Alignment.CenterVertically) {
            // The first few members, overlapping, and the name: a tap
            // opens the members.
            val shown = p.members.take(4)
            Row(
                Modifier.weight(1f).fillMaxHeight().clip(RoundedCornerShape(12.dp))
                    .clickable { ui.members = !ui.members },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                // At least one avatar's room, so the name never shifts as
                // the members come in.
                Box(Modifier.width((shown.size.coerceAtLeast(1) * 18 + 14).dp), contentAlignment = Alignment.CenterStart) {
                    shown.forEachIndexed { i, m ->
                        Box(Modifier.offset(x = (i * 18).dp)) { PartyAvatar(m.name, 32.dp, ring = Tonearm.bg) }
                    }
                }
                Column(Modifier.weight(1f).padding(start = 10.dp, end = 8.dp)) {
                    Title(p.name, size = 17.sp, weight = FontWeight.Bold)
                    Dim(partyLine(p), 13.sp)
                }
            }
            PillButton("Invite") { ui.invite = !ui.invite }
            Box {
                Box(
                    Modifier.size(40.dp).clip(CircleShape).clickable { ui.menu = true },
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.dots, size = 22.sp, filled = true) }
                if (ui.menu) {
                    PartyMenu(
                        host = p.host,
                        onDismiss = { ui.menu = false },
                        onMembers = { ui.menu = false; ui.members = !ui.members },
                        onEnd = { ui.menu = false; scope.launch(Dispatchers.IO) { core.partyEnd() } },
                    )
                }
            }
        }
        if (p.note.isNotEmpty()) Dim(p.note, 13.sp)
        if (ui.invite) {
            InviteCard(
                p,
                qr = remember(p.code) { core.partyQr(p.code) },
                onCopy = { clipboard.setText(AnnotatedString(p.link)) },
                onClose = { ui.invite = false },
            )
        }
        if (p.host) {
            PartyToggle("Let others change what's playing", p.othersControl) {
                scope.launch(Dispatchers.IO) { core.partySetOthers(!p.othersControl) }
            }
        }
        if (ui.members) {
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

/** Under the Party's name: for a guest what the host plays, for the host
 *  how many are in it. */
private fun partyLine(p: PartyView): String = when {
    !p.host && p.playing.isNotEmpty() -> p.playing
    p.members.size <= 1 -> "Just you so far"
    else -> "${p.members.size} people"
}

/** ⋯: Members, and End the Party (Leave for a guest), in a small card
 *  under the button, as the desktop's menu. */
@Composable
private fun PartyMenu(host: Boolean, onDismiss: () -> Unit, onMembers: () -> Unit, onEnd: () -> Unit) {
    val y = with(androidx.compose.ui.platform.LocalDensity.current) { 44.dp.roundToPx() }
    androidx.compose.ui.window.Popup(
        alignment = Alignment.TopEnd,
        offset = androidx.compose.ui.unit.IntOffset(0, y),
        onDismissRequest = onDismiss,
        properties = androidx.compose.ui.window.PopupProperties(focusable = true),
    ) {
        Column(
            Modifier.width(200.dp).clip(RoundedCornerShape(Tonearm.panel)).background(Tonearm.raised)
                .border(1.dp, Tonearm.line, RoundedCornerShape(Tonearm.panel)).padding(vertical = 6.dp),
        ) {
            MenuChoice("Members", Tonearm.text, onMembers)
            MenuChoice(if (host) "End the Party" else "Leave the Party", Color(0xFFFF8095), onEnd)
        }
    }
}

@Composable
private fun MenuChoice(text: String, color: Color, onClick: () -> Unit) {
    Box(
        Modifier.fillMaxWidth().height(46.dp).clickable(onClick = onClick).padding(horizontal = 16.dp),
        contentAlignment = Alignment.CenterStart,
    ) { Title(text, size = 15.sp, color = color, weight = FontWeight.Medium) }
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
    // One slim row with a small switch, so it doesn't crowd the queue.
    Row(
        Modifier.fillMaxWidth().height(40.dp).clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onToggle).padding(start = 4.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Title(title, size = 14.sp, weight = FontWeight.Medium, color = Tonearm.dim, modifier = Modifier.weight(1f))
        Box(
            Modifier.width(34.dp).height(20.dp).clip(RoundedCornerShape(10.dp))
                .background(if (on) Tonearm.accent else Color(0x30FFFFFF)),
            contentAlignment = if (on) Alignment.CenterEnd else Alignment.CenterStart,
        ) {
            Box(Modifier.padding(horizontal = 3.dp).size(14.dp).clip(CircleShape).background(Color.White))
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
