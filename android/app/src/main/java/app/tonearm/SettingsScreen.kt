package app.tonearm

import android.content.Intent
import android.net.Uri
import androidx.annotation.DrawableRes
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Account
import uniffi.tonearm_android.Choices
import uniffi.tonearm_android.Connections
import uniffi.tonearm_android.Tonearm as Core

// The desktop's settings pages (settings-page in ui/app.slint): the same
// sections, wording and cards. Keep the two alike.

/** Settings changed: what reads them (Now Playing's lyrics and cover)
 *  reads them again. */
object Prefs {
    var version by mutableIntStateOf(0)
        private set
    fun changed() { version++ }

    /** Work that outlives the page it started on, as linking Last.fm. */
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    /** The note above Connections, as the desktop's services-note. */
    var servicesNote by mutableStateOf("")
    /** The note above the account, as the desktop's settings-note. */
    var accountNote by mutableStateOf("")
}

/** A page of Settings, as a route's id. */
object SettingsPages {
    const val root = "tonearm:settings"
    const val account = "tonearm:settings/account"
    const val connections = "tonearm:settings/connections"
    const val playback = "tonearm:settings/playback"
    const val customization = "tonearm:settings/customization"
    const val storage = "tonearm:settings/storage"
    const val tonearm = "tonearm:settings/tonearm"
    const val signIn = "tonearm:settings/sign-in"
    const val spotifySignIn = "tonearm:settings/spotify-sign-in"

    fun isSettings(id: String) = id == root || id.startsWith("$root/")
}

private val brandYouTube = Color(0xFFFF0000)
internal val brandLastfm = Color(0xFFD51007)
internal val brandSpotify = Color(0xFF1DB954)
private val card = Color(0x0AFFFFFF)
private val pressedCard = Color(0x14FFFFFF)

@Composable
fun SettingsScreen(
    page: String,
    core: Core,
    onOpen: (String) -> Unit,
    onBack: () -> Unit,
    onToast: (String) -> Unit,
) {
    // Notes from linking and saving float up as a toast over the page,
    // rather than a line on top that pushes the cards down.
    LaunchedEffect(Prefs.servicesNote, Prefs.accountNote) {
        for (note in listOf(Prefs.servicesNote, Prefs.accountNote)) if (note.isNotEmpty()) onToast(note)
        Prefs.servicesNote = ""
        Prefs.accountNote = ""
    }
    when (page) {
        SettingsPages.signIn -> YouTubeSignIn(core, onBack, onToast)
        SettingsPages.spotifySignIn -> SpotifySignIn(core, onBack, onToast)
        else -> SettingsPage(
            title = when (page) {
                // Short: the card under it says YouTube Music with the logo.
                SettingsPages.account -> "Account"
                SettingsPages.tonearm -> "Tonearm account"
                SettingsPages.connections -> "Connections"
                SettingsPages.playback -> "Playback"
                SettingsPages.customization -> "Customization"
                SettingsPages.storage -> "Storage and history"
                else -> "Settings"
            },
            onBack = onBack,
        ) {
            when (page) {
                SettingsPages.account -> AccountPage(core, onOpen)
                SettingsPages.tonearm -> TonearmAccountPage(core, onToast)
                SettingsPages.connections -> ConnectionsPage(core, onOpen)
                SettingsPages.playback -> PlaybackPage(core)
                SettingsPages.customization -> CustomizationPage(core)
                SettingsPages.storage -> StoragePage(core, onToast)
                else -> RootPage(core, onOpen)
            }
        }
    }
}

