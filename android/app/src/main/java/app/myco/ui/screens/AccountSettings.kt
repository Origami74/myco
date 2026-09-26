package app.myco.ui.screens

import android.app.Activity
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.os.PersistableBundle
import android.view.WindowManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.filled.Logout
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Key
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.PersonAdd
import androidx.compose.material.icons.filled.Security
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import app.myco.core.AccountState
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.signer.ExternalSigner
import app.myco.ui.GroupLabel
import app.myco.ui.SectionCard
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

// ----------------------------------------------------------------------------
// The account row at the top of Settings, like the account row in Android's
// own Settings: who is logged in, and the way into the Account page.
// ----------------------------------------------------------------------------

@Composable
internal fun AccountHeader(account: AccountState, client: AppCoreClient, onClick: () -> Unit) {
    SectionCard {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onClick)
                .padding(horizontal = 16.dp, vertical = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            AccountAvatar(client, account, 56.dp)
            Spacer(Modifier.size(16.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    if (account.loggedIn) displayName(account) else "Not logged in",
                    fontWeight = FontWeight.SemiBold,
                    style = MaterialTheme.typography.titleLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    // No npub here: a bech32 string means nothing at a glance.
                    // The Account page shows it, next to its copy button.
                    if (account.loggedIn) "Your public identity" else "Log in, or create an identity",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Icon(
                Icons.AutoMirrored.Filled.KeyboardArrowRight,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

// ----------------------------------------------------------------------------
// The Account page: the profile, the key, logout — or, logged out, the three
// ways back in.
// ----------------------------------------------------------------------------

@Composable
internal fun AccountSettings(state: AppState, client: AppCoreClient, onBack: () -> Unit) {
    val account = state.account
    // A napplet may have published a new profile since the last look.
    LaunchedEffect(Unit) { client.dispatch(NativeActions.accountRefresh()) }

    SettingsColumn {
        SubHeader("Account", onBack)
        Spacer(Modifier.height(4.dp))
        if (account.loggedIn) {
            LoggedIn(account, client)
        } else {
            LoggedOut(account, client)
        }
    }
}

@Composable
private fun LoggedIn(account: AccountState, client: AppCoreClient) {
    val context = LocalContext.current
    // Reveal is two steps: the warning, then the key.
    var warning by remember { mutableStateOf(false) }
    var revealed by remember { mutableStateOf<String?>(null) }
    var confirmLogout by remember { mutableStateOf(false) }

    Column(
        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        AccountAvatar(client, account, 96.dp)
        Spacer(Modifier.height(4.dp))
        Text(
            displayName(account),
            style = MaterialTheme.typography.headlineSmall,
            textAlign = TextAlign.Center,
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                shortNpub(account.npub),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
            )
            IconButton(onClick = {
                copy(context, "npub", account.npub, sensitive = false)
                Toast.makeText(context, "Public key copied", Toast.LENGTH_SHORT).show()
            }) {
                Icon(Icons.Filled.ContentCopy, contentDescription = "Copy public key", modifier = Modifier.size(18.dp))
            }
        }
        if (account.about.isNotEmpty()) {
            Text(
                account.about,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
            )
        }
        when {
            account.profileLoading -> Hint("Looking for your profile…")
            account.publishPending -> Hint("Your profile goes out to Nostr the next time you're online.")
        }
    }

    val signer = account.isSigner
    if (signer) {
        // The key is in the signer app; there is nothing here to reveal.
        GroupLabel("SIGNER")
        SectionCard {
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                LeadingIcon(Icons.Filled.Security)
                Spacer(Modifier.size(14.dp))
                Column {
                    Text(
                        "Signing with ${ExternalSigner.label(context, account.signerPackage)}",
                        fontWeight = FontWeight.SemiBold,
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        "Your key stays in the signer app. It asks you before Myco posts " +
                            "as you, unless you told it to remember.",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
    } else {
        GroupLabel("SECRET KEY")
        SectionCard {
            SettingRow(
                icon = Icons.Filled.Key,
                title = "Show my secret key",
                subtitle = "Your nsec, to log in with this identity elsewhere",
                onClick = { warning = true },
            )
        }
    }

    Spacer(Modifier.height(8.dp))
    SectionCard {
        SettingRow(
            icon = Icons.AutoMirrored.Filled.Logout,
            title = "Log out",
            subtitle = if (signer) "Stop using this identity in Myco" else "Remove this identity from this phone",
            titleColor = MaterialTheme.colorScheme.error,
            onClick = { confirmLogout = true },
        )
    }

    if (warning) {
        NsecWarningDialog(
            onConfirm = {
                warning = false
                revealed = client.revealNsec().ifEmpty { null }
            },
            onDismiss = { warning = false },
        )
    }
    revealed?.let { nsec ->
        NsecRevealDialog(nsec = nsec, onDismiss = { revealed = null })
    }
    if (confirmLogout) {
        LogoutDialog(
            account = account,
            onSaveFirst = {
                confirmLogout = false
                warning = true
            },
            onLogout = {
                confirmLogout = false
                client.dispatch(NativeActions.accountLogout())
            },
            onDismiss = { confirmLogout = false },
        )
    }
}

@Composable
private fun LoggedOut(account: AccountState, client: AppCoreClient) {
    val context = LocalContext.current
    var nsecOpen by remember { mutableStateOf(false) }
    val signerInstalled = remember { ExternalSigner.isInstalled(context) }
    // NIP-55 `get_public_key`: the signer answers with the user's pubkey and
    // its own package, which every later request is addressed to.
    val signerLogin = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val data = result.data
        val pubkey = data?.getStringExtra("result").orEmpty()
        val pkg = data?.getStringExtra("package").orEmpty()
        when {
            result.resultCode != Activity.RESULT_OK || data?.getBooleanExtra("rejected", false) == true ->
                Toast.makeText(context, "The signer app didn't log you in", Toast.LENGTH_SHORT).show()
            else -> client.dispatch(NativeActions.accountLoginSigner(pubkey, pkg))
        }
    }

    Text(
        "You're not logged in. Apps that post or sign things as you can't until you are.",
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodyMedium,
        modifier = Modifier.padding(horizontal = 4.dp),
    )
    if (account.error.isNotEmpty() && !nsecOpen) {
        Text(account.error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
    }

    GroupLabel("LOG IN")
    SectionCard {
        SettingRow(
            icon = Icons.Filled.PersonAdd,
            title = "Create a new identity",
            subtitle = "Start fresh as a Myco guest",
            onClick = { client.dispatch(NativeActions.accountNewGuest()) },
        )
        RowDivider()
        SettingRow(
            icon = Icons.Filled.Key,
            title = "Log in with nsec",
            subtitle = "Paste the secret key of an identity you have",
            onClick = { nsecOpen = !nsecOpen },
        )
        if (nsecOpen) {
            NsecField(account.error) { client.dispatch(NativeActions.accountLoginNsec(it)) }
        }
        RowDivider()
        if (signerInstalled) {
            SettingRow(
                icon = Icons.Filled.Security,
                title = "Log in with a signer",
                subtitle = "Amber or another signer app — your key stays there",
                onClick = {
                    runCatching { signerLogin.launch(ExternalSigner.loginIntent()) }
                        .onFailure {
                            Toast.makeText(context, "Couldn't open the signer app", Toast.LENGTH_SHORT).show()
                        }
                },
            )
        } else {
            // Nothing to call. Say what would make it work rather than hide it.
            SettingRow(
                icon = Icons.Filled.Security,
                title = "Log in with a signer",
                subtitle = "Install a signer app such as Amber to keep your key out of Myco",
                onClick = {
                    runCatching {
                        context.startActivity(
                            Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/greenart7c3/Amber")),
                        )
                    }
                },
            )
        }
    }
}

/** The nsec entry: masked, and kept out of the keyboard's learning. */
@Composable
private fun NsecField(error: String, onSubmit: (String) -> Unit) {
    var value by remember { mutableStateOf("") }
    var visible by remember { mutableStateOf(false) }
    // Only a failure from *this* attempt is shown under the field.
    var submitted by remember { mutableStateOf(false) }

    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        OutlinedTextField(
            value = value,
            onValueChange = {
                value = it
                submitted = false
            },
            label = { Text("nsec1…") },
            singleLine = true,
            isError = submitted && error.isNotEmpty(),
            supportingText = if (submitted && error.isNotEmpty()) {
                { Text(error) }
            } else {
                null
            },
            visualTransformation = if (visible) VisualTransformation.None else PasswordVisualTransformation(),
            // Password type: most keyboards neither suggest from nor remember it.
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
            trailingIcon = {
                IconButton(onClick = { visible = !visible }) {
                    Icon(
                        if (visible) Icons.Filled.VisibilityOff else Icons.Filled.Visibility,
                        contentDescription = if (visible) "Hide key" else "Show key",
                    )
                }
            },
            modifier = Modifier.fillMaxWidth(),
        )
        Row {
            Spacer(Modifier.weight(1f))
            Button(
                enabled = value.isNotBlank(),
                onClick = {
                    submitted = true
                    onSubmit(value.trim())
                },
            ) { Text("Log in") }
        }
    }
}

// ----------------------------------------------------------------------------
// Dialogs
// ----------------------------------------------------------------------------

@Composable
private fun NsecWarningDialog(onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(Icons.Filled.Security, contentDescription = null, tint = MaterialTheme.colorScheme.error) },
        title = { Text("Never share this key") },
        text = {
            Text(
                "Your nsec is the password to your identity. Anyone who has it can post as you, " +
                    "and it can't be changed or taken back.\n\n" +
                    "Never share it with anyone — not a friend, not an app, not someone who says " +
                    "they're from Myco. Only paste it into apps you trust to log in as you.",
            )
        },
        confirmButton = { TextButton(onClick = onConfirm) { Text("Show my key") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun NsecRevealDialog(nsec: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    // No screenshots or recents thumbnail while the key is on screen.
    DisposableEffect(Unit) {
        val window = context.findActivity()?.window
        window?.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        onDispose { window?.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        // The dialog is its own window: the activity's FLAG_SECURE above
        // does not reliably reach it.
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Your secret key") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Surface(
                    shape = RoundedCornerShape(10.dp),
                    color = MaterialTheme.colorScheme.surfaceVariant,
                ) {
                    Text(
                        nsec,
                        style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                        modifier = Modifier.padding(12.dp),
                    )
                }
                Text(
                    "Store it in a password manager. Don't send it in a message.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        },
        confirmButton = {
            TextButton(onClick = {
                copy(context, "nsec", nsec, sensitive = true)
                Toast.makeText(context, "Secret key copied", Toast.LENGTH_SHORT).show()
            }) { Text("Copy") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

@Composable
private fun LogoutDialog(
    account: AccountState,
    onSaveFirst: () -> Unit,
    onLogout: () -> Unit,
    onDismiss: () -> Unit,
) {
    if (account.isSigner) {
        // Nothing to lose here: the key stays in the signer app.
        AlertDialog(
            onDismissRequest = onDismiss,
            title = { Text("Log out of ${displayName(account)}?") },
            text = {
                Text(
                    "Myco stops using this identity. Your key stays in " +
                        "${ExternalSigner.label(LocalContext.current, account.signerPackage)}, " +
                        "and you can log in with it again any time.",
                )
            },
            confirmButton = {
                TextButton(onClick = onLogout) {
                    Text("Log out", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        )
        return
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Log out of ${displayName(account)}?") },
        text = {
            Text(
                // Deliberately the same for every account: Myco cannot know
                // whether a key is also saved or in use somewhere else — a
                // "guest" may have been exported, an imported key may have
                // been made here.
                "This removes the identity from this phone. To use it again — here or anywhere " +
                    "else — you'll need its secret key (nsec).\n\n" +
                    "If you haven't saved the key somewhere safe, save it first. Without it, " +
                    "this identity can't be recovered.",
            )
        },
        confirmButton = {
            Row {
                TextButton(onClick = onSaveFirst) { Text("Show key first") }
                TextButton(onClick = onLogout) {
                    Text("Log out", color = MaterialTheme.colorScheme.error)
                }
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

// ----------------------------------------------------------------------------
// Pieces
// ----------------------------------------------------------------------------

/** The account's picture, or a person glyph while there is none. */
@Composable
internal fun AccountAvatar(client: AppCoreClient, account: AccountState, size: Dp) {
    val px = with(LocalDensity.current) { size.roundToPx() }
    val bitmap by produceState<ImageBitmap?>(null, account.avatarRev, account.status) {
        value = withContext(Dispatchers.IO) {
            runCatching { client.accountAvatar()?.let { decodeAvatar(it, px) } }.getOrNull()
        }
    }
    Box(
        modifier = Modifier
            .size(size)
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.background),
        contentAlignment = Alignment.Center,
    ) {
        val image = bitmap
        if (image != null) {
            Image(
                image,
                contentDescription = "Profile picture",
                contentScale = ContentScale.Crop,
                modifier = Modifier.size(size),
            )
        } else {
            Icon(
                Icons.Filled.Person,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(size * 0.55f),
            )
        }
    }
}

@Composable
private fun Hint(text: String) {
    Text(
        text,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodySmall,
        textAlign = TextAlign.Center,
    )
}

private fun displayName(account: AccountState): String =
    account.name.ifEmpty { shortNpub(account.npub) }

/** `npub1abcd…wxyz` — enough to tell two apart, short enough for one line. */
private fun shortNpub(npub: String): String =
    if (npub.length > 20) "${npub.take(12)}…${npub.takeLast(6)}" else npub

/** Decode at roughly the size it is drawn; a profile picture can be large. */
private fun decodeAvatar(bytes: ByteArray, targetPx: Int): ImageBitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (bounds.outWidth / (sample * 2) >= targetPx && bounds.outHeight / (sample * 2) >= targetPx) {
        sample *= 2
    }
    val opts = BitmapFactory.Options().apply { inSampleSize = sample }
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, opts)?.asImageBitmap()
}

/** Copy to the clipboard; a sensitive copy is hidden from the clipboard preview (API 33+). */
private fun copy(context: Context, label: String, text: String, sensitive: Boolean) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    val clip = ClipData.newPlainText(label, text)
    if (sensitive && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        clip.description.extras = PersistableBundle().apply {
            putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
        }
    }
    clipboard.setPrimaryClip(clip)
}

private fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}
