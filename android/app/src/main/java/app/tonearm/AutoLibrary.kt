package app.tonearm

import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.util.UnstableApi
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.Page
import uniffi.tonearm_android.Shelf
import uniffi.tonearm_android.ShelfLayout
import uniffi.tonearm_android.Song
import java.util.concurrent.ConcurrentHashMap
import uniffi.tonearm_android.Tonearm as Core

/**
 * What Android Auto lists: the phone's Home, Explore, Library and
 * Downloads, as tabs along the car's screen, each page's shelves as
 * folders, and albums, playlists and artists opening as they do on the
 * phone. Picking a song plays it the way a tap on the phone would: a song
 * in a list plays the list from it, a song card its radio.
 *
 * The car asks by media id. Each folder and song handed out is kept here
 * by a short id, so what it was (and the list around it) is known when the
 * car comes back for its children or plays it.
 */
@UnstableApi
class AutoLibrary(private val core: Core) {
    /** How a picked song or card becomes a queue. */
    enum class How { List, Radio, Item }

    private sealed interface Node
    private data class TabNode(val tab: String) : Node
    private data class ShelfNode(val shelf: Shelf, val title: String) : Node
    private data class PageNode(val browseId: String, val params: String?) : Node
    private data class PlayNode(val how: How, val items: List<Item>, val index: Int) : Node

    private val nodes = ConcurrentHashMap<String, Node>()
    private var next = 0
    /** Tabs' pages, briefly, as the car asks for them more than once. */
    private val pages = ConcurrentHashMap<String, Pair<Long, Page>>()

    private fun keep(node: Node): String {
        // A long drive's browsing stays well under this; past it, start over.
        if (nodes.size > 20_000) nodes.keys.removeIf { !it.startsWith(TAB) }
        return synchronized(this) { "n${next++}" }.also { nodes[it] = node }
    }

    fun root(): MediaItem = folder(ROOT, "Tonearm", "", null, hint = LIST)

    /** A folder's contents. Blocks on the network. */
    fun children(id: String): List<MediaItem> {
        if (id == ROOT) {
            return TABS.map { (tab, title) ->
                nodes[TAB + tab] = TabNode(tab)
                // Home and the rest list their shelves down the screen.
                folder(TAB + tab, title, "", null, hint = LIST)
            }
        }
        return when (val node = nodes[id] ?: tabOf(id)) {
            is TabNode -> if (node.tab == "downloads") {
                val songs = core.downloadsPage().shelves.flatMap { it.items }.filter { it.videoId != null }
                songs.mapIndexed { i, item -> playable(item, How.List, songs, i) }
            } else {
                pageChildren(tabPage(node.tab), node.tab)
            }
            is ShelfNode -> shelfChildren(node.shelf)
            is PageNode -> pageChildren(core.browse(node.browseId, node.params), node.browseId)
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
     *  each a folder, or the one shelf's items straight away. */
    private fun pageChildren(page: Page, name: String): List<MediaItem> {
        val header = page.header
        if (header != null && !header.banner) {
            val songs = page.songs()
            return songs.mapIndexed { i, item -> playable(item, How.List, songs, i) }
        }
        val shelves = page.shelves.filter { it.items.isNotEmpty() }
        if (shelves.size == 1) return shelfChildren(shelves[0])
        return shelves.map { shelf ->
            val title = shelf.title.ifEmpty { page.title.ifEmpty { name } }
            val id = keep(ShelfNode(shelf, title))
            val songs = shelf.items.all { it.videoId != null }
            folder(id, title, shelf.items.take(3).joinToString(", ") { it.title },
                shelf.items.firstOrNull()?.thumb, hint = if (songs) LIST else GRID)
        }
    }

    private fun shelfChildren(shelf: Shelf): List<MediaItem> = shelf.items.mapIndexedNotNull { i, item ->
        when {
            // In a list a song plays the list on from it; on a card, its radio.
            item.videoId != null -> playable(item, if (shelf.layout == ShelfLayout.LIST) How.List else How.Radio, shelf.items, i)
            item.browseId != null -> {
                val id = keep(PageNode(item.browseId!!, item.params))
                folder(id, item.title, item.line(), item.thumb, hint = LIST)
            }
            // A mix with no page of its own plays straight away.
            item.playlistId != null -> playable(item, How.Item, listOf(item), 0)
            else -> null
        }
    }

    private fun playable(item: Item, how: How, list: List<Item>, index: Int): MediaItem {
        val id = keep(PlayNode(how, list, index))
        return MediaItem.Builder().setMediaId(id).setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(item.title)
                .setArtist(item.line())
                .setArtworkUri(ArtProvider.uri(item.thumb))
                .setIsPlayable(true)
                .setIsBrowsable(false)
                .setMediaType(if (item.videoId != null) MediaMetadata.MEDIA_TYPE_MUSIC else MediaMetadata.MEDIA_TYPE_PLAYLIST)
                .build(),
        ).build()
    }

    private fun folder(id: String, title: String, subtitle: String, art: String?, hint: Int): MediaItem {
        val extras = Bundle().apply {
            putInt(BROWSABLE_HINT, hint)
            putInt(PLAYABLE_HINT, LIST)
        }
        return MediaItem.Builder().setMediaId(id).setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(title)
                .setSubtitle(subtitle.ifEmpty { null })
                .setArtworkUri(art?.let(ArtProvider::uri))
                .setIsPlayable(false)
                .setIsBrowsable(true)
                .setMediaType(MediaMetadata.MEDIA_TYPE_FOLDER_MIXED)
                .setExtras(extras)
                .build(),
        ).build()
    }

    /** Whether the car picked something handed out here. */
    fun knows(id: String) = nodes[id] is PlayNode

    /**
     * The queue a pick plays, and where in it to start: the same queue the
     * phone builds for the same tap. Blocks on the network for radios and
     * albums.
     */
    fun queueFor(id: String): Pair<List<Song>, Int>? {
        val node = nodes[id] as? PlayNode ?: return null
        return when (node.how) {
            How.List -> {
                val songs = core.listSongs(node.items)
                // Its place among the songs, as the list may hold others.
                val start = node.items.take(node.index).count { it.videoId != null }
                songs to start.coerceIn(0, (songs.size - 1).coerceAtLeast(0))
            }
            How.Radio -> core.radioSongs(node.items[node.index]) to 0
            How.Item -> core.itemSongs(node.items[node.index]) to 0
        }
    }

    companion object {
        const val ROOT = "root"
        private const val TAB = "tab:"
        private val TABS = listOf("home" to "Home", "explore" to "Explore", "library" to "Library", "downloads" to "Downloads")

        // Android Auto's content style keys, for how a folder's children show.
        const val STYLE_SUPPORTED = "android.media.browse.CONTENT_STYLE_SUPPORTED"
        const val BROWSABLE_HINT = "android.media.browse.CONTENT_STYLE_BROWSABLE_HINT"
        const val PLAYABLE_HINT = "android.media.browse.CONTENT_STYLE_PLAYABLE_HINT"
        const val LIST = 1
        const val GRID = 2
    }
}
