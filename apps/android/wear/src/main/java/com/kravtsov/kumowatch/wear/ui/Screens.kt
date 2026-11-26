package com.kravtsov.kumowatch.wear.ui

import android.app.Activity
import android.app.RemoteInput
import android.content.Context
import android.media.AudioManager
import android.view.inputmethod.EditorInfo
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CallEnd
import androidx.compose.material.icons.filled.Chat
import androidx.compose.material.icons.filled.Groups
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.PhoneAndroid
import androidx.compose.material.icons.filled.Watch
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material.icons.filled.VolumeUp
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.wear.compose.foundation.lazy.TransformingLazyColumn
import androidx.wear.compose.foundation.lazy.rememberTransformingLazyColumnState
import androidx.wear.compose.material3.Button
import androidx.wear.compose.material3.ButtonDefaults
import androidx.wear.compose.material3.CircularProgressIndicator
import androidx.wear.compose.material3.EdgeButton
import androidx.wear.compose.material3.FilledIconButton
import androidx.wear.compose.material3.FilledTonalButton
import androidx.wear.compose.material3.Icon
import androidx.wear.compose.material3.IconButtonDefaults
import androidx.wear.compose.material3.ListHeader
import androidx.wear.compose.material3.MaterialTheme
import androidx.wear.compose.material3.ScreenScaffold
import androidx.wear.compose.material3.Text
import androidx.wear.input.RemoteInputIntentHelper
import androidx.wear.input.wearableExtender
import com.kravtsov.kumowatch.WatchLink
import com.kravtsov.kumowatch.wear.QUICK_REPLIES
import com.kravtsov.kumowatch.wear.WearState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.kumo_ffi.ChannelType
import uniffi.kumo_ffi.ConnectionStatus
import uniffi.kumo_ffi.MessageItem
import uniffi.kumo_ffi.SendState

