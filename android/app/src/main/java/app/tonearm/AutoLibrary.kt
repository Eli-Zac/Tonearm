package app.tonearm

import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaConstants
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.Page
import uniffi.tonearm_android.ShelfLayout
import uniffi.tonearm_android.Song
import java.util.concurrent.ConcurrentHashMap
import uniffi.tonearm_android.Tonearm as Core

/**
 * What Android Auto lists: the phone's Home, Explore, Library and
 * Downloads, as tabs along the car's screen. A page's shelves show as
 * titled groups down one list, as on the phone (cards as tiles, lists as
 * rows), and albums, playlists and artists open as they do there. Picking
 * a song plays it the way a tap on the phone would: a song in a list plays
 * the list from it, a song card its radio. Saved songs carry the car's
 * downloaded mark, explicit ones its "E".
 *
 * The car asks by media id. Each page and song handed out is kept here by
 * a short id, so what it was (and the list around it) is known when the
 * car comes back for its children or plays it.
 */
@UnstableApi
class AutoLibrary(private val core: Core) {
    /** How a picked song or card becomes a queue. */
    enum class How { List, Radio, Item }

    private sealed interface Node
    private data class TabNode(val tab: String) : Node
    private data class PageNode(val browseId: String, val params: String?) : Node
    private data class PlayNode(val how: How, val items: List<Item>, val index: Int) : Node
    /** The song the queue is on, offered when the car starts. */
    private data object ResumeNode : Node

    private val nodes = ConcurrentHashMap<String, Node>()
    private var next = 0
    /** Tabs' pages, briefly, as the car asks for them more than once. */
    private val pages = ConcurrentHashMap<String, Pair<Long, Page>>()
    /** Songs saved on the phone, read once per list. */
    private var saved: Set<String> = emptySet()

    private fun keep(node: Node): String {
        // A long drive's browsing stays well under this; past it, start over.
        if (nodes.size > 20_000) nodes.keys.removeIf { !it.startsWith(TAB) }
        return synchronized(this) { "n${next++}" }.also { nodes[it] = node }
    }

    fun root(): MediaItem = folder(ROOT, "Tonearm", "", null, hint = LIST)

    /** What Android Auto offers to carry on with as it starts: the queue
     *  as it was left. Its one child is the song the queue is on. */
    fun recentRoot(): MediaItem = folder(RECENT, "Recent", "", null, hint = LIST)

    /** A folder's contents. Blocks on the network. */
    fun children(id: String): List<MediaItem> {
        saved = core.downloadMarks().saved.toSet()
        if (id == ROOT) {
            return TABS.map { (tab, title) ->
                nodes[TAB + tab] = TabNode(tab)
                // Albums and playlists show as tiles, songs as rows.
                // Each tab carries the phone tab bar's icon, from the same font.
                folder(TAB + tab, title, "", null, hint = GRID, icon = android.net.Uri.parse("android.resource://app.tonearm/drawable/ic_tab_$tab"))
            }
        }
        if (id == RECENT) {
            val song = core.nowPlaying() ?: return emptyList()
            nodes[RESUME] = ResumeNode
            return listOf(song(RESUME, song))
        }
        return when (val node = nodes[id] ?: tabOf(id)) {
            is TabNode -> if (node.tab == "downloads") {
                val songs = core.downloadsPage().shelves.flatMap { it.items }.filter { it.videoId != null }
                songs.mapIndexed { i, item -> playable(item, How.List, songs, i, null, LIST) }
            } else {
                pageChildren(tabPage(node.tab))
            }
            is PageNode -> pageChildren(core.browse(node.browseId, node.params))
            else -> emptyList()
        }
    }

    private fun tabOf(id: String): Node? =
        id.removePrefix(TAB).takeIf { id.startsWith(TAB) && TABS.any { t -> t.first == it } }?.let { TabNode(it) }

    private fun tabPage(tab: String): Page {
        pages[tab]?.let { (at, page) -> if (System.currentTimeMillis() - at < 120_000) return page }
        val page = when (tab) {
            "home" -> core.home()
            "explore" -> core.explore()
            else -> core.library()
        }
        pages[tab] = System.currentTimeMillis() to page
        return page
    }

