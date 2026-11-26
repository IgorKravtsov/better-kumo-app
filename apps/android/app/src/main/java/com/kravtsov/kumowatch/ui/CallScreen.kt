package com.kravtsov.kumowatch.ui

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.BluetoothAudio
import androidx.compose.material.icons.filled.CallEnd
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Headphones
import androidx.compose.material.icons.filled.PhoneAndroid
import androidx.compose.material.icons.filled.VolumeUp
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.material.icons.filled.VideocamOff
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import co.daily.model.AudioDeviceType
import co.daily.model.MediaStreamTrack
import co.daily.view.VideoView
import com.kravtsov.kumowatch.AppState
import com.kravtsov.kumowatch.AudioRoute
import com.kravtsov.kumowatch.DailyMedia
import com.kravtsov.kumowatch.CallTracks
import uniffi.kumo_ffi.CallPhase
import uniffi.kumo_ffi.CallTile
import uniffi.kumo_ffi.Presence
import uniffi.kumo_ffi.callFailureLabel
import uniffi.kumo_ffi.presenceLabel

@Composable
private fun AudioOutputButton(media: DailyMedia) {
    val routes by media.audioRoutes.collectAsState()
    val current by media.audioRoute.collectAsState()
    var open by remember { mutableStateOf(false) }
    LaunchedEffect(routes.size > 1) { if (routes.size < 2) open = false }
    Box {
        FilledIconButton(
            onClick = { open = true },
            enabled = routes.size > 1,
            modifier = Modifier.size(56.dp).semantics {
                routes.firstOrNull { it.type == current }?.let { stateDescription = routeName(it) }
            },
            colors = IconButtonDefaults.filledIconButtonColors(containerColor = Color(0xFF3A3D44), contentColor = Color.White),
        ) { Icon(routeIcon(current ?: AudioDeviceType.SPEAKERPHONE), "Audio output") }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            routes.forEach { route ->
                DropdownMenuItem(
                    text = { Text(routeName(route)) },
                    leadingIcon = { Icon(if (route.type == current) Icons.Filled.Check else routeIcon(route.type), null) },
                    onClick = {
                        open = false
                        media.setAudioRoute(route.type)
                    },
                    modifier = Modifier.semantics { selected = route.type == current },
                )
            }
        }
    }
}

private fun routeName(route: AudioRoute) = when (route.type) {
    AudioDeviceType.SPEAKERPHONE -> "Speaker"
    AudioDeviceType.EARPIECE -> "Phone"
    AudioDeviceType.WIRED -> "Headphones"
    AudioDeviceType.BLUETOOTH -> route.label.ifBlank { "Bluetooth" }
}

private fun routeIcon(type: AudioDeviceType) = when (type) {
    AudioDeviceType.SPEAKERPHONE -> Icons.Filled.VolumeUp
    AudioDeviceType.EARPIECE -> Icons.Filled.PhoneAndroid
    AudioDeviceType.WIRED -> Icons.Filled.Headphones
    AudioDeviceType.BLUETOOTH -> Icons.Filled.BluetoothAudio
}

enum class CallLayout(val title: String) { Speaker("Speaker"), Screen("Screen"), Grid("Grid") }

