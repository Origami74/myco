package app.myco.ui.screens

import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Apps
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.HelpOutline
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.WifiTethering
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

/**
 * What a napplet may do, shown the way Android shows permissions: grouped by
 * what it means to the person (posting as you, phones nearby, files…), each
 * group with an icon and one switch, and the parts behind a tap.
 *
 * Groups are sorted by how much they let an app do, so the install sheet
 * leads with what matters:
 *
 * - **Acts as you** — posts or publishes in your name where others see it.
 * - **Sensitive** — reaches people nearby or your own files.
 * - **Standard** — what most apps need (your name, opening other apps,
 *   pictures by hash). On the install sheet these fold into one row, as
 *   Android keeps "normal" permissions out of the way.
 *
 * A group takes the level of its most sensitive part the app actually gets,
 * so Files & media is standard for an app that only loads pictures and
 * "acts as you" for one that also uploads. Start up and theme are
 * [BASICS]: always granted, shown without a switch.
 */
internal enum class Tier { Standard, Sensitive, ActsAsYou }

/** Parts above [Tier.Standard]. Anything unlisted is standard. */
private val PART_TIER = mapOf(
    "relay" to Tier.ActsAsYou,
    "outbox" to Tier.ActsAsYou,
    "upload" to Tier.ActsAsYou,
    "mesh" to Tier.Sensitive,
    "fs" to Tier.Sensitive,
    "notify" to Tier.Sensitive,
)

/** Every app has these; no switch. */
internal val BASICS = setOf("shell", "theme", "config")

internal class PermissionGroup(
    val title: String,
    val icon: ImageVector,
    /** One line on the install sheet, while the group is folded. */
    val summary: String,
    /** A caveat shown under the parts, when the group is open. */
    val note: String? = null,
    val domains: List<String>,
)

private val GROUPS = listOf(
    PermissionGroup(
        "Post as you", Icons.AutoMirrored.Filled.Send,
        "Read and post on your relays, without asking.",
        note = "Can read the private data your apps keep there.",
        domains = listOf("relay", "outbox"),
    ),
    PermissionGroup(
        "Nearby devices", Icons.Filled.WifiTethering,
        "Post to and read from phones nearby.",
        note = "Can read the private data your apps keep on this phone.",
        domains = listOf("mesh"),
    ),
    PermissionGroup(
        "Files & media", Icons.Filled.Folder,
        "Pictures, files, and keeping things on this phone.",
        domains = listOf("resource", "local", "fs", "upload"),
    ),
    PermissionGroup("Profile", Icons.Filled.Person, "See your name and picture.", domains = listOf("identity")),
    PermissionGroup(
        "Apps & links", Icons.Filled.Apps,
        "Open your other apps and web links — you pick which.",
        domains = listOf("intent", "inc", "link"),
    ),
    PermissionGroup("Notifications", Icons.Filled.Notifications, "Send you notifications.", domains = listOf("notify")),
)

/** A domain no group knows: a newer napplet than this build. Shown, never hidden. */
private fun otherGroup(domains: List<String>) = PermissionGroup(
    "Other", Icons.Filled.HelpOutline,
    "Something this version of Myco doesn't know about.",
    domains = domains,
)

internal fun tierOf(domains: List<String>): Tier =
    domains.maxOfOrNull { PART_TIER[it] ?: if (GROUPS.any { g -> it in g.domains }) Tier.Standard else Tier.Sensitive }
        ?: Tier.Standard

/**
 * [domains] sorted into their groups, Basics left out: each known group with
 * the parts the list has, then Other for domains no group knows. Groups the
 * list has nothing of are left out.
 */
internal fun groupPermissions(domains: List<String>): List<Pair<PermissionGroup, List<String>>> {
    val listed = domains.filter { it !in BASICS }
    val known = GROUPS.mapNotNull { g ->
        val mine = g.domains.filter { it in listed }
        if (mine.isEmpty()) null else g to mine
    }
    val unknown = listed.filter { d -> GROUPS.none { d in it.domains } }
    return known + if (unknown.isEmpty()) emptyList() else listOf(otherGroup(unknown) to unknown)
}

/**
 * The groups for [domains], each with switches. [granted] is what is on;
 * [onChange] turns a set of domains on or off together (a group's switch) or
 * one at a time (a part's).
 *
 * [needed] are what the app says it can't do without: labelled, and turning
 * one off warns but is allowed — trying it is the person's call.
 * [installing] shows summaries and folds the standard groups into one row.
 * With [editable] false the groups are shown without switches: a sheet whose
 * answer keeps the grants the app already has.
 */
