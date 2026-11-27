package app.tonearm

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.graphicsLayer
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.RectangleShape
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import uniffi.tonearm_android.Header
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.ItemKind
import uniffi.tonearm_android.Link
import uniffi.tonearm_android.MixRow
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
    /** A song on a playlist page, retaining that playlist as queue source. */
    fun playListFromPlaylist(items: List<Item>, index: Int, playlistId: String) = playList(items, index)
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
    /** An album's moving cover as a local file, when it has one and
     *  Settings shows moving covers. Blocks. */
    fun albumMotion(album: String, artist: String, firstSong: String): String? = null
    /** Saves an album or playlist to the library, or takes it out. */
    fun setSaved(playlistId: String, save: Boolean, title: String) {}
    /** Reads or changes a playlist's persistent mix mode. */
    fun playlistMixEnabled(playlistId: String): Boolean = false
    fun setPlaylistMix(playlistId: String, on: Boolean, onComplete: () -> Unit = {}) = onComplete()
    /** Tempo, key and transition labels for the songs of a mixed playlist. */
    fun mixRows(playlistId: String, videoIds: List<String>): List<MixRow> = emptyList()
    /** Opens the editor for the transition from one song into the next. */
    fun editMix(from: Item, to: Item) {}
    /** Follows an artist, or stops. */
    fun follow(channelId: String, follow: Boolean, name: String) {}
    /** Tells the user something, in a toast. */
    fun notify(text: String) {}
    /** Saves songs on the phone, to play offline. */
    fun download(items: List<Item>) {}
    /** Saves every song of an album or playlist, fetched first. */
    fun downloadCollection(item: Item) {}
    /** Deletes these songs' downloads. */
    fun removeDownloads(videoIds: List<String>) {}
    /** An album, artist or playlist page was opened from a card or row:
     *  the history remembers it for the profile. */
    fun opened(item: Item) {}
}

