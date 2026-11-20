package app.tonearm

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.ui.graphics.Color
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.border
import androidx.media3.session.MediaController
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.Suggestions
import uniffi.tonearm_android.isLibraryPage
import uniffi.tonearm_android.Tonearm as Core

enum class Tab(val label: String, val glyph: String) {
    Home("Home", Icons.home),
    Search("Search", Icons.search),
    Library("Library", Icons.library),
}

/** A page opened from a tab, over it: an album, artist or playlist. */
data class Route(val id: String, val params: String?, val title: String)

/**
 * The app: a tab, with the pages opened from it stacked over it, the song
 * playing above the tabs, and Now Playing over it all. Each tab keeps its
 * own stack, so going back to a tab finds it where it was left.
 */
@Composable
fun App(core: Core, controller: MediaController?) {
    val scope = rememberCoroutineScope()
    var tab by remember { mutableStateOf(Tab.Home) }
    val stacks = remember { Tab.entries.associateWith { mutableStateListOf<Route>() } }
    var nowPlayingOpen by remember { mutableStateOf(false) }
    // Whether Now Playing last left by being swiped off; reset as it opens.
    var swipedAway by remember { mutableStateOf(false) }
    LaunchedEffect(nowPlayingOpen) { if (nowPlayingOpen) swipedAway = false }
    // What Now Playing shows above the song: the cover, lyrics or queue.
    var panel by rememberSaveable { mutableStateOf(Panel.Cover) }
    var toast by remember { mutableStateOf<String?>(null) }
    // The item whose menu is open, from a long press.
    var menuItem by remember { mutableStateOf<MenuRequest?>(null) }
    // Bumped when something is saved to the library or an artist followed,
    // so the library's pages load again and show it.
    var libraryEpoch by remember { mutableIntStateOf(0) }
    val now = rememberNowPlaying(controller, core)
    val playingId = now?.let { controller?.currentMediaItem?.mediaId }
    // Whether the song playing is liked: null until YouTube Music says.
    var liked by remember { mutableStateOf<Boolean?>(null) }
    LaunchedEffect(playingId) {
        liked = null
        val id = playingId ?: return@LaunchedEffect
        liked = withContext(Dispatchers.IO) { runCatching { core.isLiked(id) }.getOrNull() }
    }
    // The heart: fills at once, and goes back if YouTube Music says no.
    fun flipLike() {
        val id = playingId ?: return
        val want = liked != true
        liked = want
        scope.launch {
            try {
                withContext(Dispatchers.IO) { core.setLiked(id, want) }
            } catch (e: Exception) {
                if (playingId == id) liked = !want
                toast = e.message ?: "Couldn't change the like"
            }
        }
    }

    // A song that can't play while signed out: YouTube wants a sign in
    // before it gives out streams on many networks, so offer one.
    // Asked once per failed song, and never over the sign-in page itself:
    // coming back from another app (a password manager, say) reads the
    // same failure again.
    var needsSignIn by remember { mutableStateOf(false) }
    var answeredFor by rememberSaveable { mutableStateOf<String?>(null) }
    val failed = now?.error?.let { "${now.id}:$it" }
    LaunchedEffect(failed) {
        val signingIn = stacks.values.any { it.lastOrNull()?.id == SettingsPages.signIn }
        needsSignIn = failed != null && failed != answeredFor && !signingIn && !core.signedIn()
    }

    LaunchedEffect(toast) {
        if (toast != null) {
            delay(3500)
            toast = null
        }
    }

    // Starting something opens Now Playing on the song itself, never on
    // the lyrics or queue left open last time.
    fun openForPlay() {
        panel = Panel.Cover
        nowPlayingOpen = true
    }

    // Every play runs the core off the main thread, and says why if it fails.
    fun runPlay(call: () -> Unit) {
        scope.launch {
            try {
                withContext(Dispatchers.IO) { call() }
            } catch (e: Exception) {
                toast = e.message ?: "Couldn't play that"
            }
        }
    }

    val actions = object : Actions {
        override fun open(id: String, params: String?, title: String) {
            stacks.getValue(tab).add(Route(id, params, title))
        }
        // A song tapped opens Now Playing; it fills in as the song loads.
        override fun playList(items: List<Item>, index: Int) {
            openForPlay()
            runPlay { core.playList(items, index.toUInt()) }
        }
        override fun playRadio(item: Item) {
            openForPlay()
            runPlay { core.playRadio(item) }
        }
        // Anything starting to play opens Now Playing too.
        override fun playItem(item: Item) {
            openForPlay()
            runPlay { core.playItem(item) }
        }
        override fun playWatch(item: Item) {
            openForPlay()
            runPlay { core.playWatch(item) }
        }
        override fun menu(item: Item) {
            // On the library or one of its lists, artists are followed.
            val top = stacks.getValue(tab).lastOrNull()
            val inLibrary = if (top != null) isLibraryPage(top.id) else tab == Tab.Library
            menuItem = MenuRequest(item, inLibrary)
        }
        override val playing: String? get() = playingId
    }

    // What a song's menu does, through the app.
    val host = object : MenuHost {
        override fun run(call: () -> String?) {
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { call() }?.takeIf { it.isNotEmpty() }?.let { toast = it }
                } catch (e: Exception) {
                    toast = e.message ?: "That didn't work"
                }
            }
        }
        override fun play(call: () -> Unit) {
            openForPlay()
            runPlay(call)
        }
        override fun open(id: String, title: String) {
            nowPlayingOpen = false
            stacks.getValue(tab).add(Route(id, null, title))
        }
        override fun toggleLike() = flipLike()
        override fun libraryChanged() {
            scope.launch { libraryEpoch++ }
        }
    }

    val stack = stacks.getValue(tab)
    BackHandler(enabled = menuItem != null || nowPlayingOpen || stack.isNotEmpty() || tab != Tab.Home) {
        when {
            menuItem != null -> menuItem = null
            nowPlayingOpen -> nowPlayingOpen = false
            stack.isNotEmpty() -> stack.removeAt(stack.lastIndex)
            else -> tab = Tab.Home
        }
    }

    // Height of the mini-player and tabs under the pages, in pixels.
    var below by remember { mutableIntStateOf(0) }
    Box(Modifier.fillMaxSize().background(Tonearm.bg)) {
        Column(Modifier.fillMaxSize()) {
            // The pages end above the mini-player and tabs, so a page that
            // makes room for the keyboard counts only the part of it that
            // reaches higher than they do.
            Box(Modifier.weight(1f).consumeWindowInsets(PaddingValues(bottom = with(LocalDensity.current) { below.toDp() }))) {
                val top = stack.lastOrNull()
                if (top != null && SettingsPages.isSettings(top.id)) {
                    SettingsScreen(
                        top.id, core,
                        onOpen = { stack.add(Route(it, null, "")) },
                        onBack = { if (stack.isNotEmpty()) stack.removeAt(stack.lastIndex) },
                        onToast = { toast = it },
                    )
                } else if (top != null) {
                    // Pages run up under the status bar, so an artist's
                    // banner reaches the top edge.
                    PageScreen(top, core, actions, libraryEpoch) { stack.removeAt(stack.lastIndex) }
                } else {
                    when (tab) {
                        // Home runs up under the status bar, so its glows
                        // reach the top edge rather than stopping short.
                        Tab.Home -> Box {
                            HomeScreen(
                                core, actions,
                                onSettings = { stack.add(Route(SettingsPages.root, null, "Settings")) },
                                onAccount = { stack.add(Route(SettingsPages.account, null, "Account")) },
                            )
                            StatusFade()
                        }
                        Tab.Search -> Box(Modifier.statusBarsPadding()) { SearchScreen(core, actions) }
                        Tab.Library -> Box(Modifier.statusBarsPadding()) { LibraryScreen(core, actions, libraryEpoch) }
                    }
                }
            }
            Column(Modifier.onSizeChanged { below = it.height }) {
                if (now != null && controller != null) {
                    MiniPlayer(now, controller) { nowPlayingOpen = true }
                }
                TabBar(tab) { picked ->
                    // The tab already open goes back to its top.
                    if (picked == tab) stacks.getValue(picked).clear() else tab = picked
                }
            }
        }
        AnimatedVisibility(
            visible = nowPlayingOpen && now != null && controller != null,
            enter = slideInVertically { it },
            // Swiped away, it has slid off already: gone at once, so a tap
            // on the mini-player straight after opens a fresh one.
            exit = if (swipedAway) ExitTransition.None else slideOutVertically { it },
        ) {
            if (now != null && controller != null) {
                NowPlayingScreen(
                    now, controller, core,
                    panel = panel,
                    onPanel = { panel = it },
                    onMenu = { menuItem = MenuRequest(it) },
                    liked = liked,
                    onLike = { flipLike() },
                    onOpenArtist = { id, name -> host.open(id, name) },
                    onSwipedAway = { swipedAway = true },
                    open = nowPlayingOpen,
                ) { nowPlayingOpen = false }
            }
        }
        // A song's menu over everything, Now Playing too: the screen dims
        // and the menu rises from the bottom. Tap outside to close it.
        AnimatedVisibility(menuItem != null, enter = fadeIn(), exit = fadeOut()) {
            Box(
                Modifier.fillMaxSize().background(Color(0x99000000)).clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                ) { menuItem = null },
            )
        }
        // The last song shown stays while the menu slides away.
        var shown by remember { mutableStateOf<MenuRequest?>(null) }
        if (menuItem != null) shown = menuItem
        AnimatedVisibility(
            menuItem != null,
            modifier = Modifier.align(Alignment.BottomCenter),
            enter = slideInVertically { it },
            exit = slideOutVertically { it },
        ) {
            shown?.let { ItemMenu(it, core, host) { menuItem = null } }
        }
        toast?.let { Toast(it, Modifier.align(Alignment.BottomCenter).padding(bottom = 150.dp)) }
        if (needsSignIn) {
            SignInDialog(onDismiss = { needsSignIn = false; answeredFor = failed }) {
                needsSignIn = false
                answeredFor = failed
                nowPlayingOpen = false
                stacks.getValue(tab).add(Route(SettingsPages.signIn, null, "Sign in"))
            }
        }
    }
}