@Composable
internal fun PermissionGroupList(
    domains: List<String>,
    granted: Set<String>,
    needed: Set<String>,
    appName: String,
    installing: Boolean,
    onChange: (domains: List<String>, allow: Boolean) -> Unit,
    optional: Set<String> = emptySet(),
    editable: Boolean = true,
) {
    val context = LocalContext.current
    var open by remember { mutableStateOf(setOf<String>()) }
    var standardOpen by remember { mutableStateOf(false) }

    val groups = groupPermissions(domains)

    fun warnIfNeeded(off: List<String>) {
        if (off.any { it in needed }) {
            Toast.makeText(context, "$appName needs this — it may not work without it", Toast.LENGTH_LONG).show()
        }
    }

    Column(Modifier.fillMaxWidth()) {
        for (tier in listOf(Tier.ActsAsYou, Tier.Sensitive, Tier.Standard)) {
            val inTier = groups.filter { (_, mine) -> tierOf(mine) == tier }
            if (inTier.isEmpty()) continue
            TierHeader(tier)
            if (tier == Tier.Standard && installing && !standardOpen) {
                val allOn = inTier.all { (_, mine) -> mine.all { it in granted } }
                FoldedStandard(
                    names = inTier.joinToString(", ") { it.first.title },
                    allOn = allOn,
                    onOpen = { standardOpen = true },
                )
                continue
            }
            for ((group, mine) in inTier) {
                val isOpen = group.title in open
                GroupRow(
                    group = group,
                    parts = mine,
                    tier = tier,
                    granted = granted,
                    needed = needed,
                    optional = optional,
                    open = isOpen,
                    editable = editable,
                    summary = installing && !isOpen,
                    onToggleOpen = { open = if (isOpen) open - group.title else open + group.title },
                    onGroup = { allow ->
                        onChange(mine, allow)
                        if (!allow) warnIfNeeded(mine)
                    },
                    onPart = { domain, allow ->
                        onChange(listOf(domain), allow)
                        if (!allow) warnIfNeeded(listOf(domain))
                    },
                )
            }
        }
        if (domains.any { it in BASICS }) {
            // Always granted — but one switched off on the old sheet gets a
            // switch here, or nothing could turn it back on.
            val basicsOff = domains.filter { it in BASICS && it != "shell" && it !in granted }
            HorizontalDivider(Modifier.padding(vertical = 8.dp))
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(vertical = 6.dp)) {
                GroupIcon(Icons.Filled.Check, Tier.Standard, on = false)
                Spacer(Modifier.size(12.dp))
                Column(Modifier.weight(1f)) {
                    Text("Basics", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(
                        if (basicsOff.isEmpty()) "Start up and your theme. Every app has these."
                        else "Start up and your theme. Some are switched off.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (editable && basicsOff.isNotEmpty()) {
                    Switch(
                        checked = false,
                        onCheckedChange = { onChange(basicsOff, true) },
                        modifier = Modifier.semantics { contentDescription = "Basics" },
                    )
                }
            }
        }
    }
}

@Composable
private fun tierColor(tier: Tier): Color = when (tier) {
    Tier.ActsAsYou -> MaterialTheme.colorScheme.error
    // The theme's tertiary is its warning colour.
    Tier.Sensitive -> MaterialTheme.colorScheme.tertiary
    Tier.Standard -> MaterialTheme.colorScheme.onSurfaceVariant
}

@Composable
private fun TierHeader(tier: Tier) {
    val color = tierColor(tier)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.padding(top = 12.dp, bottom = 4.dp),
    ) {
        Box(Modifier.size(8.dp).clip(CircleShape).background(color))
        Spacer(Modifier.size(8.dp))
        Text(
            when (tier) {
                Tier.ActsAsYou -> "ACTS AS YOU"
                Tier.Sensitive -> "SENSITIVE"
                Tier.Standard -> "STANDARD"
            },
            style = MaterialTheme.typography.labelSmall,
            fontWeight = FontWeight.SemiBold,
            color = color,
        )
    }
}

@Composable
private fun GroupIcon(icon: ImageVector, tier: Tier, on: Boolean) {
    val tint = when {
        !on -> MaterialTheme.colorScheme.onSurfaceVariant
        tier == Tier.Standard -> MaterialTheme.colorScheme.primary
        else -> tierColor(tier)
    }
    val ground = if (on) tint.copy(alpha = 0.14f) else MaterialTheme.colorScheme.onSurface.copy(alpha = 0.06f)
    Box(
        modifier = Modifier.size(40.dp).clip(CircleShape).background(ground),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = tint)
    }
}