/** A settings page: the back button, the page's title, and its cards. */
@Composable
private fun SettingsPage(title: String, onBack: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    Box(Modifier.fillMaxSize()) {
        Column(
            Modifier.fillMaxSize().imePadding().verticalScroll(rememberScrollState()).statusBarsPadding()
                .padding(start = 16.dp, end = 16.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            // The heading beside the back button.
            Row(Modifier.padding(top = 8.dp).heightIn(min = 56.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.offset(x = (-8).dp).size(40.dp).clip(RoundedCornerShape(20.dp)).clickable(onClick = onBack),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(Icons.back, size = 20.sp)
                }
                // One line always: a title too long for it shrinks (to 20sp at
                // most) rather than wrapping, then ends in an ellipsis.
                androidx.compose.foundation.text.BasicText(
                    title,
                    modifier = Modifier.padding(start = 4.dp).weight(1f),
                    style = androidx.compose.ui.text.TextStyle(
                        color = Tonearm.text, fontFamily = Tonearm.font, fontWeight = FontWeight.ExtraBold,
                    ),
                    maxLines = 1,
                    overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                    autoSize = androidx.compose.foundation.text.TextAutoSize.StepBased(
                        minFontSize = 20.sp, maxFontSize = 28.sp, stepSize = 1.sp,
                    ),
                )
            }
            content()
        }
        StatusFade()
    }
}

@Composable
fun BackButton(onBack: () -> Unit) {
    Box(
        Modifier.statusBarsPadding().padding(8.dp).size(40.dp).clip(RoundedCornerShape(20.dp))
            .background(Color(0x9908080A)).clickable(onClick = onBack),
        contentAlignment = Alignment.Center,
    ) {
        Icon(Icons.back, size = 20.sp)
    }
}

// ------------------------------------------------------------------- pages

@Composable
private fun RootPage(core: Core, onOpen: (String) -> Unit) {
    val account = rememberAccount(core)
    // Read from the store each time Settings opens, so a sign in or out on
    // its page shows when coming back.
    val tonearmAccount = core.tonearmAccount()
    SubpageRow(
        title = "Tonearm account",
        subtitle = tonearmAccount?.let { "Signed in as ${it.email}" } ?: "Not signed in",
        icon = Icons.artist,
    ) { onOpen(SettingsPages.tonearm) }
    SubpageRow(
        title = "YouTube Music account",
        subtitle = when (val a = account) {
            AccountState.Loading -> null
            is AccountState.SignedIn -> "Signed in as ${a.account.name}"
            AccountState.SignedOut -> "Not signed in"
        },
        logo = R.drawable.logo_youtubemusic,
        brand = brandYouTube,
    ) { onOpen(SettingsPages.account) }
    // Discord stays on the desktop, which it has to be running beside.
    SubpageRow(title = "Connections", subtitle = "Last.fm and Spotify", icon = Icons.connections) {
        onOpen(SettingsPages.connections)
    }
    SubpageRow(title = "Playback", subtitle = "Crossfade and volume levelling", icon = Icons.speakerHigh) {
        onOpen(SettingsPages.playback)
    }
    SubpageRow(title = "Customization", subtitle = "Covers in Now playing, and lyrics translation", icon = Icons.sparkle) {
        onOpen(SettingsPages.customization)
    }
    SubpageRow(title = "Storage and history", subtitle = "Song cache, and what Tonearm remembers you played", icon = Icons.download) {
        onOpen(SettingsPages.storage)
    }
    val context = LocalContext.current
    val version = remember {
        runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull() ?: ""
    }
    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Title("Tonearm $version", size = 16.sp)
            Dim("Updates come from the GitHub releases page.", 14.sp, lines = 4)
        }
    }
}

internal sealed interface AccountState {
    data object Loading : AccountState
    data object SignedOut : AccountState
    data class SignedIn(val account: Account) : AccountState
}

/**
 * The account's picture: a sweeping bone while the account is asked for
 * (and while the picture itself loads, through [Art]); a faint circle with
 * a person when signed out or there is no picture.
 */
@Composable
internal fun AccountPhoto(account: AccountState, size: androidx.compose.ui.unit.Dp) {
    val a = (account as? AccountState.SignedIn)?.account
    when {
        account is AccountState.Loading -> Bone(Modifier.size(size), CircleShape)
        a != null && a.photo.isNotEmpty() -> Art(a.photo, size, CircleShape)
        else -> Box(Modifier.size(size).clip(CircleShape).background(Color(0x14FFFFFF)), contentAlignment = Alignment.Center) {
            Icon(Icons.artist, size = (size.value * 0.43f).sp, color = Tonearm.dim)
        }
    }
}

