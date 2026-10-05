package app.myco.ui.screens

import android.content.Context
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
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
import androidx.compose.material.icons.outlined.BatteryFull
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import app.myco.LocalSystemAsker
import app.myco.MainActivity
import app.myco.onboarding.SystemAsk
import app.myco.ui.onboarding.MeshIcon
import app.myco.ui.onboarding.PhonesIcon
import app.myco.vpn.MycoVpnService
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** One read of everything the Permissions page shows. */
private data class PermissionSnapshot(
    val nearbyGranted: Boolean,
    val nearbyAsked: Boolean,
    val vpnPrepared: Boolean,
    val vpnAsked: Boolean,
    val tunnelUp: Boolean,
    val notificationsOn: Boolean,
    val batteryExempt: Boolean,
) {
    companion object {
        /** Binder calls (`VpnService.prepare`, power manager): off the main thread. */
        fun read(context: Context): PermissionSnapshot {
            val prefs = context.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE)
            return PermissionSnapshot(
                nearbyGranted = SystemAsk.Nearby.granted(context),
                nearbyAsked = prefs.getBoolean(MainActivity.PREF_NEARBY_ASKED, false),
                vpnPrepared = SystemAsk.Vpn.granted(context),
                vpnAsked = prefs.getBoolean(MainActivity.PREF_VPN_ASKED, false),
                tunnelUp = MycoVpnService.isUp(),
                notificationsOn = SystemAsk.Notifications.granted(context),
                batteryExempt = SystemAsk.Battery.granted(context),
            )
        }
    }
}

/**
 * **Settings › Permissions** — what Myco has from Android, one row each, with
 * the way to fix it beside it. Permissions only: the mesh switch lives where
 * it always has (Settings › Mesh, the status pill).
 *
 * With the mesh on, the mesh rows' Fix reopens the setup popup on "Enable
 * mesh?"; its "Yes" leads to the explain cards, and they ask Android and bring
 * the lanes up. With it off there is nothing to start, so they show the same
 * explanation in the explain dialog, whose "Continue" asks. Notifications and
 * the battery exemption are not part of setup at all — they are asked for
 * here, the same way, when the user wants them. Nothing on this page asks
 * Android itself: every Allow goes through [app.myco.SystemAsker].
 */
@Composable
internal fun PermissionsSettings(
    meshEnabled: Boolean,
    onFixNearby: () -> Unit,
    onFixConnection: () -> Unit,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    var snap by remember { mutableStateOf<PermissionSnapshot?>(null) }
    // Re-read while the page is resumed: every fix happens in another screen
    // (a system prompt, Android's settings), and the row should say so the
    // moment the user is back.
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(Unit) {
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            while (true) {
                snap = withContext(Dispatchers.IO) { PermissionSnapshot.read(context) }
                delay(1000)
            }
        }
    }

    val asker = LocalSystemAsker.current

    SettingsColumn {
        SubHeader("Permissions", onBack)
        val s = snap ?: return@SettingsColumn

        PermissionRow(
            icon = { PhonesIcon(it, Modifier.size(22.dp)) },
            title = "Nearby phones",
            status = when {
                s.nearbyGranted -> "Allowed — Bluetooth and Wi-Fi can find nearby mesh devices"
                !s.nearbyAsked -> "Not asked yet — lets Bluetooth and Wi-Fi find nearby mesh devices"
                else -> "Not allowed — Myco can’t look for phones nearby"
            },
            action = if (s.nearbyGranted) null else if (s.nearbyAsked) "Fix" else "Allow",
            onAction = {
                // Mesh on: the setup popup, whose "Yes" leads to the explain
                // card and starts the radios. Mesh off: the explanation, then
                // the ask — nothing is to start.
                if (meshEnabled) onFixNearby() else asker?.explain(SystemAsk.Nearby)
            },
        )
        PermissionRow(
            icon = { MeshIcon(it, Modifier.size(22.dp)) },
            title = "Mesh connection",
            status = when {
                // Said before "Allow" puts Android's VPN prompt up (mesh off).
                !s.vpnPrepared && !s.vpnAsked ->
                    "Not asked yet — connects this phone to the FIPS mesh; your regular internet doesn’t go through it"
                !s.vpnPrepared -> "Not allowed, or another app’s VPN holds the slot"
                meshEnabled && !s.tunnelUp -> "Allowed, but not running"
                meshEnabled -> "Running — this phone is on the FIPS mesh"
                else -> "Allowed — connects this phone to the FIPS mesh"
            },
            action = when {
                !s.vpnPrepared -> if (s.vpnAsked) "Fix" else "Allow"
                meshEnabled && !s.tunnelUp -> "Fix"
                else -> null
            },
            onAction = {
                // Mesh on: the setup popup, whose "Yes" leads to the explain
                // card and brings the tunnel up. Mesh off: the explanation,
                // then only Android's consent; no tunnel to start.
                if (meshEnabled) onFixConnection() else asker?.explain(SystemAsk.Vpn)
            },
        )
        PermissionRow(
            icon = { Icon(Icons.Outlined.Notifications, contentDescription = null, tint = it) },
            title = "Notifications",
            status = if (s.notificationsOn) {
                "On — you hear when someone sends you a file"
            } else {
                "Off — tell me when someone sends me a file"
            },
            action = if (s.notificationsOn) null else "Allow",
            // Below API 33 there is no prompt: the ask opens Android's
            // notification settings, where they were switched off.
            onAction = { asker?.explain(SystemAsk.Notifications) },
        )
        PermissionRow(
            icon = { Icon(Icons.Outlined.BatteryFull, contentDescription = null, tint = it) },
            title = "Keep running",
            status = if (s.batteryExempt) {
                "On — Android won’t pause Myco in the background"
            } else {
                "Optional — stay connected while the screen is off"
            },
            action = if (s.batteryExempt) null else "Allow",
            onAction = { asker?.explain(SystemAsk.Battery) },
        )
    }
}

@Composable
private fun PermissionCard(content: @Composable () -> Unit) {
    Surface(
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Box(Modifier.padding(horizontal = 16.dp, vertical = 14.dp)) { content() }
    }
}

@Composable
private fun PermissionRow(
    icon: @Composable (androidx.compose.ui.graphics.Color) -> Unit,
    title: String,
    status: String,
    action: String?,
    onAction: () -> Unit,
) {
    PermissionCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(36.dp)
                    .background(MaterialTheme.colorScheme.onSurface.copy(alpha = 0.08f), CircleShape),
            ) { icon(MaterialTheme.colorScheme.onSurfaceVariant) }
            Spacer(Modifier.size(14.dp))
            Column(Modifier.weight(1f)) {
                Text(title, fontWeight = FontWeight.SemiBold, fontSize = 15.sp)
                Text(status, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
            }
            if (action != null) {
                Spacer(Modifier.size(8.dp))
                OutlinedButton(
                    onClick = onAction,
                    border = BorderStroke(1.dp, MaterialTheme.colorScheme.primary),
                ) {
                    Text(action, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.SemiBold)
                }
            }
        }
    }
}
