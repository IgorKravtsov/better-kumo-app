package com.kravtsov.kumowatch.ui

import android.content.Intent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import com.kravtsov.kumowatch.AppState
import com.kravtsov.kumowatch.Relay
import com.kravtsov.kumowatch.RelayWidget
import android.appwidget.AppWidgetManager
import android.content.ComponentName
import androidx.compose.material3.Switch
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProfileScreen(state: AppState, onBack: () -> Unit) {
    val user by state.user.collectAsState()
    val space by state.space.collectAsState()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var pickSpace by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }

    if (pickSpace) {
        SpacesScreen(state) { pickSpace = false; onBack() }
        return
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("You") },
                navigationIcon = { IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            ListItem(
                leadingContent = { Avatar(user?.displayName ?: "?", "") },
                headlineContent = { Text(user?.displayName ?: "") },
                supportingContent = { Text(user?.email ?: "") },
            )
            HorizontalDivider()
            ListItem(
                headlineContent = { Text("Space") },
                supportingContent = { Text(space?.displayName ?: "-") },
                modifier = Modifier.clickable { pickSpace = true },
            )
            val relay by state.relayEnabled.collectAsState()
            ListItem(
                headlineContent = { Text("Mirror notifications") },
                supportingContent = { Text("DMs, group messages, mentions and nudges as phone notifications. Keeps a connection open.") },
                trailingContent = { Switch(checked = relay, onCheckedChange = { Relay.setEnabled(context, it) }) },
            )
            ListItem(
                headlineContent = { Text("Add toggle widget to home screen") },
                supportingContent = { Text("One tap turns mirroring on or off") },
                modifier = Modifier.clickable {
                    context.getSystemService(AppWidgetManager::class.java).requestPinAppWidget(
                        ComponentName(context, RelayWidget::class.java), null, null,
                    )
                },
            )
            ListItem(
                headlineContent = { Text("Sign in on another device") },
                supportingContent = { Text("Shares a one-time sign-in link, valid for an hour") },
                modifier = Modifier.clickable {
                    scope.launch {
                        state.transferLink()
                            .onSuccess { link ->
                                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, link)
                                context.startActivity(Intent.createChooser(send, "Sign-in link"))
                            }
                            .onFailure { message = it.message }
                    }
                },
            )
            ListItem(
                headlineContent = { Text("Notifications relay") },
                supportingContent = {
                    Text(state.relay.url ?: "Run `kumo-relay pair --url <address>` on your Mac and open the link here")
                },
                trailingContent = {
                    if (state.relay.configured) {
                        androidx.compose.material3.TextButton(onClick = { state.relay.forget(); message = "Relay forgotten" }) { Text("Forget") }
                    }
                },
            )
            ListItem(
                headlineContent = { Text("Sign out", color = MaterialTheme.colorScheme.error) },
                modifier = Modifier.clickable { state.signOut() },
            )
            message?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            ListItem(headlineContent = { Text("Core ${state.core.coreVersion()}", style = MaterialTheme.typography.labelSmall) })
        }
    }
}