/** When a song couldn't play signed out: why, and a way in. Styled as the settings' choice dialogs. */
@Composable
fun SignInDialog(onDismiss: () -> Unit, onSignIn: () -> Unit) {
    androidx.compose.ui.window.Dialog(onDismissRequest = onDismiss) {
        Column(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Tonearm.raised)
                .border(1.dp, Tonearm.line, RoundedCornerShape(16.dp))
                .padding(20.dp),
        ) {
            Title("Sign in to play songs", size = 18.sp, weight = FontWeight.Bold)
            Spacer(Modifier.height(8.dp))
            Text(
                "YouTube needs you signed in to play songs on this network.",
                color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = 15.sp,
            )
            Spacer(Modifier.height(20.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                PillButton("Not now", onClick = onDismiss)
                PillButton("Sign in", primary = true, onClick = onSignIn)
            }
        }
    }
}

/** A fade behind the status bar, so the clock and battery read over a
 *  bright banner or Home's glows, on pages that run up under it. */
@Composable
fun StatusFade() {
    Box(
        Modifier.fillMaxWidth().windowInsetsTopHeight(WindowInsets.statusBars.add(WindowInsets(top = 24.dp)))
            .background(
                androidx.compose.ui.graphics.Brush.verticalGradient(
                    0f to androidx.compose.ui.graphics.Color(0x8C000000),
                    1f to androidx.compose.ui.graphics.Color.Transparent,
                ),
            ),
    )
}