@Composable
fun SignInScreen(state: WearState) {
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val list = rememberTransformingLazyColumnState()
    ScreenScaffold(scrollState = list) { padding ->
        TransformingLazyColumn(state = list, contentPadding = padding) {
            item { ListHeader { Text("Better Kumo") } }
            item {
                Text(
                    error ?: "Sign in on your phone's Better Kumo app first, keep the phone nearby, then tap below.",
                    textAlign = TextAlign.Center,
                    style = MaterialTheme.typography.bodySmall,
                    color = if (error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            item {
                Button(
                    onClick = {
                        busy = true
                        error = null
                        scope.launch {
                            state.signInFromPhone().onFailure { error = it.message ?: "Sign-in failed" }
                            busy = false
                        }
                    },
                    enabled = !busy,
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text(if (busy) "Signing in…" else "Sign in from phone") },
                )
            }
        }
    }
}

@Composable
fun HomeScreen(state: WearState, onCall: () -> Unit, onOffice: () -> Unit, onChats: () -> Unit) {
    val call by state.shownCall.collectAsState()
    val callsOnPhone by state.callsOnPhone.collectAsState()
    val channels by state.channels.collectAsState()
    val connection by state.connection.collectAsState()
    val unread = channels.count { it.unread > 0u }
    val list = rememberTransformingLazyColumnState()
    ScreenScaffold(scrollState = list) { padding ->
        TransformingLazyColumn(state = list, contentPadding = padding) {
            item { ListHeader { Text("Better Kumo") } }
            if (call.state.active) {
                item {
                    Button(
                        onClick = onCall,
                        modifier = Modifier.fillMaxWidth(),
                        colors = ButtonDefaults.buttonColors(),
                        icon = { Icon(Icons.Filled.CallEnd, contentDescription = null) },
                        label = { Text(if (call.onPhone) "In call on phone" else "In call", maxLines = 1) },
                        secondaryLabel = {
                            Text(call.state.roomName.ifBlank { "Joining…" }, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        },
                    )
                }
            }
            item {
                FilledTonalButton(
                    onClick = onOffice,
                    modifier = Modifier.fillMaxWidth(),
                    icon = { Icon(Icons.Filled.Groups, contentDescription = null) },
                    label = { Text("Office") },
                    secondaryLabel = { Text("Join a room or a person") },
                )
            }
            item {
                FilledTonalButton(
                    onClick = onChats,
                    modifier = Modifier.fillMaxWidth(),
                    icon = { Icon(Icons.Filled.Chat, contentDescription = null) },
                    label = { Text("Chats") },
                    secondaryLabel = if (unread > 0) ({ Text("$unread unread") }) else null,
                )
            }
            item {
                FilledTonalButton(
                    onClick = { state.setCallsOnPhone(!callsOnPhone) },
                    enabled = !call.state.active,
                    modifier = Modifier.fillMaxWidth(),
                    icon = {
                        Icon(if (callsOnPhone) Icons.Filled.PhoneAndroid else Icons.Filled.Watch, contentDescription = null)
                    },
                    label = { Text(if (callsOnPhone) "Calls on phone" else "Calls on watch") },
                    secondaryLabel = {
                        Text(if (callsOnPhone) "Phone mic and speaker" else "Watch mic and speaker", maxLines = 1)
                    },
                )
            }
            if (connection != ConnectionStatus.ONLINE) {
                item {
                    Text(
                        when (connection) {
                            ConnectionStatus.OFFLINE -> "Offline"
                            else -> "Connecting…"
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            item {
                Button(
                    onClick = { state.signOut() },
                    modifier = Modifier.fillMaxWidth(),
                    colors = ButtonDefaults.outlinedButtonColors(),
                    border = ButtonDefaults.outlinedButtonBorder(enabled = true),
                    label = { Text("Sign out") },
                )
            }
        }
    }
}

@Composable
fun OfficeScreen(state: WearState, onJoined: () -> Unit) {
    DisposableEffect(Unit) {
        state.openOffice()
        onDispose { state.closeOffice() }
    }
    val space by state.space.collectAsState()
    val callsOnPhone by state.callsOnPhone.collectAsState()
    val busy by state.busy.collectAsState()
    val scope = rememberCoroutineScope()
    val list = rememberTransformingLazyColumnState()
    ScreenScaffold(scrollState = list) { padding ->
        TransformingLazyColumn(state = list, contentPadding = padding) {
            item { ListHeader { Text(if (busy != null) "Joining…" else "Office") } }
            val view = space
            if (view == null || !view.connected) {
                item { CircularProgressIndicator() }
                return@TransformingLazyColumn
            }
            val elsewhere = if (callsOnPhone) null else state.otherDeviceRoom(view)
            if (elsewhere != null) {
                item {
                    Text(
                        "You're in $elsewhere on another device. Joining here too would echo.",
                        textAlign = TextAlign.Center,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
            for (floor in view.floors) {
                val rooms = floor.rooms.sortedByDescending { it.people.size }
                if (rooms.isEmpty() && floor.commonArea.isEmpty()) continue
                item(key = "floor-${floor.id}") { ListHeader { Text(floor.name) } }
                items(rooms.size, key = { "room-${rooms[it].zoneId}" }) { i ->
                    val room = rooms[i]
                    val here = room.zoneId == view.selfZoneId
                    FilledTonalButton(
                        onClick = {
                            scope.launch { if (state.joinRoom(room.floorId, room.zoneId)) onJoined() }
                        },
                        enabled = room.open && busy == null,
                        modifier = Modifier.fillMaxWidth(),
                        label = { Text(room.name + if (here) " · you" else "", maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        secondaryLabel = {
                            Text(
                                when {
                                    !room.open -> "Closed"
                                    room.people.isEmpty() -> "Empty"
                                    else -> room.people.joinToString { it.name }
                                },
                                maxLines = 2,
                                overflow = TextOverflow.Ellipsis,
                            )
                        },
                    )
                }
                // Not joinable: the core joins people only in a room.
                val common = floor.commonArea.filter { !it.isSelf }
                items(common.size, key = { "common-${floor.id}-${common[it].key}" }) { i ->
                    Text(
                        "${common[i].name} · common area",
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 2.dp),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

@Composable
fun CallScreen(state: WearState, onEnded: () -> Unit) {
    val shown by state.shownCall.collectAsState()
    val call = shown.state
    val context = LocalContext.current
    val busy by state.busy.collectAsState()
    val moving = busy == WearState.Busy.MOVING
    var seenActive by remember { mutableStateOf(false) }
    var seenBusy by remember { mutableStateOf(false) }
    LaunchedEffect(call.phase, busy) {
        if (call.active) seenActive = true
        if (busy != null) seenBusy = true
        if ((seenActive || seenBusy) && busy == null && !call.active) onEnded()
    }
    val list = rememberTransformingLazyColumnState()
    ScreenScaffold(
        scrollState = list,
    ) { padding ->
        TransformingLazyColumn(state = list, contentPadding = padding) {
            item {
                ListHeader {
                    Text(call.roomName.ifBlank { "Call" }, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            item {
                Text(
                    when {
                        moving -> "Moving…"
                        busy != null && !call.active -> "Joining…"
                        shown.onPhone -> "${call.status} · on phone"
                        else -> call.status
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            item {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    // Not an EdgeButton: that one hides under the participant list until it is scrolled to the end.
                    FilledIconButton(
                        onClick = {
                            state.leaveCall()
                            onEnded()
                        },
                        colors = IconButtonDefaults.filledIconButtonColors(
                            containerColor = MaterialTheme.colorScheme.error,
                            contentColor = MaterialTheme.colorScheme.onError,
                        ),
                    ) { Icon(Icons.Filled.CallEnd, contentDescription = "Leave") }
                    FilledIconButton(
                        onClick = { state.toggleMic() },
                        colors = if (call.micOn) IconButtonDefaults.filledIconButtonColors()
                        else IconButtonDefaults.filledTonalIconButtonColors(),
                    ) {
                        Icon(
                            if (call.micOn) Icons.Filled.Mic else Icons.Filled.MicOff,
                            contentDescription = if (call.micOn) "Mute" else "Unmute",
                        )
                    }
                    if (!shown.onPhone) {
                        FilledIconButton(
                            onClick = { showVolume(context) },
                            colors = IconButtonDefaults.filledTonalIconButtonColors(),
                        ) { Icon(Icons.Filled.VolumeUp, contentDescription = "Volume") }
                    }
                }
            }
            if (call.movable) {
                item {
                    FilledTonalButton(
                        onClick = { state.moveCall() },
                        enabled = busy == null,
                        modifier = Modifier.fillMaxWidth(),
                        icon = {
                            Icon(
                                if (shown.onPhone) Icons.Filled.Watch else Icons.Filled.PhoneAndroid,
                                contentDescription = null,
                            )
                        },
                        label = { Text(if (shown.onPhone) "Move to watch" else "Move to phone") },
                    )
                }
            }
            val people = call.people
            items(people.size, key = { people[it].key }) { i ->
                val tile = people[i]
                Row(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        tile.name + if (tile.isLocal) " (you)" else "",
                        modifier = Modifier.weight(1f),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        fontWeight = if (tile.speaking) FontWeight.Bold else FontWeight.Normal,
                        color = if (tile.speaking) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
                    )
                    if (!tile.micOn) {
                        Icon(
                            Icons.Filled.MicOff,
                            contentDescription = "Muted",
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

private fun showVolume(context: Context) {
    val audio = context.getSystemService(AudioManager::class.java)
    audio.adjustStreamVolume(AudioManager.STREAM_VOICE_CALL, AudioManager.ADJUST_SAME, AudioManager.FLAG_SHOW_UI)
}

@Composable
fun ChatsScreen(state: WearState, onOpen: (String) -> Unit) {
    val channels by state.channels.collectAsState()
    val shown = remember(channels) {
        channels.sortedWith(compareByDescending<uniffi.kumo_ffi.ChannelItem> { it.unread > 0u }.thenByDescending { it.lastMessageAt })
            .take(40)
    }
    val list = rememberTransformingLazyColumnState()
    ScreenScaffold(scrollState = list) { padding ->
        TransformingLazyColumn(state = list, contentPadding = padding) {
            item { ListHeader { Text("Chats") } }
            items(shown.size, key = { shown[it].url }) { i ->
                val channel = shown[i]
                val preview = buildString {
                    if (channel.channelType != ChannelType.DIRECT && channel.lastSenderName.isNotBlank()) {
                        append(channel.lastSenderName).append(": ")
                    }
                    append(channel.lastMessageText)
                }
                FilledTonalButton(
                    onClick = { onOpen(channel.url) },
                    modifier = Modifier.fillMaxWidth(),
                    label = {
                        Text(
                            (if (channel.unread > 0u) "● " else "") + channel.title,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            fontWeight = if (channel.unread > 0u) FontWeight.Bold else FontWeight.Normal,
                        )
                    },
                    secondaryLabel = { Text(preview, maxLines = 2, overflow = TextOverflow.Ellipsis) },
                )
            }
        }
    }
}

private const val REPLY_KEY = "reply"

private fun MessageItem.plain(): String = when {
    text.isNotBlank() -> text
    images.isNotEmpty() -> "📷 Photo"
    fileName.isNotBlank() -> "📎 $fileName"
    else -> "…"
}

@Composable
fun ChannelScreen(state: WearState, channelUrl: String) {
    val scope = rememberCoroutineScope()
    var messages by remember { mutableStateOf(emptyList<MessageItem>()) }
    val list = rememberTransformingLazyColumnState()
    val context = LocalContext.current
    val channels by state.channels.collectAsState()
    val title = channels.firstOrNull { it.url == channelUrl }?.title ?: "Chat"

    suspend fun reload() {
        messages = withContext(Dispatchers.IO) { state.core.messages(channelUrl, 20u) }
    }

    // Open only while visible: an open chat marks new messages read, and a call keeps the core running.
    LifecycleStartEffect(channelUrl) {
        state.core.openChannel(channelUrl)
        onStopOrDispose { state.core.closeChannel(channelUrl) }
    }
    LaunchedEffect(channelUrl) {
        var scrolled = false
        suspend fun show() {
            reload()
            if (!scrolled && messages.isNotEmpty()) {
                scrolled = true
                list.scrollToItem(messages.size)
            }
        }
        state.messageChanges.onSubscription { show() }.collect { if (it == channelUrl) show() }
    }

    fun send(text: String) {
        val trimmed = text.trim()
        if (trimmed.isEmpty()) return
        scope.launch {
            try {
                state.sendWhenOnline(channelUrl, trimmed)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toast.makeText(context, "Message not sent", Toast.LENGTH_SHORT).show()
            }
            reload()
            list.animateScrollToItem(messages.size)
        }
    }

    val replyLauncher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode != Activity.RESULT_OK) return@rememberLauncherForActivityResult
        val data = result.data ?: return@rememberLauncherForActivityResult
        RemoteInput.getResultsFromIntent(data)?.getCharSequence(REPLY_KEY)?.let { send(it.toString()) }
    }
    fun openReply() {
        val intent = RemoteInputIntentHelper.createActionRemoteInputIntent()
        val input = RemoteInput.Builder(REPLY_KEY)
            .setLabel("Reply")
            .wearableExtender {
                setEmojisAllowed(true)
                setInputActionType(EditorInfo.IME_ACTION_SEND)
            }
            .build()
        RemoteInputIntentHelper.putRemoteInputsExtra(intent, listOf(input))
        replyLauncher.launch(intent)
    }

    ScreenScaffold(
        scrollState = list,
        edgeButton = { EdgeButton(onClick = ::openReply) { Text("Reply") } },
    ) { padding ->
        TransformingLazyColumn(state = list, contentPadding = padding) {
            item { ListHeader { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) } }
            items(messages.size, key = { messages[it].id }) { i ->
                val message = messages[i]
                val showSender = !message.isMine && (i == 0 || messages[i - 1].senderId != message.senderId)
                Column(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 2.dp),
                    horizontalAlignment = if (message.isMine) Alignment.End else Alignment.Start,
                ) {
                    if (showSender) {
                        Text(
                            message.senderName,
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }
                    Text(
                        message.plain(),
                        style = MaterialTheme.typography.bodyMedium,
                        textAlign = if (message.isMine) TextAlign.End else TextAlign.Start,
                        color = when (message.state) {
                            SendState.FAILED -> MaterialTheme.colorScheme.error
                            SendState.PENDING -> MaterialTheme.colorScheme.onSurfaceVariant
                            SendState.SENT -> MaterialTheme.colorScheme.onSurface
                        },
                    )
                }
            }
            items(QUICK_REPLIES.size, key = { "quick-$it" }) { i ->
                Button(
                    onClick = { send(QUICK_REPLIES[i]) },
                    modifier = Modifier.fillMaxWidth(),
                    colors = ButtonDefaults.outlinedButtonColors(),
                    border = ButtonDefaults.outlinedButtonBorder(enabled = true),
                    label = { Text(QUICK_REPLIES[i]) },
                )
            }
        }
    }
}
