package app.tonearm

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.ItemKind
import uniffi.tonearm_android.Link
import uniffi.tonearm_android.MenuAction
import uniffi.tonearm_android.MenuIcon
import uniffi.tonearm_android.MenuLine
import uniffi.tonearm_android.Song
import uniffi.tonearm_android.Trend
import uniffi.tonearm_android.Tonearm as Core

/** What an item's menu needs from the app. */
interface MenuHost {
    /** Runs a call to the core off the main thread. What it returns, if
     *  anything, is shown to the user, as is any failure. */
    fun run(call: () -> String?)
    /** Starts something playing, and opens Now Playing over it. */
    fun play(call: () -> Unit)
    /** Opens a page, leaving Now Playing if it is open. */
    fun open(id: String, title: String)
    /** Likes or unlikes the song playing, as Now Playing's heart. */
    fun toggleLike()
    /** Something was saved to or taken out of the library, or an artist
     *  followed: the library shows it again. */
    fun libraryChanged()
}

/** A long press: the item, and whether it was on the library, where the
 *  artists listed are followed. */
data class MenuRequest(val item: Item, val inLibrary: Boolean = false)

/** A song in the queue, as the item its menu works on. */
fun Song.asItem() = Item(
    kind = ItemKind.SONG,
    title = title,
    subtitle = artist,
    thumb = cover,
    duration = duration,
    videoId = videoId,
    browseId = null,
    playlistId = null,
    params = null,
    artist = artist,
    album = album,
    explicit = false,
    artists = artists,
    albumId = albumId,
    libraryPlaylist = null,
    saved = false,
    color = null,
    trend = Trend.NONE,
)

/**
 * An item's menu, with the lines the desktop's right click menu offers,
 * which come from the core: a song's play next, queue, artists, album,
 * radios, like and add to playlist; an album's or playlist's play,
 * shuffle, artists, open and library; an artist's open, shuffle, radio
 * and follow. Opens from the bottom of the screen on a long press.
 */
@Composable
fun ItemMenu(request: MenuRequest, core: Core, host: MenuHost, onClose: () -> Unit) {
    val item = request.item
    // Add to playlist turns the menu into the list of the user's playlists.
    var picking by remember(request) { mutableStateOf(false) }
    // Only locks inside the core, so cheap enough to read as it opens.
    val lines = remember(request) { core.menu(item, request.inLibrary) }
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp))
            .background(Tonearm.island)
            .navigationBarsPadding()
            .padding(top = 10.dp, bottom = 8.dp),
    ) {
        // A grab bar, as sheets have.
        Box(
            Modifier.align(Alignment.CenterHorizontally).size(width = 36.dp, height = 4.dp)
                .clip(RoundedCornerShape(2.dp)).background(Tonearm.line),
        )
        Row(Modifier.padding(horizontal = 20.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Art(item.thumb, 48.dp, if (item.round()) CircleShape else RoundedCornerShape(6.dp))
            Column(Modifier.weight(1f).padding(start = 14.dp)) {
                Title(item.title, size = 16.sp)
                val under = item.artist.ifEmpty { item.subtitle }
                if (under.isNotEmpty()) Dim(under, 14.sp)
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Tonearm.line))
        if (picking) {
            PlaylistPicker(item, core, host, onClose)
        } else {
            Entries(item, lines, core, host, onClose) { picking = true }
        }
    }
}

private fun glyph(icon: MenuIcon) = when (icon) {
    MenuIcon.PLAY -> Icons.play
    MenuIcon.SHUFFLE -> Icons.shuffle
    MenuIcon.PLAY_NEXT -> Icons.playNext
    MenuIcon.ADD_QUEUE -> Icons.addQueue
    MenuIcon.ARTIST -> Icons.artist
    MenuIcon.ALBUM -> Icons.album
    MenuIcon.RADIO -> Icons.radio
    MenuIcon.DOWNLOAD -> Icons.download
    MenuIcon.TRASH -> Icons.trash
    MenuIcon.HEART -> Icons.heart
    MenuIcon.OPEN -> Icons.open
    MenuIcon.LIBRARY -> Icons.library
}

@Composable
private fun Entries(
    item: Item,
    lines: List<MenuLine>,
    core: Core,
    host: MenuHost,
    onClose: () -> Unit,
    onPick: () -> Unit,
) {
    // A page's title until it loads: the artist's or album's name.
    fun titleOf(id: String) = item.artists.firstOrNull { it.id == id }?.name
        ?: if (id == item.albumId) item.album else item.title
    fun pick(line: MenuLine) {
        when (val action = line.action) {
            is MenuAction.AddToPlaylist -> return onPick()
            else -> onClose()
        }
        when (val action = line.action) {
            is MenuAction.PlayNext -> host.run { core.playNext(item) }
            is MenuAction.AddToQueue -> host.run { core.addToQueue(item) }
            is MenuAction.Browse -> host.open(action.id, titleOf(action.id))
            is MenuAction.SongRadio -> host.play { core.playRadio(item) }
            is MenuAction.ArtistRadio -> host.play { core.playArtist(action.id, false) }
            is MenuAction.ArtistShuffle -> host.play { core.playArtist(action.id, true) }
            is MenuAction.Like -> item.videoId?.let { id -> host.run { core.like(id); "Liked ${item.title}" } }
            is MenuAction.LikePlaying -> host.toggleLike()
            is MenuAction.Play -> host.play { core.playCollection(item, false) }
            is MenuAction.Shuffle -> host.play { core.playCollection(item, true) }
            is MenuAction.Library -> host.run {
                core.setSaved(action.playlist, action.save, item.title).also { host.libraryChanged() }
            }
            is MenuAction.Follow -> host.run {
                core.follow(action.channel, action.follow, item.title).also { host.libraryChanged() }
            }
            // Not offered until the phone has downloads.
            is MenuAction.ToggleDownload, is MenuAction.DownloadAll, is MenuAction.AddToPlaylist -> {}
        }
    }
    Column(Modifier.padding(vertical = 6.dp)) {
        lines.forEach { line -> Entry(glyph(line.icon), line.label) { pick(line) } }
    }
}

/** The user's playlists, to put the song in one. */
@Composable
private fun PlaylistPicker(item: Item, core: Core, host: MenuHost, onClose: () -> Unit) {
    val playlists by produceState<List<Link>?>(null, item) {
        value = withContext(Dispatchers.IO) { runCatching { core.myPlaylists() }.getOrDefault(emptyList()) }
    }
    val list = playlists
    when {
        list == null -> Loading(Modifier.fillMaxWidth().height(160.dp))
        list.isEmpty() -> Box(Modifier.fillMaxWidth().height(100.dp), contentAlignment = Alignment.Center) {
            Dim("No playlists to add to", 15.sp)
        }
        else -> LazyColumn(Modifier.fillMaxWidth().heightIn(max = 420.dp).padding(vertical = 6.dp)) {
            items(list) { playlist ->
                Entry(Icons.library, playlist.name) {
                    onClose()
                    val id = playlist.id ?: return@Entry
                    val video = item.videoId ?: return@Entry
                    host.run {
                        core.addToPlaylist(id, video)
                        "Added ${item.title} to ${playlist.name}"
                    }
                }
            }
        }
    }
}

@Composable
private fun Entry(glyph: String, label: String, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 22.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(glyph, size = 22.sp, color = Tonearm.dim)
        Spacer(Modifier.width(18.dp))
        Title(label, size = 16.sp, weight = FontWeight.Medium)
    }
}
