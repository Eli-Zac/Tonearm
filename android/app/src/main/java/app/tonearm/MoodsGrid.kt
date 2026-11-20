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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.Page
import uniffi.tonearm_android.Tonearm as Core

/** YouTube Music's "Moods & genres", as Explore lists them. */
private const val MOODS = "FEmusic_moods_and_genres"

/** Kept for the app's life: the list barely changes, and Search shows it
 *  every time it opens with nothing typed. */
private var moodsPage: Page? = null

/**
 * Search with nothing typed: every mood and genre as a button, under its
 * heading, two to a row, with the colour YouTube Music gives it as a
 * stripe down its left edge. A button opens that mood's page.
 */
@Composable
fun MoodsGrid(core: Core, onOpen: (Item) -> Unit) {
    val page by produceState(moodsPage) {
        if (value == null) {
            value = withContext(Dispatchers.IO) { runCatching { core.browse(MOODS, null) }.getOrNull() }
                ?.also { moodsPage = it }
        }
    }
    val shown = page
    if (shown == null) {
        MoodsSkeleton()
        return
    }
    val shelves = shown.shelves.filter { s -> s.items.any { it.color != null } }
    if (shelves.isEmpty()) {
        Message("Search songs, albums, artists and playlists")
        return
    }
    LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp)) {
        shelves.forEach { shelf ->
            if (shelf.title.isNotEmpty()) {
                item(contentType = "title") {
                    Title(shelf.title, size = 20.sp, weight = FontWeight.Bold,
                        modifier = Modifier.padding(top = 22.dp, bottom = 10.dp))
                }
            }
            items(shelf.items.filter { it.color != null }.chunked(2), contentType = { "row" }) { pair ->
                Row(Modifier.fillMaxWidth().padding(bottom = 10.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    pair.forEach { MoodButton(it, Modifier.weight(1f)) { onOpen(it) } }
                    if (pair.size == 1) Spacer(Modifier.weight(1f))
                }
            }
        }
    }
}

/** A mood: filled with its own colour, run diagonally from a deep blend
 *  with the app's dark into a darker one, so the cards glow in the
 *  mood's colour without leaving the app's muted look. */
@Composable
private fun MoodButton(item: Item, modifier: Modifier, onClick: () -> Unit) {
    val hue = item.color?.let { Color(it.toInt()) } ?: Tonearm.accent
    val fill = remember(hue) {
        Brush.linearGradient(
            0f to lerp(Tonearm.bg, hue, 0.62f),
            1f to lerp(Tonearm.bg, hue, 0.18f),
            start = Offset.Zero,
            end = Offset.Infinite,
        )
    }
    Box(
        modifier.height(64.dp).clip(RoundedCornerShape(Tonearm.radius)).background(fill).clickable(onClick = onClick)
            .padding(horizontal = 14.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        Title(item.title, size = 15.sp, weight = FontWeight.Bold)
    }
}

/** The grid's bones while the list comes. */
@Composable
private fun MoodsSkeleton() {
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        Bone(Modifier.padding(top = 22.dp, bottom = 12.dp).width(170.dp).height(20.dp), RoundedCornerShape(7.dp))
        repeat(7) {
            Row(Modifier.fillMaxWidth().padding(bottom = 10.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Bone(Modifier.weight(1f).height(64.dp), RoundedCornerShape(Tonearm.radius))
                Bone(Modifier.weight(1f).height(64.dp), RoundedCornerShape(Tonearm.radius))
            }
        }
    }
}
