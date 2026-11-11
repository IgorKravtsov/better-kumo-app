package com.kravtsov.kumowatch.ui

import android.Manifest
import android.content.pm.PackageManager
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
import androidx.compose.material.icons.filled.CallEnd
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.material.icons.filled.VideocamOff
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import co.daily.model.MediaStreamTrack
import co.daily.view.VideoView
import com.kravtsov.kumowatch.AppState
import com.kravtsov.kumowatch.CallPhase
import com.kravtsov.kumowatch.CallTile

enum class CallLayout(val title: String) { Speaker("Speaker"), Screen("Screen"), Grid("Grid") }

@Composable
fun CallScreen(state: AppState, onBack: () -> Unit) {
    val call by state.call.state.collectAsState()
    val context = LocalContext.current
    var layout by rememberSaveable { mutableStateOf(CallLayout.Speaker) }
    val permissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {}
    LaunchedEffect(Unit) {
        val needed = listOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.CAMERA)
            .filter { context.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (needed.isNotEmpty()) permissions.launch(needed.toTypedArray())
    }
    val sharing = call.tiles.firstOrNull { it.screen != null }
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
                        CallPhase.Joined -> "${call.tiles.size} in call"
                        CallPhase.Failed -> call.error ?: "Could not join"
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
                call.phase != CallPhase.Joined -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                layout == CallLayout.Grid -> LazyVerticalGrid(GridCells.Fixed(2), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    items(call.tiles, key = { it.id.uuid.toString() }) { tile ->
                        Tile(state, tile, speaking = tile.id in call.speaking, modifier = Modifier.aspectRatio(0.8f))
                    }
                }
                else -> {
                    val main = if (layout == CallLayout.Screen && sharing != null) null else
                        call.tiles.firstOrNull { it.id == call.activeSpeaker && !it.isLocal }
                            ?: call.tiles.firstOrNull { !it.isLocal } ?: call.tiles.firstOrNull()
                    Column {
                        Box(Modifier.weight(1f).fillMaxWidth()) {
                            if (main == null && sharing?.screen != null) {
                                Video(sharing.screen, Modifier.fillMaxSize().clip(RoundedCornerShape(12.dp)))
                                Text(
                                    "${sharing.name} is sharing",
                                    color = Color.White,
                                    modifier = Modifier.align(Alignment.BottomStart).padding(8.dp)
                                        .background(Color(0x99000000), RoundedCornerShape(6.dp)).padding(horizontal = 6.dp),
                                )
                            } else if (main != null) {
                                Tile(state, main, speaking = main.id in call.speaking, modifier = Modifier.fillMaxSize())
                            }
                        }
                        val others = call.tiles.filter { it.id != main?.id }
                        if (others.isNotEmpty()) {
                            LazyRow(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                items(others, key = { it.id.uuid.toString() }) { tile ->
                                    Tile(state, tile, speaking = tile.id in call.speaking, modifier = Modifier.size(width = 96.dp, height = 120.dp))
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
            FilledIconButton(onClick = { state.call.toggleMic() }, modifier = Modifier.size(56.dp)) {
                Icon(if (call.micOn) Icons.Filled.Mic else Icons.Filled.MicOff, "Microphone")
            }
            FilledIconButton(onClick = { state.call.toggleCamera() }, modifier = Modifier.size(56.dp)) {
                Icon(if (call.cameraOn) Icons.Filled.Videocam else Icons.Filled.VideocamOff, "Camera")
            }
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
private fun Tile(state: AppState, tile: CallTile, speaking: Boolean, modifier: Modifier) {
    val person = state.personByKey(tile.userId)
    Box(
        modifier.clip(RoundedCornerShape(12.dp)).background(Color(0xFF1F2229))
            .border(if (speaking) 3.dp else 0.dp, if (speaking) Color(0xFF2EB67D) else Color.Transparent, RoundedCornerShape(12.dp)),
        contentAlignment = Alignment.Center,
    ) {
        if (tile.camera != null) {
            Video(tile.camera, Modifier.fillMaxSize(), mirror = tile.isLocal)
        } else {
            Avatar(tile.name, person?.avatarUrl ?: "", size = 72.dp)
        }
        Row(
            Modifier.align(Alignment.BottomStart).padding(6.dp)
                .background(Color(0x99000000), RoundedCornerShape(6.dp)).padding(horizontal = 6.dp, vertical = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (!tile.micOn) {
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
