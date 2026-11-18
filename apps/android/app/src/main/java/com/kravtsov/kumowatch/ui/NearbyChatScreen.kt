package com.kravtsov.kumowatch.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.dp
import com.kravtsov.kumowatch.AppState
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.kumo_ffi.NearbyChat
import uniffi.kumo_ffi.NearbyMessage

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NearbyChatScreen(state: AppState, onBack: () -> Unit) {
    val relay = state.relay
    val scope = rememberCoroutineScope()
    var chat by remember { mutableStateOf<NearbyChat?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var draft by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    val list = rememberLazyListState()

    LaunchedEffect(relay) {
        while (true) {
            runCatching { relay.nearbyChat() }
                .onSuccess { chat = it; error = null }
                .onFailure { error = it.message ?: "Cannot read Nearby Chat" }
            delay(2_000)
        }
    }
    val messages = chat?.messages.orEmpty()
    LaunchedEffect(messages.lastOrNull()?.id) {
        if (messages.isNotEmpty()) list.animateScrollToItem(messages.size - 1)
    }

    fun send() {
        val text = draft
        sending = true
        scope.launch {
            runCatching { relay.sendNearby(text) }
                .onSuccess {
                    if (draft == text) draft = ""
                    error = null
                    runCatching { relay.nearbyChat() }.onSuccess { chat = it }
                }
                .onFailure { error = it.message ?: "Cannot send" }
            sending = false
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(chat?.zoneName ?: "Nearby Chat") },
                navigationIcon = { IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).imePadding()) {
            LazyColumn(
                state = list,
                modifier = Modifier.weight(1f).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(8.dp),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp),
            ) {
                val current = chat
                if (current != null && current.messages.isEmpty()) {
                    item {
                        Text(
                            "No messages in ${current.zoneName} yet. Nearby Chat is not saved.",
                            color = MaterialTheme.colorScheme.outline,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                }
                items(messages, key = { it.id }) { NearbyBubble(it) }
            }
            error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 16.dp))
            }
            Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(
                    value = draft,
                    onValueChange = { draft = it },
                    placeholder = { Text("Message ${chat?.zoneName ?: "Nearby"}") },
                    maxLines = 5,
                    modifier = Modifier.weight(1f),
                )
                IconButton(onClick = ::send, enabled = chat != null && !sending && draft.isNotBlank()) {
                    Icon(Icons.AutoMirrored.Filled.Send, "Send")
                }
            }
        }
    }
}

@Composable
private fun NearbyBubble(message: NearbyMessage) {
    Column(
        Modifier.fillMaxWidth(),
        horizontalAlignment = if (message.mine) Alignment.End else Alignment.Start,
    ) {
        if (!message.mine) {
            Text(message.senderName, color = colorFor(message.senderName), style = MaterialTheme.typography.labelMedium)
        }
        Text(
            message.text,
            color = if (message.mine) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier
                .widthIn(max = 300.dp)
                .clip(RoundedCornerShape(16.dp))
                .background(if (message.mine) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant)
                .padding(horizontal = 12.dp, vertical = 8.dp),
        )
        Text(shortTime(message.sentAtMs), color = MaterialTheme.colorScheme.outline, style = MaterialTheme.typography.labelSmall)
    }
}
