package app.myco

import android.app.Activity
import android.app.ActivityManager
import android.graphics.Bitmap
import android.graphics.Color
import android.view.View
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

/**
 * The chrome-less WebView task plumbing [NsiteActivity] and [NappletActivity]
 * share: edge-to-edge insets, system-bar contrast, and the Recents card.
 *
 * A **helper both call**, deliberately not a base class. The two hosts differ in
 * everything that matters — intent contract, request interception, navigation
 * policy, lifecycle, trust boundary — and a shared superclass is how nsite
 * semantics would leak into the class that hosts untrusted napplet code by
 * inheritance rather than by anyone deciding to.
 *
 * Nothing here knows what is being displayed. It is layout and window dressing.
 */
object ChromelessChrome {

    /**
     * Pad [root] for the system bars and the IME.
     *
     * We draw edge-to-edge and expect pages to pad via `env(safe-area-inset-*)`,
     * but older WebViews map only display cutouts into that env() — not the nav
     * bar — so bottom content hides behind the 3-button bar. Padding the WebView
     * itself does not reliably shrink its CSS viewport (the page still lays out
     * at full `100dvh` and is merely clipped), so the *parent* is padded: that
     * shrinks the WebView's layout height and with it the page's viewport.
     *
     * @param wantsFullHeight re-read on every insets pass, so a page that opts
     *   into the status-bar region — or navigation to one that does not —
     *   changes the padding rather than keeping the first answer forever.
     */
    fun applyInsets(root: View, wantsFullHeight: () -> Boolean) {
        ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
            val nav = insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom
            // Reserve whichever is taller: the nav bar (idle) or the soft keyboard
            // (open). Shrinking the WebView above the IME keeps a composer visible
            // on WebViews too old for `interactive-widget`/visualViewport handling.
            // Newer WebViews then see no occlusion, so their own keyboard logic
            // becomes a no-op — no double lift.
            val top = if (wantsFullHeight()) {
                0
            } else {
                // Take the display cutout too: on a notched device the cutout can
                // extend past the status bar, and content under it is physically
                // unreadable rather than merely cluttered.
                maxOf(
                    insets.getInsets(WindowInsetsCompat.Type.statusBars()).top,
                    insets.getInsets(WindowInsetsCompat.Type.displayCutout()).top,
                )
            }
            v.setPadding(0, top, 0, maxOf(nav, ime))
            insets
        }
    }

    /** Re-run the insets pass after [wantsFullHeight] would answer differently. */
    fun requestInsets(root: View) = ViewCompat.requestApplyInsets(root)

    /**
     * Match the system-bar icon contrast to whatever the page is showing.
     *
     * The palette cannot be predicted — an nsite's is arbitrary, and a napplet's
     * is inside an opaque-origin iframe — so the page's own `theme-color` (or
     * computed background) is read and the icons flipped to suit. A missing,
     * transparent or unparseable value leaves the current appearance alone.
     *
     * Runs on the WebView's JS callback, i.e. the UI thread.
     */
    fun syncBarContrast(activity: Activity, webView: WebView) {
        webView.evaluateJavascript(BG_PROBE_JS) { raw ->
            val color = parseCssColor(unquoteJs(raw)) ?: return@evaluateJavascript
            // "Light appearance" = dark icons, for a light bar background.
            val lightBg = isLightColor(color)
            WindowCompat.getInsetsController(activity.window, webView).apply {
                isAppearanceLightStatusBars = lightBg
                isAppearanceLightNavigationBars = lightBg
            }
        }
    }

    /**
     * Ask the page whether it declared `viewport-fit=cover` — the standard signal
     * that it handles safe areas itself, and so the one honest opt-in to the
     * status-bar region. Anything else, including the common case of no viewport
     * meta at all, keeps the status bar reserved.
     */
    fun probeFullHeight(webView: WebView, onResult: (Boolean) -> Unit) {
        webView.evaluateJavascript(VIEWPORT_FIT_PROBE_JS) { raw ->
            onResult(unquoteJs(raw) == "cover")
        }
    }

    /** Give the Recents card a label and, when one decoded, an icon. */
    fun applyTaskDescription(activity: Activity, label: String, icon: Bitmap?) {
        @Suppress("DEPRECATION")
        val desc = if (icon != null) {
            ActivityManager.TaskDescription(label, icon)
        } else {
            ActivityManager.TaskDescription(label)
        }
        activity.setTaskDescription(desc)
    }

    /** Read the page's declared theme-color, else its computed background. */
    private const val BG_PROBE_JS = """
        (function () {
          try {
            var m = document.querySelector('meta[name="theme-color"]');
            if (m && m.content) return m.content;
            var b = document.body ? getComputedStyle(document.body).backgroundColor : '';
            if (b && b !== 'rgba(0, 0, 0, 0)' && b !== 'transparent') return b;
            return getComputedStyle(document.documentElement).backgroundColor || '';
          } catch (e) { return ''; }
        })()
    """

    private const val VIEWPORT_FIT_PROBE_JS = """
        (function () {
          try {
            var m = document.querySelector('meta[name="viewport"]');
            var c = m ? (m.getAttribute('content') || '') : '';
            return /viewport-fit\s*=\s*cover/i.test(c) ? 'cover' : 'auto';
          } catch (e) { return 'auto'; }
        })()
    """

    /** Strip the JSON quoting `evaluateJavascript` wraps a returned string in. */
    fun unquoteJs(raw: String?): String {
        val s = raw?.trim().orEmpty()
        if (s.length < 2 || s == "null" || !s.startsWith("\"")) return s
        return s.substring(1, s.length - 1).replace("\\\"", "\"").replace("\\\\", "\\")
    }

    /**
     * Parse a CSS color — `#rgb`/`#rrggbb`, a name, or `rgb()`/`rgba()`. Null if
     * unparseable or fully transparent, i.e. no usable background to contrast
     * against.
     */
    private fun parseCssColor(value: String): Int? {
        val v = value.trim()
        if (v.isEmpty()) return null
        val rgb = Regex("rgba?\\(([^)]+)\\)").find(v)
        if (rgb != null) {
            val p = rgb.groupValues[1].split(",").map { it.trim() }
            if (p.size < 3) return null
            val r = p[0].toFloatOrNull()?.toInt() ?: return null
            val g = p[1].toFloatOrNull()?.toInt() ?: return null
            val b = p[2].toFloatOrNull()?.toInt() ?: return null
            if ((p.getOrNull(3)?.toFloatOrNull() ?: 1f) == 0f) return null
            return Color.rgb(r, g, b)
        }
        return runCatching { Color.parseColor(v) }.getOrNull()
    }

    /** Perceptual luminance test: true if [color] reads as a light background. */
    private fun isLightColor(color: Int): Boolean {
        val luminance =
            0.299 * Color.red(color) + 0.587 * Color.green(color) + 0.114 * Color.blue(color)
        return luminance > 140 // 0..255; midline biased slightly toward "dark icons".
    }
}
