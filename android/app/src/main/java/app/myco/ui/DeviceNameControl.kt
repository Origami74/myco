package app.myco.ui

import android.content.Context
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
    ownNpub: String,
    name: String,
): String {
    DeviceName.set(context, name)
    return DeviceName.current(context, ownNpub)
}
