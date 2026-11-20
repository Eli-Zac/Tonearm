package app.tonearm

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.session.MediaController
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
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
    var toast by remember { mutableStateOf<String?>(null) }
    val now = rememberNowPlaying(controller)
    val playingId = now?.let { controller?.currentMediaItem?.mediaId }

    LaunchedEffect(toast) {
        if (toast != null) {
            delay(3500)
            toast = null
        }
    }

    // Every play runs the core off the main thread, and says why if it fails.
    fun play(call: () -> Unit) {
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
        override fun playList(items: List<Item>, index: Int) = play { core.playList(items, index.toUInt()) }
        override fun playRadio(item: Item) = play { core.playRadio(item) }
        override fun playItem(item: Item) = play { core.playItem(item) }
        override fun playWatch(item: Item) = play { core.playWatch(item) }
        override val playing: String? get() = playingId
    }

    val stack = stacks.getValue(tab)
    BackHandler(enabled = nowPlayingOpen || stack.isNotEmpty() || tab != Tab.Home) {
        when {
            nowPlayingOpen -> nowPlayingOpen = false
            stack.isNotEmpty() -> stack.removeAt(stack.lastIndex)
            else -> tab = Tab.Home
        }
    }

    Box(Modifier.fillMaxSize().background(Tonearm.bg)) {
        Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f).statusBarsPadding()) {
                val top = stack.lastOrNull()
                if (top != null) {
                    PageScreen(top, core, actions) { stack.removeAt(stack.lastIndex) }
                } else {
                    when (tab) {
                        Tab.Home -> HomeScreen(core, actions)
                        Tab.Search -> SearchScreen(core, actions)
                        Tab.Library -> LibraryScreen(core, actions)
                    }
                }
            }
            if (now != null && controller != null) {
                MiniPlayer(now, controller) { nowPlayingOpen = true }
            }
            TabBar(tab) { picked ->
                // The tab already open goes back to its top.
                if (picked == tab) stacks.getValue(picked).clear() else tab = picked
            }
        }
        toast?.let { Toast(it, Modifier.align(Alignment.BottomCenter).padding(bottom = 150.dp)) }
        AnimatedVisibility(
            visible = nowPlayingOpen && now != null && controller != null,
            enter = slideInVertically { it },
            exit = slideOutVertically { it },
        ) {
            if (now != null && controller != null) {
                NowPlayingScreen(now, controller, actions) { nowPlayingOpen = false }
            }
        }
    }
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
fun HomeScreen(core: Core, actions: Actions) {
    // A mood chip shows Home again, picked for that mood.
    var chip by remember { mutableStateOf<Pair<String, String?>?>(null) }
    PageView(
        key = chip ?: "home",
        load = {
            withContext(Dispatchers.IO) {
                chip?.let { (id, params) -> core.browse(id, params) } ?: core.home()
            }
        },
        more = { token -> withContext(Dispatchers.IO) { core.more(token) } },
        actions = actions,
        top = { page ->
            Column {
                Title("Home", size = 28.sp, weight = FontWeight.ExtraBold,
                    modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 10.dp))
                if (page.chips.isNotEmpty()) {
                    LazyRow(
                        contentPadding = PaddingValues(horizontal = 16.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        items(page.chips) { c ->
                            val on = c.selected
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
fun LibraryScreen(core: Core, actions: Actions) {
    PageView(
        key = "library",
        load = { withContext(Dispatchers.IO) { core.library() } },
        more = { token -> withContext(Dispatchers.IO) { core.more(token) } },
        actions = actions,
        top = {
            Title("Library", size = 28.sp, weight = FontWeight.ExtraBold,
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 4.dp))
        },
    )
}

@Composable
fun SearchScreen(core: Core, actions: Actions) {
    val focus = LocalFocusManager.current
    var query by rememberSaveable { mutableStateOf("") }
    var searched by rememberSaveable { mutableStateOf<String?>(null) }
    Column(Modifier.fillMaxSize()) {
        SearchBox(query, { query = it }) {
            focus.clearFocus()
            searched = query.trim().ifEmpty { null }
        }
        val words = searched
        if (words == null) {
            Message("Search songs, albums, artists and playlists")
        } else {
            PageView(
                key = "search:$words",
                load = { withContext(Dispatchers.IO) { core.searchPage(words) } },
                more = { token -> withContext(Dispatchers.IO) { core.more(token) } },
                actions = actions,
            )
        }
    }
}

@Composable
fun SearchBox(query: String, onChange: (String) -> Unit, onSearch: () -> Unit) {
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
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

// ------------------------------------------------------------------ pages

/** An album, artist or playlist, with a back arrow over it. */
@Composable
fun PageScreen(route: Route, core: Core, actions: Actions, onBack: () -> Unit) {
    Box(Modifier.fillMaxSize()) {
        PageView(
            key = route,
            load = { withContext(Dispatchers.IO) { core.browse(route.id, route.params) } },
            more = { token -> withContext(Dispatchers.IO) { core.more(token) } },
            actions = actions,
            top = { page ->
                // Pages without a header (a mood, "Show all") get a title.
                if (page.header == null) {
                    Title(page.title.ifEmpty { route.title }, size = 26.sp, weight = FontWeight.ExtraBold, lines = 2,
                        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 56.dp, bottom = 4.dp))
                }
            },
        )
        Box(
            Modifier.padding(8.dp).size(40.dp).clip(RoundedCornerShape(20.dp))
                .background(androidx.compose.ui.graphics.Color(0x9908080A)).clickable(onClick = onBack),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.back, size = 20.sp)
        }
    }
}
