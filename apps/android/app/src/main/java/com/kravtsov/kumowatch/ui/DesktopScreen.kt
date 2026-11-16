package com.kravtsov.kumowatch.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.WavingHand
import androidx.compose.material.icons.automirrored.filled.DirectionsWalk
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Map
import androidx.compose.material.icons.filled.Smartphone
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.material.icons.filled.VideocamOff
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconToggleButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.kravtsov.kumowatch.AppState
import com.kravtsov.kumowatch.DeskState
import com.kravtsov.kumowatch.DeskRect
import com.kravtsov.kumowatch.focus
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DesktopScreen(state: AppState, onOpenCall: () -> Unit = {}, onBack: () -> Unit) {
    val relay = state.relay
    val scope = rememberCoroutineScope()
    var desk by remember { mutableStateOf<DeskState?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var statusDraft by remember { mutableStateOf<String?>(null) }

    fun act(block: suspend () -> DeskState?) {
        scope.launch {
            runCatching { block() }
                .onSuccess { r -> r?.let { desk = it }; error = null }
                .onFailure { error = it.message }
        }
    }

    val callState by state.call.state.collectAsState()
    LaunchedEffect(Unit) {
        while (true) {
            runCatching { relay.desktop() }.onSuccess { d ->
                desk = d
                error = null
            }.onFailure { error = it.message }
            delay(1000)
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Desktop") },
                navigationIcon = { IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
            )
        },
    ) { padding ->
        val d = desk
        if (d == null) {
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                if (error != null) Text(error ?: "", modifier = Modifier.padding(24.dp)) else CircularProgressIndicator()
            }
            return@Scaffold
        }
        val zoneName = d.zones.firstOrNull { it.id == d.selfZone }?.name ?: "Common area"
        LazyColumn(Modifier.fillMaxSize().padding(padding)) {
            item {
                Column(Modifier.padding(horizontal = 16.dp)) {
                    Text(zoneName, style = MaterialTheme.typography.titleMedium)
                    Text(d.floors.firstOrNull { it.id == d.floorId }?.name ?: "", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.outline)
                    error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
                }
            }
            item { FloorMap(d, Modifier.padding(12.dp), onTap = { x, y ->
                val zone = d.zones.firstOrNull { x >= it.x && x <= it.x + it.w && y >= it.y && y <= it.y + it.h }
                act { if (zone != null && zone.id != d.selfZone) relay.zone(zone.id) else relay.move(x, y) }
            }) }
            item {
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                    FilledIconToggleButton(checked = d.audio, onCheckedChange = { on -> act { relay.media(audio = on) } }) {
                        Icon(if (d.audio) Icons.Filled.Mic else Icons.Filled.MicOff, "Desktop microphone")
                    }
                    FilledIconToggleButton(checked = d.video, onCheckedChange = { on -> act { relay.media(video = on) } }) {
                        Icon(if (d.video) Icons.Filled.Videocam else Icons.Filled.VideocamOff, "Desktop camera")
                    }
                    androidx.compose.material3.FilledTonalIconButton(onClick = { act { relay.home() } }) {
                        Icon(Icons.Filled.Home, "Go home")
                    }
                    Spacer(Modifier.width(8.dp))
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        listOf("available", "away", "focusing").forEach { s ->
                            FilterChip(selected = d.availability == s, onClick = { act { relay.status(s, statusDraft ?: d.selfMessage) } }, label = { Text(s.replaceFirstChar { it.uppercase() }) })
                        }
                    }
                }
            }
            item {
                val draft = statusDraft ?: d.selfMessage
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    androidx.compose.material3.OutlinedTextField(
                        value = draft,
                        onValueChange = { statusDraft = it.take(100) },
                        placeholder = { Text("Status message") },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(
                        onClick = {
                            val text = draft.trim()
                            act { relay.status(d.availability.ifBlank { "available" }, text) }
                            statusDraft = null
                        },
                        enabled = statusDraft != null && statusDraft != d.selfMessage,
                    ) { Text("Save") }
                }
            }
            if (d.floors.size > 1) {
                item {
                    Row(Modifier.padding(horizontal = 16.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        d.floors.forEach { f ->
                            FilterChip(selected = f.id == d.floorId, onClick = { if (f.id != d.floorId) act { relay.floor(f.id) } }, label = { Text(f.name) })
                        }
                    }
                }
            }
            item {
                val inCall = state.call.isRemote && callState.phase != com.kravtsov.kumowatch.CallPhase.Idle
                androidx.compose.material3.Button(
                    onClick = {
                        if (inCall) {
                            onOpenCall()
                        } else {
                            scope.launch {
                                runCatching {
                                    state.joinDesktopCall()
                                }.onSuccess {
                                    onOpenCall()
                                }.onFailure { error = it.message }
                            }
                        }
                    },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                ) { Text(if (inCall) "Open call" else "Use phone for audio and video") }
            }
            item { StatusLegend() }
            item { Text("People on this floor", style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(16.dp, 12.dp, 16.dp, 4.dp)) }
            items(d.people.filter { !it.self }.sortedBy { it.name }, key = { it.id }) { p ->
                ListItem(
                    leadingContent = { StatusAvatar(p.name, p.avatar, size = 40.dp, availability = p.availability, idle = p.idle) },
                    headlineContent = { Text(p.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    supportingContent = {
                        Column {
                            Text(
                                statusLabel(p.availability, p.idle) + (if (p.message.isNotBlank() && !p.idle) " · ${p.message}" else ""),
                                color = statusColor(p.availability, p.idle),
                                style = MaterialTheme.typography.bodySmall,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            Text(d.zones.firstOrNull { it.id == p.zoneId }?.name ?: "Common area", style = MaterialTheme.typography.bodySmall)
                        }
                    },
                    trailingContent = {
                        Row {
                            androidx.compose.material3.FilledTonalIconButton(onClick = { act { if (p.zoneId != null) relay.zone(p.zoneId) else relay.move(p.x, p.y) } }) {
                                Icon(Icons.AutoMirrored.Filled.DirectionsWalk, "Go to ${p.name}")
                            }
                            androidx.compose.material3.FilledTonalIconButton(onClick = { act { relay.nudge(p.id); null } }) {
                                Icon(Icons.Filled.WavingHand, "Nudge ${p.name}")
                            }
                        }
                    },
                )
            }
        }
    }
}

@Composable
fun FloorMap(d: DeskState, modifier: Modifier = Modifier, viewport: DeskRect? = null, onTap: ((Double, Double) -> Unit)? = null) {
    if (d.width <= 0 || d.height <= 0) return
    val region = viewport ?: DeskRect(0.0, 0.0, d.width, d.height)
    val zoneColor = MaterialTheme.colorScheme.secondaryContainer
    val mineColor = MaterialTheme.colorScheme.primaryContainer
    val border = MaterialTheme.colorScheme.outline
    val density = LocalDensity.current
    BoxWithConstraints(
        modifier.fillMaxWidth().aspectRatio((region.w / region.h).toFloat())
            .clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surfaceVariant),
    ) {
        val scale = constraints.maxWidth / region.w
        Canvas(
            Modifier.fillMaxSize().pointerInput(d.width, d.height, region, onTap) {
                if (onTap != null) detectTapGestures { offset -> onTap(offset.x / scale + region.x, offset.y / scale + region.y) }
            },
        ) {
            val s = scale.toFloat()
            d.zones.forEach { z ->
                val tl = Offset(((z.x - region.x) * s).toFloat(), ((z.y - region.y) * s).toFloat())
                val size = Size((z.w * s).toFloat(), (z.h * s).toFloat())
                drawRect(if (z.id == d.selfZone) mineColor else zoneColor, tl, size)
                drawRect(border, tl, size, style = Stroke(1f))
            }
        }
        d.people.filter { region.contains(it.x, it.y, 100.0) }.sortedBy { it.self }.forEach { p ->
            val avatar = if (p.self) 28.dp else 22.dp
            val labelWidth = 56.dp
            val half = with(density) { (labelWidth / 2).roundToPx() }
            val top = with(density) { (avatar / 2).roundToPx() }
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier
                    .offset { IntOffset(((p.x - region.x) * scale).toInt() - half, ((p.y - region.y) * scale).toInt() - top) }
                    .width(labelWidth),
            ) {
                StatusAvatar(p.name, p.avatar, size = avatar, availability = p.availability, idle = p.idle, emphasize = p.self)
                Text(
                    if (p.self) "You" else p.name.substringBefore(' '),
                    style = MaterialTheme.typography.labelSmall.copy(fontSize = 9.sp),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.background(MaterialTheme.colorScheme.surface.copy(alpha = 0.8f), RoundedCornerShape(4.dp))
                        .padding(horizontal = 3.dp),
                )
            }
        }
    }
}

