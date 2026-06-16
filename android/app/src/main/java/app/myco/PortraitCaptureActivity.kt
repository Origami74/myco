package app.myco

import com.journeyapps.barcodescanner.CaptureActivity

/**
 * A portrait-locked capture activity for the in-app QR scanner. The
 * zxing-android-embedded default `CaptureActivity` follows the sensor and tends
 * to open sideways; this subclass is pinned to portrait in the manifest.
 */
class PortraitCaptureActivity : CaptureActivity()