@Composable
fun TabBar(tab: Tab, onPick: (Tab) -> Unit) {
    Column(Modifier.fillMaxWidth().background(Tonearm.bg)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(Tonearm.line))
        Row(Modifier.fillMaxWidth().navigationBarsPadding().height(62.dp)) {
            Tab.entries.forEach { t ->
                val on = t == tab
                Column(
                    Modifier.weight(1f).fillMaxHeight().clickable { onPick(t) },
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    // The page open shows the icon filled, as the desktop's menu.
                    Icon(t.glyph, size = 24.sp, filled = on && t != Tab.Search, color = if (on) Tonearm.text else Tonearm.dim)
                    Spacer(Modifier.height(3.dp))
                    Text(t.label, color = if (on) Tonearm.text else Tonearm.dim, fontFamily = Tonearm.font,
                        fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium, fontSize = 11.sp)
                }
            }
        }
    }
}

@Composable
fun Toast(text: String, modifier: Modifier = Modifier) {
    Box(
        modifier.padding(horizontal = 24.dp).clip(RoundedCornerShape(Tonearm.radius))
            .background(Tonearm.raised).padding(horizontal = 16.dp, vertical = 12.dp),
    ) {
        Title(text, size = 14.sp, weight = FontWeight.Medium, lines = 3)
    }
}

