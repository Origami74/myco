package app.myco.ui.theme

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Test

class ThemeTest {
    @Test
    fun `dark scheme is pure AMOLED black with white content and brand accent`() {
        val colors = mycoColorScheme(darkTheme = true)

        assertEquals(Color.Black, colors.background)
        assertEquals(Color.Black, colors.surface)
        assertEquals(Color.Black, colors.surfaceVariant)
        assertEquals(Color.Black, colors.surfaceDim)
        assertEquals(Color.Black, colors.surfaceBright)
        assertEquals(Color.Black, colors.surfaceContainerLowest)
        assertEquals(Color.Black, colors.surfaceContainerLow)
        assertEquals(Color.Black, colors.surfaceContainer)
        assertEquals(Color.Black, colors.surfaceContainerHigh)
        assertEquals(Color.Black, colors.surfaceContainerHighest)
        assertEquals(Color.Black, colors.surfaceTint)
        assertEquals(Color.White, colors.onBackground)
        assertEquals(Color.White, colors.onSurface)
        assertEquals(Color.White, colors.onSurfaceVariant)
        assertEquals(AmoledAccent, colors.primary)
        assertEquals(Color.Black, colors.onPrimary)
        assertEquals(Color(0xFFFFB74D), colors.tertiary)
        assertEquals(Color.Black, colors.onTertiary)
        assertEquals(Color.Black, colors.tertiaryContainer)
        assertEquals(Color(0xFFFFB74D), colors.onTertiaryContainer)
    }

    @Test
    fun `light scheme remains the existing brand palette`() {
        val colors = mycoColorScheme(darkTheme = false)

        assertEquals(ScreenBg, colors.background)
        assertEquals(Ink, colors.onBackground)
        assertEquals(CardBg, colors.surfaceVariant)
        assertEquals(Emerald, colors.primary)
        assertEquals(Color(0xFFB45309), colors.tertiary)
        assertEquals(Color.White, colors.onTertiary)
    }
}
