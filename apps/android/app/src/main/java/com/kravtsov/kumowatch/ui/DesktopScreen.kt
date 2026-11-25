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
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.material3.Surface
import androidx.compose.runtime.derivedStateOf
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.material.icons.filled.Search
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.filled.Close
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.withFrameNanos
import androidx.compose.runtime.saveable.rememberSaveable
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
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.material.icons.filled.ZoomOutMap
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.runtime.DisposableEffect
import com.kravtsov.kumowatch.AppState
import kotlinx.coroutines.launch
import uniffi.kumo_ffi.CallSource
import uniffi.kumo_ffi.DesktopRect
import uniffi.kumo_ffi.DesktopState
import uniffi.kumo_ffi.RelayClient
import uniffi.kumo_ffi.availabilityChoices
import uniffi.kumo_ffi.availabilityLabel
import uniffi.kumo_ffi.presenceLabel

@Composable
fun WatchDesktop(state: AppState) {
    DisposableEffect(state) {
        state.relay.watchDesktop()
        onDispose { state.relay.unwatchDesktop() }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DesktopScreen(state: AppState, onOpenCall: () -> Unit = {}, onOpenNearby: () -> Unit = {}, onBack: () -> Unit) {
    val relay = state.relay
    val scope = rememberCoroutineScope()
    val snapshot by state.desktop.collectAsState()
    var statusDraft by remember { mutableStateOf<String?>(null) }
    // TextFieldValue, not String: the inline and pinned fields hand focus back and forth and must keep the caret.
    var query by rememberSaveable(stateSaver = TextFieldValue.Saver) { mutableStateOf(TextFieldValue("")) }
    var searching by rememberSaveable { mutableStateOf(false) }
    var openingSearch by remember { mutableStateOf(false) }
    var focusSearch by remember { mutableStateOf(false) }
    val focusManager = LocalFocusManager.current
    fun closeSearch() {
        focusManager.clearFocus()
        query = TextFieldValue("")
        searching = false
    }
    BackHandler(enabled = searching) { closeSearch() }

    fun act(block: suspend RelayClient.() -> Unit) {
        scope.launch { runCatching { relay.block() } }
    }

    WatchDesktop(state)
    val callState by state.call.collectAsState()

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Desktop") },
                navigationIcon = { IconButton(onClick = { if (searching) closeSearch() else onBack() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
            )
        },
    ) { padding ->
        val d = snapshot.state
        val error = snapshot.error
        if (d == null) {
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                if (error != null) Text(error, modifier = Modifier.padding(24.dp)) else CircularProgressIndicator()
            }
            return@Scaffold
        }
        val term = if (searching) query.text.trim() else ""
        val listState = rememberLazyListState()
        val peopleIds = remember(d.people) { d.people.map { it.id }.toSet() }
        // The header row has scrolled up past the top: people rows are on screen but it isn't.
        val pinSearch by remember(peopleIds) {
            derivedStateOf {
                val visible = listState.layoutInfo.visibleItemsInfo
                // Half the header off screen already clips its field, so count it as gone.
                val header = visible.firstOrNull { it.key == "people-header" }
                (header == null || header.offset < -header.size / 2) && visible.any { it.key in peopleIds }
            }
        }
        var pinnedFocused by remember { mutableStateOf(false) }
        var inlineFocused by remember { mutableStateOf(false) }
        val pinnedFocus = remember { FocusRequester() }
        LaunchedEffect(pinSearch) {
            if (pinSearch && inlineFocused && searching) {
                withFrameNanos { }
                if (searching) runCatching { pinnedFocus.requestFocus() }
            }
        }
        // Filtering can shrink the list and bring the header row back; keep typing in its field.
        LaunchedEffect(pinSearch) { if (!pinSearch && pinnedFocused && searching) focusSearch = true }
        // A new filter result starts from its first match, not from wherever the old list was scrolled.
        LaunchedEffect(term) { if (searching && pinSearch) listState.scrollToItem(0) }
        Box(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding).imePadding()) {
            LazyColumn(Modifier.fillMaxSize(), state = listState) {
                if (!searching) {
                    item {
                        Column(Modifier.padding(horizontal = 16.dp)) {
                            Text(d.place, style = MaterialTheme.typography.titleMedium)
                            Text(d.floorName, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.outline)
                            error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
                        }
                    }
                    item { FloorMap(d, Modifier.padding(12.dp), onTap = { x, y -> act { tap(x, y) } }) }
                    item {
                        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                            FilledIconToggleButton(checked = d.audio, onCheckedChange = { on -> act { setMedia(on, null) } }) {
                                Icon(if (d.audio) Icons.Filled.Mic else Icons.Filled.MicOff, "Desktop microphone")
                            }
                            FilledIconToggleButton(checked = d.video, onCheckedChange = { on -> act { setMedia(null, on) } }) {
                                Icon(if (d.video) Icons.Filled.Videocam else Icons.Filled.VideocamOff, "Desktop camera")
                            }
                            androidx.compose.material3.FilledTonalIconButton(onClick = { act { goHome() } }) {
                                Icon(Icons.Filled.Home, "Go home")
                            }
                            Spacer(Modifier.width(8.dp))
                            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                availabilityChoices().forEach { a ->
                                    FilterChip(
                                        selected = d.availability == a,
                                        onClick = { act { setStatus(a, statusDraft ?: d.statusMessage) } },
                                        label = { Text(availabilityLabel(a)) },
                                    )
                                }
                            }
                        }
                    }
                    item {
                        val draft = statusDraft ?: d.statusMessage
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
                                    act { setStatusMessage(draft) }
                                    statusDraft = null
                                },
                                enabled = statusDraft != null && statusDraft != d.statusMessage,
                            ) { Text("Save") }
                        }
                    }
                    if (d.floors.size > 1) {
                        item {
                            Row(Modifier.padding(horizontal = 16.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                d.floors.forEach { f ->
                                    FilterChip(selected = f.id == d.floorId, onClick = { if (f.id != d.floorId) act { switchFloor(f.id) } }, label = { Text(f.name) })
                                }
                            }
                        }
                    }
                    item {
                        val inCall = callState.source == CallSource.DESKTOP
                        androidx.compose.material3.Button(
                            onClick = {
                                if (!inCall) state.joinDesktopCall()
                                onOpenCall()
                            },
                            enabled = callState.source != CallSource.OFFICE,
                            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                        ) { Text(if (inCall) "Open call" else "Use phone for audio and video") }
                    }
                    item {
                        androidx.compose.material3.OutlinedButton(
                            onClick = onOpenNearby,
                            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                        ) { Text("Nearby Chat in ${d.place}") }
                    }
                    item { StatusLegend() }
                }
                item(key = "people-header") {
                    PeopleSearchHeader(query, { if (searching) query = it }, searching, focusSearch, onFocused = { focusSearch = false }, onFocusChange = { inlineFocused = it }, onOpen = {
                        if (!openingSearch && !searching) {
                            openingSearch = true
                            scope.launch {
                                // Bring the header to the top first, so removing the rows above it doesn't jump the list.
                                try {
                                    listState.layoutInfo.visibleItemsInfo.firstOrNull { it.key == "people-header" }?.let { listState.animateScrollToItem(it.index) }
                                } finally {
                                    searching = true
                                    focusSearch = true
                                    openingSearch = false
                                }
                            }
                        }
                    }, onClose = ::closeSearch)
                }
                if (searching && error != null) {
                    item(key = "error") { Text(error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 16.dp)) }
                }
                val people = d.people.filter {
                    !it.isSelf && (term.isEmpty() || it.name.contains(term, ignoreCase = true) || it.place.contains(term, ignoreCase = true))
                }
                if (people.isEmpty() && term.isNotEmpty()) {
                    item(key = "people-none") { Text("No one matches \"$term\"", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(16.dp)) }
                }
                items(people, key = { it.id }) { p ->
                    ListItem(
                        modifier = Modifier.animateItem(),
                        leadingContent = { StatusAvatar(p.name, p.avatarUrl, size = 40.dp, presence = p.presence) },
                        headlineContent = { Text(p.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        supportingContent = {
                            Column {
                                Text(
                                    p.statusLine,
                                    color = statusColor(p.presence),
                                    style = MaterialTheme.typography.bodySmall,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                Text(p.place, style = MaterialTheme.typography.bodySmall)
                            }
                        },
                        trailingContent = {
                            Row {
                                androidx.compose.material3.FilledTonalIconButton(onClick = { act { goTo(p.id) } }) {
                                    Icon(Icons.AutoMirrored.Filled.DirectionsWalk, "Go to ${p.name}")
                                }
                                androidx.compose.material3.FilledTonalIconButton(onClick = { act { nudge(p.id) } }) {
                                    Icon(Icons.Filled.WavingHand, "Nudge ${p.name}")
                                }
                            }
                        },
                    )
                }
            }
            AnimatedVisibility(
                visible = pinSearch,
                enter = slideInVertically { -it } + fadeIn(),
                exit = slideOutVertically { -it } + fadeOut(),
                modifier = Modifier.align(Alignment.TopCenter),
            ) {
                Surface(color = MaterialTheme.colorScheme.surface, shadowElevation = 3.dp) {
                    SearchField(
                        query,
                        { if (searching) query = it },
                        onClose = if (searching) ::closeSearch else null,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).focusRequester(pinnedFocus).onFocusChanged {
                            pinnedFocused = it.isFocused
                            if (it.isFocused) searching = true
                        },
                    )
                }
            }
        }
    }
}

