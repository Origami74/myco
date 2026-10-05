package app.myco.ui.screens

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.net.VpnService
import android.os.Build
import android.os.SystemClock
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.app.ActivityCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import app.myco.MainActivity
import app.myco.onboarding.MeshPermissions
import app.myco.onboarding.MeshSetup
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
                nearbyGranted = MeshPermissions.nearbyGranted(context),
                nearbyAsked = prefs.getBoolean(MainActivity.PREF_NEARBY_ASKED, false),
                vpnPrepared = MeshPermissions.vpnPrepared(context),
                vpnAsked = prefs.getBoolean(MainActivity.PREF_VPN_ASKED, false),
                tunnelUp = MycoVpnService.isUp(),
                notificationsOn = MeshPermissions.notificationsEnabled(context),
                batteryExempt = MeshPermissions.ignoringBatteryOptimizations(context),
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
 * mesh?"; its "Yes" asks Android and brings the lanes up. With it off they ask Android directly, since
 * there is nothing to start. Notifications and the battery exemption are not
 * part of setup at all — they are asked for here, when the user wants them.
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

    // POST_NOTIFICATIONS (API 33+). A request Android answers faster than
    // anyone could tap was refused without a dialog — "don't ask again" — so
    // go to the app's notification settings instead (same heuristic as the
    // nearby step, see MeshSetup.FAST_ANSWER_MS).
    var notifAskedAt by remember { mutableLongStateOf(0L) }
    val notifLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        val elapsed = SystemClock.elapsedRealtime() - notifAskedAt
        val rationale = (context as? Activity)?.let {
            ActivityCompat.shouldShowRequestPermissionRationale(it, Manifest.permission.POST_NOTIFICATIONS)
        } == true
        if (MeshSetup.refusedForGood(granted, elapsed, rationale)) openNotificationSettings(context)
    }

    // Asked from here only while the mesh is off; with it on, the setup popup
    // asks (see the rows). The results need no handling beyond the next
    // re-read, except a refusal for good, which goes to the app's settings.
    var nearbyAskedAt by remember { mutableLongStateOf(0L) }
    val nearbyLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { results ->
        val allGranted = results.isNotEmpty() && results.values.all { it }
        val rationale = (context as? Activity)?.let { a ->
            results.keys.any { ActivityCompat.shouldShowRequestPermissionRationale(a, it) }
        } == true
        if (results.isNotEmpty() &&
            MeshSetup.refusedForGood(allGranted, SystemClock.elapsedRealtime() - nearbyAskedAt, rationale)
        ) {
            runCatching {
                context.startActivity(
                    Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}")),
                )
            }
        }
    }
    val vpnLauncher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {}

    SettingsColumn {
        SubHeader("Permissions", onBack)
        val s = snap ?: return@SettingsColumn

        PermissionRow(
            icon = { PhonesIcon(it, Modifier.size(22.dp)) },
            title = "Nearby phones",
            status = when {
                s.nearbyGranted -> "Allowed — Bluetooth and Wi-Fi can find phones nearby"
                !s.nearbyAsked -> "Not asked yet"
                else -> "Not allowed — Myco can’t look for phones nearby"
            },
            action = if (s.nearbyGranted) null else if (s.nearbyAsked) "Fix" else "Allow",
            onAction = {
                // Mesh on: the setup popup, whose "Yes" asks and starts the
                // radios. Mesh off: just ask — nothing is to start.
                if (meshEnabled) {
                    onFixNearby()
                } else {
                    context.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE).edit()
                        .putBoolean(MainActivity.PREF_NEARBY_ASKED, true).apply()
                    nearbyAskedAt = SystemClock.elapsedRealtime()
                    nearbyLauncher.launch(MeshPermissions.nearby(context).toTypedArray())
                }
            },
        )
        PermissionRow(
            icon = { MeshIcon(it, Modifier.size(22.dp)) },
            title = "Mesh connection",
            status = when {
                !s.vpnPrepared && !s.vpnAsked -> "Not asked yet"
                !s.vpnPrepared -> "Not allowed, or another app’s VPN holds the slot"
                meshEnabled && !s.tunnelUp -> "Allowed, but not running"
                meshEnabled -> "Running — the VPN only links Myco phones"
                else -> "Allowed — the VPN only links Myco phones"
            },
            action = when {
                !s.vpnPrepared -> if (s.vpnAsked) "Fix" else "Allow"
                meshEnabled && !s.tunnelUp -> "Fix"
                else -> null
            },
            onAction = {
                // Mesh on: the setup popup, whose "Yes" asks and brings the
                // tunnel up. Mesh off: only Android's consent; no tunnel to start.
                val consent = VpnService.prepare(context)
                if (meshEnabled) {
                    onFixConnection()
                } else if (consent != null) {
                    context.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE).edit()
                        .putBoolean(MainActivity.PREF_VPN_ASKED, true).apply()
                    vpnLauncher.launch(consent)
                }
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
            onAction = {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    notifAskedAt = SystemClock.elapsedRealtime()
                    notifLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                } else {
                    // No runtime permission below 33: notifications were
                    // switched off in Android's settings, so that's the fix.
                    openNotificationSettings(context)
                }
            },
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
            onAction = { requestBatteryExemption(context) },
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

private fun openNotificationSettings(context: Context) {
    runCatching {
        context.startActivity(
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName),
        )
    }
}

/**
 * Ask Android to leave Myco out of battery optimisation, so phones that
 * suspend background apps don't cut the radios while the screen is off.
 *
 * The direct request needs `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` in the
 * manifest. Google Play only allows that permission for a short list of app
 * types; Myco ships through GitHub Releases and Zapstore, which have no such
 * rule (F-Droid doesn't either). Should a store ever object, the fallback
 * below — Android's own list of apps — needs no permission at all.
 */
private fun requestBatteryExemption(context: Context) {
    runCatching {
        context.startActivity(
            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${context.packageName}")),
        )
    }.onFailure {
        runCatching { context.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
    }
}
