package app.tonearm

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.ItemKind
import uniffi.tonearm_android.Link
import uniffi.tonearm_android.Song
import uniffi.tonearm_android.Tonearm as Core

/** What a song's menu needs from the app. */
interface MenuHost {
    /** Runs a call to the core off the main thread. What it returns, if
     *  anything, is shown to the user, as is any failure. */
    fun run(call: () -> String?)
    /** Opens a page, leaving Now Playing if it is open. */
    fun open(id: String, title: String)
    val signedIn: Boolean
}

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
)

/**
 * A song's menu, as the desktop's right click menu offers it for a song:
 * play next, add to queue, go to its artists and album, its radio and its
 * artists', and, signed in, like and add to playlist. Opens from the
 * bottom of the screen on a long press.
 */
@Composable
fun SongMenu(item: Item, core: Core, host: MenuHost, onClose: () -> Unit) {
    // Add to playlist turns the menu into the list of the user's playlists.
    var picking by remember(item) { mutableStateOf(false) }
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
            Art(item.thumb, 48.dp, RoundedCornerShape(6.dp))
            Column(Modifier.weight(1f).padding(start = 14.dp)) {
                Title(item.title, size = 16.sp)
                Dim(item.artist.ifEmpty { item.subtitle }, 14.sp)
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Tonearm.line))
        if (picking) {
            PlaylistPicker(item, core, host, onClose)
        } else {
            Entries(item, core, host, onClose) { picking = true }
        }
    }
}

@Composable
private fun Entries(item: Item, core: Core, host: MenuHost, onClose: () -> Unit, onPick: () -> Unit) {
    fun pick(action: () -> Unit) {
        onClose()
        action()
    }
    val linked = item.artists.filter { it.id != null }
    val several = linked.size > 1
    Column(Modifier.padding(vertical = 6.dp)) {
        Entry(Icons.playNext, "Play next") { pick { host.run { core.playNext(item) } } }
        Entry(Icons.addQueue, "Add to queue") { pick { host.run { core.addToQueue(item) } } }
        for (artist in linked) {
            Entry(Icons.artist, if (several) "Go to ${artist.name}" else "Go to artist") {
                pick { host.open(artist.id!!, artist.name) }
            }
        }
        item.albumId?.let { album ->
            Entry(Icons.album, "Go to album") { pick { host.open(album, item.album) } }
        }
        Entry(Icons.radio, "Song radio") { pick { host.run { core.playRadio(item); null } } }
        for (artist in linked) {
            Entry(Icons.radio, if (several) "${artist.name} radio" else "Artist radio") {
                pick { host.run { core.artistRadio(artist.id!!); null } }
            }
        }
        if (host.signedIn) {
            val id = item.videoId
            if (id != null) {
                Entry(Icons.heart, "Like") { pick { host.run { core.like(id); "Liked ${item.title}" } } }
                Entry(Icons.addQueue, "Add to playlist", onClick = onPick)
            }
        }
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
