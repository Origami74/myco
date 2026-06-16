package app.myco

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import app.myco.core.AppCoreClient

/** Fetch an nsite's icon from the local gateway (the manifest's `/favicon.ico`,
 *  falling back to common icon paths). Used for the Recents task icon and the
 *  home-screen shortcut. */
object NsiteIcons {
    /** Many `.ico` files are really PNG, so BitmapFactory often decodes them. */
    private val ICON_PATHS = listOf("/favicon.ico", "/favicon.png", "/apple-touch-icon.png")

    /** Blocking — call off the UI thread. Returns null if no icon decodes. */
    fun fetch(client: AppCoreClient, host: String): Bitmap? =
        ICON_PATHS.firstNotNullOfOrNull { path ->
            runCatching {
                val res = client.gatewayGet(host, path, "")
                if (res.status == 200 && res.body.isNotEmpty()) {
                    BitmapFactory.decodeByteArray(res.body, 0, res.body.size)
                } else {
                    null
                }
            }.getOrNull()
        }
}