/** Who is signed in. Asked again when the sign in changes. */
@Composable
internal fun rememberAccount(core: Core): AccountState {
    val state by produceState<AccountState>(AccountState.Loading, Prefs.version) {
        value = withContext(Dispatchers.IO) {
            if (!core.signedIn()) AccountState.SignedOut
            else core.account()?.let { AccountState.SignedIn(it) } ?: AccountState.SignedOut
        }
    }
    return state
}

@Composable
private fun AccountPage(core: Core, onOpen: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    val account = rememberAccount(core)
    val signedIn = account is AccountState.SignedIn
    Section(
        title = "YouTube Music",
        logo = R.drawable.logo_youtubemusic,
        brand = brandYouTube,
        about = if (account is AccountState.SignedOut) {
            "Sign in with your Google account on the YouTube Music page that opens. Tonearm keeps the sign in on this phone and sends it only to music.youtube.com."
        } else {
            "Your library, playlists and recommendations come from this account. The sign in stays on this phone and is sent only to music.youtube.com."
        },
    ) {
        // Who it is, its picture and name; skeletons hold the room while
        // the account is asked for.
        if (account !is AccountState.SignedOut) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                val a = (account as? AccountState.SignedIn)?.account
                AccountPhoto(account, 56.dp)
                Column(Modifier.padding(start = 14.dp).weight(1f).height(46.dp), verticalArrangement = Arrangement.Center) {
                    if (a == null) {
                        TextSkeleton(0.6f, 18.dp)
                        Spacer(Modifier.height(6.dp))
                        TextSkeleton(0.4f, 14.dp)
                    } else {
                        Title(a.name, size = 18.sp, weight = FontWeight.Bold)
                        if (a.handle.isNotEmpty()) Dim(a.handle, 14.sp)
                    }
                }
            }
        }
        AccountRow(
            linked = signedIn,
            title = when (account) {
                is AccountState.SignedIn -> "Signed in as ${account.account.name}"
                AccountState.SignedOut -> "Not signed in"
                AccountState.Loading -> ""
            },
            linkText = "Sign in",
            unlinkText = "Sign out",
            onLink = { Prefs.accountNote = ""; onOpen(SettingsPages.signIn) },
            onUnlink = {
                scope.launch {
                    withContext(Dispatchers.IO) { core.signOut() }
                    Prefs.changed()
                }
            },
        )
    }
}

@Composable
private fun PlaybackPage(core: Core) {
    Card {
        SettingRow(
            "Crossfade",
            "As a song ends, the next one fades in while it fades out. Skipping a song still switches at once.",
            Modifier.padding(20.dp),
        ) {
            Dropdown(remember { core.crossfadeChoices() }, "Crossfade") { core.setCrossfade(it.toUInt()) }
        }
    }
    Card {
        SettingRow(
            "Volume levelling",
            "Plays every song at about the same loudness, using YouTube's own measure of each one. Loud also lifts quiet songs a little.",
            Modifier.padding(20.dp),
        ) {
            Dropdown(remember { core.levellingChoices() }, "Volume levelling") { core.setLevelling(it.toUInt()) }
        }
    }
}

@Composable
private fun CustomizationPage(core: Core) {
    Card {
        SettingRow(
            "Now playing cover",
            "What Now playing shows for the cover. Animated cover is the album's moving cover from Apple Music, when it has one. Without a clip, the still cover shows.",
            Modifier.padding(20.dp),
        ) {
            Dropdown(remember { core.coverChoices() }, "Now playing cover") {
                core.setCover(it.toUInt())
                Prefs.changed()
            }
        }
    }
    Card {
        SettingRow(
            "Translate lyrics",
            "Shows a translation under each line of lyrics. The lyrics' own translation is used when they have one, and Google Translate otherwise.",
            Modifier.padding(20.dp),
        ) {
            Dropdown(remember { core.translateChoices() }, "Translate lyrics") {
                core.setTranslate(it.toUInt())
                Prefs.changed()
            }
        }
    }
}

