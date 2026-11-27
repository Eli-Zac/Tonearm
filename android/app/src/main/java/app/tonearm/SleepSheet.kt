package app.tonearm

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.tonearm_android.SleepTimer

/**
 * The sleep timer's choices, as the desktop's menu: Off, the lengths and
 * the end of the song, the running one in the accent with the time left
 * above. Rises from the bottom as the Devices sheet does.
 */
@Composable
fun SleepSheet(options: List<String>, timer: SleepTimer?, onPick: (Int) -> Unit) {
    val running = timer?.choice?.toInt() ?: 0
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(topStart = Tonearm.panel, topEnd = Tonearm.panel))
            .background(Tonearm.raised)
            .navigationBarsPadding()
            .padding(top = 10.dp, bottom = 12.dp),
    ) {
        Box(
            Modifier.align(Alignment.CenterHorizontally).size(width = 36.dp, height = 4.dp)
                .clip(RoundedCornerShape(2.dp)).background(Tonearm.line),
        )
        Column(Modifier.padding(horizontal = 20.dp, vertical = 14.dp)) {
            Title("Sleep timer", size = 20.sp, weight = FontWeight.Bold)
            val left = timer?.text.orEmpty()
            if (left.isNotEmpty()) Dim(left, 13.sp, modifier = Modifier.padding(top = 2.dp))
        }
        options.forEachIndexed { i, option ->
            val on = i == running
            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 8.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .clickable { onPick(i) }
                    .heightIn(min = 48.dp)
                    .padding(horizontal = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Title(
                    option, size = 15.sp,
                    color = if (on) Tonearm.accent else Tonearm.text,
                    weight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                    modifier = Modifier.weight(1f),
                )
                if (on) Box(Modifier.size(6.dp).clip(CircleShape).background(Tonearm.accent))
            }
        }
    }
}
