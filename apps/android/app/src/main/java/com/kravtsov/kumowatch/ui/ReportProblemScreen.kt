package com.kravtsov.kumowatch.ui

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import com.kravtsov.kumowatch.Diagnostics
import com.kravtsov.kumowatch.IssueReport
import com.kravtsov.kumowatch.RelayPairingStorage
import android.content.ActivityNotFoundException
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

private const val MAX_DIAGNOSTICS_CHARS = 1_000_000
/** Long pastes would overflow the saved-state Bundle; GitHub's link holds far less anyway. */
private const val MAX_DRAFT_CHARS = 20_000

/**
 * GET /v1/diagnostics on the paired relay; null on any error, a 404 from older relays, or after 8 s
 * in all. The form stops waiting at the deadline even if a slow reply keeps the socket busy.
 */
private suspend fun fetchDiagnostics(context: Context): Diagnostics? {
    // Keystore decryption off the main thread.
    val pairing = withContext(Dispatchers.IO) { RelayPairingStorage(context).load() } ?: return null
    val connection = runCatching {
        URL(pairing.url.trimEnd('/') + "/v1/diagnostics").openConnection() as HttpURLConnection
    }.getOrNull() ?: return null
    val abandoned = java.util.concurrent.atomic.AtomicBoolean(false)
    // Outside the caller's scope, since a blocked read ignores cancellation; at the deadline or when
    // the form goes away, disconnecting closes the socket, which ends the read, and a worker that
    // had not started yet sees `abandoned` and never connects.
    val reply = CoroutineScope(Dispatchers.IO).async { requestDiagnostics(connection, pairing.secret, abandoned) }
    return try {
        withTimeoutOrNull(8_000) { reply.await() }
    } finally {
        if (!reply.isCompleted) {
            abandoned.set(true)
            connection.disconnect()
        }
    }
}

private fun requestDiagnostics(
    connection: HttpURLConnection,
    secret: String,
    abandoned: java.util.concurrent.atomic.AtomicBoolean,
): Diagnostics? {
    if (abandoned.get()) return null
    return runCatching {
        try {
            connection.connectTimeout = 5000
            connection.readTimeout = 5000
            connection.setRequestProperty("Authorization", "Bearer $secret")
            if (connection.responseCode != 200) return@runCatching null
            val text = connection.inputStream.bufferedReader().use { reader ->
                val buffer = CharArray(MAX_DIAGNOSTICS_CHARS + 1)
                var read = 0
                while (read < buffer.size) {
                    val n = reader.read(buffer, read, buffer.size - read)
                    if (n < 0) break
                    read += n
                }
                if (read > MAX_DIAGNOSTICS_CHARS) return@runCatching null
                String(buffer, 0, read)
            }
            // Strict: a proxy's JSON error page must not pass for a relay's answer.
            val json = JSONObject(text)
            val version = json.opt("version") as? String ?: return@runCatching null
            val os = json.opt("os") as? String ?: return@runCatching null
            val log = json.opt("log") as? String ?: return@runCatching null
            Diagnostics(version, os, log)
        } finally {
            connection.disconnect()
        }
    }.getOrNull()
}

/** A new file for each share: an app still holding an earlier grant never sees a later log. */
private fun logFile(context: Context, log: String): java.io.File {
    val dir = java.io.File(context.cacheDir, "logs").apply { mkdirs() }
    val dayAgo = System.currentTimeMillis() - 24 * 60 * 60 * 1000L
    dir.listFiles()?.filter { it.lastModified() < dayAgo }?.forEach { it.delete() }
    return java.io.File.createTempFile("better-kumo-log-", ".txt", dir).apply { writeText(log + "\n") }
}

