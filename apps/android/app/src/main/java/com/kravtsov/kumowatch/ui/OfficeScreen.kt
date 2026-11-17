package com.kravtsov.kumowatch.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.kravtsov.kumowatch.AppState
import uniffi.kumo_ffi.CallPhase
import uniffi.kumo_ffi.PersonItem
import uniffi.kumo_ffi.RoomItem

@Composable
fun OfficeScreen(state: AppState, onOpenCall: () -> Unit, onOpenDesktop: () -> Unit = {}) {
    val view by state.spaceView.collectAsState()
    val callState by state.call.collectAsState()
    var query by remember { mutableStateOf("") }
    val expanded = remember { mutableStateMapOf<String, Boolean>() }

    DisposableEffect(Unit) {
        state.openSpace()
        onDispose { state.closeSpace() }
    }

    val current = view
    if (current == null || (!current.connected && current.floors.isEmpty())) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        return
    }
    val selfZone = current.selfZoneId
    LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
        item {
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                placeholder = { Text("Find a person") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().padding(12.dp),
            )
        }
        if (state.relay.isPaired()) {
            item { DesktopCard(state, onOpen = onOpenDesktop) }
        }
        if (callState.phase != CallPhase.Idle) {
            item {
                Card(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp).clickable(onClick = onOpenCall)) {
                    Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("In call", style = MaterialTheme.typography.labelMedium)
                            Text(callState.roomName.ifBlank { "Connecting…" }, fontWeight = FontWeight.SemiBold)
                        }
                        TextButton(onClick = onOpenCall) { Text("Open") }
                        TextButton(onClick = { state.leaveCall() }) { Text("Leave") }
                    }
                }
            }
        }
        if (query.isNotBlank()) {
            val all = current.floors.flatMap { f -> f.rooms.flatMap { r -> r.people.map { it to (f.name + " · " + r.name) } } + f.commonArea.map { it to f.name } } +
                current.away.map { it to "Not in the office" }
            val matches = all.filter { it.first.name.contains(query, ignoreCase = true) }
            items(matches, key = { "q" + it.first.key }) { (p, where) ->
                Row(
                    Modifier.fillMaxWidth().clickable(enabled = !p.isSelf) {
                        state.joinPerson(p.uid)
                        onOpenCall()
                    }.padding(horizontal = 16.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    StatusAvatar(p.name, p.avatarUrl, size = 40.dp, presence = p.presence)
                    Spacer(Modifier.width(12.dp))
                    Column {
                        Text(p.name + if (p.isSelf) " (you)" else "")
                        Text(where, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.outline)
                    }
                }
            }
            return@LazyColumn
        }
        current.floors.forEach { floor ->
            val occupied = floor.rooms.filter { it.people.isNotEmpty() }
            val empty = floor.rooms.filter { it.people.isEmpty() && it.open }
            item(key = "floor" + floor.id) {
                Text(
                    floor.name,
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 4.dp),
                )
            }
            items(occupied, key = { it.zoneId }) { room ->
                RoomCard(room, isMine = room.zoneId == selfZone) {
                    state.joinRoom(room.floorId, room.zoneId)
                    onOpenCall()
                }
            }
            if (floor.commonArea.isNotEmpty()) {
                item(key = "common" + floor.id) {
                    Column(Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
                        Text("Common area", style = MaterialTheme.typography.labelLarge)
                        PeopleRow(floor.commonArea)
                    }
                }
            }
            if (empty.isNotEmpty()) {
                val open = expanded[floor.id] == true
                item(key = "empty" + floor.id) {
                    TextButton(onClick = { expanded[floor.id] = !open }, modifier = Modifier.padding(start = 4.dp)) {
                        Text(if (open) "Hide empty rooms" else "${empty.size} empty rooms")
                    }
                }
                if (open) {
                    items(empty, key = { it.zoneId }) { room ->
                        Row(
                            Modifier.fillMaxWidth().clickable {
                                state.joinRoom(room.floorId, room.zoneId)
                                onOpenCall()
                            }.padding(horizontal = 24.dp, vertical = 10.dp),
                        ) { Text(room.name) }
                    }
                }
            }
        }
    }
}

@Composable
private fun RoomCard(room: RoomItem, isMine: Boolean, onJoin: () -> Unit) {
    Card(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)) {
        Column(Modifier.padding(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(room.name, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                if (isMine) {
                    Text("You are here", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                } else {
                    FilledTonalButton(onClick = onJoin, enabled = room.open) { Text(if (room.open) "Join" else "Closed") }
                }
            }
            PeopleRow(room.people)
        }
    }
}

@Composable
private fun PeopleRow(people: List<PersonItem>) {
    LazyRow(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(top = 8.dp)) {
        items(people, key = { it.key }) { p ->
            Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.width(64.dp)) {
                StatusAvatar(p.name, p.avatarUrl, size = 40.dp, presence = p.presence)
                Text(
                    p.name.substringBefore(' ') + if (p.isSelf) " (you)" else "",
                    style = MaterialTheme.typography.labelSmall,
                    maxLines = 1,
                )
            }
        }
    }
}
