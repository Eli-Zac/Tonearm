package app.tonearm

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.Canvas
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.platform.LocalContext
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import uniffi.tonearm_android.QrMatrix
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.AccountDevice
import uniffi.tonearm_android.Pairing
import uniffi.tonearm_android.TonearmAccount
import uniffi.tonearm_android.Tonearm as Core

// The Tonearm account (not the YouTube Music one): the same account on every
// device, signed in with an emailed code or a code another device shows.

@Composable
internal fun TonearmAccountPage(core: Core, onToast: (String) -> Unit) {
    var account by remember { mutableStateOf<TonearmAccount?>(core.tonearmAccount()) }
    val signedIn = account
    if (signedIn == null) {
        SignedOut(core, onToast) { account = it }
    } else {
        SignedIn(core, signedIn, onToast) { account = null }
    }
}

/** A QR code, drawn white on black as a scanner likes it, with its blank border. */
@Composable
private fun QrView(qr: QrMatrix, modifier: Modifier = Modifier) {
    val size = qr.size.toInt()
    Canvas(modifier.background(Color.White, RoundedCornerShape(12.dp))) {
        val quiet = 2
        val module = this.size.width / (size + quiet * 2)
        for (y in 0 until size) for (x in 0 until size) {
            if (qr.dark[y * size + x]) {
                drawRect(
                    Color.Black,
                    topLeft = Offset((x + quiet) * module, (y + quiet) * module),
                    size = Size(module + 0.5f, module + 0.5f),
                )
            }
        }
    }
}

/** Runs [work] off the main thread and gives its result, or the error as a toast. */
private suspend fun <T> attempt(onToast: (String) -> Unit, work: () -> T): T? =
    withContext(Dispatchers.IO) { runCatching(work) }.fold(
        onSuccess = { it },
        onFailure = {
            onToast(it.message?.takeIf { m -> m.isNotBlank() } ?: "Something went wrong")
            null
        },
    )

@Composable
private fun SignedOut(core: Core, onToast: (String) -> Unit, onSignedIn: (TonearmAccount) -> Unit) {
    val scope = rememberCoroutineScope()
    var email by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var sent by remember { mutableStateOf(false) }
    var pairCode by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }

    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Title("Sign in with email", size = 16.sp)
            Dim(
                "Your settings and pins follow you to every device you sign in on. Tonearm emails you a six digit code; there is no password.",
                14.sp, lines = 5,
            )
            Field(email, { email = it }, "Email address")
            if (sent) {
                Field(code, { code = it.filter(Char::isDigit).take(6) }, "6 digit code")
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                if (!sent) {
                    PillButton(if (busy) "Sending…" else "Email me a code", primary = true) {
                        if (busy || email.isBlank()) return@PillButton
                        busy = true
                        scope.launch {
                            val ok = attempt(onToast) { core.accountRequestCode(email.trim()) }
                            busy = false
                            if (ok != null) {
                                sent = true
                                onToast("Code sent. It works for 10 minutes.")
                            }
                        }
                    }
                } else {
                    PillButton(if (busy) "Signing in…" else "Sign in", primary = true) {
                        if (busy || code.length != 6) return@PillButton
                        busy = true
                        scope.launch {
                            val signed = attempt(onToast) { core.accountSignIn(email.trim(), code) }
                            busy = false
                            if (signed != null) onSignedIn(signed)
                        }
                    }
                    PillButton("Send again") {
                        if (busy) return@PillButton
                        scope.launch {
                            if (attempt(onToast) { core.accountRequestCode(email.trim()) } != null) {
                                onToast("Sent a new code.")
                            }
                        }
                    }
                }
            }
        }
    }

    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Title("Use a pairing code", size = 16.sp)
            Dim(
                "Already signed in on another device? Open Pair a device there, and enter the code it shows.",
                14.sp, lines = 4,
            )
            Field(pairCode, { pairCode = it.uppercase().filter { c -> c.isLetterOrDigit() || c == '-' }.take(9) }, "XXXX-XXXX")
            val context = LocalContext.current
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                PillButton(if (busy) "Joining…" else "Join", primary = true) {
                    if (busy || pairCode.replace("-", "").length != 8) return@PillButton
                    busy = true
                    scope.launch {
                        val signed = attempt(onToast) { core.accountPairClaim(pairCode) }
                        busy = false
                        if (signed != null) onSignedIn(signed)
                    }
                }
                PillButton("Scan a QR code") {
                    if (busy) return@PillButton
                    // Google's scanner screen reads it; no camera permission here.
                    val options = GmsBarcodeScannerOptions.Builder()
                        .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                        .build()
                    GmsBarcodeScanning.getClient(context, options).startScan()
                        .addOnSuccessListener { barcode ->
                            val code = core.accountScanCode(barcode.rawValue.orEmpty())
                            if (code == null) {
                                onToast("That QR code isn't a Tonearm pairing code.")
                            } else {
                                busy = true
                                scope.launch {
                                    val signed = attempt(onToast) { core.accountPairClaim(code) }
                                    busy = false
                                    if (signed != null) onSignedIn(signed)
                                }
                            }
                        }
                        .addOnFailureListener { onToast("Couldn't open the scanner: ${it.message ?: "unknown error"}") }
                }
            }
        }
    }
}