@Composable
fun DesktopCard(state: AppState, onOpen: () -> Unit) {
    val relay = state.relay
    val callState by state.call.state.collectAsState()
    var desk by remember { mutableStateOf<DeskState?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        while (true) {
            runCatching { relay.desktop() }
                .onSuccess { desk = it; error = null }
                .onFailure { error = it.message }
            delay(3000)
        }
    }
    val inPhoneCall = state.call.isRemote && callState.phase != com.kravtsov.kumowatch.CallPhase.Idle
    val d = desk
    androidx.compose.material3.ElevatedCard(
        onClick = onOpen,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
    ) {
        Column(Modifier.padding(16.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
                Column(Modifier.weight(1f)) {
                    Text("YOUR DESKTOP", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline)
                    if (d != null) {
                        Text(
                            d.zones.firstOrNull { it.id == d.selfZone }?.name ?: "Common area",
                            style = MaterialTheme.typography.titleLarge,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Box(Modifier.size(8.dp).clip(androidx.compose.foundation.shape.CircleShape).background(statusColor(d.availability)))
                            Spacer(Modifier.width(6.dp))
                            val floor = d.floors.firstOrNull { it.id == d.floorId }?.name.orEmpty()
                            Text(
                                statusLabel(d.availability) + if (d.floors.size > 1 && floor.isNotBlank()) " · $floor" else "",
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                    } else {
                        Text(if (error == null) "Connecting…" else "Desktop not reachable", style = MaterialTheme.typography.titleLarge)
                    }
                }
                if (d != null) {
                    if (inPhoneCall) {
                        Icon(Icons.Filled.Smartphone, "Phone is in the call", tint = MaterialTheme.colorScheme.primary)
                    } else {
                        Icon(if (d.audio) Icons.Filled.Mic else Icons.Filled.MicOff, "Desktop microphone", tint = if (d.audio) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline)
                        Spacer(Modifier.width(8.dp))
                        Icon(if (d.video) Icons.Filled.Videocam else Icons.Filled.VideocamOff, "Desktop camera", tint = if (d.video) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline)
                    }
                }
            }
        }
    }
}
