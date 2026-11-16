package app.myco.ui

import android.content.Context
import app.myco.ble.BleRadio
import app.myco.core.AppCoreClient
import app.myco.share.DeviceName

/**
 * Change this device's name.
 *
 * The name lives in the local preference, which every screen reads back and
 * which goes out in neighborhood invitations (as `from`) and in this phone's
 * offer on nearby links, so the core is told it for those. It is never put in
 * a radio advert.
 *
 * Passing a blank name clears the override, and the name falls back to the
 * phone's own or the generated one; the resolved value is returned.
 */
fun applyDeviceName(
    context: Context,
    client: AppCoreClient,
    ownNpub: String,
    name: String,
): String {
    DeviceName.set(context, name)
    val resolved = DeviceName.current(context, ownNpub)
    client.dispatch(app.myco.core.NativeActions.setNearbyName(resolved))
    // Harmless to re-assert, and it is what the advertised name is keyed on —
    // a radio that came up before the identity was ready has none yet.
    BleRadio.localNodeAddrHex = client.state().nodeAddrHex
    return resolved
}

/**
 * The generated name, without storing it, until the user has answered the
 * setup popup's name step. Nothing is stored here, so what the user picks is
 * what [applyDeviceName] keeps next.
 */
fun publishProvisionalName(client: AppCoreClient, ownNpub: String): String {
    val name = DeviceName.generated(ownNpub)
    client.dispatch(app.myco.core.NativeActions.setNearbyName(name))
    BleRadio.localNodeAddrHex = client.state().nodeAddrHex
    return name
}