@Composable
private fun FoldedStandard(names: String, allOn: Boolean, onOpen: () -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .combinedClickableSafe(onOpen)
            .padding(vertical = 8.dp),
    ) {
        GroupIcon(Icons.Filled.Check, Tier.Standard, on = allOn)
        Spacer(Modifier.size(12.dp))
        Column(Modifier.weight(1f)) {
            Text(names, style = MaterialTheme.typography.titleSmall)
            Text(
                (if (allOn) "Allowed — what most apps need" else "Some turned off") + " · tap to review",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Icon(Icons.Filled.ExpandMore, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun GroupRow(
    group: PermissionGroup,
    parts: List<String>,
    tier: Tier,
    granted: Set<String>,
    needed: Set<String>,
    optional: Set<String>,
    open: Boolean,
    editable: Boolean,
    summary: Boolean,
    onToggleOpen: () -> Unit,
    onGroup: (Boolean) -> Unit,
    onPart: (String, Boolean) -> Unit,
) {
    val on = parts.count { it in granted }
    val anyOn = on > 0
    val isNeeded = parts.any { it in needed }
    Surface(
        shape = RoundedCornerShape(20.dp),
        color = if (open) MaterialTheme.colorScheme.onSurface.copy(alpha = 0.05f) else Color.Transparent,
        modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp),
    ) {
        Column {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(20.dp))
                    .combinedClickableSafe(onToggleOpen)
                    .padding(vertical = 8.dp, horizontal = 4.dp),
            ) {
                GroupIcon(group.icon, tier, on = anyOn)
                Spacer(Modifier.size(12.dp))
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            group.title,
                            style = MaterialTheme.typography.titleSmall,
                            color = if (anyOn) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (isNeeded) NeedsThis()
                    }
                    val (line, color) = when {
                        summary -> group.summary to MaterialTheme.colorScheme.onSurfaceVariant
                        !anyOn && isNeeded -> "Off · the app may not work" to MaterialTheme.colorScheme.tertiary
                        !anyOn -> "Not allowed" to MaterialTheme.colorScheme.onSurfaceVariant
                        on < parts.size -> "$on of ${parts.size} allowed" to MaterialTheme.colorScheme.primary
                        else -> "Allowed" to MaterialTheme.colorScheme.onSurfaceVariant
                    }
                    Text(line, style = MaterialTheme.typography.bodySmall, color = color)
                }
                Icon(
                    Icons.Filled.ExpandMore,
                    contentDescription = if (open) "Hide details" else "Show details",
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.rotate(if (open) 180f else 0f),
                )
                if (editable) {
                    Spacer(Modifier.size(8.dp))
                    Switch(
                        checked = anyOn,
                        onCheckedChange = onGroup,
                        modifier = Modifier.semantics { contentDescription = group.title },
                    )
                }
            }
            if (open) {
                Column(Modifier.padding(start = 56.dp, end = 4.dp, bottom = 8.dp)) {
                    parts.forEach { domain ->
                        val c = capabilityWording(domain)
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(vertical = 4.dp)) {
                            Column(Modifier.weight(1f)) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Box(
                                        Modifier.size(6.dp).clip(CircleShape)
                                            .background(tierColor(PART_TIER[domain] ?: Tier.Standard)),
                                    )
                                    Spacer(Modifier.size(6.dp))
                                    Text(
                                        if (domain in optional) "${c.title} (optional)" else c.title,
                                        style = MaterialTheme.typography.bodyMedium,
                                    )
                                }
                                Text(c.detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                            if (editable) {
                                Spacer(Modifier.size(8.dp))
                                Switch(
                                    checked = domain in granted,
                                    onCheckedChange = { onPart(domain, it) },
                                    modifier = Modifier.scale(0.8f).semantics { contentDescription = c.title },
                                )
                            }
                        }
                    }
                    group.note?.let { note ->
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 4.dp)) {
                            Icon(Icons.Filled.Lock, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
                            Spacer(Modifier.size(6.dp))
                            Text(note, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun NeedsThis() {
    Surface(shape = RoundedCornerShape(8.dp), color = MaterialTheme.colorScheme.secondaryContainer) {
        Text(
            "Needs this",
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSecondaryContainer,
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
        )
    }
}
