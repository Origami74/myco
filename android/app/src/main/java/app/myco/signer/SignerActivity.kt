package app.myco.signer

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts

/**
 * An invisible activity that shows the signer app's approval screen for one
 * request and hands the answer back to [ExternalSigner].
 *
 * Its own activity rather than whichever Myco screen is in front, because a
 * signing request can come from any of them — usually a napplet window — and
 * an activity result has to be received by the activity that asked.
 */
class SignerActivity : ComponentActivity() {
    private var requestId = ""

    private val launcher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val data = result.data
        val answer = when {
            // NIP-55: not OK means the signer failed, not that the user said no.
            result.resultCode != Activity.RESULT_OK -> Result.failure(IllegalStateException("the signer app failed"))
            data?.getBooleanExtra("rejected", false) == true -> Result.failure(IllegalStateException(ExternalSigner.REJECTED))
            else -> {
                val event = data?.getStringExtra("event")?.takeIf { it.isNotBlank() }
                val sig = data?.getStringExtra("result")?.takeIf { it.isNotBlank() }
                (event ?: sig)?.let { Result.success(it) }
                    ?: Result.failure(IllegalStateException("the signer app sent nothing back"))
            }
        }
        ExternalSigner.complete(requestId, answer)
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestId = intent.getStringExtra(EXTRA_ID).orEmpty()
        // Recreated (rotation): the signer is already showing; wait for it.
        if (savedInstanceState != null) return
        val signerIntent = Intent(
            Intent.ACTION_VIEW,
            Uri.parse("nostrsigner:" + intent.getStringExtra(EXTRA_PAYLOAD).orEmpty()),
        ).apply {
            `package` = intent.getStringExtra(EXTRA_PACKAGE)
            putExtra("type", intent.getStringExtra(EXTRA_TYPE))
            putExtra("id", requestId)
            putExtra("current_user", intent.getStringExtra(EXTRA_CURRENT_USER))
        }
        try {
            launcher.launch(signerIntent)
        } catch (e: ActivityNotFoundException) {
            ExternalSigner.complete(requestId, Result.failure(IllegalStateException("the signer app is not installed")))
            finish()
        }
    }

    override fun onDestroy() {
        // Gone without an answer (the user backed out of everything): say so
        // now rather than leaving the request to time out. A second complete
        // after a real answer is a no-op.
        if (isFinishing) {
            ExternalSigner.complete(requestId, Result.failure(IllegalStateException(ExternalSigner.REJECTED)))
        }
        super.onDestroy()
    }

    companion object {
        private const val EXTRA_ID = "app.myco.signer.ID"
        private const val EXTRA_TYPE = "app.myco.signer.TYPE"
        private const val EXTRA_PAYLOAD = "app.myco.signer.PAYLOAD"
        private const val EXTRA_CURRENT_USER = "app.myco.signer.CURRENT_USER"
        private const val EXTRA_PACKAGE = "app.myco.signer.PACKAGE"

        fun intent(context: Context, req: ExternalSigner.Request): Intent =
            Intent(context, SignerActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                putExtra(EXTRA_ID, req.id)
                putExtra(EXTRA_TYPE, req.type)
                putExtra(EXTRA_PAYLOAD, req.payload)
                putExtra(EXTRA_CURRENT_USER, req.currentUser)
                putExtra(EXTRA_PACKAGE, req.pkg)
            }
    }
}