/** A tap on a card or row that isn't a song in a list. */
fun Actions.tap(item: Item) {
    when {
        item.videoId != null -> playRadio(item)
        item.browseId != null -> {
            opened(item)
            open(item.browseId!!, item.params, item.title)
        }
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
fun Item.line(showAlbum: Boolean = true): String =
    if (kind == ItemKind.SONG && artist.isNotEmpty()) {
        listOf(artist, if (showAlbum) album else "").filter { it.isNotEmpty() }.joinToString(" • ")
    } else {
        subtitle
    }

fun Item.round(): Boolean = kind == ItemKind.ARTIST

// ------------------------------------------------------------------ rows

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ItemRow(item: Item, playing: Boolean, onClick: () -> Unit, onPlay: (() -> Unit)? = null, rank: Int = 0, showAlbum: Boolean = true) {
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
                Dim(item.line(showAlbum))
            }
        }
        // A song's download slot, kept whether or not it is saved, as the
        // desktop's rows: saved, downloading, or empty.
        item.videoId?.let { DownloadMark(it) }
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

fun LazyListScope.shelf(
    shelf: Shelf,
    actions: Actions,
    onAlbum: Boolean = false,
    mixing: Boolean = false,
    mixRows: Map<String, MixRow> = emptyMap(),
    playlistId: String? = null,
    mixItems: Map<String, Item> = emptyMap(),
) {
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
            Column {
                ItemRow(
                    item,
                    playing = item.videoId != null && item.videoId == actions.playing,
                    onClick = {
                        if (item.videoId != null) {
                            if (playlistId != null) actions.playListFromPlaylist(shelf.items, i, playlistId)
                            else actions.playList(shelf.items, i)
                        } else actions.tap(item)
                    },
                    // A long press: its menu.
                    onPlay = { actions.menu(item) },
                    rank = if (shelf.ranked) i + 1 else 0,
                    // An album's own page needs no album name on every row.
                    showAlbum = !onAlbum,
                )
                if (mixing && item.videoId != null) {
                    MixRowLine(mixRows[item.videoId]) { row ->
                        row.toVideoId?.let { next ->
                            val from = mixItems[row.fromVideoId]
                            val to = mixItems[next]
                            if (from != null && to != null) actions.editMix(from, to)
                        }
                    }
                }
            }
        }
        // Pages to open and nothing to show for them, such as Explore's
        // "New releases", "Charts" and "Moods & genres": buttons, not
        // empty cards.
        ShelfLayout.CARDS if shelf.items.all { it.thumb.isEmpty() && it.videoId == null } -> item(contentType = "buttons") {
            LazyRow(
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(shelf.items) { item -> Chip(item.title) { actions.tap(item) } }
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
fun PageHeader(
    header: Header,
    songs: List<Item>,
    actions: Actions,
    complete: Boolean = true,
    canMix: Boolean = false,
    mixing: Boolean? = false,
    onMixToggle: (Boolean) -> Unit = {},
    /** The page's id and params, as it was opened. */
    pageId: String? = null,
    pageParams: String? = null,
) {
    Column(Modifier.fillMaxWidth()) {
        // Every page's art fills the top, up under the status bar, fading
        // into the page with the title over it: an artist's wide banner,
        // or an album or playlist's square cover, whole.
        // An album's moving cover, where Apple Music has one and Settings
        // allows it, plays over the still once its first frame is drawn.
        val album = !header.banner && header.label in setOf("Album", "EP", "Single")
        var motion by remember(header.title, header.byline) { mutableStateOf<String?>(null) }
        if (album) {
            LaunchedEffect(header.title, header.byline) {
                motion = withContext(Dispatchers.IO) {
                    runCatching { actions.albumMotion(header.title, header.byline, songs.firstOrNull()?.title ?: "") }.getOrNull()
                }
            }
        }
        HeaderArt(header.thumb, header.banner, motion) {
            Title(header.title, size = if (header.banner) 40.sp else 32.sp, weight = FontWeight.ExtraBold, lines = 2,
                modifier = Modifier.align(Alignment.BottomStart).padding(horizontal = 16.dp))
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
        // The desktop's row: the big round play button, then icons alone.
        // An artist's play button plays their shuffle, as YouTube Music's.
        val play: (() -> Unit)? = when {
            header.shuffle != null && header.banner -> { { actions.playWatch(header.shuffle!!) } }
            header.playlistId != null -> { { actions.playItem(playlistItem(header.playlistId!!, header.title)) } }
            songs.isNotEmpty() -> { { actions.playList(songs, 0) } }
            else -> null
        }
        val shuffle: (() -> Unit)? = when {
            header.banner -> null
            header.shuffle != null -> { { actions.playWatch(header.shuffle!!) } }
            header.playlistId != null && songs.size > 1 -> {
                { actions.playListFromPlaylist(songs.shuffled(), 0, header.playlistId!!) }
            }
            songs.size > 1 -> { { actions.playList(songs.shuffled(), 0) } }
            else -> null
        }
        // Spotify's arrangement with our own buttons: the page's small
        // actions on the left (save or follow, then its menu, which also
        // has an artist's radio), shuffle and the big round play button at
        // the right end, under the thumb. Save and follow flip at once.
        var saved by remember(header) { mutableStateOf(header.saved) }
        var followed by remember(header) { mutableStateOf(header.followed) }
        Row(
            Modifier.fillMaxWidth().padding(start = 4.dp, end = 16.dp, top = 8.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val channel = header.channelId
            val following = followed
            if (header.banner && channel != null && following != null) {
                PillButton(
                    if (following) "Following" else "Follow",
                    modifier = Modifier.padding(start = 12.dp, end = 4.dp),
                ) { followed = !following; actions.follow(channel, !following, header.title) }
            }
            val playlist = header.playlistId
            val isSaved = saved
            if (!header.banner && playlist != null && isSaved != null) {
                HeroIcon(Icons.heart, filled = isSaved, color = if (isSaved) Tonearm.accent else null) {
                    saved = !isSaved
                    actions.setSaved(playlist, !isSaved, header.title)
                }
            }
            // Download, after save, as Spotify has it: an album's or
            // playlist's songs. A playlist still loading its rest is
            // fetched whole first.
            // Once every song is saved, a tap offers to delete them again.
            if (!header.banner) {
                val downloads = LocalDownloads.current
                var removing by remember { mutableStateOf(false) }
                HeaderDownload(songs) {
                    val ids = songs.mapNotNull { it.videoId }
                    when {
                        ids.all { it in downloads.saved } -> removing = true
                        !complete && header.playlistId != null -> actions.downloadCollection(header.asItem(saved))
                        else -> actions.download(songs)
                    }
                }
                if (removing) {
                    val count = songs.count { downloads.isSaved(it.videoId) }
                    ConfirmDialog(
                        title = "Remove downloads?",
                        text = "${if (count == 1) "1 song" else "$count songs"} from ${header.title} will be deleted from this phone.",
                        confirm = "Remove",
                        onDismiss = { removing = false },
                    ) {
                        removing = false
                        actions.removeDownloads(songs.mapNotNull { it.videoId })
                    }
                }
            }
            HeroIcon(Icons.dots, filled = true) { actions.menu(header.asItem(saved, pageId, pageParams)) }
            Spacer(Modifier.weight(1f))
            shuffle?.let { HeroIcon(Icons.shuffle, onClick = it) }
            if (canMix) {
                if (mixing == null) {
                    Bone(Modifier.padding(end = 4.dp).size(48.dp), CircleShape)
                } else {
                    MixButton(mixing) { onMixToggle(!mixing) }
                }
            }
            play?.let {
                Spacer(Modifier.width(8.dp))
                PlayDisc(mixed = mixing == true, size = 56.dp, onClick = it)
            }
        }
        if (header.about.isNotEmpty() && header.banner) {
            Dim(header.about, 13.sp, lines = 3, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
        }
    }
}

/** An icon on its own in the row under a page's title, as the desktop's
 *  `HeroIcon`: soft white, 28 tall, in a 48dp touch target. */
@Composable
fun HeroIcon(glyph: String, filled: Boolean = false, color: Color? = null, onClick: () -> Unit) {
    Box(
        Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(glyph, size = 26.sp, filled = filled, color = color ?: Color(0xB3FFFFFF))
    }
}

@Composable
private fun MixButton(mixing: Boolean, onClick: () -> Unit) {
    Box(
        Modifier.size(48.dp).clip(CircleShape).clickable(
            interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() },
            indication = null,
            onClick = onClick,
        ),
        contentAlignment = Alignment.Center,
    ) {
        SlidersMark(if (mixing) Tonearm.accent else Color(0xB3FFFFFF))
    }
}

@Composable
private fun SlidersMark(color: Color) {
    Canvas(Modifier.size(18.dp)) {
        val stroke = 1.5.dp.toPx()
        val centers = listOf(size.width * 0.18f, size.width * 0.5f, size.width * 0.82f)
        val knobs = listOf(size.height * 0.35f, size.height * 0.68f, size.height * 0.46f)
        centers.forEachIndexed { index, x ->
            drawLine(color, Offset(x, size.height * 0.08f), Offset(x, size.height * 0.92f), stroke, cap = androidx.compose.ui.graphics.StrokeCap.Round)
            drawLine(color, Offset(x - stroke * 1.5f, knobs[index]), Offset(x + stroke * 1.5f, knobs[index]), stroke * 1.4f, cap = androidx.compose.ui.graphics.StrokeCap.Round)
        }
    }
}

@Composable
private fun MixRowLine(row: MixRow?, onEdit: (MixRow) -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(32.dp).padding(start = 80.dp, end = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (row == null) {
            Bone(Modifier.width(28.dp).height(12.dp))
            Spacer(Modifier.width(10.dp))
            Bone(Modifier.width(26.dp).height(12.dp))
        } else if (row.analysisState.toInt() != 0) {
            // No tempo or key yet: say why, as the desktop's playlist does.
            // The words and the bar take the same room whichever shows, so
            // the row doesn't move as the analysis goes on.
            val state = row.analysisState.toInt()
            Column(Modifier.width(100.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                if (state == 1) {
                    Title("Analysing ${(row.analysisFraction.coerceIn(0f, 1f) * 100).toInt()}%", size = 12.sp, weight = FontWeight.Medium)
                } else {
                    Dim(if (state == 2) "Queued" else "After a play", 12.sp, lines = 1)
                }
                Box(
                    Modifier.fillMaxWidth().height(3.dp).clip(RoundedCornerShape(2.dp))
                        .background(if (state == 1) Color(0x40FFFFFF) else Color.Transparent),
                ) {
                    if (state == 1) {
                        Box(
                            Modifier.fillMaxWidth(row.analysisFraction.coerceIn(0f, 1f)).fillMaxHeight()
                                .background(Tonearm.accent),
                        )
                    }
                }
            }
        } else {
            Dim(row.bpm, 12.sp, modifier = Modifier.widthIn(min = 28.dp))
            Spacer(Modifier.width(10.dp))
            if (row.key.isNotEmpty()) {
                Box(
                    Modifier.clip(RoundedCornerShape(5.dp))
                        .background(Color(row.keyColor.toInt()))
                        .padding(horizontal = 5.dp, vertical = 1.dp),
                ) {
                    Title(row.key, size = 11.sp, color = Color(0xFF101014), weight = FontWeight.Bold)
                }
            } else Spacer(Modifier.width(26.dp))
        }
        Spacer(Modifier.weight(1f))
        if (row == null) {
            Bone(Modifier.width(58.dp).height(22.dp), RoundedCornerShape(11.dp))
        } else if (row.toVideoId != null) {
            Box(
                Modifier.height(22.dp).clip(RoundedCornerShape(11.dp))
                    .background(Color(0x14FFFFFF))
                    .clickable(
                        interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() },
                        indication = null,
                    ) { onEdit(row) }
                    .padding(horizontal = 10.dp),
                contentAlignment = Alignment.Center,
            ) {
                Title(row.transition, size = 11.sp, weight = FontWeight.Medium, color = Tonearm.dim)
            }
        } else {
            Spacer(Modifier.width(58.dp))
        }
    }
}

private val albumLabels = setOf("Album", "EP", "Single")

/** The page itself as an item, for its menu: the same lines a long press
 *  on its card gives. `saved` is the page's save state as it is now. */
fun Header.asItem(saved: Boolean?, pageId: String? = null, pageParams: String? = null): Item = Item(
    kind = when {
        banner -> ItemKind.ARTIST
        label in albumLabels -> ItemKind.ALBUM
        else -> ItemKind.PLAYLIST
    },
    title = title, subtitle = listOf(label, byline).filter { it.isNotEmpty() }.joinToString(" • "),
    thumb = thumb, duration = "", videoId = null,
    // The page's own id, so the menu can pin it to Quick access.
    browseId = if (banner) channelId else pageId, playlistId = playlistId, params = if (banner) null else pageParams,
    artist = byline, album = "", explicit = false,
    artists = if (!banner && byline.isNotEmpty()) listOf(Link(byline, bylineId)) else emptyList(),
    albumId = null, libraryPlaylist = if (saved != null) playlistId else null, saved = saved == true,
    color = null, trend = Trend.NONE,
)

/** How a page header's art is shaped: an artist's banner is wider than tall. */
fun headerRatio(banner: Boolean) = if (banner) 1.25f else 1f

/**
 * A page's art across the full width, from the top edge, fading into the
 * page from part way down; a bone sweeps there until it arrives. `over` is
 * drawn on top (the title, at the bottom).
 */
@Composable
fun HeaderArt(url: String, banner: Boolean, motion: String? = null, over: @Composable BoxScope.() -> Unit = {}) {
    Box(Modifier.fillMaxWidth().aspectRatio(headerRatio(banner))) {
        var loaded by remember(url) { mutableStateOf(false) }
        if (!loaded) Bone(Modifier.fillMaxSize(), RectangleShape)
        AsyncImage(
            model = url.ifEmpty { null },
            contentDescription = null,
            contentScale = ContentScale.Crop,
            onSuccess = { loaded = true },
            modifier = Modifier.fillMaxSize(),
        )
        // The moving cover, faded in over the still once it draws.
        motion?.let { MotionCover(it, Modifier.fillMaxSize()) }
        // A square cover keeps more of itself clear before the fade.
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(
                    (if (banner) 0.45f else 0.6f) to Color.Transparent,
                    1f to Tonearm.bg,
                ),
            ),
        )
        over()
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

/**
 * The round play button at the top of a page, as the desktop's `PlayDisc`:
 * usually a solid disc in the accent colour; in mix mode a brushed metal
 * disc in a glowing accent ring, parted from it by a thin dark line and a
 * silver rim. The two fade into each other as mix mode turns on or off.
 */
@Composable
fun PlayDisc(mixed: Boolean, size: Dp = 56.dp, onClick: () -> Unit) {
    val level by animateFloatAsState(if (mixed) 1f else 0f, tween(300, easing = FastOutSlowInEasing), label = "mix")
    Box(Modifier.size(size), contentAlignment = Alignment.Center) {
        if (level < 0.99f) {
            Box(Modifier.graphicsLayer { alpha = 1f - level }) {
                RoundButton(Icons.play, accent = true, size = size, onClick = onClick)
            }
        }
        if (level > 0.01f) {
            Box(
                Modifier.size(size).graphicsLayer { alpha = level }
                    .drawBehind {
                        // The glow round the ring.
                        drawCircle(
                            Brush.radialGradient(
                                0.6f to Tonearm.accent.copy(alpha = 0.6f),
                                1f to Color.Transparent,
                                center = center, radius = this.size.minDimension * 0.5f + 11.dp.toPx(),
                            ),
                            radius = this.size.minDimension * 0.5f + 11.dp.toPx(),
                        )
                    }
                    .clip(CircleShape)
                    .clickable(onClick = onClick)
                    .drawBehind {
                        val r = this.size.minDimension / 2
                        drawCircle(Tonearm.accent, r)
                        drawCircle(Color(0xFF141414), r - 3.dp.toPx())
                        // Conic gradients start at the top, as the desktop's.
                        rotate(-90f) {
                            drawCircle(
                                Brush.sweepGradient(
                                    0f to Color(0xFF8E8E94), 45f / 360 to Color(0xFFC9C9CE),
                                    90f / 360 to Color(0xFF6A6A70), 140f / 360 to Color(0xFFE6E6EA),
                                    200f / 360 to Color(0xFF8A8A90), 270f / 360 to Color(0xFF5C5C62),
                                    315f / 360 to Color(0xFFD8D8DC), 1f to Color(0xFF8E8E94),
                                ),
                                r - 4.dp.toPx(),
                            )
                            // Brushed metal: light bands on the diagonal,
                            // dark across the middle.
                            drawCircle(
                                Brush.sweepGradient(
                                    0f to Color(0xFF4A464C), 30f / 360 to Color(0xFF5E5A60),
                                    80f / 360 to Color(0xFF2A272B), 95f / 360 to Color(0xFF1D1B1E),
                                    125f / 360 to Color(0xFF6A666C), 140f / 360 to Color(0xFFA6A3A8),
                                    170f / 360 to Color(0xFF5A565C), 220f / 360 to Color(0xFF3E3A40),
                                    265f / 360 to Color(0xFF2A272B), 275f / 360 to Color(0xFF1D1B1E),
                                    305f / 360 to Color(0xFF6A666C), 320f / 360 to Color(0xFFA6A3A8),
                                    345f / 360 to Color(0xFF5A565C), 1f to Color(0xFF4A464C),
                                ),
                                r - 6.dp.toPx(),
                            )
                        }
                    },
                contentAlignment = Alignment.Center,
            ) {
                Icon(Icons.play, size = (size.value * 0.46f).sp, filled = true, color = Color.White)
            }
        }
    }
}

/** A chip, as the desktop's: a rounded rectangle, faintly lit, lit
 *  further while pressed, and white when it is the one picked. Home's
 *  moods, Explore's pages and Search's tabs all use it. */
@Composable
fun Chip(label: String, selected: Boolean = false, onClick: () -> Unit) {
    val press = remember { MutableInteractionSource() }
    val pressed by press.collectIsPressedAsState()
    Box(
        Modifier.height(34.dp).clip(RoundedCornerShape(CHIP_RADIUS))
            .background(if (selected) Tonearm.text else if (pressed) Color(0x24FFFFFF) else Color(0x14FFFFFF))
            .clickable(interactionSource = press, indication = null, onClick = onClick)
            .padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (selected) Tonearm.bg else Tonearm.text, fontFamily = Tonearm.font,
            fontWeight = FontWeight.SemiBold, fontSize = 14.sp, maxLines = 1)
    }
}

/** A chip's corners, for its bones too. */
val CHIP_RADIUS = 8.dp

/** The app's one button, as the desktop's `PillButton`: a pill, faintly
 *  lit; white for the main action, red for one that undoes something. */
@Composable
fun PillButton(
    text: String,
    primary: Boolean = false,
    danger: Boolean = false,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val press = remember { MutableInteractionSource() }
    val pressed by press.collectIsPressedAsState()
    val background = when {
        primary -> if (pressed) Color.White else Color(0xFFF0F0F3)
        danger -> if (pressed) Color(0x33FF4F6D) else Color(0x1AFF4F6D)
        else -> if (pressed) Color(0x24FFFFFF) else Color(0x14FFFFFF)
    }
    Box(
        modifier.height(36.dp).clip(RoundedCornerShape(18.dp)).background(background)
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

// ------------------------------------------------------------------- pages

/** Pages loaded lately, by `PageView`'s key, with where each was scrolled
 *  to. A page kept for under ten minutes shows as it was; an older one
 *  shows at once too while a newer one loads over it. Read and written on
 *  the main thread only. */
object PageCache {
    private const val FRESH_MS = 10 * 60_000L
    private const val MOST = 30

    class Entry(val page: Page, val at: Long, val index: Int = 0, val offset: Int = 0) {
        fun fresh() = System.currentTimeMillis() - at < FRESH_MS
    }

    private val entries = object : LinkedHashMap<Any, Entry>(16, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Any, Entry>?) = size > MOST
    }

    fun get(key: Any): Entry? = entries[key]

    /** Just loaded: kept as new, at the top. */
    fun loaded(key: Any, page: Page) {
        entries[key] = Entry(page, System.currentTimeMillis())
    }

    /** The same page grown by more shelves: as old as it was. */
    fun update(key: Any, page: Page) {
        val old = entries[key] ?: return
        if (old.page !== page) entries[key] = Entry(page, old.at, old.index, old.offset)
    }

    fun scrolled(key: Any, index: Int, offset: Int) {
        val old = entries[key] ?: return
        entries[key] = Entry(old.page, old.at, index, offset)
    }
}

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
    /** The id and params the page was opened with, for its own menu. */
    pageId: String? = null,
    pageParams: String? = null,
    /** The page runs up under the status bar, and `StatusBar` goes over
     *  it: Home, Explore and the pages opened from them. */
    underStatusBar: Boolean = false,
) {
    Box(Modifier.fillMaxSize()) {
        var scrolled by remember(key) { mutableStateOf(false) }
        PageViewBody(key, load, more, actions, top, topWhileLoading, skeleton, pageId, pageParams) { scrolled = it }
        if (underStatusBar) StatusBar(scrolled)
    }
}

@Composable
private fun PageViewBody(
    key: Any,
    load: suspend () -> Page,
    more: suspend (String) -> Page,
    actions: Actions,
    top: (@Composable (Page?) -> Unit)?,
    topWhileLoading: Boolean,
    skeleton: PageSkeleton,
    pageId: String?,
    pageParams: String?,
    /** Whether the list is scrolled off its top, as it changes. */
    onScrolled: (Boolean) -> Unit,
) {
    // Shown again at once when it was loaded lately: going to another tab,
    // or into an album and back, takes the page off the screen, and it
    // would otherwise load again from nothing.
    val kept = remember(key) { PageCache.get(key) }
    var page by remember(key) { mutableStateOf(kept?.page) }
    var error by remember(key) { mutableStateOf<String?>(null) }
    var loadingMore by remember(key) { mutableStateOf(false) }
    LaunchedEffect(key) {
        if (kept != null && kept.fresh()) return@LaunchedEffect
        try {
            page = load().also { PageCache.loaded(key, it) }
        } catch (e: Exception) {
            // A kept page stays up when the newer one will not load.
            if (page == null) error = e.message ?: "Couldn't load this page"
        }
    }
    // More shelves scrolled into: kept with the page.
    LaunchedEffect(key, page) { page?.let { PageCache.update(key, it) } }
    val shown = page
    when {
        // A page that keeps its top while loading keeps it on failing too:
        // Library's way to Downloads stays there offline.
        error != null && shown == null -> Column(Modifier.fillMaxSize()) {
            if (topWhileLoading) top?.invoke(null)
            Message(error!!)
        }
        shown == null -> Column(Modifier.fillMaxSize()) {
            if (topWhileLoading) top?.invoke(null)
            PageSkeletonView(skeleton)
        }
        else -> {
            val state = rememberLazyListState(kept?.index ?: 0, kept?.offset ?: 0)
            // Where it was scrolled to, for coming back to it.
            DisposableEffect(key) {
                onDispose { PageCache.scrolled(key, state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset) }
            }
            val header = shown.header
            val playlistId = header?.playlistId
            val canMix = header?.editable == true && playlistId != null
            var mixing by remember(key, playlistId) { mutableStateOf<Boolean?>(if (canMix) null else false) }
            var mixRefresh by remember(key) { mutableIntStateOf(0) }
            var mixRows by remember(key) { mutableStateOf<Map<String, MixRow>>(emptyMap()) }
            val songs = remember(shown) { shown.songs() }
            val songIds = remember(songs) { songs.mapNotNull { it.videoId } }
            val mixItems = remember(songs) { songs.mapNotNull { item -> item.videoId?.let { it to item } }.toMap() }
            LaunchedEffect(canMix, playlistId) {
                mixing = if (canMix && playlistId != null) {
                    withContext(Dispatchers.IO) { actions.playlistMixEnabled(playlistId) }
                } else {
                    false
                }
            }
            LaunchedEffect(canMix, playlistId, mixing, songIds, mixRefresh, MixSaves.count) {
                if (!canMix || playlistId == null || mixing != true) {
                    mixRows = emptyMap()
                    return@LaunchedEffect
                }
                while (true) {
                    val rows = withContext(Dispatchers.IO) { actions.mixRows(playlistId, songIds) }
                    mixRows = rows.associateBy { it.fromVideoId }
                    if (rows.none { it.analyzing }) break
                    delay(750)
                }
            }
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
            LaunchedEffect(state) {
                // Whether it can go back up: at the top it can't, even when
                // the first item is the empty `top` of a page with a header,
                // which the list counts as scrolled past.
                snapshotFlow { state.canScrollBackward }
                    .collect { onScrolled(it) }
            }
            LazyColumn(state = state, contentPadding = PaddingValues(bottom = 24.dp)) {
                top?.let { t -> item(contentType = "top") { t(shown) } }
                shown.header?.let { h ->
                    item(contentType = "header") {
                        PageHeader(
                            h,
                            shown.songs(),
                            actions,
                            complete = shown.continuation == null,
                            canMix = canMix,
                            mixing = mixing,
                            onMixToggle = { on ->
                                val id = playlistId ?: return@PageHeader
                                mixing = on
                                actions.setPlaylistMix(id, on) { mixRefresh++ }
                            },
                            pageId = pageId,
                            pageParams = pageParams,
                        )
                    }
                }
                val onAlbum = shown.header?.label in setOf("Album", "EP", "Single")
                val sourcePlaylist = playlistId?.takeIf { shown.header?.label == "Playlist" }
                shown.shelves.forEach {
                    shelf(it, actions, onAlbum, mixing == true, mixRows, sourcePlaylist, mixItems)
                }
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