@Composable
private fun StoragePage(core: Core, onToast: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    // Counting the cache reads the disk, so it comes a moment later; its
    // line keeps its room meanwhile.
    var epoch by remember { mutableIntStateOf(0) }
    val note by produceState<String?>(null, epoch) {
        value = withContext(Dispatchers.IO) { core.storageNote() }
    }
    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            SettingRow(
                "Song cache",
                "Songs you play are kept so they start at once next time. The least recently played go first when it fills up.",
            ) {
                Dropdown(remember { core.cacheChoices() }, "Song cache") {
                    scope.launch {
                        withContext(Dispatchers.IO) { core.setCache(it.toUInt()) }
                        epoch++
                    }
                }
            }
            SettingRow("Storage", note, subtitleSkeleton = note == null) {
                PillButton("Clear cache") {
                    scope.launch {
                        withContext(Dispatchers.IO) { core.clearCache() }
                        epoch++
                        onToast("Cache cleared")
                    }
                }
            }
        }
    }
    Card {
        SettingRow(
            "Listening history",
            "Tonearm remembers what you play and open on this phone, and your recent searches. It stays on this phone.",
            Modifier.padding(20.dp),
        ) {
            PillButton("Clear") {
                scope.launch {
                    withContext(Dispatchers.IO) { core.clearHistory() }
                    onToast("Listening history cleared")
                }
            }
        }
    }
}

@Composable
private fun ConnectionsPage(core: Core, onOpen: (String) -> Unit) {
    val context = LocalContext.current
    var shown by remember { mutableStateOf(core.connections()) }
    // Read again as Last.fm links in the background.
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            shown = core.connections()
        }
    }
    val c = shown
    var lastfmKey by remember { mutableStateOf(c.lastfmKey) }
    var lastfmSecret by remember { mutableStateOf(c.lastfmSecret) }
    var spotifyId by remember { mutableStateOf(c.spotifyId) }
    var spotifySecret by remember { mutableStateOf(c.spotifySecret) }
    Section(
        title = "Last.fm",
        logo = R.drawable.logo_lastfm,
        brand = brandLastfm,
        about = "Keeps a record of what you listen to. A track scrobbles after it has played for half its length or four minutes." +
            if (c.lastfmBuiltin) "" else " This build has no Last.fm keys of its own. Enter an API key and shared secret from last.fm/api/account/create.",
    ) {
        if (!c.lastfmBuiltin && c.lastfmUser.isEmpty()) {
            Field(lastfmKey, { lastfmKey = it }, "Last.fm API key")
            Field(lastfmSecret, { lastfmSecret = it }, "Last.fm shared secret", secret = true)
        }
        val linked = c.lastfmUser.isNotEmpty()
        AccountRow(
            linked = linked,
            title = if (linked) "Linked as ${c.lastfmUser}" else "Not linked",
            subtitle = if (linked) "" else "Connect opens Last.fm in your browser. Approve Tonearm there and it links by itself.",
            onLink = { linkLastfm(core, context, lastfmKey, lastfmSecret) { shown = core.connections() } },
            onUnlink = {
                core.lastfmUnlink()
                Prefs.servicesNote = "Last.fm is unlinked"
                shown = core.connections()
            },
        )
    }
    Section(
        title = "Spotify",
        logo = R.drawable.logo_spotify,
        brand = brandSpotify,
        about = "Optional. With credentials from developer.spotify.com, Tonearm looks each track up on Spotify and reports Spotify's title, artist and album to Last.fm.",
    ) {
        if (!c.spotifyLinked) {
            Field(spotifyId, { spotifyId = it }, "Spotify client ID")
            Field(spotifySecret, { spotifySecret = it }, "Spotify client secret", secret = true)
        }
        AccountRow(
            linked = c.spotifyLinked,
            title = if (c.spotifyLinked) "Linked" else "Not linked",
            subtitle = if (c.spotifyLinked) "Track details come from Spotify." else "",
            onLink = {
                if (spotifyId.isBlank() || spotifySecret.isBlank()) {
                    Prefs.servicesNote = "Enter your Spotify client ID and secret first."
                } else {
                    core.saveSpotify(spotifyId, spotifySecret)
                    Prefs.servicesNote = "Saved"
                }
                shown = core.connections()
            },
            onUnlink = {
                core.spotifyUnlink()
                Prefs.servicesNote = "Spotify is unlinked"
                shown = core.connections()
            },
        )
        // The desktop copies the sp_dc cookie by hand; a phone has a web
        // view to sign in with.
        AccountRow(
            linked = c.canvasLinked,
            title = if (c.canvasLinked) "Spotify sign in connected" else "Spotify sign in not connected",
            subtitle = if (c.canvasLinked) {
                "Your Spotify sign in is kept on this phone, for Spotify's own features as they come to the app."
            } else {
                "Connect opens Spotify's sign in page. The sign in stays on this phone. It is a private Spotify sign in, so it can stop working when Spotify changes its web player; connect again then."
            },
            onLink = { onOpen(SettingsPages.spotifySignIn) },
            onUnlink = {
                core.canvasUnlink()
                Prefs.servicesNote = "Spotify sign in removed"
                shown = core.connections()
            },
        )
    }
}

