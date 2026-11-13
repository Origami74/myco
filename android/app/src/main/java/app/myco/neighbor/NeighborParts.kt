package app.myco.neighbor

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/*
 * Shared bits of the Neighbors screens.
 *
 * What the screens may say: only what this phone knows. A neighborhood is
 * open-ended — nobody can count its members — so the screens name the people
 * *you* are connected with in it, and how many of those are here now. Never a
 * total.
 */

/** The people you're connected with in a neighborhood. */
internal fun NeighborhoodsState.peopleIn(id: String): List<NeighborhoodsState.PeerNode> =
    peers.filter { id in it.neighborhoods }

/** Everyone reachable right now, once each. */
internal fun NeighborhoodsState.hereNow(): List<NeighborhoodsState.PeerNode> =
    peers.filter { it.reachable }.distinctBy { it.key }

/** "With Bob, Mia + 2", or null when you're not connected with anyone in it. */
internal fun withNames(people: List<NeighborhoodsState.PeerNode>): String? {
    if (people.isEmpty()) return null
    val names = people.map { it.name }.distinct()
    val shown = names.take(3).joinToString(", ")
    val more = names.size - 3
    return if (more > 0) "With $shown + $more" else "With $shown"
}

/** A neighborhood's kind, in words. */
internal fun kindOf(n: NeighborhoodsState.Neighborhood): String =
    if (n.autoAccept) "Open · anyone nearby with the name" else "Invite only"

/** The display name for a neighborhood. */
internal fun titleOf(n: NeighborhoodsState.Neighborhood): String = n.name

/**
 * How far posts go, as three presets in place of hop counts.
 *
 * - **Close**: to the people you're connected with; they don't pass it on.
 * - **Friends of friends**: one step further.
 * - **Wide**: as far as Myco carries anything.
 */
internal enum class Reach(val label: String, val explain: String, val publish: Int, val backlog: Int, val passOn: Boolean) {
    Close("Close", "Only people you're connected with see it, and they don't pass it on.", 1, 1, false),
    Friends("Friends of friends", "People you're connected with, and the people they're connected with.", 2, 1, true),
    Wide("Wide", "Passed on a few steps — good for open neighborhoods.", 3, 2, true);

    companion object {
        fun of(n: NeighborhoodsState.Neighborhood): Reach? =
            entries.firstOrNull { it.publish == n.publishHops && it.backlog == n.backlogHops && it.passOn == n.passOn }
    }
}

private val AVATAR_TINTS = listOf(
    Color(0xFFFEF3C7) to Color(0xFF92400E),
    Color(0xFFDBEAFE) to Color(0xFF1E3A8A),
    Color(0xFFFCE7F3) to Color(0xFF9D174D),
    Color(0xFFDCFCE7) to Color(0xFF166534),
    Color(0xFFE0E7FF) to Color(0xFF3730A3),
    Color(0xFFFFE4E6) to Color(0xFF9F1239),
)

/** A person's round initial, tinted stably by their key, with a dot when here now. */
@Composable
internal fun Avatar(name: String, key: String, here: Boolean, size: Dp = 44.dp, modifier: Modifier = Modifier) {
    val (bg, ink) = AVATAR_TINTS[(key.hashCode() and 0x7fffffff) % AVATAR_TINTS.size]
    Box(modifier.size(size)) {
        Box(
            Modifier.size(size).background(if (here) bg else Color(0xFFF1F5F9), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                name.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?",
                color = if (here) ink else Color(0xFF64748B),
                fontWeight = FontWeight.Bold,
                style = if (size > 36.dp) MaterialTheme.typography.titleMedium else MaterialTheme.typography.labelLarge,
            )
        }
        if (here) {
            Box(
                Modifier
                    .size(size / 3.5f)
                    .align(Alignment.TopEnd)
                    .offset(x = 1.dp, y = (-1).dp)
                    .background(Color(0xFF22C55E), CircleShape)
                    .border(2.dp, MaterialTheme.colorScheme.surface, CircleShape),
            )
        }
    }
}
