package com.localnotebook.agent.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

val CanvasDark = Color(0xFF131314)
val PanelDark = Color(0xFF1E1F20)
val ElevatedDark = Color(0xFF282A2C)
val HoverDark = Color(0xFF333537)
val AccentBlue = Color(0xFFA8C7FA)
val TextPrimaryDark = Color(0xFFE3E5EA)
val TextSecondaryDark = Color(0xFFC4C7C5)
val TextMutedDark = Color(0xFF8E918F)

private val DarkColors = darkColorScheme(
    primary = AccentBlue,
    onPrimary = Color(0xFF131314),
    background = CanvasDark,
    onBackground = TextPrimaryDark,
    surface = PanelDark,
    onSurface = TextPrimaryDark,
    surfaceVariant = ElevatedDark,
    onSurfaceVariant = TextSecondaryDark,
    outline = Color(0xFF3C4043),
)

private val LightColors = lightColorScheme(
    primary = Color(0xFF1A73E8),
    onPrimary = Color.White,
    background = Color(0xFFF4F6F9),
    onBackground = Color(0xFF1F2328),
    surface = Color.White,
    onSurface = Color(0xFF1F2328),
    surfaceVariant = Color(0xFFF0F3F8),
    onSurfaceVariant = Color(0xFF4B5563),
    outline = Color(0xFFD0D7DE),
)

private val MidnightColors = darkColorScheme(
    primary = Color(0xFF818CF8),
    onPrimary = Color(0xFF0D1117),
    background = Color(0xFF0D1117),
    onBackground = Color(0xFFE6EDF5),
    surface = Color(0xFF161B26),
    onSurface = Color(0xFFE6EDF5),
    surfaceVariant = Color(0xFF1F2737),
    onSurfaceVariant = Color(0xFFB8C4D8),
    outline = Color(0xFF38455E),
)

private val ForestColors = darkColorScheme(
    primary = Color(0xFF6EE7B7),
    onPrimary = Color(0xFF0F1614),
    background = Color(0xFF0F1614),
    onBackground = Color(0xFFE4EFEA),
    surface = Color(0xFF17221F),
    onSurface = Color(0xFFE4EFEA),
    surfaceVariant = Color(0xFF202E2A),
    onSurfaceVariant = Color(0xFFB6CCC3),
    outline = Color(0xFF354F47),
)

private val WarmPaperColors = lightColorScheme(
    primary = Color(0xFFB45309),
    onPrimary = Color.White,
    background = Color(0xFFF5EFE6),
    onBackground = Color(0xFF2C2218),
    surface = Color(0xFFFFFDF8),
    onSurface = Color(0xFF2C2218),
    surfaceVariant = Color(0xFFEFE7DA),
    onSurfaceVariant = Color(0xFF5B4B3A),
    outline = Color(0xFFCBBBA4),
)

private val RoseDuskColors = darkColorScheme(
    primary = Color(0xFFF4A5C4),
    onPrimary = Color(0xFF161217),
    background = Color(0xFF161217),
    onBackground = Color(0xFFF1E8F2),
    surface = Color(0xFF211B23),
    onSurface = Color(0xFFF1E8F2),
    surfaceVariant = Color(0xFF2C242F),
    onSurfaceVariant = Color(0xFFCABCC9),
    outline = Color(0xFF4E3F54),
)

@Composable
fun LocalNotebookTheme(
    themeMode: String,
    baseFontSize: Int,
    content: @Composable () -> Unit,
) {
    val colorScheme = when (themeMode) {
        "light" -> LightColors
        "system" -> if (isSystemInDarkTheme()) DarkColors else LightColors
        "midnight" -> MidnightColors
        "forest" -> ForestColors
        "warm_paper" -> WarmPaperColors
        "rose_dusk" -> RoseDuskColors
        else -> DarkColors
    }
    val fs = baseFontSize.coerceIn(13, 18)
    val typography = Typography(
        titleLarge = TextStyle(fontSize = (fs + 5).sp, lineHeight = (fs + 12).sp, fontWeight = FontWeight.SemiBold),
        titleMedium = TextStyle(fontSize = (fs + 2).sp, lineHeight = (fs + 8).sp, fontWeight = FontWeight.SemiBold),
        bodyLarge = TextStyle(fontSize = fs.sp, lineHeight = (fs * 1.72f).sp, fontWeight = FontWeight.Normal),
        bodyMedium = TextStyle(fontSize = (fs - 1).sp, lineHeight = ((fs - 1) * 1.65f).sp, fontWeight = FontWeight.Normal),
        labelMedium = TextStyle(fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium),
    )

    MaterialTheme(
        colorScheme = colorScheme,
        typography = typography,
        content = content,
    )
}
