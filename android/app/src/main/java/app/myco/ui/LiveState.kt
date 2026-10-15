package app.myco.ui

import android.app.ActivityManager
import android.content.Context
import androidx.compose.runtime.State
import androidx.compose.runtime.staticCompositionLocalOf
import app.myco.core.AppState

/**
 * The latest full core snapshot, per-second diagnostics included, provided by
 * [MycoApp].
 *
 * The screens get [app.myco.core.forShell]'s projection, which only changes
 * when something they draw does. A screen that shows the diagnostic numbers
 * (RTT, last heard, storage counts) reads them here instead:
 *
 * ```
 * val live by LocalLiveState.current
 * ```
 *
 * It is a [State], not a value, so only the scopes that read `.value` recompose
 * on each poll, not everything under the provider.
 */
val LocalLiveState = staticCompositionLocalOf<State<AppState>> {
    error("LocalLiveState is provided by MycoApp")
}

/** How often the shell polls the core: once a second, or every two on a low-RAM device. */
fun shellPollMs(context: Context): Long {
    val am = context.getSystemService(ActivityManager::class.java)
    return if (am?.isLowRamDevice == true) 2_000L else 1_000L
}
