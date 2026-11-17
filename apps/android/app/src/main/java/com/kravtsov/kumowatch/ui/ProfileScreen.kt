package com.kravtsov.kumowatch.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.clickable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.kravtsov.kumowatch.AppState
import com.kravtsov.kumowatch.Relay
import com.kravtsov.kumowatch.RelayWidget
import android.appwidget.AppWidgetManager
import android.content.ComponentName
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import kotlinx.coroutines.launch
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.OutlinedTextField
import androidx.compose.runtime.LaunchedEffect
import uniffi.kumo_ffi.MyStatus
import uniffi.kumo_ffi.NotificationDelivery
import uniffi.kumo_ffi.NotificationKinds
import uniffi.kumo_ffi.RelayStatus
import uniffi.kumo_ffi.myStatusChoices
import uniffi.kumo_ffi.myStatusLabel
import uniffi.kumo_ffi.profileKinds

private val notificationKinds: List<Triple<String, (NotificationKinds) -> Boolean, (NotificationKinds, Boolean) -> NotificationKinds>> = listOf(
    Triple("Messages", { it.messages }, { s, v -> s.copy(messages = v) }),
    Triple("@Mentions", { it.mentions }, { s, v -> s.copy(mentions = v) }),
    Triple("Nudges", { it.nudges }, { s, v -> s.copy(nudges = v) }),
    Triple("Knocks", { it.knocks }, { s, v -> s.copy(knocks = v) }),
    Triple("Broadcasts", { it.broadcasts }, { s, v -> s.copy(broadcasts = v) }),
    Triple("Public Chat", { it.publicChat }, { s, v -> s.copy(publicChat = v) }),
    Triple("Nearby Chat", { it.nearbyChat }, { s, v -> s.copy(nearbyChat = v) }),
    Triple("Joined the Floor", { it.floorEntries }, { s, v -> s.copy(floorEntries = v) }),
)

private val deliveryOptions = listOf("This phone" to NotificationDelivery.PHONE, "ntfy" to NotificationDelivery.NTFY)

