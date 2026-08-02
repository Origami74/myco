package app.myco.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

// Brand palette — fixed values, so the UI renders identically on every API 29+
// (no Material You / dynamic-color dependency, which would need API 31+).
val Emerald = Color(0xFF059669)
val EmeraldSoft = Color(0xFFD1FAE5)
val EmeraldInk = Color(0xFF064E3B)
val Indigo = Color(0xFF6366F1)
val IndigoSoft = Color(0xFFE0E7FF)
val IndigoInk = Color(0xFF3730A3)
val StatusConnected = Color(0xFF22C55E)
val StatusReachable = Color(0xFF14B8A6)
/// One peer: the mesh works but has no redundancy — lose that peer and you are
/// alone, so it reads as a caution rather than a fault.
val StatusThin = Color(0xFFF59E0B)
/// No peers at all: nothing to sync with, nothing to reach. A real fault state.
val StatusAlone = Color(0xFFEF4444)
val Ink = Color(0xFF0F172A)
val Slate = Color(0xFF64748B)
val Hairline = Color(0xFFE7E9EE)
val CardBg = Color(0xFFF4F5F7)
val ScreenBg = Color(0xFFFFFFFF)

/** Bright emerald retained as the primary brand accent on AMOLED black. */
val AmoledAccent = Color(0xFF34D399)
private val LightWarning = Color(0xFFB45309)
private val LightWarningContainer = Color(0xFFFFF7ED)
private val LightOnWarningContainer = Color(0xFF9A3412)
private val AmoledWarning = Color(0xFFFFB74D)
private val AmoledOutline = Color(0xFF3F3F46)
private val AmoledError = Color(0xFFFF6B6B)

private val MycoLightColors = lightColorScheme(
    primary = Emerald,
    onPrimary = Color.White,
    primaryContainer = EmeraldSoft,
    onPrimaryContainer = EmeraldInk,
    secondary = Indigo,
    onSecondary = Color.White,
    secondaryContainer = IndigoSoft,
    onSecondaryContainer = IndigoInk,
    tertiary = LightWarning,
    onTertiary = Color.White,
    tertiaryContainer = LightWarningContainer,
    onTertiaryContainer = LightOnWarningContainer,
    background = ScreenBg,
    onBackground = Ink,
    surface = ScreenBg,
    onSurface = Ink,
    surfaceVariant = CardBg,
    onSurfaceVariant = Slate,
    outline = Hairline,
    outlineVariant = Hairline,
    error = Color(0xFFDC2626),
)

private val MycoAmoledColors = darkColorScheme(
    primary = AmoledAccent,
    onPrimary = Color.Black,
    primaryContainer = Color.Black,
    onPrimaryContainer = AmoledAccent,
    secondary = Color(0xFFA5B4FC),
    onSecondary = Color.Black,
    secondaryContainer = Color.Black,
    onSecondaryContainer = Color(0xFFA5B4FC),
    tertiary = AmoledWarning,
    onTertiary = Color.Black,
    tertiaryContainer = Color.Black,
    onTertiaryContainer = AmoledWarning,
    background = Color.Black,
    onBackground = Color.White,
    surface = Color.Black,
    onSurface = Color.White,
    surfaceVariant = Color.Black,
    onSurfaceVariant = Color.White,
    surfaceDim = Color.Black,
    surfaceBright = Color.Black,
    surfaceContainerLowest = Color.Black,
    surfaceContainerLow = Color.Black,
    surfaceContainer = Color.Black,
    surfaceContainerHigh = Color.Black,
    surfaceContainerHighest = Color.Black,
    surfaceTint = Color.Black,
    outline = AmoledOutline,
    outlineVariant = AmoledOutline,
    error = AmoledError,
    onError = Color.Black,
    errorContainer = Color.Black,
    onErrorContainer = AmoledError,
)

internal fun mycoColorScheme(darkTheme: Boolean): ColorScheme =
    if (darkTheme) MycoAmoledColors else MycoLightColors

/** Vibrant tile colors for nsite icons, assigned deterministically by host. */
val TilePalette = listOf(
    Color(0xFF2563EB), Color(0xFFF59E0B), Color(0xFF10B981), Color(0xFF7C3AED),
    Color(0xFFE11D48), Color(0xFF0EA5E9), Color(0xFFEC4899), Color(0xFF4F46E5),
    Color(0xFF0D9488), Color(0xFFD97706),
)

fun tileColorFor(key: String): Color {
    val h = key.hashCode()
    val idx = (if (h < 0) -h else h) % TilePalette.size
    return TilePalette[idx]
}

/** Avatar colors for Circle contacts, by npub. */
val AvatarPalette = listOf(
    Color(0xFFE11D48), Color(0xFF2563EB), Color(0xFFF59E0B), Color(0xFF7C3AED),
    Color(0xFF0D9488), Color(0xFFEC4899), Color(0xFF059669), Color(0xFF4F46E5),
)

fun avatarColorFor(key: String): Color {
    val h = key.hashCode()
    val idx = (if (h < 0) -h else h) % AvatarPalette.size
    return AvatarPalette[idx]
}

private val MycoTypography = Typography(
    // Big screen titles ("Apps", "Circle", …).
    displaySmall = TextStyle(fontWeight = FontWeight.ExtraBold, fontSize = 34.sp, lineHeight = 40.sp),
)

@Composable
fun MycoTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = mycoColorScheme(darkTheme),
        typography = MycoTypography,
        content = content,
    )
}