@Composable
private fun SearchField(query: TextFieldValue, onQuery: (TextFieldValue) -> Unit, onClose: (() -> Unit)?, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val focusManager = LocalFocusManager.current
    androidx.compose.material3.OutlinedTextField(
        value = query,
        onValueChange = onQuery,
        enabled = enabled,
        placeholder = { Text("Find a person") },
        leadingIcon = { Icon(Icons.Filled.Search, null) },
        trailingIcon = onClose?.let { { IconButton(onClick = it) { Icon(Icons.Filled.Close, "Close search") } } },
        singleLine = true,
        shape = RoundedCornerShape(50),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
        keyboardActions = KeyboardActions(onSearch = { focusManager.clearFocus() }),
        modifier = modifier,
    )
}

@Composable
private fun PeopleSearchHeader(
    query: TextFieldValue,
    onQuery: (TextFieldValue) -> Unit,
    searching: Boolean,
    focusRequested: Boolean,
    onFocused: () -> Unit,
    onFocusChange: (Boolean) -> Unit,
    onOpen: () -> Unit,
    onClose: () -> Unit,
) {
    AnimatedContent(
        targetState = searching,
        transitionSpec = {
            if (targetState) {
                (fadeIn(tween(220, delayMillis = 60)) + expandHorizontally(tween(320), expandFrom = Alignment.End))
                    .togetherWith(fadeOut(tween(120)) + slideOutHorizontally(tween(200)) { -it / 6 })
            } else {
                (fadeIn(tween(220, delayMillis = 60)) + slideInHorizontally(tween(260)) { -it / 6 })
                    .togetherWith(fadeOut(tween(120)) + shrinkHorizontally(tween(260), shrinkTowards = Alignment.End))
            }.using(SizeTransform(clip = false))
        },
        contentAlignment = Alignment.CenterEnd,
        label = "people-search",
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 8.dp, bottom = 4.dp),
    ) { open ->
        if (open) {
            val focus = remember { FocusRequester() }
            SearchField(query, onQuery, onClose, enabled = searching, modifier = Modifier.fillMaxWidth().focusRequester(focus).onFocusChanged { onFocusChange(it.isFocused) })
            LaunchedEffect(focusRequested, searching) {
                if (focusRequested && searching) {
                    focus.requestFocus()
                    onFocused()
                }
            }
        } else {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text("People on this floor", style = MaterialTheme.typography.labelLarge, modifier = Modifier.weight(1f).semantics { heading() })
                androidx.compose.material3.FilledTonalIconButton(onClick = onOpen) { Icon(Icons.Filled.Search, "Search people") }
            }
        }
    }
}