@Composable
fun CallScreen(state: AppState, onBack: () -> Unit) {
    val call by state.call.collectAsState()
    val tracks by state.media.tracks.collectAsState()
    val context = LocalContext.current
    var layout by rememberSaveable { mutableStateOf(CallLayout.Speaker) }
    val permissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { granted ->
        if (granted[Manifest.permission.BLUETOOTH_CONNECT] == true) state.media.refreshAudioRoutes()
    }
    LaunchedEffect(Unit) {
        val bluetooth = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) listOf(Manifest.permission.BLUETOOTH_CONNECT) else emptyList()
        val needed = (listOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.CAMERA) + bluetooth)
            .filter { context.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (needed.isNotEmpty()) permissions.launch(needed.toTypedArray())
    }
    val sharing = call.tiles.firstOrNull { it.hasScreen }
    val sharingScreen = sharing?.participantId?.let { tracks[it]?.screen }
    val joinFailed by state.joinFailed.collectAsState()
    LaunchedEffect(joinFailed) {
        if (joinFailed) onBack()
    }
    LaunchedEffect(sharing != null) {
        if (sharing != null) layout = CallLayout.Screen else if (layout == CallLayout.Screen) layout = CallLayout.Speaker
    }

    Column(Modifier.fillMaxSize().background(Color(0xFF111318)).statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White) }
            Column(Modifier.weight(1f)) {
                Text(call.roomName.ifBlank { "Room" }, color = Color.White, style = MaterialTheme.typography.titleMedium)
                Text(
                    when (call.phase) {
                        CallPhase.Joined -> {
                            val inCall = call.tiles.count { it.inCall }
                            val away = call.tiles.size - inCall
                            "$inCall in call" + if (away > 0) " · $away in the room" else ""
                        }
                        is CallPhase.Failed -> callFailureLabel((call.phase as CallPhase.Failed).reason)
                        CallPhase.Idle -> "Not in a call"
                        else -> "Connecting…"
                    },
                    color = Color(0xFFB0B3BA),
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }
        Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CallLayout.entries.forEach { l ->
                FilterChip(
                    selected = layout == l,
                    onClick = { layout = l },
                    enabled = l != CallLayout.Screen || sharing != null,
                    label = { Text(l.title) },
                )
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth().padding(8.dp)) {
            when {
                call.phase is CallPhase.Failed -> Icon(Icons.Filled.CallEnd, null, tint = Color(0xFFB0B3BA), modifier = Modifier.align(Alignment.Center).size(48.dp))
                call.phase != CallPhase.Joined -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                layout == CallLayout.Grid -> LazyVerticalGrid(GridCells.Fixed(2), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    items(call.tiles, key = { it.key }) { tile ->
                        Tile(tile, tracks, modifier = Modifier.aspectRatio(0.8f))
                    }
                }
                else -> {
                    val main = if (layout == CallLayout.Screen && sharing != null) null else
                        call.tiles.firstOrNull { it.key == call.activeSpeaker && !it.isLocal }
                            ?: call.tiles.firstOrNull { !it.isLocal } ?: call.tiles.firstOrNull()
                    Column {
                        Box(Modifier.weight(1f).fillMaxWidth()) {
                            if (main == null && sharing != null && sharingScreen != null) {
                                Video(sharingScreen, Modifier.fillMaxSize().clip(RoundedCornerShape(12.dp)))
                                Text(
                                    "${sharing.name} is sharing",
                                    color = Color.White,
                                    modifier = Modifier.align(Alignment.BottomStart).padding(8.dp)
                                        .background(Color(0x99000000), RoundedCornerShape(6.dp)).padding(horizontal = 6.dp),
                                )
                            } else if (main != null) {
                                Tile(main, tracks, modifier = Modifier.fillMaxSize())
                            }
                        }
                        val others = call.tiles.filter { it.key != main?.key }
                        if (others.isNotEmpty()) {
                            LazyRow(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                items(others, key = { it.key }) { tile ->
                                    Tile(tile, tracks, modifier = Modifier.size(width = 96.dp, height = 120.dp))
                                }
                            }
                        }
                    }
                }
            }
        }
        Row(
            Modifier.fillMaxWidth().padding(16.dp),
            horizontalArrangement = Arrangement.SpaceEvenly,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            FilledIconButton(onClick = { state.toggleMic() }, modifier = Modifier.size(56.dp)) {
                Icon(if (call.micOn) Icons.Filled.Mic else Icons.Filled.MicOff, "Microphone")
            }
            FilledIconButton(onClick = { state.toggleCamera() }, modifier = Modifier.size(56.dp)) {
                Icon(if (call.cameraOn) Icons.Filled.Videocam else Icons.Filled.VideocamOff, "Camera")
            }
            AudioOutputButton(state.media)
            FilledIconButton(
                onClick = {
                    state.leaveCall()
                    onBack()
                },
                modifier = Modifier.size(56.dp),
                colors = IconButtonDefaults.filledIconButtonColors(containerColor = Color(0xFFD93025)),
            ) { Icon(Icons.Filled.CallEnd, "Leave", tint = Color.White) }
        }
    }
}

@Composable
private fun Tile(tile: CallTile, tracks: Map<String, CallTracks>, modifier: Modifier) {
    val speaking = tile.speaking
    val avatarUrl = tile.avatar
    val camera = if (tile.hasCamera) tile.participantId?.let { tracks[it]?.camera } else null
    Box(
        modifier.clip(RoundedCornerShape(12.dp)).background(Color(0xFF1F2229))
            .border(if (speaking) 3.dp else 0.dp, if (speaking) Color(0xFF2EB67D) else Color.Transparent, RoundedCornerShape(12.dp)),
        contentAlignment = Alignment.Center,
    ) {
        if (camera != null) {
            Video(camera, Modifier.fillMaxSize(), mirror = tile.isLocal)
        } else if (tile.presence != Presence.OFFLINE || !tile.inCall) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                StatusAvatar(tile.name, avatarUrl, size = 64.dp, presence = tile.presence)
                if (!tile.inCall) {
                    Text(
                        presenceLabel(tile.presence) + " · not in call",
                        color = statusColor(tile.presence),
                        style = MaterialTheme.typography.labelSmall,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
            }
        } else {
            Avatar(tile.name, avatarUrl, size = 72.dp)
        }
        Row(
            Modifier.align(Alignment.BottomStart).padding(6.dp)
                .background(Color(0x99000000), RoundedCornerShape(6.dp)).padding(horizontal = 6.dp, vertical = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (!tile.micOn && tile.inCall) {
                Icon(Icons.Filled.MicOff, null, tint = Color(0xFFFF6B6B), modifier = Modifier.size(14.dp))
            }
            Text(tile.name + if (tile.isLocal) " (you)" else "", color = Color.White, style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun Video(track: MediaStreamTrack, modifier: Modifier, mirror: Boolean = false) {
    AndroidView(
        factory = { VideoView(it) },
        update = {
            it.track = track
            it.mirrorHorizontally = mirror
        },
        modifier = modifier,
    )
}