    /** An album or playlist lists its songs; anything else its shelves,
     *  one after another under their titles. */
    private fun pageChildren(page: Page): List<MediaItem> {
        val header = page.header
        if (header != null && !header.banner) {
            val songs = page.songs()
            return songs.mapIndexed { i, item -> playable(item, How.List, songs, i, null, LIST) }
        }
        val shelves = page.shelves.filter { it.items.isNotEmpty() }
        val titled = shelves.size > 1
        return shelves.flatMap { shelf ->
            val group = if (titled) shelf.title.ifEmpty { null } else null
            // A shelf of cards stays tiles in the car; a list stays rows.
            val style = if (shelf.layout == ShelfLayout.LIST) LIST else GRID
            shelf.items.mapIndexedNotNull { i, item ->
                when {
                    // In a list a song plays the list on from it; on a card, its radio.
                    item.videoId != null -> playable(
                        item, if (shelf.layout == ShelfLayout.LIST) How.List else How.Radio, shelf.items, i, group, style,
                    )
                    item.browseId != null -> {
                        val id = keep(PageNode(item.browseId!!, item.params))
                        folder(id, item.title, item.line(), item.thumb, hint = GRID, group = group, single = style)
                    }
                    // A mix with no page of its own plays straight away.
                    item.playlistId != null -> playable(item, How.Item, listOf(item), 0, group, style)
                    else -> null
                }
            }
        }
    }

    private fun playable(item: Item, how: How, list: List<Item>, index: Int, group: String?, single: Int): MediaItem {
        val id = keep(PlayNode(how, list, index))
        val extras = Bundle().apply {
            group?.let { putString(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_GROUP_TITLE, it) }
            putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_SINGLE_ITEM, single)
            if (item.explicit) putLong(MediaConstants.EXTRAS_KEY_IS_EXPLICIT, 1L)
            if (item.videoId != null && item.videoId in saved) {
                putLong(MediaConstants.EXTRAS_KEY_DOWNLOAD_STATUS, MediaConstants.EXTRAS_VALUE_STATUS_DOWNLOADED)
            }
        }
        return MediaItem.Builder().setMediaId(id).setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(item.title)
                .setArtist(item.line())
                .setArtworkUri(ArtProvider.uri(item.thumb))
                .setIsPlayable(true)
                .setIsBrowsable(false)
                .setMediaType(if (item.videoId != null) MediaMetadata.MEDIA_TYPE_MUSIC else MediaMetadata.MEDIA_TYPE_PLAYLIST)
                .setExtras(extras)
                .build(),
        ).build()
    }

    private fun song(id: String, song: Song): MediaItem = MediaItem.Builder().setMediaId(id).setMediaMetadata(
        MediaMetadata.Builder()
            .setTitle(song.title)
            .setArtist(song.artist)
            .setArtworkUri(ArtProvider.uri(song.cover))
            .setIsPlayable(true)
            .setIsBrowsable(false)
            .setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC)
            .build(),
    ).build()

    private fun folder(
        id: String,
        title: String,
        subtitle: String,
        art: String?,
        hint: Int,
        group: String? = null,
        single: Int? = null,
        /** A tab's icon, in place of art. */
        icon: android.net.Uri? = null,
    ): MediaItem {
        val extras = Bundle().apply {
            // How what is inside shows: albums as tiles or rows, songs as rows.
            putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_BROWSABLE, hint)
            putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_PLAYABLE, LIST)
            group?.let { putString(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_GROUP_TITLE, it) }
            single?.let { putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_SINGLE_ITEM, it) }
        }
        return MediaItem.Builder().setMediaId(id).setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(title)
                .setSubtitle(subtitle.ifEmpty { null })
                .setArtist(subtitle.ifEmpty { null })
                .setArtworkUri(icon ?: art?.let(ArtProvider::uri))
                .setIsPlayable(false)
                .setIsBrowsable(true)
                .setMediaType(MediaMetadata.MEDIA_TYPE_FOLDER_MIXED)
                .setExtras(extras)
                .build(),
        ).build()
    }

    /** Whether the car picked something handed out here. */
    fun knows(id: String) = nodes[id] is PlayNode || nodes[id] == ResumeNode

    /**
     * The queue a pick plays, and where in it to start: the same queue the
     * phone builds for the same tap. Blocks on the network for radios and
     * albums.
     */
    fun queueFor(id: String): Pair<List<Song>, Int>? {
        return when (val node = nodes[id]) {
            // Carrying on: the queue as it is, from the song it is on.
            ResumeNode -> core.queue() to core.playback().index.toInt()
            is PlayNode -> when (node.how) {
                How.List -> {
                    val songs = core.listSongs(node.items)
                    // Its place among the songs, as the list may hold others.
                    val start = node.items.take(node.index).count { it.videoId != null }
                    songs to start.coerceIn(0, (songs.size - 1).coerceAtLeast(0))
                }
                How.Radio -> core.radioSongs(node.items[node.index]) to 0
                How.Item -> core.itemSongs(node.items[node.index]) to 0
            }
            else -> null
        }
    }

    companion object {
        const val ROOT = "root"
        const val RECENT = "recent"
        private const val RESUME = "recent:song"
        private const val TAB = "tab:"
        private val TABS = listOf("home" to "Home", "explore" to "Explore", "library" to "Library", "downloads" to "Downloads")

        const val STYLE_SUPPORTED = "android.media.browse.CONTENT_STYLE_SUPPORTED"
        val LIST = MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM
        val GRID = MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_GRID_ITEM
    }
}