// ------------------------------------------------------------------- tabs

@Composable
fun HomeScreen(core: Core, actions: Actions, onSettings: () -> Unit, onAccount: () -> Unit) {
    val account = rememberAccount(core)
    // A mood chip shows Home again, picked for that mood.
    var chip by remember { mutableStateOf<Pair<String, String?>?>(null) }
    // The chips last shown: they stay up while a chip's page loads, with
    // only the shelves under them loading.
    var lastChips by remember { mutableStateOf<List<uniffi.tonearm_android.Chip>>(emptyList()) }
    PageView(
        topWhileLoading = true,
        key = chip ?: "home",
        load = {
            withContext(Dispatchers.IO) {
                chip?.let { (id, params) -> core.browse(id, params) } ?: core.home()
            }
        },
        more = { token -> withContext(Dispatchers.IO) { core.more(token) } },
        actions = actions,
        top = { page ->
            // Under the status bar: the glows start there, the words below it.
            Column(Modifier.homeGlow(WindowInsets.statusBars.getTop(LocalDensity.current).toFloat()).statusBarsPadding()) {
                // Settings beside the title, as the desktop's menu has it.
                Row(
                    Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 14.dp, bottom = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Title("Home", size = 28.sp, weight = FontWeight.ExtraBold, modifier = Modifier.weight(1f))
                    Box(
                        Modifier.size(44.dp).clip(RoundedCornerShape(22.dp)).clickable(onClick = onSettings),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(Icons.settings, size = 24.sp, color = Tonearm.text)
                    }
                    // The account's picture right of the gear, as the
                    // desktop's account box; opens the account page.
                    Box(
                        Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onAccount),
                        contentAlignment = Alignment.Center,
                    ) {
                        AccountPhoto(account, 32.dp)
                    }
                }
                val chips = page?.chips ?: lastChips
                if (page != null && page.chips.isNotEmpty()) SideEffect { lastChips = page.chips }
                // None yet at all: their bones, as the desktop's Home.
                if (page == null && chips.isEmpty()) ChipsSkeleton()
                if (chips.isNotEmpty()) {
                    LazyRow(
                        contentPadding = PaddingValues(horizontal = 16.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        items(chips) { c ->
                            // While a page loads, the chip just tapped is the
                            // one lit (or none, going back to Home).
                            val on = if (page != null) c.selected else chip == (c.browseId to c.params)
                            Box(
                                Modifier.height(34.dp).clip(RoundedCornerShape(17.dp))
                                    .background(if (on) Tonearm.text else Tonearm.raised)
                                    .clickable {
                                        chip = if (on) null else c.browseId to c.params
                                    }
                                    .padding(horizontal = 14.dp),
                                contentAlignment = Alignment.Center,
                            ) {
                                Text(c.title, color = if (on) Tonearm.bg else Tonearm.text, fontFamily = Tonearm.font,
                                    fontWeight = FontWeight.Medium, fontSize = 14.sp)
                            }
                        }
                    }
                }
            }
        },
    )
}

@Composable
fun LibraryScreen(core: Core, actions: Actions, epoch: Int) {
    PageView(
        key = "library$epoch",
        load = { withContext(Dispatchers.IO) { core.library() } },
        more = { token -> withContext(Dispatchers.IO) { core.more(token) } },
        actions = actions,
        topWhileLoading = true,
        top = {
            Title("Library", size = 28.sp, weight = FontWeight.ExtraBold,
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 4.dp))
        },
    )
}

