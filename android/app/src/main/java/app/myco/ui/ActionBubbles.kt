package app.myco.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp

/** One round, icon-only action. [label] is what a screen reader says. */
data class BubbleAction(val icon: ImageVector, val label: String, val onClick: () -> Unit)

/**
 * A screen's actions as round icon bubbles stacked in its bottom-right
 * corner: [main] at the bottom, in the primary colour, and [above] over it,
 * quieter. Place it with [modifier] (usually `align(BottomEnd)`).
 */
@Composable
fun ActionBubbles(main: BubbleAction, above: BubbleAction, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier.padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Bubble(
            above,
            color = MaterialTheme.colorScheme.secondaryContainer,
            tint = MaterialTheme.colorScheme.onSecondaryContainer,
        )
        Bubble(
            main,
            color = MaterialTheme.colorScheme.primary,
            tint = MaterialTheme.colorScheme.onPrimary,
        )
    }
}

@Composable
private fun Bubble(action: BubbleAction, color: androidx.compose.ui.graphics.Color, tint: androidx.compose.ui.graphics.Color) {
    Surface(
        shape = CircleShape,
        color = color,
        shadowElevation = 6.dp,
        modifier = Modifier
            .size(56.dp)
            .clickable(role = Role.Button, onClickLabel = action.label, onClick = action.onClick),
    ) {
        Box(contentAlignment = Alignment.Center) {
            Icon(action.icon, contentDescription = action.label, tint = tint, modifier = Modifier.size(26.dp))
        }
    }
}
