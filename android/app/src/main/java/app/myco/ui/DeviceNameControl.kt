package app.myco.ui

import android.content.Context
import app.myco.ble.BleRadio
import app.myco.core.AppCoreClient
import app.myco.share.DeviceName

/**
 * Change this device's name.
 *
 * The name lives in the local preference, which every screen reads back and
 * which goes out in neighborhood invitations (as `from`). It is never put in
 * a radio advert or a nearby offer.
 *
 * Passing a blank name clears the override, and the name falls back to the
 * generated one; the resolved value is returned.
 */
fun applyDeviceName(
    context: Context,
    client: AppCoreClient,
    ownNpub: String,
    name: String,
): String {
    DeviceName.set(context, name)
    val resolved = DeviceName.current(context, ownNpub)
    // Harmless to re-assert, and it is what the advertised name is keyed on —
    // a radio that came up before the identity was ready has none yet.
    BleRadio.localNodeAddrHex = client.state().nodeAddrHex
    return resolved
}