@Composable
fun SearchScreen(core: Core, actions: Actions) {
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    // Whether the suggestions are up: from the box taking the cursor or
    // typing, until a search, a tap on something, or Back. The keyboard
    // going away leaves them up.
    var suggesting by remember { mutableStateOf(false) }
    // Clearing focus alone can leave the keyboard up.
    fun putAway() {
        suggesting = false
        keyboard?.hide()
        focus.clearFocus()
    }
    var query by rememberSaveable { mutableStateOf("") }
    var searched by rememberSaveable { mutableStateOf<String?>(null) }
    // Which tab of the results: 0 all, then songs, albums, artists,
    // playlists, podcasts. A new search starts on All.
    var searchTab by rememberSaveable { mutableIntStateOf(0) }
    // Anything tapped in the results puts the keyboard away first.
    val tapped = object : Actions by actions {
        override fun open(id: String, params: String?, title: String) { putAway(); actions.open(id, params, title) }
        // A song in search results plays with the queue YouTube Music makes
        // for it, its radio, not the rest of the results.
        override fun playList(items: List<Item>, index: Int) { putAway(); actions.playRadio(items[index]) }
        override fun menu(item: Item) { putAway(); actions.menu(item) }
        override fun playRadio(item: Item) { putAway(); actions.playRadio(item) }
        override fun playItem(item: Item) { putAway(); actions.playItem(item) }
        override fun playWatch(item: Item) { putAway(); actions.playWatch(item) }
    }
    val scope = rememberCoroutineScope()
    var focused by remember { mutableStateOf(false) }
    var recent by remember { mutableStateOf(emptyList<String>()) }
    LaunchedEffect(Unit) { recent = withContext(Dispatchers.IO) { core.recentSearches() } }
    // The list for what is typed: shown at once when fetched a moment ago,
    // else asked for once typing pauses. A newer letter cancels the wait.
    var found by remember { mutableStateOf<Suggestions?>(null) }
    var finding by remember { mutableStateOf(false) }
    LaunchedEffect(query) {
        val typed = query.trim()
        if (typed.isEmpty()) {
            found = null
            finding = false
            return@LaunchedEffect
        }
        core.cachedSuggestions(typed)?.let {
            found = it
            finding = false
            return@LaunchedEffect
        }
        found = null
        finding = true
        delay(core.suggestDelayMs().toLong())
        // A failed list isn't worth a message; Search still searches.
        found = withContext(Dispatchers.IO) { runCatching { core.suggestions(typed) }.getOrNull() }
        finding = false
    }
    fun search(text: String) {
        val words = text.trim()
        if (words.isEmpty()) return
        putAway()
        query = words
        searched = words
        searchTab = 0
        scope.launch {
            recent = withContext(Dispatchers.IO) {
                core.rememberSearch(words)
                core.recentSearches()
            }
        }
    }
    // The list shows from the box taking the cursor, once there is
    // something to list. Back is not one of its steps: with the keyboard up
    // the keyboard hears it first; else it leaves Search for Home, as the
    // app's own Back does (the user asked).
    LaunchedEffect(focused) { if (focused) suggesting = true }
    val listing = suggesting && (query.isNotBlank() || recent.isNotEmpty())
    Column(Modifier.fillMaxSize()) {
        SearchBox(query, { query = it; suggesting = true }, { focused = it }) { search(query) }
        val words = searched
        if (listing) {
            SuggestList(
                typed = query,
                recent = recent,
                found = found,
                loading = finding,
                onSearch = ::search,
                // A song just plays: the list stays as it was under Now
                // Playing, rather than giving way to a search behind it.
                // Anything else opens its page, so the list goes.
                onItem = {
                    if (it.videoId != null) {
                        keyboard?.hide()
                        actions.tap(it)
                    } else {
                        putAway()
                        tapped.tap(it)
                    }
                },
                onItemMenu = { tapped.menu(it) },
            )
        } else if (words == null || query.isBlank()) {
            // Nothing typed: the moods and genres, as Explore lists them.
            MoodsGrid(core) { tapped.open(it.browseId ?: return@MoodsGrid, it.params, it.title) }
        } else {
            // The desktop's tabs over the results. They stay put while a
            // tab loads; only the results under them make way.
            SearchTabs(searchTab) { searchTab = it }
            val tab = searchTab
            PageView(
                key = "search:$words:$tab",
                load = { withContext(Dispatchers.IO) { core.searchTab(words, tab.toUInt()) } },
                more = { token -> withContext(Dispatchers.IO) { core.searchMore(token) } },
                actions = tapped,
                skeleton = PageSkeleton.Rows,
            )
        }
    }
}

