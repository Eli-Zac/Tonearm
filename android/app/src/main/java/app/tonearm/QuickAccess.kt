package app.tonearm

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.compose.AsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.QuickTile
import uniffi.tonearm_android.Tonearm as Core

/** Quick access changed here: Home's section reads it again. A sync that
 *  brings pins from another device bumps `Prefs.version`, which it reads
 *  again on too. */
object QuickAccess {
    var version by mutableIntStateOf(0)
        private set
    private val main = android.os.Handler(android.os.Looper.getMainLooper())
    /** From any thread: the count moves on the main one. */
    fun changed() { main.post { version++ } }
    /** The tiles last shown, so Home coming back on screen draws them at
     *  once instead of nothing until they are read again, which made what
     *  is under them jump. Main thread only. */
    var shown: List<QuickTile> = emptyList()
}

/** Tiles on a page: three rows of three. */
private const val PER_PAGE = 9

/**
 * Quick access at the top of Home: the pins the desktop's side menu
 * lists, then what you opened and played lately, three by three and
 * swiped page to page, as YouTube Music's app lays its own out. The rest
 * is the app's own: a shelf title, and cards as every shelf draws them,
 * art with the title under it. A tap opens or plays, a long press gives
 * the menu.
 */
@Composable
fun QuickAccessShelf(core: Core, actions: Actions) {
    val tiles by produceState(QuickAccess.shown, QuickAccess.version, Prefs.version) {
        value = withContext(Dispatchers.IO) { runCatching { core.quickAccessTiles() }.getOrDefault(emptyList()) }
        QuickAccess.shown = value
    }
    if (tiles.isEmpty()) return
    val pages = (tiles.size + PER_PAGE - 1) / PER_PAGE
    Column(Modifier.fillMaxWidth()) {
        // As every shelf's title (`shelf` in Ui.kt).
        Title(
            "Quick access", size = 20.sp, weight = FontWeight.Bold,
            modifier = Modifier.padding(start = 16.dp, end = 8.dp, top = 22.dp, bottom = 10.dp),
        )
        val pager = rememberPagerState { pages }
        HorizontalPager(
            pager,
            contentPadding = PaddingValues(horizontal = 16.dp),
            pageSpacing = 14.dp,
            verticalAlignment = Alignment.Top,
        ) { page ->
            val onPage = tiles.drop(page * PER_PAGE).take(PER_PAGE)
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                onPage.chunked(3).forEach { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        row.forEach { tile -> QuickCard(tile, actions, Modifier.weight(1f)) }
                        // A short last row keeps its cards the same size.
                        repeat(3 - row.size) { Spacer(Modifier.weight(1f)) }
                    }
                }
            }
        }
        if (pages > 1) {
            Row(
                Modifier.fillMaxWidth().padding(top = 12.dp),
                horizontalArrangement = Arrangement.Center,
            ) {
                repeat(pages) { i ->
                    Box(
                        Modifier.padding(horizontal = 4.dp).size(6.dp).clip(CircleShape)
                            .background(if (i == pager.currentPage) Tonearm.text else Tonearm.faint),
                    )
                }
            }
        }
    }
}

/** A card as `ItemCard` draws one, sized to its column: the art with the
 *  card corners (round for an artist), the title under it, and a pin in
 *  the dark disc the cards' play button uses when it is pinned. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun QuickCard(tile: QuickTile, actions: Actions, modifier: Modifier) {
    val item = tile.item
    val shape = if (item.round()) CircleShape else RoundedCornerShape(Tonearm.radius)
    Column(
        modifier.combinedClickable(
            onClick = { actions.tap(item) },
            onLongClick = { actions.menu(item) },
        ),
    ) {
        Box {
            AsyncImage(
                model = item.thumb.ifEmpty { null },
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxWidth().aspectRatio(1f).clip(shape).background(Tonearm.raised),
            )
            if (tile.pinned) {
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .padding(6.dp)
                        .size(26.dp)
                        .clip(CircleShape)
                        .background(Color(0xCC08080A)),
                    contentAlignment = Alignment.Center,
                ) {
                    // Regular: the fill font is cut down and has no pin.
                    Icon(Icons.pin, size = 14.sp)
                }
            }
        }
        Spacer(Modifier.height(6.dp))
        Title(
            item.title, size = 13.sp, lines = 1,
            modifier = if (item.round()) Modifier.fillMaxWidth() else Modifier,
        )
    }
}
