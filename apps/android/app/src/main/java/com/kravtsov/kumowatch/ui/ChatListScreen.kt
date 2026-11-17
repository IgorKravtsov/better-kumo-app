package com.kravtsov.kumowatch.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AccountCircle
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material3.FloatingActionButton
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.launch
import androidx.compose.material.icons.filled.Groups
import androidx.compose.material.icons.filled.Tag
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.kravtsov.kumowatch.AppState
import uniffi.kumo_ffi.ChannelItem
import uniffi.kumo_ffi.ChannelType
import uniffi.kumo_ffi.ConnectionStatus

enum class ChatFilter(val title: String) { All("All"), Unread("Unread"), Channels("Channels"), Direct("Direct") }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatListScreen(state: AppState, onOpen: (String) -> Unit, onProfile: () -> Unit) {
    val channels by state.channels.collectAsState()
    val connection by state.connection.collectAsState()
    val space by state.space.collectAsState()
    var filter by rememberSaveable { mutableStateOf(ChatFilter.All) }
    var picking by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val visible = channels.filter {
        when (filter) {
            ChatFilter.All -> true
            ChatFilter.Unread -> it.unread > 0u
            ChatFilter.Channels -> it.channelType == ChannelType.CHANNEL
            ChatFilter.Direct -> it.channelType == ChannelType.DIRECT || it.channelType == ChannelType.GROUP
        }
    }

    if (picking) {
        PeopleSheet(
            title = "New message",
            people = remember { state.core.spacePeople() },
            onDismiss = { picking = false },
            onPick = { p ->
                picking = false
                scope.launch { runCatching { state.core.openDirect(p.uid) }.onSuccess(onOpen) }
            },
        )
    }
    Scaffold(
        floatingActionButton = {
            FloatingActionButton(onClick = { picking = true }) { Icon(Icons.Filled.Edit, "New message") }
        },
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(space?.displayName ?: "Chat")
                        connectionLabel(connection)?.let {
                            Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline)
                        }
                    }
                },
                actions = { IconButton(onClick = onProfile) { Icon(Icons.Filled.AccountCircle, "Profile") } },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            LazyRow(
                contentPadding = PaddingValues(horizontal = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(ChatFilter.entries) { f ->
                    FilterChip(selected = f == filter, onClick = { filter = f }, label = { Text(f.title) })
                }
            }
            LazyColumn(Modifier.fillMaxSize()) {
                items(visible, key = { it.url }) { channel -> ChannelRow(channel) { onOpen(channel.url) } }
            }
        }
    }
}

private fun connectionLabel(status: ConnectionStatus): String? = when (status) {
    ConnectionStatus.ONLINE -> null
    ConnectionStatus.SYNCING -> "Updating…"
    ConnectionStatus.CONNECTING -> "Connecting…"
    ConnectionStatus.OFFLINE -> "Offline"
}

@Composable
private fun ChannelRow(channel: ChannelItem, onClick: () -> Unit) {
    val unread = channel.unread > 0u
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        when (channel.channelType) {
            ChannelType.CHANNEL -> IconAvatar { Icon(Icons.Filled.Tag, null) }
            ChannelType.GROUP -> IconAvatar { Icon(Icons.Filled.Groups, null) }
            else -> Avatar(channel.title, channel.avatarUrl, online = channel.isOnline)
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    channel.title,
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = if (unread) FontWeight.Bold else FontWeight.Normal,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text(shortTime(channel.lastMessageAt), style = MaterialTheme.typography.labelSmall)
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                val prefix = if (channel.channelType != ChannelType.DIRECT && channel.lastSenderName.isNotBlank()) {
                    "${channel.lastSenderName.substringBefore(' ')}: "
                } else ""
                Text(
                    prefix + channel.lastMessageText,
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (unread) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.outline,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (unread) {
                    Spacer(Modifier.width(8.dp))
                    Badge(if (channel.mentions > 0u) "@" else channel.unread.toString())
                }
            }
        }
    }
}

@Composable
private fun IconAvatar(content: @Composable () -> Unit) {
    Box(
        Modifier.padding(0.dp).clip(CircleShape).background(MaterialTheme.colorScheme.secondaryContainer)
            .padding(8.dp),
        contentAlignment = Alignment.Center,
    ) { content() }
}

@Composable
private fun Badge(text: String) {
    Box(
        Modifier.clip(CircleShape).background(MaterialTheme.colorScheme.primary).padding(horizontal = 7.dp, vertical = 2.dp),
    ) {
        Text(text, color = MaterialTheme.colorScheme.onPrimary, style = MaterialTheme.typography.labelSmall)
    }
}
