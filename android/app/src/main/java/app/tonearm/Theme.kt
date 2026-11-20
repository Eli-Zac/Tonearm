package app.tonearm

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** The desktop app's look, from `Theme` in ui/app.slint. Keep the two the
 *  same: change a colour there, change it here. */
object Tonearm {
    val bg = Color(0xFF08080A)
    val island = Color(0xFF131316)
    val raised = Color(0xFF1C1C21)
    val hover = Color(0x12FFFFFF)
    val line = Color(0x14FFFFFF)
    val text = Color(0xFFF5F5F7)
    val dim = Color(0xFF9C9CA6)
    val faint = Color(0x59FFFFFF)
    val accent = Color(0xFFFF4F6D)
    val radius = 14.dp

    val font = FontFamily(
        Font(R.font.figtree_regular, FontWeight.Normal),
        Font(R.font.figtree_medium, FontWeight.Medium),
        Font(R.font.figtree_semibold, FontWeight.SemiBold),
        Font(R.font.figtree_bold, FontWeight.Bold),
        Font(R.font.figtree_extrabold, FontWeight.ExtraBold),
    )
}

/** Phosphor icons, the desktop's own font files and code points (`Icons`
 *  in ui/app.slint). Only the glyphs the desktop uses are in the fonts. */
object Icons {
    val regular = FontFamily(Font(R.font.phosphor))
    val fill = FontFamily(Font(R.font.phosphor_fill))

    const val play = ""
    const val pause = ""
    const val prev = ""
    const val next = ""
    const val shuffle = ""
    const val repeat = ""
    const val repeatOne = ""
    const val heart = ""
    const val home = ""
    const val explore = ""
    const val library = ""
    const val search = ""
    const val back = ""
    const val down = ""
    const val queue = ""
    const val lyrics = ""
    const val dots = ""
    const val radio = ""
    const val nowPlaying = ""
}

@Composable
fun Icon(
    glyph: String,
    modifier: Modifier = Modifier,
    size: TextUnit = 22.sp,
    color: Color = Tonearm.text,
    filled: Boolean = false,
) {
    Text(
        glyph,
        modifier = modifier,
        style = TextStyle(
            fontFamily = if (filled) Icons.fill else Icons.regular,
            fontSize = size,
            color = color,
            lineHeight = size,
        ),
    )
}

@Composable
fun TonearmTheme(content: @Composable () -> Unit) {
    val type = MaterialTheme.typography
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Tonearm.accent,
            background = Tonearm.bg,
            surface = Tonearm.bg,
            surfaceVariant = Tonearm.island,
            onBackground = Tonearm.text,
            onSurface = Tonearm.text,
            onSurfaceVariant = Tonearm.dim,
            outline = Tonearm.line,
        ),
        typography = type.copy(
            bodyLarge = type.bodyLarge.copy(fontFamily = Tonearm.font),
            bodyMedium = type.bodyMedium.copy(fontFamily = Tonearm.font),
            bodySmall = type.bodySmall.copy(fontFamily = Tonearm.font),
            titleMedium = type.titleMedium.copy(fontFamily = Tonearm.font),
            labelLarge = type.labelLarge.copy(fontFamily = Tonearm.font),
        ),
        content = content,
    )
}