/** The desktop's search tabs, in its order. */
private val SEARCH_TABS = listOf("All", "Songs", "Albums", "Artists", "Playlists", "Podcasts")

/** The results' tabs, as chips like Home's: the one showing lit. */
@Composable
fun SearchTabs(picked: Int, onPick: (Int) -> Unit) {
    LazyRow(
        contentPadding = PaddingValues(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.padding(bottom = 4.dp),
    ) {
        items(SEARCH_TABS.size) { i ->
            val on = i == picked
            Box(
                Modifier.height(34.dp).clip(RoundedCornerShape(17.dp))
                    .background(if (on) Tonearm.text else Tonearm.raised)
                    .clickable { if (!on) onPick(i) }
                    .padding(horizontal = 14.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(SEARCH_TABS[i], color = if (on) Tonearm.bg else Tonearm.text, fontFamily = Tonearm.font,
                    fontWeight = FontWeight.Medium, fontSize = 14.sp)
            }
        }
    }
}

@Composable
fun SearchBox(query: String, onChange: (String) -> Unit, onFocus: (Boolean) -> Unit, onSearch: () -> Unit) {
    Row(
        Modifier
            .padding(horizontal = 16.dp, vertical = 12.dp)
            .fillMaxWidth()
            .height(48.dp)
            .clip(RoundedCornerShape(Tonearm.radius))
            .background(Tonearm.island)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.search, color = Tonearm.dim, size = 20.sp)
        Spacer(Modifier.width(12.dp))
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) {
                Text("Search", color = Tonearm.faint, fontFamily = Tonearm.font, fontSize = 16.sp)
            }
            BasicTextField(
                value = query,
                onValueChange = onChange,
                singleLine = true,
                textStyle = TextStyle(color = Tonearm.text, fontFamily = Tonearm.font, fontSize = 16.sp),
                cursorBrush = SolidColor(Tonearm.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { onSearch() }),
                modifier = Modifier.fillMaxWidth().onFocusChanged { onFocus(it.isFocused) },
            )
        }
    }
}

// ------------------------------------------------------------------ pages

/** An album, artist or playlist, with a back arrow over it. */
@Composable
fun PageScreen(route: Route, core: Core, actions: Actions, libraryEpoch: Int, onBack: () -> Unit) {
    Box(Modifier.fillMaxSize()) {
        PageView(
            // A library page loads again when something is saved or followed.
            key = if (isLibraryPage(route.id)) route to libraryEpoch else route,
            load = { withContext(Dispatchers.IO) { core.browse(route.id, route.params) } },
            more = { token -> withContext(Dispatchers.IO) { core.more(token) } },
            actions = actions,
            // What kind of page it is shows in its id, so the skeleton fits
            // before anything has loaded; the rest keep their title up.
            skeleton = when {
                route.id.startsWith("UC") || route.id.startsWith("MPLAUC") -> PageSkeleton.Artist
                route.id.startsWith("MPREb") || route.id.startsWith("VL") || route.id.startsWith("OLAK") -> PageSkeleton.Detail
                else -> PageSkeleton.Shelves
            },
            topWhileLoading = !(route.id.startsWith("UC") || route.id.startsWith("MPLAUC") ||
                route.id.startsWith("MPREb") || route.id.startsWith("VL") || route.id.startsWith("OLAK")),
            top = { page ->
                // Pages without a header (a mood, "Show all") get a title,
                // clear of the status bar and the back button.
                if (page?.header == null) {
                    Title(page?.title?.ifEmpty { null } ?: route.title, size = 26.sp, weight = FontWeight.ExtraBold, lines = 2,
                        modifier = Modifier.statusBarsPadding()
                            .padding(start = 16.dp, end = 16.dp, top = 56.dp, bottom = 4.dp))
                }
            },
        )
        StatusFade()
        Box(
            Modifier.statusBarsPadding().padding(8.dp).size(40.dp).clip(RoundedCornerShape(20.dp))
                .background(androidx.compose.ui.graphics.Color(0x9908080A)).clickable(onClick = onBack),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.back, size = 20.sp)
        }
    }
}
