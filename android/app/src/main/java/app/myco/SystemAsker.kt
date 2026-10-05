package app.myco

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.SystemClock
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.staticCompositionLocalOf
import app.myco.onboarding.AskMechanism
import app.myco.onboarding.MeshSetup
import app.myco.onboarding.SystemAsk
import app.myco.ui.onboarding.Confirmed

/**
 * How one [SystemAsk] went.
 *
 * @param granted Android has nothing left to ask, read again once the prompt closed.
 * @param resultOk the consent or intent activity answered `RESULT_OK`.
 * @param answered a real answer came back — not a request Android cut short.
 * @param refusedForGood refused without a dialog ("don't ask again"); see [MeshSetup.refusedForGood].
 * @param elapsedMs from launch to result; see [MeshSetup.FAST_ANSWER_MS].
 */
data class AskResult(
    val ask: SystemAsk,
    val granted: Boolean,
    val resultOk: Boolean,
    val answered: Boolean,
    val refusedForGood: Boolean,
    val elapsedMs: Long,
)

/**
 * The one place Myco puts an Android prompt up: every [SystemAsk]'s
 * launcher is registered here, once, and [launch] is the only way to use
 * them. It takes a [Confirmed], which only a tap on an explanation's
 * button makes (`ExplainCard`, `AskButton`) — so a prompt can't fire on its
 * own, or without its explanation having been on screen.
 *
 * Owned by [MainActivity], the only Activity whose screens ask; Compose
 * reaches it through [LocalSystemAsker]. One prompt at a time: a launch while
 * another prompt is up is dropped, as Android would drop it.
 *
 * Screens outside the setup popup ask through [explain], which puts the
 * explain dialog up; its "Continue" comes back here as [launch]. Results go
 * to [onResult] in the Activity, which survives being recreated behind a
 * prompt ([save] / [restore]).
 */
class SystemAsker(
    private val activity: ComponentActivity,
    private val onResult: (AskResult) -> Unit,
) {
    /** The explain dialog's ask, while it is up; null when closed. */
    val explaining = mutableStateOf<SystemAsk?>(null)

    /** Bumped on every result, so a screen re-reads [SystemAsk.granted]. */
    val revision = mutableIntStateOf(0)

    /** The prompt that is up, if any. */
    private var pending: SystemAsk? = null
    private var launchedAt = 0L

    private val permissionLaunchers: Map<SystemAsk, ActivityResultLauncher<Array<String>>> =
        SystemAsk.entries.filter { it.mechanism == AskMechanism.Permissions }.associateWith { ask ->
            activity.registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { results ->
                onPermissionResult(ask, results)
            }
        }

    private val intentLaunchers: Map<SystemAsk, ActivityResultLauncher<Intent>> =
        SystemAsk.entries.filter { it.mechanism != AskMechanism.Permissions }.associateWith { ask ->
            activity.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
                val ok = result.resultCode == android.app.Activity.RESULT_OK
                finish(ask, granted = ok || ask.granted(activity), resultOk = ok, answered = true, refusedForGood = false)
            }
        }

    /**
     * Show [ask]'s explanation in the explain dialog. Its "Continue" is
     * the tap that asks. Nothing happens while a prompt is up.
     */
    fun explain(ask: SystemAsk) {
        if (pending == null) explaining.value = ask
    }

    fun dismissExplain() {
        explaining.value = null
    }

    /**
     * Put [confirmed]'s prompt up. Returns whether a prompt was launched:
     * false while another is up, or when Android has nothing to ask (the
     * caller carries on as if granted). Notifications below API 33 have no
     * runtime permission; their "prompt" is the app's notification settings.
     */
    fun launch(confirmed: Confirmed): Boolean {
        val ask = confirmed.ask
        explaining.value = null
        if (pending != null) return false
        when (ask.mechanism) {
            AskMechanism.Permissions -> {
                val needed = ask.permissions(activity).filterNot { SystemAsk.granted(activity, listOf(it)) }
                if (needed.isEmpty()) {
                    if (ask == SystemAsk.Notifications && !ask.granted(activity)) openNotificationSettings(activity)
                    return false
                }
                begin(ask)
                permissionLaunchers.getValue(ask).launch(needed.toTypedArray())
            }
            AskMechanism.VpnConsent, AskMechanism.Intent -> {
                val intent = ask.intent(activity) ?: return false
                begin(ask)
                runCatching { intentLaunchers.getValue(ask).launch(intent) }.onFailure {
                    pending = null
                    // The direct battery request needs a manifest permission
                    // some stores object to; Android's own list needs none.
                    if (ask == SystemAsk.Battery) {
                        runCatching { activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
                    }
                    return false
                }
            }
        }
        return true
    }

    private fun begin(ask: SystemAsk) {
        pending = ask
        launchedAt = SystemClock.elapsedRealtime()
        // Settings › Permissions says "Not asked yet" until the first ask.
        val asked = when (ask) {
            SystemAsk.Nearby -> MainActivity.PREF_NEARBY_ASKED
            SystemAsk.Vpn -> MainActivity.PREF_VPN_ASKED
            else -> null
        }
        if (asked != null) {
            activity.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE).edit().putBoolean(asked, true).apply()
        }
    }

    private fun onPermissionResult(ask: SystemAsk, results: Map<String, Boolean>) {
        val granted = ask.granted(activity)
        // An empty result is a request Android cut short (the Activity went
        // away under it), not an answer — never read that as "for good".
        val answered = results.isNotEmpty()
        val rationale = results.filterValues { !it }.keys.any { activity.shouldShowRequestPermissionRationale(it) }
        val forGood = answered && MeshSetup.refusedForGood(
            allGranted = granted,
            elapsedMs = SystemClock.elapsedRealtime() - launchedAt,
            anyRationale = rationale,
        )
        finish(ask, granted = granted, resultOk = granted, answered = answered, refusedForGood = forGood)
    }

    private fun finish(ask: SystemAsk, granted: Boolean, resultOk: Boolean, answered: Boolean, refusedForGood: Boolean) {
        val elapsed = SystemClock.elapsedRealtime() - launchedAt
        pending = null
        revision.intValue++
        onResult(AskResult(ask, granted, resultOk, answered, refusedForGood, elapsed))
    }

    /** A result can land in a recreated Activity; it needs to know what was up and since when. */
    fun save(out: Bundle) {
        out.putString(STATE_PENDING, pending?.name)
        out.putLong(STATE_LAUNCHED_AT, launchedAt)
        out.putString(STATE_EXPLAINING, explaining.value?.name)
    }

    fun restore(state: Bundle?) {
        state ?: return
        pending = state.getString(STATE_PENDING)?.let { n -> SystemAsk.entries.firstOrNull { it.name == n } }
        launchedAt = state.getLong(STATE_LAUNCHED_AT)
        explaining.value = state.getString(STATE_EXPLAINING)?.let { n -> SystemAsk.entries.firstOrNull { it.name == n } }
    }

    private companion object {
        const val STATE_PENDING = "system_ask_pending"
        const val STATE_LAUNCHED_AT = "system_ask_launched_at"
        const val STATE_EXPLAINING = "system_ask_explaining"
    }
}

/** The app's [SystemAsker]; null outside [MainActivity], where nothing asks. */
val LocalSystemAsker = staticCompositionLocalOf<SystemAsker?> { null }

/** The app's notification settings: what Notifications asks through below API 33, and after a refusal for good. */
fun openNotificationSettings(context: Context) {
    runCatching {
        context.startActivity(
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName),
        )
    }
}
