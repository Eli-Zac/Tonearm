package app.tonearm

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.ItemKind
import uniffi.tonearm_android.Tonearm as Core

/** Liked Music and Episodes for later: shortcuts at the top, not rows. */
private const val LIKED = "VLLM"
private const val EPISODES = "VLSE"

/** What the library's chips show. */
private enum class Shown(val label: String, val kind: ItemKind?) {
    All("All", null),
    Playlists("Playlists", ItemKind.PLAYLIST),
    Albums("Albums", ItemKind.ALBUM),
    Artists("Artists", ItemKind.ARTIST),
}

/**
 * The library as one list, kept between visits like `PageCache`'s pages:
 * every page of it read at once, so a filter or A to Z covers all of it
 * and not only the first page. Main thread only.
 */
private object LibraryStore {
    private const val FRESH_MS = 10 * 60_000L
    /** How many pages of the library to read at most. */
    private const val PAGES = 30

    var epoch = -1
        private set
    var items: List<Item>? = null
        private set
    private var at = 0L
    var index = 0
    var offset = 0

    fun kept(epoch: Int): List<Item>? = items.takeIf { this.epoch == epoch }

    fun fresh(epoch: Int) = kept(epoch) != null && System.currentTimeMillis() - at < FRESH_MS

    /** Reads the library off the main thread. */
    suspend fun load(core: Core, epoch: Int): List<Item> {
        val all = withContext(Dispatchers.IO) {
            val out = ArrayList<Item>()
            val seen = HashSet<String>()
            var page = core.library()
            var pages = 1
            while (true) {
                for (shelf in page.shelves) for (item in shelf.items) {
                    val key = item.browseId ?: item.videoId ?: item.title
                    if (seen.add(key)) out.add(item)
                }
                val next = page.continuation ?: break
                if (pages++ >= PAGES) break
                page = core.more(next)
            }
            out
        }
        if (this.epoch != epoch) {
            index = 0
            offset = 0
        }
        this.epoch = epoch
        items = all
        at = System.currentTimeMillis()
        return all
    }
}

/**
 * Library: the heading, chips to show only playlists, albums or artists,
 * Liked Music and Downloads as shortcuts side by side, a line to sort by,
 * then everything as rows, one look for all of it. It used to be the
 * landing page as it came, a carousel of cards with no title over a
 * list, with Downloads above both.
 */
