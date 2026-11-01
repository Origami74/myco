package app.myco.vpn

import android.content.Context
import app.myco.MainActivity

/**
 * Whether Myco wants the VPN, and why.
 *
 * Myco's own mesh traffic never needs it: the core runs its own TCP/IP stack
 * on the node's mesh address (`myco-core`'s `mesh_net`), so the relay, Blossom
 * and pairing reach peers with no VPN slot taken. The VPN ([MycoVpnService])
 * is only for **other apps** — "Allow other apps to use mesh" — and for the
 * exit proxy, which carries other apps' traffic by definition.
 */
object MeshSharing {
    /** "Allow other apps to use mesh". Off unless the user turns it on. */
    const val PREF = "share_mesh"

    private fun prefs(context: Context) =
        context.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE)

    /** The "Allow other apps to use mesh" switch. */
    fun enabled(context: Context): Boolean = prefs(context).getBoolean(PREF, false)

    fun setEnabled(context: Context, enabled: Boolean) {
        prefs(context).edit().putBoolean(PREF, enabled).apply()
    }

    /** Whether the VPN should run: other apps allowed on the mesh, or an exit proxy set. */
    fun vpnWanted(context: Context): Boolean =
        enabled(context) || prefs(context).getString(MainActivity.PREF_EXIT_PROXY, "").orEmpty().isNotBlank()
}