@Composable
private fun Hint(text: String) {
    ListItem(headlineContent = { Text(text, style = MaterialTheme.typography.bodySmall) })
}

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
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState())) {
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
            val delivery by state.delivery.collectAsState()
            val phoneMirroring by state.relayEnabled.collectAsState()
            val profile by state.notificationProfile.collectAsState()
            val officeStatus by state.myStatus.collectAsState()
            var pickedStatus by remember { mutableStateOf<MyStatus?>(null) }
            val paired = state.relay.isPaired()
            var status by remember { mutableStateOf<RelayStatus?>(null) }
            var statusFailed by remember { mutableStateOf(false) }
            var busy by remember { mutableStateOf(false) }
            var topic by remember { mutableStateOf("") }
            var savedTopic by remember { mutableStateOf<String?>(null) }
            fun showStatus(current: RelayStatus) {
                status = current
                statusFailed = false
                savedTopic = current.ntfyTopic
                topic = current.ntfyTopic ?: ""
            }
            LaunchedEffect(paired) {
                status = null
                statusFailed = false
                if (paired) {
                    state.syncNotificationProfile()
                        .onSuccess { current -> current?.let { showStatus(it) } }
                        .onFailure { statusFailed = true }
                }
            }
            fun relayUpdate(action: suspend () -> RelayStatus?) {
                scope.launch {
                    busy = true
                    runCatching { action() }
                        .onSuccess { current -> current?.let { showStatus(it) } }
                        .onFailure { message = it.message }
                    busy = false
                }
            }
            val phone = delivery == NotificationDelivery.PHONE
            val masterOn = if (phone) phoneMirroring else status?.mirroring == true

            ListItem(
                headlineContent = {
                    Text("Notifications", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
                },
            )
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(horizontal = 16.dp)) {
                deliveryOptions.forEachIndexed { index, (label, option) ->
                    SegmentedButton(
                        selected = delivery == option,
                        onClick = { state.setDelivery(option) },
                        shape = SegmentedButtonDefaults.itemShape(index, deliveryOptions.size),
                    ) { Text(label) }
                }
            }
            ListItem(
                headlineContent = { Text("Notifications on") },
                supportingContent = {
                    Text(
                        if (phone) {
                            "This phone keeps its own connection open and shows notifications itself."
                        } else {
                            "The relay on your Mac publishes alerts to your ntfy topic. Applies to every paired phone."
                        },
                    )
                },
                trailingContent = {
                    Switch(
                        checked = masterOn,
                        enabled = phone || (status != null && !busy),
                        onCheckedChange = { value ->
                            if (phone) {
                                Relay.setEnabled(context, value)
                            } else {
                                relayUpdate { state.relay.setMirroring(value).also { RelayWidget.updateAll(context) } }
                            }
                        },
                    )
                },
            )
            val subscribedTopic = status?.ntfyTopic
            if (phone && subscribedTopic != null) {
                Hint("If the ntfy app on this phone is subscribed to $subscribedTopic, unsubscribe there to avoid duplicates.")
            }
            if (!phone) {
                val needs = buildList {
                    if (!paired) add("Pair a relay below.")
                    if (paired && statusFailed) add("The relay is unreachable right now.")
                    if (paired && status != null && subscribedTopic == null) add("Set an ntfy topic below.")
                    if (!paired || subscribedTopic == null) add("Install the ntfy app and subscribe to the topic.")
                }
                if (needs.isNotEmpty()) Hint(needs.joinToString(" "))
                status?.ntfyUrl?.let { url ->
                    ListItem(
                        headlineContent = { Text("Open in ntfy") },
                        supportingContent = { Text(url) },
                        modifier = Modifier.clickable {
                            runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) }
                                .onFailure { message = "No app can open $url" }
                        },
                    )
                }
            }
            if (masterOn) {
                val nowStatus = if (phone) officeStatus else status?.myStatus ?: MyStatus.UNKNOWN
                val selected = pickedStatus ?: nowStatus
                val kinds = profileKinds(profile, selected)
                val statuses = myStatusChoices()
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(horizontal = 16.dp)) {
                    statuses.forEachIndexed { index, option ->
                        SegmentedButton(
                            selected = selected == option,
                            onClick = { pickedStatus = option },
                            shape = SegmentedButtonDefaults.itemShape(index, statuses.size),
                        ) { Text(myStatusLabel(option), maxLines = 1) }
                    }
                }
                Hint("Now: ${myStatusLabel(nowStatus)}")
                ListItem(
                    headlineContent = { Text("All for ${myStatusLabel(selected)}", fontWeight = FontWeight.SemiBold) },
                    trailingContent = {
                        Switch(
                            checked = notificationKinds.all { (_, isOn, _) -> isOn(kinds) },
                            enabled = !busy,
                            onCheckedChange = { value ->
                                val all = notificationKinds.fold(kinds) { acc, (_, _, withValue) -> withValue(acc, value) }
                                relayUpdate { state.applyNotificationKinds(selected, all).getOrThrow() }
                            },
                        )
                    },
                )
                notificationKinds.forEach { (label, isOn, withValue) ->
                    ListItem(
                        headlineContent = { Text(label) },
                        modifier = Modifier.padding(start = 16.dp),
                        trailingContent = {
                            Switch(
                                checked = isOn(kinds),
                                enabled = !busy,
                                onCheckedChange = { value ->
                                    relayUpdate { state.applyNotificationKinds(selected, withValue(kinds, value)).getOrThrow() }
                                },
                            )
                        },
                    )
                }
                Hint(
                    if (phone) {
                        "Unknown applies when this phone isn't connected to the Office, or you're idle or offline."
                    } else {
                        "Unknown applies when the Desktop app isn't running, or you're idle or offline."
                    },
                )
            }
            Hint(
                "With ntfy, Nudges, Knocks, Broadcasts, Public Chat and Nearby Chat need the Desktop app running on your Mac. " +
                    "This phone gets them from its own connection.",
            )
            ListItem(
                headlineContent = { Text("Add toggle widget to home screen") },
                supportingContent = { Text("One tap turns notifications on or off for the chosen delivery") },
                modifier = Modifier.clickable {
                    context.getSystemService(AppWidgetManager::class.java).requestPinAppWidget(
                        ComponentName(context, RelayWidget::class.java), null, null,
                    )
                },
            )
            HorizontalDivider()
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
                    Text(state.relay.pairedUrl() ?: "Run `kumo-relay pair --url <address>` on your Mac and open the link here")
                },
                trailingContent = {
                    if (paired) {
                        androidx.compose.material3.TextButton(onClick = { state.relay.forget(); message = "Relay forgotten" }) { Text("Forget") }
                    }
                },
            )
            if (paired) {
                ListItem(
                    headlineContent = {
                        OutlinedTextField(
                            value = topic,
                            onValueChange = { topic = it },
                            label = { Text("ntfy topic") },
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                    },
                    supportingContent = { Text("Relay alerts are published to this ntfy topic. Leave it empty to turn ntfy off.") },
                    trailingContent = {
                        if (topic.trim() != (savedTopic ?: "")) {
                            androidx.compose.material3.TextButton(onClick = {
                                scope.launch {
                                    runCatching { state.relay.setNtfyTopic(topic.trim().ifEmpty { null }) }
                                        .onSuccess {
                                            showStatus(it)
                                            message = if (it.ntfyTopic == null) "ntfy turned off" else "ntfy topic saved"
                                        }
                                        .onFailure { message = it.message }
                                }
                            }) { Text("Save") }
                        }
                    },
                )
            }
            ListItem(
                headlineContent = { Text("Sign out", color = MaterialTheme.colorScheme.error) },
                modifier = Modifier.clickable { state.signOut() },
            )
            message?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            ListItem(headlineContent = { Text("Core ${state.core.coreVersion()}", style = MaterialTheme.typography.labelSmall) })
        }
    }
}