@Composable
private fun SignedIn(core: Core, account: TonearmAccount, onToast: (String) -> Unit, onSignedOut: () -> Unit) {
    val scope = rememberCoroutineScope()
    var refresh by remember { mutableIntStateOf(0) }
    val devices by produceState<List<AccountDevice>?>(null, refresh) {
        value = attempt(onToast) { core.accountDevices() }
        // A device the account dropped is signed out by the server's 401.
        if (value == null && core.tonearmAccount() == null) onSignedOut()
    }
    var pairing by remember { mutableStateOf<Pairing?>(null) }
    var left by remember { mutableIntStateOf(0) }
    LaunchedEffect(pairing) {
        val p = pairing ?: return@LaunchedEffect
        left = p.expiresIn.toInt()
        while (left > 0) {
            delay(1000)
            left--
        }
        pairing = null
    }

    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Title("Signed in as ${account.email}", size = 16.sp, lines = 2)
            Dim("This account's settings follow you to every device signed in to it.", 14.sp, lines = 3)
            Row {
                PillButton("Sign out", danger = true) {
                    scope.launch {
                        withContext(Dispatchers.IO) { core.accountSignOut() }
                        onSignedOut()
                    }
                }
            }
        }
    }

    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Title("Pair a device", size = 16.sp)
            val p = pairing
            if (p == null) {
                Dim("Shows a code to enter on another device, so it joins this account without email.", 14.sp, lines = 4)
                PillButton("Show a code", primary = true) {
                    scope.launch { attempt(onToast) { core.accountPairStart() }?.let { pairing = it } }
                }
            } else {
                val qr = remember(p.code) { core.accountPairQr(p.code) }
                if (qr != null) QrView(qr, Modifier.size(200.dp))
                Title(p.code, size = 32.sp, weight = FontWeight.ExtraBold)
                Dim("Enter it on the other device within ${left / 60}:${(left % 60).toString().padStart(2, '0')}. It works once.", 14.sp, lines = 3)
                PillButton("Done") { pairing = null }
            }
        }
    }

    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Title("Devices", size = 16.sp)
            val list = devices
            if (list == null) {
                TextSkeleton(0.6f, 18.dp)
                TextSkeleton(0.45f, 18.dp)
            } else {
                list.forEach { d ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(
                            Modifier.size(40.dp).clip(RoundedCornerShape(10.dp)).background(Color(0x14FFFFFF)),
                            contentAlignment = Alignment.Center,
                        ) {
                            Icon(if (d.kind == "computer") Icons.desktop else Icons.mobile, size = 22.sp, filled = true)
                        }
                        Column(Modifier.weight(1f).padding(horizontal = 14.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Title(if (d.current) "${d.name} (this phone)" else d.name, size = 16.sp, lines = 2)
                            Dim(listOf(d.model, "Last used ${ago(d.lastSeen)}").filter { it.isNotEmpty() }.joinToString(" · "), 14.sp, lines = 2)
                        }
                        if (!d.current) {
                            PillButton("Remove", danger = true) {
                                scope.launch {
                                    attempt(onToast) { core.accountRemoveDevice(d.id) }
                                    refresh++
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** "just now", "5 minutes ago", "3 days ago". */
private fun ago(millis: Long): String {
    val seconds = ((System.currentTimeMillis() - millis) / 1000).coerceAtLeast(0)
    return when {
        seconds < 90 -> "just now"
        seconds < 3600 -> "${seconds / 60} minutes ago"
        seconds < 86400 -> "${seconds / 3600} hours ago"
        else -> "${seconds / 86400} days ago"
    }
}
