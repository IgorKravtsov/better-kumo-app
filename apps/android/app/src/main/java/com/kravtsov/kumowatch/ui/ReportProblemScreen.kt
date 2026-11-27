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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

/** GET /v1/diagnostics on the paired relay; null on any error, a 404 from older relays, or a 5 s timeout. */
private suspend fun fetchDiagnostics(context: Context): Diagnostics? = withContext(Dispatchers.IO) {
    val pairing = RelayPairingStorage(context).load() ?: return@withContext null
    runCatching {
        val connection = URL(pairing.url.trimEnd('/') + "/v1/diagnostics").openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 5000
            connection.readTimeout = 5000
            connection.setRequestProperty("Authorization", "Bearer ${pairing.secret}")
            if (connection.responseCode != 200) return@runCatching null
            val json = JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
            Diagnostics(json.optString("version"), json.optString("os"), json.optString("log"))
        } finally {
            connection.disconnect()
        }
    }.getOrNull()
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
    var title by rememberSaveable { mutableStateOf("") }
    var description by rememberSaveable { mutableStateOf("") }
    var includeLog by rememberSaveable { mutableStateOf(true) }
    var loading by remember { mutableStateOf(paired) }
    var diagnostics by remember { mutableStateOf<Diagnostics?>(null) }
    LaunchedEffect(paired) {
        if (paired) diagnostics = fetchDiagnostics(context)
        loading = false
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
                    "The computer's recent log goes along, with names and ids removed.",
                Modifier.padding(16.dp),
            )
            OutlinedTextField(
                value = title,
                onValueChange = { title = it },
                label = { Text("Title") },
                placeholder = { Text("Short summary") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
            )
            OutlinedTextField(
                value = description,
                onValueChange = { description = it },
                label = { Text("What happened") },
                placeholder = { Text("What did you do, what did you expect, what happened instead?") },
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                modifier = Modifier.fillMaxWidth().heightIn(min = 160.dp).padding(16.dp),
            )
            ListItem(
                headlineContent = { Text("Include the computer's log") },
                supportingContent = when {
                    !paired -> { { Text("Pair a computer to include its log.") } }
                    loading -> { { Text("Getting the log…") } }
                    log == null -> { { Text("The computer didn't send a log. Update Better Kumo there.") } }
                    else -> null
                },
                trailingContent = {
                    Switch(checked = paired && includeLog, enabled = paired, onCheckedChange = { includeLog = it })
                },
            )
            Button(
                enabled = !loading && (title.isNotBlank() || description.isNotBlank()),
                onClick = {
                    val current = diagnostics
                    val computer = when {
                        !paired -> "not paired"
                        current == null -> "log not available"
                        else -> "Better Kumo ${current.version} on ${current.os}"
                    }
                    val url = IssueReport.url(title, description, phoneDescription(context), computer, log?.takeIf { includeLog })
                    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) }
                        .onSuccess { onDone() }
                },
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
            ) { Text("Open GitHub") }
            if (log != null) {
                TextButton(
                    onClick = {
                        val file = java.io.File(context.cacheDir, "logs/better-kumo-log.txt")
                        file.parentFile?.mkdirs()
                        file.writeText(log + "\n")
                        val uri = androidx.core.content.FileProvider.getUriForFile(
                            context, "${context.packageName}.logs", file,
                        )
                        val send = Intent(Intent.ACTION_SEND)
                            .setType("text/plain")
                            .putExtra(Intent.EXTRA_STREAM, uri)
                            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        context.startActivity(Intent.createChooser(send, "Share the log"))
                    },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                ) { Text("Share the log file…") }
            }
        }
    }
}
