package app.tonearm

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.compose.AsyncImage
import uniffi.tonearm_android.Header
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.ItemKind
import uniffi.tonearm_android.Page
import uniffi.tonearm_android.Shelf
import uniffi.tonearm_android.ShelfLayout
import uniffi.tonearm_android.Trend

/** What a tap on an item does, decided as the desktop does. */
interface Actions {
    /** Opens an album, artist or playlist page. */
    fun open(id: String, params: String?, title: String)
    /** A song in a list: plays the list from it. */
    fun playList(items: List<Item>, index: Int)
    /** A song card: plays it, then its radio. */
    fun playRadio(item: Item)
    /** A card's play button: plays the album, playlist, artist or mix. */
    fun playItem(item: Item)
    /** An artist's shuffle or radio. */
    fun playWatch(item: Item)
    /** An item's menu, on a long press. */
    fun menu(item: Item)
    /** The video id playing, to mark its row. */
    val playing: String?
}

/** A tap on a card or row that isn't a song in a list. */
fun Actions.tap(item: Item) {
    when {
        item.videoId != null -> playRadio(item)
        item.browseId != null -> open(item.browseId!!, item.params, item.title)
        item.playlistId != null -> playItem(item)
    }
}

// ------------------------------------------------------------------ text

@Composable
fun Title(text: String, size: TextUnit = 16.sp, color: Color = Tonearm.text, weight: FontWeight = FontWeight.SemiBold, lines: Int = 1, modifier: Modifier = Modifier) {
    Text(text, modifier = modifier, color = color, fontFamily = Tonearm.font, fontWeight = weight,
        fontSize = size, maxLines = lines, overflow = TextOverflow.Ellipsis, lineHeight = size * 1.25f)
}

@Composable
fun Dim(text: String, size: TextUnit = 14.sp, lines: Int = 1, modifier: Modifier = Modifier) {
    Text(text, modifier = modifier, color = Tonearm.dim, fontFamily = Tonearm.font, fontSize = size,
        maxLines = lines, overflow = TextOverflow.Ellipsis, lineHeight = size * 1.3f)
}

@Composable
fun Art(url: String?, size: Dp, shape: Shape = RoundedCornerShape(8.dp), modifier: Modifier = Modifier) {
    // A bone sweeps where the cover is coming, as the desktop's
    // `ArtPending`; one that never comes stops it after a while, so an
    // idle screen stays idle.
    val model = url?.ifEmpty { null }
    var loading by remember(model) { mutableStateOf(model != null) }
    LaunchedEffect(model) {
        kotlinx.coroutines.delay(8000)
        loading = false
    }
    Box(modifier.size(size).clip(shape).background(Tonearm.raised)) {
        if (loading) Bone(Modifier.matchParentSize(), androidx.compose.ui.graphics.RectangleShape)
        AsyncImage(
            model = model,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            onState = {
                if (it is coil3.compose.AsyncImagePainter.State.Success || it is coil3.compose.AsyncImagePainter.State.Error) {
                    loading = false
                }
            },
            modifier = Modifier.matchParentSize(),
        )
    }
}

/** What a row says under its title: the artist and album for a song,
 *  YouTube Music's own line for the rest. */
fun Item.line(): String =
    if (kind == ItemKind.SONG && artist.isNotEmpty()) {
        listOf(artist, album).filter { it.isNotEmpty() }.joinToString(" • ")
    } else {
        subtitle
    }

fun Item.round(): Boolean = kind == ItemKind.ARTIST

// ------------------------------------------------------------------ rows

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ItemRow(item: Item, playing: Boolean, onClick: () -> Unit, onPlay: (() -> Unit)? = null, rank: Int = 0) {
    Row(
        Modifier
            .fillMaxWidth()
            .combinedClickable(onClick = onClick, onLongClick = onPlay)
            .padding(horizontal = 16.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (rank > 0) Rank(rank, item.trend, playing)
        Art(item.thumb, 50.dp, if (item.round()) CircleShape else RoundedCornerShape(8.dp))
        Column(Modifier.weight(1f).padding(start = 14.dp)) {
            Title(item.title, color = if (playing) Tonearm.accent else Tonearm.text)
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (item.explicit) Explicit()
                Dim(item.line())
            }
        }
        if (item.duration.isNotEmpty()) Dim(item.duration, 13.sp, modifier = Modifier.padding(start = 8.dp))
    }
}