/** Opens Last.fm's approval page, then asks every few seconds whether the
 *  user approved there, for three minutes, as the desktop does. */
private fun linkLastfm(core: Core, context: android.content.Context, key: String, secret: String, refresh: () -> Unit) {
    Prefs.servicesNote = "Opening Last.fm"
    Prefs.scope.launch {
        val url = try {
            core.lastfmBegin(key, secret)
        } catch (e: Exception) {
            Prefs.servicesNote = e.message ?: "Couldn't reach Last.fm"
            return@launch
        }
        withContext(Dispatchers.Main) {
            runCatching {
                context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            Prefs.servicesNote = "Approve Tonearm in the browser page that opened. Tonearm connects by itself once you do."
        }
        val until = System.currentTimeMillis() + 180_000
        while (System.currentTimeMillis() < until) {
            delay(3000)
            if (!core.connections().lastfmWaiting) return@launch
            val user = core.lastfmFinish()
            if (user != null) {
                Prefs.servicesNote = "Last.fm is connected"
                withContext(Dispatchers.Main) { refresh() }
                return@launch
            }
        }
        core.lastfmCancel()
        Prefs.servicesNote = "Last.fm did not confirm in time. Press Connect to try again."
    }
}

// -------------------------------------------------------------- the parts

@Composable
internal fun Card(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Box(modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(card)) { content() }
}

/** A row that opens a sub page: an icon tile (or a logo on its brand
 *  colour), its title and a line under it, and an arrow. */
@Composable
internal fun SubpageRow(
    title: String,
    subtitle: String?,
    icon: String? = null,
    @DrawableRes logo: Int? = null,
    brand: Color? = null,
    onClick: () -> Unit,
) {
    val press = remember { MutableInteractionSource() }
    val pressed by press.collectIsPressedAsState()
    Row(
        Modifier.fillMaxWidth().height(68.dp).clip(RoundedCornerShape(12.dp))
            .background(if (pressed) pressedCard else card)
            .clickable(interactionSource = press, indication = null, onClick = onClick)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(36.dp).clip(RoundedCornerShape(9.dp)).background(brand ?: Color(0x14FFFFFF)),
            contentAlignment = Alignment.Center,
        ) {
            if (logo != null) {
                Image(painterResource(logo), null, Modifier.size(20.dp), colorFilter = ColorFilter.tint(Color.White))
            } else if (icon != null) {
                Icon(icon, size = 20.sp)
            }
        }
        Column(Modifier.weight(1f).padding(horizontal = 14.dp)) {
            Title(title, size = 16.sp)
            Spacer(Modifier.height(3.dp))
            if (subtitle == null) TextSkeleton(0.55f, 14.dp) else Dim(subtitle, 14.sp)
        }
        Icon(Icons.right, size = 16.sp, color = Tonearm.dim)
    }
}

/** A group of settings on a card: a logo tile and title, a line about
 *  what it does, then its controls. */