@Composable
fun LibraryScreen(core: Core, actions: Actions, epoch: Int) {
    var items by remember(epoch) { mutableStateOf(LibraryStore.kept(epoch)) }
    var error by remember(epoch) { mutableStateOf<String?>(null) }
    var shown by rememberSaveable { mutableStateOf(Shown.All) }
    var byName by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(epoch) {
        if (LibraryStore.fresh(epoch)) return@LaunchedEffect
        try {
            items = LibraryStore.load(core, epoch)
            error = null
        } catch (e: Exception) {
            // What was shown stays up when a newer list will not load.
            if (items == null) error = e.message ?: "Couldn't load your library"
        }
    }
    val all = items.orEmpty()
    val liked = all.firstOrNull { it.browseId == LIKED }
    val episodes = all.firstOrNull { it.browseId == EPISODES }
    val rows = remember(all, shown, byName) {
        all.filter { it.browseId != LIKED && it.browseId != EPISODES }
            .filter { shown.kind == null || it.kind == shown.kind }
            .let { list -> if (byName) list.sortedBy { it.title.lowercase() } else list }
    }

    val state = rememberLazyListState(LibraryStore.index, LibraryStore.offset)
    DisposableEffect(Unit) {
        onDispose {
            LibraryStore.index = state.firstVisibleItemIndex
            LibraryStore.offset = state.firstVisibleItemScrollOffset
        }
    }
    val scrolled by remember { derivedStateOf { state.canScrollBackward } }
    Box(Modifier.fillMaxSize()) {
    LazyColumn(state = state, contentPadding = PaddingValues(bottom = 24.dp)) {
        item(contentType = "heading") { Box(Modifier.statusBarsPadding()) { TabHeading("Library") } }
        item(contentType = "chips") {
            LazyRow(
                contentPadding = PaddingValues(horizontal = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(Shown.entries) { s ->
                    Chip(s.label, selected = s == shown) { shown = if (s == shown) Shown.All else s }
                }
            }
        }
        item(contentType = "shortcuts") {
            Shortcuts(core, liked, episodes, loading = items == null && error == null, actions)
        }
        item(contentType = "sort") {
            Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 18.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Dim(
                    when {
                        items == null -> " "
                        rows.size == 1 -> "1 item"
                        else -> "${rows.size} items"
                    },
                    13.sp,
                    modifier = Modifier.weight(1f),
                )
                Box(
                    Modifier.height(32.dp).clip(RoundedCornerShape(CHIP_RADIUS))
                        .clickable { byName = !byName }
                        .padding(horizontal = 8.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Title(if (byName) "A to Z" else "Recent activity", size = 13.sp, color = Tonearm.dim)
                }
            }
        }
        when {
            error != null -> item { Message(error!!) }
            items == null -> items(8) { RowSkeleton(it + 2) }
            rows.isEmpty() -> item {
                Dim(
                    if (shown == Shown.All) "Nothing in your library yet." else "No ${shown.label.lowercase()} in your library.",
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 24.dp),
                )
            }
            else -> items(rows, key = { it.browseId ?: it.videoId ?: it.title }, contentType = { "row" }) { item ->
                ItemRow(item, playing = false, onClick = { actions.tap(item) }, onPlay = { actions.menu(item) })
            }
        }
    }
    StatusBar(scrolled)
    }
}

/** Liked Music and Downloads side by side, and Episodes for later under
 *  them when the library has it. Downloads is there even when the library
 *  can't load, so offline songs are always a tap away. */
@Composable
private fun Shortcuts(core: Core, liked: Item?, episodes: Item?, loading: Boolean, actions: Actions) {
    val downloads = LocalDownloads.current
    val summary by produceState("", downloads.saved) {
        value = withContext(Dispatchers.IO) { core.downloadsSummary() }
    }
    Column(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val half = Modifier.weight(1f)
            when {
                liked != null -> Shortcut(liked.title, "", half, art = { Art(liked.thumb, 56.dp, RoundedCornerShape(0.dp)) }) {
                    actions.tap(liked)
                }
                loading -> Bone(half.height(56.dp), RoundedCornerShape(CHIP_RADIUS))
                else -> Spacer(half)
            }
            Shortcut("Downloads", summary, half, art = { DownloadsTile(56.dp, RoundedCornerShape(0.dp)) }) {
                actions.open(DOWNLOADS_ROUTE, null, "Downloads")
            }
        }
        if (episodes != null) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Shortcut(episodes.title, "", Modifier.weight(1f), art = { Art(episodes.thumb, 56.dp, RoundedCornerShape(0.dp)) }) {
                    actions.tap(episodes)
                }
                Spacer(Modifier.weight(1f))
            }
        }
    }
}

/** A shortcut: art at the left of a faintly lit tile, as the chips are. */
@Composable
private fun Shortcut(title: String, line: String, modifier: Modifier, art: @Composable () -> Unit, onClick: () -> Unit) {
    Row(
        modifier.height(56.dp).clip(RoundedCornerShape(CHIP_RADIUS)).background(Color(0x14FFFFFF)).clickable(onClick = onClick),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        art()
        Column(Modifier.weight(1f).padding(horizontal = 10.dp)) {
            // "Auto playlist" says nothing here, so those have no line and
            // their titles get two.
            Title(title, size = 14.sp, lines = if (line.isEmpty()) 2 else 1)
            if (line.isNotEmpty()) Dim(line, 12.sp)
        }
    }
}
