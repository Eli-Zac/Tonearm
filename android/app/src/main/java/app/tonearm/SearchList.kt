package app.tonearm

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.tonearm_android.Item
import uniffi.tonearm_android.Suggestions

/**
 * The list under the search box, as the desktop's: with nothing typed, the
 * recent searches; as you type, up to five suggested searches with the
 * typed part bold, then the first few results of the search itself. Rows
 * of skeleton stand in while the first list for what is typed is fetched.
 */
@Composable
fun SuggestList(
    typed: String,
    recent: List<String>,
    found: Suggestions?,
    loading: Boolean,
    onSearch: (String) -> Unit,
    onItem: (Item) -> Unit,
    onItemMenu: (Item) -> Unit,
    modifier: Modifier = Modifier,
) {
    LazyColumn(modifier.fillMaxSize(), contentPadding = PaddingValues(horizontal = 8.dp, vertical = 2.dp)) {
        if (typed.isBlank()) {
            if (recent.isNotEmpty()) {
                item("recent-title") {
                    Text(
                        "Recent searches", color = Tonearm.dim, fontFamily = Tonearm.font,
                        fontWeight = FontWeight.Bold, fontSize = 12.sp,
                        modifier = Modifier.height(28.dp).padding(start = 14.dp).wrapContentHeight(Alignment.CenterVertically),
                    )
                }
            }
            items(recent, key = { "recent:$it" }) { query ->
                SearchRow(Icons.recent, query, "") { onSearch(query) }
            }
            return@LazyColumn
        }
        if (found == null) {
            if (loading) items(6) { SkeletonRow(it) }
            return@LazyColumn
        }
        items(found.searches, key = { "search:${it.query}" }) { s ->
            SearchRow(Icons.search, s.head, s.tail) { onSearch(s.query) }
        }
        items(found.items.size, key = { "item:$it" }) { i ->
            val item = found.items[i]
            SuggestItemRow(item, { onItem(item) }, { onItemMenu(item) })
        }
    }
}

/** A search to run: the icon, then `head` bold and `tail` plain. */
@Composable
private fun SearchRow(glyph: String, head: String, tail: String, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(48.dp).clip(RoundedCornerShape(8.dp)).clickable(onClick = onClick)
            .padding(start = 8.dp, end = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(44.dp), contentAlignment = Alignment.Center) {
            Icon(glyph, color = Tonearm.dim, size = 20.sp)
        }
        Spacer(Modifier.width(12.dp))
        Text(
            buildAnnotatedString {
                withStyle(SpanStyle(color = Tonearm.text, fontWeight = FontWeight.Bold)) { append(head) }
                withStyle(SpanStyle(color = Color(0xCCFFFFFF))) { append(tail) }
            },
            fontFamily = Tonearm.font, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
        )
    }
}

/** One of the search's first results: its art, name and what it is. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun SuggestItemRow(item: Item, onClick: () -> Unit, onLongClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(60.dp).clip(RoundedCornerShape(8.dp))
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(start = 8.dp, end = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Art(item.thumb, 44.dp, if (item.round()) CircleShape else RoundedCornerShape(6.dp))
        Column(Modifier.weight(1f).padding(start = 12.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Title(item.title, size = 16.sp)
            Dim(item.subtitle, 13.sp)
        }
    }
}

/** Stands in for a result row: a square and two bars, gently pulsing. The
 *  bars' lengths vary by `seed` so a stack doesn't look stamped out, as
 *  the desktop's `SkeletonRow`. The pulse runs in drawing only. */
@Composable
private fun SkeletonRow(seed: Int) {
    val pulse = rememberInfiniteTransition(label = "skeleton")
    val alpha by pulse.animateFloat(
        initialValue = 0.06f,
        targetValue = 0.14f,
        animationSpec = infiniteRepeatable(tween(900, easing = FastOutSlowInEasing), RepeatMode.Reverse),
        label = "alpha",
    )
    val bone = Modifier.drawBehind { drawRect(Color.White.copy(alpha = alpha)) }
    Row(Modifier.fillMaxWidth().height(60.dp).padding(start = 8.dp, end = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(44.dp).clip(RoundedCornerShape(6.dp)).then(bone))
        Column(Modifier.weight(1f).padding(start = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.fillMaxWidth(0.62f - (seed * 7 % 5) * 0.055f).height(11.dp).clip(RoundedCornerShape(4.dp)).then(bone))
            Box(Modifier.fillMaxWidth(0.34f - (seed * 3 % 4) * 0.035f).height(9.dp).clip(RoundedCornerShape(4.dp)).then(bone))
        }
    }
}
