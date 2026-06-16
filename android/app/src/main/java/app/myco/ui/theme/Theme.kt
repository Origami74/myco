package app.myco.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
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
val Ink = Color(0xFF0F172A)
val Slate = Color(0xFF64748B)
val Hairline = Color(0xFFE7E9EE)
val CardBg = Color(0xFFF4F5F7)
val ScreenBg = Color(0xFFFFFFFF)

private val MycoLightColors = lightColorScheme(
    primary = Emerald,
    onPrimary = Color.White,
    primaryContainer = EmeraldSoft,
    onPrimaryContainer = EmeraldInk,
    secondary = Indigo,
    onSecondary = Color.White,
    secondaryContainer = IndigoSoft,
    onSecondaryContainer = IndigoInk,
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
fun MycoTheme(content: @Composable () -> Unit) {
    // Light-only for now: the mockups are light, and a fixed scheme is the most
    // consistent choice across API levels.
    MaterialTheme(colorScheme = MycoLightColors, typography = MycoTypography, content = content)
}
