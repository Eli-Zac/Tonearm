package app.tonearm

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.webkit.CookieManager
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Tonearm as Core

/** Google's sign in, going on to YouTube Music once signed in. */
private const val YOUTUBE_SIGN_IN =
    "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&passive=true" +
        "&continue=https%3A%2F%2Fmusic.youtube.com%2F"
private const val YOUTUBE_MUSIC = "https://music.youtube.com"

private const val SPOTIFY_SIGN_IN = "https://accounts.spotify.com/login?continue=https%3A%2F%2Fopen.spotify.com%2F"
private const val SPOTIFY_WEB = "https://open.spotify.com"

/**
 * YouTube Music's sign in, in a web view: once the page holds a signed-in
 * cookie for music.youtube.com, the core signs in with it (and checks the
 * account), and this goes back to the account page. The cookie is kept by
 * the core alone; the web view's own copy is cleared after.
 */
@Composable
fun YouTubeSignIn(core: Core, onBack: () -> Unit, onToast: (String) -> Unit) {
    SignInWeb(
        title = "Sign in to YouTube Music",
        start = YOUTUBE_SIGN_IN,
        cookiesOf = YOUTUBE_MUSIC,
        // Signed in only once on music.youtube.com itself: Google's pages
        // set SAPISID a step before YouTube Music has its own.
        ready = { url, cookies -> url.startsWith(YOUTUBE_MUSIC) && core.isSignInCookie(cookies) },
        accept = { cookies ->
            if (core.signIn(cookies)) {
                Prefs.accountNote = ""
                val name = core.account()?.name
                if (name != null) "Signed in as $name" else "Signed in"
            } else {
                Prefs.accountNote = "YouTube Music did not accept that sign in. Try again."
                null
            }
        },
        onBack = onBack,
        onToast = onToast,
    )
}

/** Spotify's web sign in, for its sp_dc cookie, which the desktop has
 *  copied from a browser by hand. */
@Composable
fun SpotifySignIn(core: Core, onBack: () -> Unit, onToast: (String) -> Unit) {
    SignInWeb(
        title = "Sign in to Spotify",
        start = SPOTIFY_SIGN_IN,
        cookiesOf = SPOTIFY_WEB,
        ready = { _, cookies -> cookies.split(';').any { it.trim().startsWith("sp_dc=") } },
        accept = { cookies ->
            if (core.spotifySignIn(cookies)) {
                Prefs.servicesNote = "Spotify sign in connected"
                "Spotify sign in connected"
            } else {
                null
            }
        },
        onBack = onBack,
        onToast = onToast,
    )
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun SignInWeb(
    title: String,
    start: String,
    cookiesOf: String,
    ready: (url: String, cookies: String) -> Boolean,
    /** Signs in with the cookies, off the main thread: what to say, or
     *  null when it didn't take. */
    accept: (String) -> String?,
    onBack: () -> Unit,
    onToast: (String) -> Unit,
) {
    var url by remember { mutableStateOf("") }
    var checking by remember { mutableStateOf(false) }
    var done by remember { mutableStateOf(false) }
    // The page is watched as it goes, and every second, since sign ins
    // finish through scripts as much as through page loads.
    LaunchedEffect(Unit) {
        while (!done) {
            delay(1000)
            val cookies = CookieManager.getInstance().getCookie(cookiesOf) ?: continue
            if (checking || !ready(url, cookies)) continue
            checking = true
            val said = withContext(Dispatchers.IO) { runCatching { accept(cookies) }.getOrNull() }
            checking = false
            if (said != null) {
                done = true
                Prefs.changed()
                onToast(said)
                onBack()
            } else {
                done = true
                onBack()
            }
        }
    }
    // The core has its own copy; the web view keeps none.
    DisposableEffect(Unit) {
        onDispose {
            CookieManager.getInstance().removeAllCookies(null)
            CookieManager.getInstance().flush()
        }
    }
    Column(Modifier.fillMaxSize().background(Tonearm.bg).statusBarsPadding().imePadding()) {
        Row(Modifier.fillMaxWidth().height(56.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(56.dp)) { BackButtonInline(onBack) }
            Title(title, size = 18.sp, weight = FontWeight.Bold, modifier = Modifier.weight(1f))
            // Keeps its room while the sign in is checked.
            Box(Modifier.width(64.dp).padding(end = 16.dp), contentAlignment = Alignment.CenterEnd) {
                if (checking) Dim("Checking", 13.sp)
            }
        }
        AndroidView(
            modifier = Modifier.fillMaxWidth().weight(1f),
            factory = { context ->
                // A fresh sign in each time, not the last one's.
                CookieManager.getInstance().removeAllCookies(null)
                WebView(context).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    // Google turns embedded web views away; without the web
                    // view marks it is the phone's own Chrome to them.
                    settings.userAgentString = settings.userAgentString
                        .replace("; wv", "")
                        .replace(Regex("Version/\\S+ "), "")
                    CookieManager.getInstance().setAcceptCookie(true)
                    CookieManager.getInstance().setAcceptThirdPartyCookies(this, true)
                    setBackgroundColor(android.graphics.Color.rgb(8, 8, 10))
                    webViewClient = object : WebViewClient() {
                        override fun onPageStarted(view: WebView, address: String, favicon: Bitmap?) {
                            url = address
                        }
                        override fun onPageFinished(view: WebView, address: String) {
                            url = address
                        }
                    }
                    loadUrl(start)
                }
            },
            onRelease = { it.destroy() },
        )
    }
}

@Composable
private fun BackButtonInline(onBack: () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Box(
            Modifier.size(40.dp).clip(RoundedCornerShape(20.dp))
                .background(androidx.compose.ui.graphics.Color(0x14FFFFFF))
                .clickable(onClick = onBack),
            contentAlignment = Alignment.Center,
        ) { Icon(Icons.back, size = 20.sp) }
    }
}
