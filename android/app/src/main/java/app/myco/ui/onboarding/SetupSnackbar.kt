package app.myco.ui.onboarding

import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarData
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarVisuals
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.myco.onboarding.SetupNotice
import app.myco.ui.theme.AmoledAccent

/**
 * The snackbar the setup popup leaves behind: an honest one-line summary of
 * how it ended, with a way into Settings › Permissions when something was
 * skipped.
 */
class SetupSnackbarVisuals(val notice: SetupNotice) : SnackbarVisuals {
    override val message: String = when (notice) {
        SetupNotice.AllSet -> "You’re set up — Myco will find phones nearby on its own."
        SetupNotice.NearbyMissing ->
            "Mesh is on, but Myco can’t look for phones nearby — fix it in Settings › Permissions."
        SetupNotice.MeshOff -> "Mesh is off — fix it in Settings › Permissions."
        SetupNotice.Declined -> "Mesh is off — turn it on any time in Settings."
    }
    override val actionLabel: String? = if (notice == SetupNotice.AllSet) null else "Settings"
    override val withDismissAction: Boolean = false
    override val duration: SnackbarDuration =
        if (notice == SetupNotice.AllSet) SnackbarDuration.Short else SnackbarDuration.Long

    /** Anything short of "all set" carries the amber mark. */
    val warning: Boolean get() = notice != SetupNotice.AllSet
}

/** Draws [SetupSnackbarVisuals]; any other snackbar falls back to its message. */
@Composable
fun SetupSnackbar(data: SnackbarData) {
    val visuals = data.visuals
    val warning = (visuals as? SetupSnackbarVisuals)?.warning == true
    val dark = MaterialTheme.colorScheme.background.luminance() < 0.5f
    Surface(
        shape = RoundedCornerShape(12.dp),
        // The design's zinc-800 on AMOLED black; Material's inverse surface on white.
        color = if (dark) Color(0xFF27272A) else MaterialTheme.colorScheme.inverseSurface,
        contentColor = if (dark) Color(0xFFF4F4F5) else MaterialTheme.colorScheme.inverseOnSurface,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.padding(start = 14.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
        ) {
            if (warning) {
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .size(18.dp)
                        .border(1.5.dp, MaterialTheme.colorScheme.tertiary, CircleShape),
                ) {
                    Text(
                        "!",
                        color = MaterialTheme.colorScheme.tertiary,
                        fontWeight = FontWeight.Bold,
                        fontSize = 11.sp,
                    )
                }
                Spacer(Modifier.size(12.dp))
            }
            Text(
                visuals.message,
                fontSize = 13.sp,
                lineHeight = 18.sp,
                modifier = Modifier.weight(1f).padding(vertical = 8.dp),
            )
            visuals.actionLabel?.let { label ->
                TextButton(onClick = { data.performAction() }) {
                    Text(
                        label,
                        // Dark ground in both themes, so the bright accent in both.
                        color = AmoledAccent,
                        fontWeight = FontWeight.Bold,
                        fontSize = 14.sp,
                    )
                }
            }
        }
    }
}