@Composable
private fun Section(
    title: String,
    about: String,
    @DrawableRes logo: Int,
    brand: Color,
    content: @Composable ColumnScope.() -> Unit,
) {
    Card {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(32.dp).clip(RoundedCornerShape(8.dp)).background(brand), contentAlignment = Alignment.Center) {
                    Image(painterResource(logo), null, Modifier.size(18.dp), colorFilter = ColorFilter.tint(Color.White))
                }
                Title(title, size = 16.sp, modifier = Modifier.padding(start = 12.dp))
            }
            if (about.isNotEmpty()) {
                Text(about, color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = 14.sp, lineHeight = 19.sp)
            }
            content()
        }
    }
}

/** A setting: its title and a line under it, its control on the right. */
@Composable
private fun SettingRow(
    title: String,
    subtitle: String?,
    modifier: Modifier = Modifier,
    subtitleSkeleton: Boolean = false,
    control: @Composable () -> Unit,
) {
    Row(modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 16.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Title(title, size = 16.sp, lines = 2)
            if (subtitleSkeleton) {
                Box(Modifier.height(19.dp), contentAlignment = Alignment.CenterStart) { TextSkeleton(0.8f, 14.dp) }
            } else if (!subtitle.isNullOrEmpty()) {
                Text(subtitle, color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = 14.sp, lineHeight = 19.sp)
            }
        }
        control()
    }
}

/** The last row of a service card: whether it is linked, and the button
 *  that links or unlinks it. */
@Composable
private fun AccountRow(
    linked: Boolean,
    title: String,
    subtitle: String = "",
    linkText: String = "Connect",
    unlinkText: String = "Unlink account",
    onLink: () -> Unit,
    onUnlink: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(Tonearm.line))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(end = 16.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(8.dp).clip(CircleShape).background(if (linked) Color(0xFF3BA55D) else Color(0x40FFFFFF)))
                    if (title.isEmpty()) {
                        Box(Modifier.padding(start = 8.dp).height(20.dp).width(140.dp), contentAlignment = Alignment.CenterStart) {
                            TextSkeleton(1f, 16.dp)
                        }
                    } else {
                        Title(title, size = 16.sp, lines = 2, modifier = Modifier.padding(start = 8.dp))
                    }
                }
                if (subtitle.isNotEmpty()) {
                    Text(subtitle, color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = 14.sp, lineHeight = 19.sp)
                }
            }
            if (linked) PillButton(unlinkText, danger = true, onClick = onUnlink)
            else PillButton(linkText, primary = true, onClick = onLink)
        }
    }
}

@Composable
fun PillButton(text: String, primary: Boolean = false, danger: Boolean = false, onClick: () -> Unit) {
    val press = remember { MutableInteractionSource() }
    val pressed by press.collectIsPressedAsState()
    val background = when {
        primary -> if (pressed) Color.White else Color(0xFFF0F0F3)
        danger -> if (pressed) Color(0x33FF4F6D) else Color(0x1AFF4F6D)
        else -> if (pressed) Color(0x24FFFFFF) else Color(0x14FFFFFF)
    }
    Box(
        Modifier.height(36.dp).clip(RoundedCornerShape(18.dp)).background(background)
            .clickable(interactionSource = press, indication = null, onClick = onClick)
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            color = when {
                primary -> Color(0xFF08080A)
                danger -> Color(0xFFFF8095)
                else -> Tonearm.text
            },
            fontFamily = Tonearm.font, fontWeight = FontWeight.SemiBold, fontSize = 14.sp, maxLines = 1,
        )
    }
}

