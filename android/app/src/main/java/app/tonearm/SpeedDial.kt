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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.compose.AsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.DialItem
import uniffi.tonearm_android.Tonearm as Core

/** Quick access changed here: Home's speed dial reads it again. A sync
 *  that brings pins from another device bumps `Prefs.version`, which it
 *  reads again on too. */
object QuickAccess {
    var version by mutableIntStateOf(0)
        private set
    private val main = android.os.Handler(android.os.Looper.getMainLooper())
    /** From any thread: the count moves on the main one. */
    fun changed() { main.post { version++ } }
}

/** Tiles on a page of the speed dial: three rows of three. */
private const val PER_PAGE = 9

/**
 * Home's speed dial, as YouTube Music's app has one: your pins from Quick
 * access first, then what you opened and played lately, three by three,
 * swiped page to page. A tap opens or plays it, a long press its menu, as
 * cards elsewhere.
 */
@Composable
fun SpeedDial(core: Core, actions: Actions) {
    val account = rememberAccount(core)
    val tiles by produceState<List<DialItem>>(emptyList(), QuickAccess.version, Prefs.version) {
        value = withContext(Dispatchers.IO) { runCatching { core.speedDial() }.getOrDefault(emptyList()) }
    }
    if (tiles.isEmpty()) return
    val pages = (tiles.size + PER_PAGE - 1) / PER_PAGE
    Column(Modifier.fillMaxWidth().padding(top = 18.dp, bottom = 6.dp)) {
        Row(
            Modifier.padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (account is AccountState.SignedIn) {
                AccountPhoto(account, 44.dp)
                Spacer(Modifier.width(12.dp))
            }
            Column {
                (account as? AccountState.SignedIn)?.account?.name?.takeIf { it.isNotBlank() }?.let {
                    Dim(it.uppercase(), 12.sp)
                }
                Title("Speed dial", size = 24.sp, weight = FontWeight.Bold)
            }
        }
        Spacer(Modifier.height(14.dp))
        val pager = rememberPagerState { pages }
        HorizontalPager(
            pager,
            contentPadding = PaddingValues(horizontal = 16.dp),
            pageSpacing = 16.dp,
            verticalAlignment = Alignment.Top,
        ) { page ->
            val onPage = tiles.drop(page * PER_PAGE).take(PER_PAGE)
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                onPage.chunked(3).forEach { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        row.forEach { tile -> DialTile(tile, actions, Modifier.weight(1f)) }
                        // A short last row keeps its tiles the same size.
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
                        Modifier.padding(horizontal = 4.dp).size(8.dp).clip(CircleShape)
                            .background(if (i == pager.currentPage) Tonearm.text else Tonearm.line),
                    )
                }
            }
        }
    }
}

/** One tile: the cover filling a rounded square, the name over a shade at
 *  the bottom, and a pin at the top right when it is pinned. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DialTile(tile: DialItem, actions: Actions, modifier: Modifier) {
    val item = tile.item
    Box(
        modifier
            .aspectRatio(1f)
            .clip(RoundedCornerShape(10.dp))
            .background(Tonearm.raised)
            .combinedClickable(
                onClick = { actions.tap(item) },
                onLongClick = { actions.menu(item) },
            ),
    ) {
        AsyncImage(
            model = item.thumb.ifEmpty { null },
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize(),
        )
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(0.55f to Color.Transparent, 1f to Color(0xCC000000)),
            ),
        )
        if (tile.pinned) {
            Icon(
                Icons.pin, size = 18.sp, filled = true,
                modifier = Modifier.align(Alignment.TopEnd).padding(8.dp),
            )
        }
        Title(
            item.title, size = 13.sp, weight = FontWeight.SemiBold,
            modifier = Modifier.align(Alignment.BottomStart).padding(horizontal = 10.dp, vertical = 8.dp),
        )
    }
}