private fun phoneDescription(context: Context): String {
    val info = context.packageManager.getPackageInfo(context.packageName, 0)
    @Suppress("DEPRECATION")
    val code = if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()
    return "Better Kumo Android ${info.versionName} ($code), ${Build.MANUFACTURER} ${Build.MODEL}, Android ${Build.VERSION.RELEASE}"
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ReportProblemScreen(paired: Boolean, onDone: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var error by remember { mutableStateOf<String?>(null) }
    var opened by rememberSaveable { mutableStateOf(false) }
    var title by rememberSaveable { mutableStateOf("") }
    var description by rememberSaveable { mutableStateOf("") }
    var includeLog by rememberSaveable { mutableStateOf(true) }
    var loading by remember { mutableStateOf(paired) }
    var diagnostics by remember { mutableStateOf<Diagnostics?>(null) }
    var attempt by remember { mutableStateOf(0) }
    LaunchedEffect(paired, attempt) {
        if (paired) {
            loading = true
            diagnostics = fetchDiagnostics(context)
        }
        loading = false
    }
    val save = androidx.activity.compose.rememberLauncherForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.CreateDocument("text/plain"),
    ) { uri ->
        val text = diagnostics?.log ?: return@rememberLauncherForActivityResult
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            error = runCatching {
                withContext(Dispatchers.IO) {
                    context.contentResolver.openOutputStream(uri)?.use { it.write((text + "\n").toByteArray()) }
                }
            }.exceptionOrNull()?.let { "Couldn't save the log: ${it.message ?: it.javaClass.simpleName}" }
        }
    }
    val log = diagnostics?.log?.takeIf { it.isNotBlank() }
    BackHandler(onBack = onDone)

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Report a problem") },
                navigationIcon = { IconButton(onClick = onDone) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).imePadding().verticalScroll(rememberScrollState())) {
            Text(
                "Describe the problem here. GitHub opens with it filled in: add screenshots there and tap Submit. " +
                    "The computer's recent log goes along unless you switch it off. Names, ids and paths it " +
                    "recognizes are removed, and GitHub gets the draft as soon as it opens, so look it over first.",
                Modifier.padding(16.dp),
            )
            OutlinedTextField(
                value = title,
                onValueChange = { title = it.take(MAX_DRAFT_CHARS) },
                label = { Text("Title") },
                placeholder = { Text("Short summary") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
            )
            OutlinedTextField(
                value = description,
                onValueChange = { description = it.take(MAX_DRAFT_CHARS) },
                label = { Text("What happened") },
                placeholder = { Text("What did you do, what did you expect, what happened instead?") },
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                // Bounded: inside the page's scroll, a tall paste scrolls within the field.
                maxLines = 12,
                modifier = Modifier.fillMaxWidth().heightIn(min = 160.dp, max = 320.dp).padding(16.dp),
            )
            ListItem(
                headlineContent = { Text("Include the computer's log") },
                supportingContent = when {
                    !paired -> { { Text("Pair a computer to include its log.") } }
                    loading -> { { Text("Getting the log…") } }
                    log == null -> {
                        {
                            Text(
                                "The computer didn't send its log. It may be asleep or offline, " +
                                    "or Better Kumo there may need an update.",
                            )
                        }
                    }
                    else -> null
                },
                trailingContent = {
                    Switch(checked = paired && includeLog, enabled = paired, onCheckedChange = { includeLog = it })
                },
            )
            Button(
                enabled = (!loading || !includeLog) && (title.isNotBlank() || description.isNotBlank()),
                onClick = {
                    val current = diagnostics
                    val computer = when {
                        !paired -> "not paired"
                        current == null -> "log not available"
                        else -> "Better Kumo ${current.version} on ${current.os}"
                    }
                    val url = IssueReport.url(title, description, phoneDescription(context), computer, log?.takeIf { includeLog })
                    // The form stays: GitHub may ask to sign in, and the log file is shared from here.
                    try {
                        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
                        error = null
                        opened = true
                    } catch (e: ActivityNotFoundException) {
                        error = "No browser can open GitHub. Install or enable one, then try again."
                    }
                },
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
            ) { Text("Open GitHub") }
            if (log != null) {
                TextButton(
                    onClick = {
                        scope.launch {
                            error = null
                            try {
                                val file = withContext(Dispatchers.IO) { logFile(context, log) }
                                val uri = androidx.core.content.FileProvider.getUriForFile(
                                    context, "${context.packageName}.logs", file,
                                )
                                val send = Intent(Intent.ACTION_SEND)
                                    .setType("text/plain")
                                    .putExtra(Intent.EXTRA_STREAM, uri)
                                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                                context.startActivity(Intent.createChooser(send, "Share the log"))
                            } catch (e: Exception) {
                                error = "Couldn't share the log: ${e.message ?: e.javaClass.simpleName}"
                            }
                        }
                    },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                ) { Text("Share the log file…") }
                // Somewhere GitHub's upload picker can reach, such as Downloads.
                TextButton(
                    onClick = { save.launch("better-kumo-log.txt") },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                ) { Text("Save the log file…") }
            } else if (paired && !loading) {
                TextButton(
                    onClick = { attempt++ },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                ) { Text("Try again to get the log") }
            }
            if (opened) {
                Text(
                    "GitHub opened with your report. Attach screenshots and the log file there, then tap Submit. " +
                        "Your text stays here until you go back.",
                    Modifier.padding(16.dp),
                )
            }
            error?.let {
                Text(it, color = androidx.compose.material3.MaterialTheme.colorScheme.error, modifier = Modifier.padding(16.dp))
            }
        }
    }
}