private const val MIN_ZOOM = 0.5
private const val MAX_ZOOM = 8.0

private fun mapRegion(bounds: DesktopRect, zoom: Double, center: Pair<Double, Double>?): DesktopRect {
    val w = bounds.w / zoom
    val h = bounds.h / zoom
    if (zoom <= 1.0 || center == null) return DesktopRect(bounds.x + (bounds.w - w) / 2, bounds.y + (bounds.h - h) / 2, w, h)
    val cx = center.first.coerceIn(bounds.x + w / 2, bounds.x + bounds.w - w / 2)
    val cy = center.second.coerceIn(bounds.y + h / 2, bounds.y + bounds.h - h / 2)
    return DesktopRect(cx - w / 2, cy - h / 2, w, h)
}

@Composable
fun FloorMap(d: DesktopState, modifier: Modifier = Modifier, onTap: ((Double, Double) -> Unit)? = null) {
    if (d.bounds.w <= 0 || d.bounds.h <= 0) return
    val bounds = d.bounds
    var zoom by remember { mutableDoubleStateOf(1.0) }
    var center by remember { mutableStateOf<Pair<Double, Double>?>(null) }
    val region = mapRegion(bounds, zoom, center)
    val zoneColor = MaterialTheme.colorScheme.secondaryContainer
    val mineColor = MaterialTheme.colorScheme.primaryContainer
    val border = MaterialTheme.colorScheme.outline
    val density = LocalDensity.current
    BoxWithConstraints(
        modifier.fillMaxWidth().aspectRatio((bounds.w / bounds.h).toFloat())
            .clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surfaceVariant)
            .pointerInput(bounds) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    var travel = Offset.Zero
                    var active = false
                    do {
                        val event = awaitPointerEvent()
                        val fingers = event.changes.count { it.pressed }
                        val zoomChange = event.calculateZoom()
                        val pan = event.calculatePan()
                        travel += pan
                        if (!active) active = fingers >= 2 || (zoom > 1.0 && travel.getDistance() > viewConfiguration.touchSlop)
                        if (active) {
                            val current = mapRegion(bounds, zoom, center)
                            val unitsPerPx = current.w / size.width
                            val next = (zoom * zoomChange).coerceIn(MIN_ZOOM, MAX_ZOOM)
                            val moved = (current.x + current.w / 2 - pan.x * unitsPerPx) to (current.y + current.h / 2 - pan.y * unitsPerPx)
                            val r = mapRegion(bounds, next, moved)
                            zoom = next
                            center = if (next <= 1.0) null else (r.x + r.w / 2) to (r.y + r.h / 2)
                            event.changes.forEach { if (it.positionChanged()) it.consume() }
                        }
                    } while (event.changes.any { it.pressed })
                }
            },
    ) {
        val scale = constraints.maxWidth / region.w
        Canvas(
            Modifier.fillMaxSize().pointerInput(d.bounds, region, onTap) {
                if (onTap != null) detectTapGestures { offset -> onTap(offset.x / scale + region.x, offset.y / scale + region.y) }
            },
        ) {
            val s = scale.toFloat()
            d.zones.forEach { z ->
                val tl = Offset(((z.x - region.x) * s).toFloat(), ((z.y - region.y) * s).toFloat())
                val size = Size((z.w * s).toFloat(), (z.h * s).toFloat())
                drawRect(if (z.isMine) mineColor else zoneColor, tl, size)
                drawRect(border, tl, size, style = Stroke(1f))
            }
        }
        d.people.filter { visible(region, it.x, it.y) }.forEach { p ->
            val avatar = if (p.isSelf) 28.dp else 22.dp
            val labelWidth = 56.dp
            val half = with(density) { (labelWidth / 2).roundToPx() }
            val top = with(density) { (avatar / 2).roundToPx() }
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier
                    .offset { IntOffset(((p.x - region.x) * scale).toInt() - half, ((p.y - region.y) * scale).toInt() - top) }
                    .width(labelWidth),
            ) {
                StatusAvatar(p.name, p.avatarUrl, size = avatar, presence = p.presence, emphasize = p.isSelf)
                Text(
                    if (p.isSelf) "You" else p.name.substringBefore(' '),
                    style = MaterialTheme.typography.labelSmall.copy(fontSize = 9.sp),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.background(MaterialTheme.colorScheme.surface.copy(alpha = 0.8f), RoundedCornerShape(4.dp))
                        .padding(horizontal = 3.dp),
                )
            }
        }
        if (zoom != 1.0 || center != null) {
            IconButton(
                onClick = {
                    zoom = 1.0
                    center = null
                },
                modifier = Modifier.align(Alignment.TopEnd).padding(4.dp).size(32.dp)
                    .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.85f), CircleShape),
            ) {
                Icon(Icons.Filled.ZoomOutMap, "Show the whole floor", Modifier.size(18.dp))
            }
        }
    }
}

private fun visible(region: DesktopRect, x: Double, y: Double, margin: Double = 100.0) =
    x >= region.x - margin && x <= region.x + region.w + margin && y >= region.y - margin && y <= region.y + region.h + margin

@Composable
fun DesktopCard(state: AppState, onOpen: () -> Unit) {
    val callState by state.call.collectAsState()
    val snapshot by state.desktop.collectAsState()
    WatchDesktop(state)
    val inPhoneCall = callState.source == CallSource.DESKTOP
    val d = snapshot.state
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
                            d.place,
                            style = MaterialTheme.typography.titleLarge,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Box(Modifier.size(8.dp).clip(androidx.compose.foundation.shape.CircleShape).background(statusColor(d.presence)))
                            Spacer(Modifier.width(6.dp))
                            Text(
                                presenceLabel(d.presence) + if (d.floors.size > 1 && d.floorName.isNotBlank()) " · ${d.floorName}" else "",
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                    } else {
                        Text(if (snapshot.error == null) "Connecting…" else "Desktop not reachable", style = MaterialTheme.typography.titleLarge)
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