/**
 * A chart row's place before its art, as the desktop's numbered rows: the
 * number, and where the chart marks it, a green rise, a red fall or a grey
 * dot for no change, left of it.
 */
@Composable
fun Rank(rank: Int, trend: Trend, playing: Boolean) {
    val charted = trend != Trend.NONE
    Row(
        Modifier.width(if (charted) 46.dp else 28.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (charted) {
            Box(Modifier.width(14.dp), contentAlignment = Alignment.CenterStart) {
                when (trend) {
                    Trend.UP -> Canvas(Modifier.size(12.dp, 8.dp)) {
                        drawPath(Path().apply {
                            moveTo(size.width / 2, 0f); lineTo(size.width, size.height); lineTo(0f, size.height); close()
                        }, Color(0xFF3EC46D))
                    }
                    Trend.DOWN -> Canvas(Modifier.size(12.dp, 8.dp)) {
                        drawPath(Path().apply {
                            moveTo(0f, 0f); lineTo(size.width, 0f); lineTo(size.width / 2, size.height); close()
                        }, Color(0xFFFF4F5E))
                    }
                    else -> Box(Modifier.padding(start = 3.5.dp).size(5.dp).clip(CircleShape).background(Tonearm.faint))
                }
            }
        }
        Text(
            rank.toString(), modifier = Modifier.weight(1f), color = if (playing) Tonearm.accent else Tonearm.dim,
            fontFamily = Tonearm.font, fontSize = 16.sp, textAlign = TextAlign.Center, maxLines = 1,
        )
    }
    Spacer(Modifier.width(12.dp))
}

/** The small "E" before an explicit song's artist. */
@Composable
fun Explicit() {
    Box(
        Modifier.padding(end = 6.dp).size(14.dp).clip(RoundedCornerShape(3.dp)).background(Tonearm.faint),
        contentAlignment = Alignment.Center,
    ) {
        Text("E", color = Tonearm.bg, fontFamily = Tonearm.font, fontWeight = FontWeight.Bold, fontSize = 9.sp, lineHeight = 9.sp)
    }
}

// ----------------------------------------------------------------- cards

@Composable
fun ItemCard(item: Item, width: Dp, wide: Boolean, actions: Actions) {
    val shape = if (item.round()) CircleShape else RoundedCornerShape(Tonearm.radius)
    val artHeight = if (wide) width * 9f / 16f else width
    Column(
        Modifier.width(width).combinedClickable(
            onClick = { actions.tap(item) },
            // Its menu, on a long press, as the desktop's right click.
            onLongClick = { actions.menu(item) },
        ),
    ) {
        Box {
            AsyncImage(
                model = item.thumb.ifEmpty { null },
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.width(width).height(artHeight).clip(shape).background(Tonearm.raised),
            )
            // Albums, playlists and mixes play from the card, as on desktop.
            if (item.videoId == null && !item.round()) {
                Box(
                    Modifier
                        .align(Alignment.BottomEnd)
                        .padding(8.dp)
                        .size(36.dp)
                        .clip(CircleShape)
                        .background(Color(0xCC08080A))
                        .clickable { actions.playItem(item) },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(Icons.play, size = 18.sp, filled = true)
                }
            }
        }
        Spacer(Modifier.height(8.dp))
        // One line, cut short: a shelf is as tall as its tallest card on
        // screen, so a title wrapping to two would make it jump as that
        // card scrolled into view.
        Title(item.title, size = 14.sp, lines = 1,
            modifier = if (item.round()) Modifier.fillMaxWidth() else Modifier)
        Dim(item.line(), 13.sp, lines = 1)
    }
}

// ---------------------------------------------------------------- shelves

fun LazyListScope.shelf(shelf: Shelf, actions: Actions) {
    if (shelf.title.isNotEmpty()) {
        item(contentType = "title") {
            Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 22.dp, bottom = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Title(shelf.title, size = 20.sp, weight = FontWeight.Bold, modifier = Modifier.weight(1f))
                // A quiet "Show all" on the right, as the desktop's shelf titles.
                shelf.moreId?.let { id ->
                    Box(
                        Modifier.height(28.dp).clip(RoundedCornerShape(14.dp))
                            .clickable { actions.open(id, shelf.moreParams, shelf.title) }
                            .padding(horizontal = 8.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Title("Show all", size = 14.sp, color = Tonearm.dim)
                    }
                }
            }
        }
    }
    when (shelf.layout) {
        ShelfLayout.LIST -> itemsIndexed(shelf.items, contentType = { _, _ -> "row" }) { i, item ->
            ItemRow(
                item,
                playing = item.videoId != null && item.videoId == actions.playing,
                onClick = { if (item.videoId != null) actions.playList(shelf.items, i) else actions.tap(item) },
                // A long press: its menu.
                onPlay = { actions.menu(item) },
                rank = if (shelf.ranked) i + 1 else 0,
            )
        }
        // Pages to open and nothing to show for them, such as Explore's
        // "New releases", "Charts" and "Moods & genres": buttons, not
        // empty cards.
        ShelfLayout.CARDS if shelf.items.all { it.thumb.isEmpty() && it.videoId == null } -> item(contentType = "buttons") {
            LazyRow(
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(shelf.items) { item ->
                    Box(
                        Modifier.height(44.dp).clip(RoundedCornerShape(Tonearm.radius)).background(Tonearm.raised)
                            .clickable { actions.tap(item) }.padding(horizontal = 16.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Title(item.title, size = 15.sp)
                    }
                }
            }
        }
        ShelfLayout.CARDS, ShelfLayout.WIDE -> item(contentType = "cards") {
            val wide = shelf.layout == ShelfLayout.WIDE
            LazyRow(
                contentPadding = PaddingValues(horizontal = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                items(shelf.items) { ItemCard(it, if (wide) 260.dp else 150.dp, wide, actions) }
            }
        }
        ShelfLayout.GRID -> item(contentType = "grid") {
            // Four songs to a column, columns side by side: "Quick picks".
            LazyRow(contentPadding = PaddingValues(end = 16.dp)) {
                items(shelf.items.chunked(4)) { column ->
                    Column(Modifier.width(320.dp)) {
                        column.forEach { item ->
                            ItemRow(
                                item,
                                playing = item.videoId != null && item.videoId == actions.playing,
                                onClick = { actions.tap(item) },
                                onPlay = { actions.menu(item) },
                            )
                        }
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------- header

@Composable
fun PageHeader(header: Header, songs: List<Item>, actions: Actions) {
    Column(Modifier.fillMaxWidth()) {
        if (header.banner) {
            // An artist: their banner across the top, fading into the page.
            Box(Modifier.fillMaxWidth().aspectRatio(1.25f)) {
                AsyncImage(
                    model = header.thumb.ifEmpty { null },
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
                Box(
                    Modifier.fillMaxSize().background(
                        Brush.verticalGradient(0.45f to Color.Transparent, 1f to Tonearm.bg),
                    ),
                )
                Title(header.title, size = 40.sp, weight = FontWeight.ExtraBold, lines = 2,
                    modifier = Modifier.align(Alignment.BottomStart).padding(horizontal = 16.dp))
            }
        } else {
            // Square art sits below the status bar and the back button.
            Box(Modifier.fillMaxWidth().statusBarsPadding().padding(top = 48.dp), contentAlignment = Alignment.Center) {
                Art(header.thumb, 240.dp, RoundedCornerShape(Tonearm.radius))
            }
            Spacer(Modifier.height(18.dp))
            Title(header.title, size = 26.sp, weight = FontWeight.ExtraBold, lines = 3,
                modifier = Modifier.padding(horizontal = 16.dp))
        }
        Column(Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
            if (header.byline.isNotEmpty()) {
                Title(
                    header.byline, size = 15.sp, weight = FontWeight.Medium,
                    modifier = Modifier.clickable(enabled = header.bylineId != null) {
                        header.bylineId?.let { actions.open(it, null, header.byline) }
                    },
                )
            }
            val meta = listOf(header.label, header.meta).filter { it.isNotEmpty() }.joinToString(" • ")
            if (meta.isNotEmpty()) Dim(meta, 13.sp, lines = 2, modifier = Modifier.padding(top = 2.dp))
        }
        Row(
            Modifier.padding(horizontal = 16.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            val play: (() -> Unit)? = when {
                header.shuffle != null && header.banner -> null
                header.playlistId != null -> { { actions.playItem(playlistItem(header.playlistId!!, header.title)) } }
                songs.isNotEmpty() -> { { actions.playList(songs, 0) } }
                else -> null
            }
            play?.let { RoundButton(Icons.play, accent = true, onClick = it) }
            header.shuffle?.let { s -> PillButton(Icons.shuffle, "Shuffle") { actions.playWatch(s) } }
            header.radio?.let { r -> PillButton(Icons.radio, "Radio") { actions.playWatch(r) } }
        }
        if (header.about.isNotEmpty() && header.banner) {
            Dim(header.about, 13.sp, lines = 3, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
        }
    }
}

fun playlistItem(playlistId: String, title: String) = Item(
    kind = ItemKind.PLAYLIST, title = title, subtitle = "", thumb = "", duration = "",
    videoId = null, browseId = null, playlistId = playlistId, params = null,
    artist = "", album = "", explicit = false, artists = emptyList(), albumId = null,
    libraryPlaylist = null, saved = false, color = null, trend = Trend.NONE,
)

@Composable
fun RoundButton(glyph: String, accent: Boolean = false, size: Dp = 52.dp, onClick: () -> Unit) {
    Box(
        Modifier.size(size).clip(CircleShape)
            .background(if (accent) Tonearm.accent else Tonearm.text)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(glyph, size = (size.value * 0.42f).sp, filled = true, color = if (accent) Tonearm.text else Tonearm.bg)
    }
}

@Composable
fun PillButton(glyph: String, label: String, onClick: () -> Unit) {
    Row(
        Modifier.height(40.dp).clip(RoundedCornerShape(20.dp)).background(Tonearm.raised)
            .clickable(onClick = onClick).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(glyph, size = 18.sp)
        Spacer(Modifier.width(8.dp))
        Title(label, size = 14.sp)
    }
}

// ------------------------------------------------------------------- pages

/** The songs listed down a page, for its play button. */
fun Page.songs(): List<Item> =
    shelves.filter { it.layout == ShelfLayout.LIST }.flatMap { it.items }.filter { it.videoId != null }

/**
 * A page of shelves, loaded with `load`, with more shelves loaded as it is
 * scrolled to the end. `top` goes above the shelves.
 */
@Composable
fun PageView(
    key: Any,
    load: suspend () -> Page,
    more: suspend (String) -> Page,
    actions: Actions,
    /** Above the shelves. Given null while the page loads, when
     *  `topWhileLoading`: Home and Library keep their heading (and Home its
     *  chips) up then, with only what is under them loading. */
    top: (@Composable (Page?) -> Unit)? = null,
    topWhileLoading: Boolean = false,
    /** What stands in for the page while it loads, as on the desktop. */
    skeleton: PageSkeleton = PageSkeleton.Shelves,
) {
    var page by remember(key) { mutableStateOf<Page?>(null) }
    var error by remember(key) { mutableStateOf<String?>(null) }
    var loadingMore by remember(key) { mutableStateOf(false) }
    LaunchedEffect(key) {
        try {
            page = load()
        } catch (e: Exception) {
            error = e.message ?: "Couldn't load this page"
        }
    }
    val shown = page
    when {
        error != null && shown == null -> Message(error!!)
        shown == null -> Column(Modifier.fillMaxSize()) {
            if (topWhileLoading) top?.invoke(null)
            PageSkeletonView(skeleton)
        }
        else -> {
            val state = rememberLazyListState()
            // Near the end: the next shelves.
            val nearEnd by remember(key) {
                derivedStateOf {
                    val last = state.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
                    last >= state.layoutInfo.totalItemsCount - 3
                }
            }
            LaunchedEffect(nearEnd, shown.continuation) {
                val token = shown.continuation
                if (nearEnd && token != null && !loadingMore) {
                    loadingMore = true
                    try {
                        val next = more(token)
                        page = shown.copy(shelves = shown.shelves + next.shelves, continuation = next.continuation)
                    } catch (e: Exception) {
                        page = shown.copy(continuation = null)
                    } finally {
                        loadingMore = false
                    }
                }
            }
            LazyColumn(state = state, contentPadding = PaddingValues(bottom = 24.dp)) {
                top?.let { t -> item(contentType = "top") { t(shown) } }
                shown.header?.let { h -> item(contentType = "header") { PageHeader(h, shown.songs(), actions) } }
                shown.shelves.forEach { shelf(it, actions) }
                // The next shelves on their way: rows of bones, as the
                // desktop shows under a list still loading.
                if (loadingMore) items(3) { RowSkeleton(it + 5) }
            }
        }
    }
}

@Composable
fun Loading(modifier: Modifier = Modifier.fillMaxSize()) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(color = Tonearm.accent, strokeWidth = 2.dp, modifier = Modifier.size(28.dp))
    }
}

@Composable
fun Message(text: String) {
    Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
        Dim(text, 15.sp, lines = 6)
    }
}