@Composable
internal fun Field(value: String, onChange: (String) -> Unit, placeholder: String, secret: Boolean = false) {
    var focused by remember { mutableStateOf(false) }
    // While focused, the whole pill (and a little room round it) is kept
    // in view, asked again as the keyboard slides up, so it ends clear of
    // the keyboard rather than half under it.
    val requester = remember { androidx.compose.foundation.relocation.BringIntoViewRequester() }
    val density = LocalDensity.current
    val keyboard = WindowInsets.ime.getBottom(density)
    var size by remember { mutableStateOf(androidx.compose.ui.unit.IntSize.Zero) }
    LaunchedEffect(focused, keyboard) {
        if (focused && size.height > 0) {
            val room = with(density) { 16.dp.toPx() }
            requester.bringIntoView(
                androidx.compose.ui.geometry.Rect(0f, -room, size.width.toFloat(), size.height + room),
            )
        }
    }
    Box(
        Modifier.fillMaxWidth().height(40.dp)
            .onSizeChanged { size = it }
            .bringIntoViewRequester(requester)
            .clip(RoundedCornerShape(20.dp))
            .background(if (focused) Color(0x1CFFFFFF) else Color(0x10FFFFFF))
            .border(1.dp, if (focused) Color(0x40FFFFFF) else Color.Transparent, RoundedCornerShape(20.dp))
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        if (value.isEmpty()) Dim(placeholder, 14.sp)
        BasicTextField(
            value = value,
            onValueChange = onChange,
            singleLine = true,
            textStyle = TextStyle(color = Tonearm.text, fontFamily = Tonearm.font, fontSize = 14.sp),
            cursorBrush = SolidColor(Tonearm.accent),
            visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
            modifier = Modifier.fillMaxWidth().onFocusChanged { focused = it.isFocused },
        )
    }
}

/** The desktop's dropdown: the choice and a chevron on a pill; a tap opens
 *  the list under it, the picked one bold with a dot beside it. */
@Composable
private fun Dropdown(choices: Choices, title: String, onPick: (Int) -> Unit) {
    ChoiceDropdown(title, choices.names, choices.selected.toInt(), onPick)
}

@Composable
internal fun ChoiceDropdown(title: String, options: List<String>, selectedIndex: Int, onPick: (Int) -> Unit) {
    var selected by remember(title, options) { mutableIntStateOf(selectedIndex) }
    LaunchedEffect(selectedIndex) { selected = selectedIndex }
    var open by remember { mutableStateOf(false) }
    val press = remember { MutableInteractionSource() }
    val pressed by press.collectIsPressedAsState()
    var buttonWidth by remember { mutableIntStateOf(0) }
    val density = LocalDensity.current
    Box {
        Row(
            Modifier.height(34.dp).widthIn(min = 140.dp).clip(RoundedCornerShape(10.dp))
                .background(if (pressed || open) Color(0x24FFFFFF) else Color(0x14FFFFFF))
                .clickable(interactionSource = press, indication = null) { open = true }
                .onSizeChanged { buttonWidth = it.width }
                .padding(start = 14.dp, end = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            // As wide as its label, at least 140: the chevron at the end.
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text(
                options.getOrElse(selected) { "" },
                color = Tonearm.text, fontFamily = Tonearm.font, fontWeight = FontWeight.SemiBold, fontSize = 14.sp,
                maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                modifier = Modifier.widthIn(max = 150.dp),
            )
            Spacer(Modifier.width(10.dp))
            Icon(Icons.down, size = 14.sp, color = Tonearm.dim)
        }
        // On a phone the choices open in a dialog over the page, the
        // setting's name on top, rather than a list dropping from the pill.
        if (open) {
            Dialog(onDismissRequest = { open = false }) {
                Column(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Tonearm.raised)
                        .border(1.dp, Tonearm.line, RoundedCornerShape(16.dp))
                        .padding(vertical = 12.dp),
                ) {
                    Title(title, size = 18.sp, weight = FontWeight.Bold,
                        modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 6.dp, bottom = 8.dp))
                    Column(
                        Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()).padding(horizontal = 8.dp),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        options.forEachIndexed { i, name ->
                            val on = i == selected
                            Row(
                                Modifier.fillMaxWidth().height(48.dp).clip(RoundedCornerShape(10.dp))
                                    .clickable {
                                        selected = i
                                        open = false
                                        onPick(i)
                                    }
                                    .padding(horizontal = 12.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(
                                    name, color = if (on) Tonearm.text else Tonearm.dim, fontFamily = Tonearm.font,
                                    fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal, fontSize = 16.sp,
                                    modifier = Modifier.weight(1f),
                                )
                                if (on) Box(Modifier.size(8.dp).clip(CircleShape).background(Tonearm.accent))
                            }
                        }
                    }
                }
            }
        }
    }
}
