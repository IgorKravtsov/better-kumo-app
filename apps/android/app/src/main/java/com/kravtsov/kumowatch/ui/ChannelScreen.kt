package com.kravtsov.kumowatch.ui

import android.content.ClipData
import android.content.ClipboardManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.AddPhotoAlternate
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Gif
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import com.kravtsov.kumowatch.AppState
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.kumo_ffi.MemberItem
import uniffi.kumo_ffi.MessageItem
import uniffi.kumo_ffi.OutgoingFile
import uniffi.kumo_ffi.SendState
import uniffi.kumo_ffi.attachmentLimits

private const val PAGE = 50
private val QUICK_REACTIONS = listOf("👍", "🙌", "😂", "❤️", "🔥", "👀")
private val LIMITS = attachmentLimits()

private class PendingAttachment(val name: String, val bytes: ByteArray, val preview: ImageBitmap)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChannelScreen(
    state: AppState,
    channelUrl: String,
    thread: MessageItem? = null,
    onOpenThread: (MessageItem) -> Unit = {},
    onOpenChannel: (String) -> Unit = {},
    onBack: () -> Unit,
) {
    val channels by state.channels.collectAsState()
    val typing by state.typing.collectAsState()
    val channel = channels.firstOrNull { it.url == channelUrl }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()
    val parentId = thread?.id
    fun load(limit: Int): List<MessageItem> =
        if (parentId != null) state.core.thread(channelUrl, parentId) else state.core.messages(channelUrl, limit.toUInt())

    var limit by remember { mutableIntStateOf(PAGE) }
    var messages by remember { mutableStateOf(load(PAGE)) }
    var ready by remember { mutableStateOf(messages.isNotEmpty() || parentId != null) }
    var loadingOlder by remember { mutableStateOf(false) }
    var reachedStart by remember { mutableStateOf(parentId != null) }
    var draft by remember(channelUrl, parentId) { mutableStateOf(TextFieldValue("")) }
    val mentionMap = remember(channelUrl, parentId) { mutableStateMapOf<String, String>() }
    var editing by remember { mutableStateOf<MessageItem?>(null) }
    var actionsFor by remember { mutableStateOf<MessageItem?>(null) }
    val attachments = remember(channelUrl, parentId) { mutableStateListOf<PendingAttachment>() }
    var preparing by remember(channelUrl, parentId) { mutableIntStateOf(0) }
    val members = remember(channelUrl) { state.core.channelMembers(channelUrl) }
    val initialUnread = remember(channelUrl) { if (parentId == null) (channel?.unread ?: 0u).toInt() else 0 }
    var viewer by remember { mutableStateOf<String?>(null) }
    var showDetails by remember { mutableStateOf(false) }
    var showGifs by remember { mutableStateOf(false) }

    suspend fun reload() {
        messages = withContext(Dispatchers.IO) { load(limit) }
        if (messages.isNotEmpty()) ready = true
    }

    DisposableEffect(channelUrl, parentId) {
        if (parentId == null) state.core.openChannel(channelUrl)
        onDispose { if (parentId == null) state.core.closeChannel(channelUrl) }
    }
    LaunchedEffect(channelUrl, parentId) {
        if (parentId != null) {
            runCatching { state.core.loadThread(channelUrl, parentId) }
            reload()
        }
    }
    LaunchedEffect(channelUrl, parentId) {
        state.messageChanges.filter { it == channelUrl }.collect { reload() }
    }
    LaunchedEffect(channelUrl) {
        delay(8000)
        ready = true
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(LIMITS.maxCount.toInt())) { uris ->
        val room = LIMITS.maxCount.toInt() - attachments.size - preparing
        val accepted = uris.take(room.coerceAtLeast(0))
        if (accepted.isEmpty()) return@rememberLauncherForActivityResult
        preparing += accepted.size
        scope.launch {
            accepted.forEach { uri ->
                val prepared = withContext(Dispatchers.IO) {
                    runCatching {
                        compressImage(context, uri, LIMITS)?.let { bitmap ->
                            PendingAttachment("photo-${UUID.randomUUID().toString().take(8)}.jpg", jpegBytes(bitmap, LIMITS), thumbnail(bitmap))
                        }
                    }.getOrNull()
                }
                preparing -= 1
                if (prepared != null && attachments.size < LIMITS.maxCount.toInt()) attachments.add(prepared)
            }
        }
    }

    val reversed = remember(messages) { messages.asReversed() }
    val nearTop by remember {
        derivedStateOf {
            val last = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            reversed.isNotEmpty() && last >= reversed.size - 15
        }
    }
    LaunchedEffect(nearTop, reversed.size) {
        if (!nearTop || loadingOlder || reachedStart || reversed.isEmpty()) return@LaunchedEffect
        loadingOlder = true
        val oldest = messages.first().createdAt
        val local = withContext(Dispatchers.IO) { state.core.messagesBefore(channelUrl, oldest, PAGE.toUInt()) }
        if (local.size < PAGE) {
            val fetched = runCatching { state.core.loadOlder(channelUrl, oldest) }.getOrDefault(0u)
            if (fetched == 0u) reachedStart = true
        }
        limit += PAGE
        reload()
        loadingOlder = false
    }
    val awayFromBottom by remember { derivedStateOf { listState.firstVisibleItemIndex > 3 } }
    LaunchedEffect(Unit) {
        snapshotFlow { messages.lastOrNull()?.let { it.id to it.isMine } }.collect { newest ->
            // A send swaps its local id for the server's; key anchoring would leave it just below the viewport.
            if (newest?.second == true || listState.firstVisibleItemIndex <= 1) listState.scrollToItem(0)
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column(Modifier.clickable(enabled = thread == null) { showDetails = true }) {
                        Text(if (thread != null) "Thread" else channel?.title ?: "", maxLines = 1)
                        val names = typing[channelUrl].orEmpty()
                        if (names.isNotEmpty()) {
                            Text(
                                names.joinToString() + if (names.size == 1) " is typing…" else " are typing…",
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.primary,
                            )
                        } else if (thread != null) {
                            Text(channel?.title ?: "", style = MaterialTheme.typography.labelSmall)
                        } else if (channel != null && channel.memberCount > 2u) {
                            Text("${channel.memberCount} members", style = MaterialTheme.typography.labelSmall)
                        }
                    }
                },
                navigationIcon = { IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).imePadding()) {
            Box(Modifier.weight(1f)) {
                if (!ready) CircularProgressIndicator(Modifier.align(Alignment.Center))
                LazyColumn(
                    state = listState,
                    reverseLayout = true,
                    modifier = Modifier.fillMaxSize(),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    itemsIndexed(reversed, key = { _, m -> m.id }) { index, message ->
                        val older = reversed.getOrNull(index + 1)
                        val grouped = older != null && older.senderId == message.senderId &&
                            message.createdAt - older.createdAt < 5 * 60_000 && sameDay(older.createdAt, message.createdAt)
                        Column {
                            if (older == null || !sameDay(older.createdAt, message.createdAt)) DaySeparator(message.createdAt)
                            if (initialUnread > 0 && index == initialUnread - 1) UnreadDivider()
                            MessageRow(
                                message = message,
                                grouped = grouped,
                                onTap = {
                                    if (message.state == SendState.FAILED) {
                                        scope.launch { runCatching { state.core.retryMessage(channelUrl, message.id) } }
                                    }
                                },
                                onLongPress = { if (message.state == SendState.SENT) actionsFor = message },
                                onReaction = { key -> scope.launch { runCatching { state.core.toggleReaction(channelUrl, message.id, key) } } },
                                onReplies = { if (thread == null) onOpenThread(message) },
                                onImage = { viewer = it },
                            )
                        }
                    }
                    if (thread != null) {
                        item(key = "parent") {
                            Column {
                                MessageRow(thread, grouped = false, onTap = {}, onLongPress = {}, onReaction = {}, onReplies = {}, onImage = { viewer = it })
                                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                            }
                        }
                    }
                    if (loadingOlder) {
                        item { Box(Modifier.fillMaxWidth().padding(8.dp), Alignment.Center) { CircularProgressIndicator(Modifier.size(20.dp)) } }
                    }
                }
                if (awayFromBottom) {
                    SmallFloatingActionButton(
                        onClick = { scope.launch { listState.animateScrollToItem(0) } },
                        modifier = Modifier.align(Alignment.BottomEnd).padding(12.dp),
                    ) { Icon(Icons.Filled.KeyboardArrowDown, "Latest") }
                }
            }
            MentionSuggestions(members, draft) { member ->
                val (text, cursor) = insertMention(draft, member.name)
                mentionMap[member.name] = member.uid
                draft = TextFieldValue(text, TextRange(cursor))
            }
            editing?.let { e ->
                Row(
                    Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 12.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Editing message", style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f))
                    IconButton(onClick = {
                        editing = null
                        draft = TextFieldValue("")
                    }) { Icon(Icons.Filled.Close, "Cancel edit") }
                }
            }
            if (editing == null) {
                AttachmentStrip(attachments, preparing) { attachments.remove(it) }
            }
            Composer(
                value = draft,
                enabled = ready && channel?.frozen != true,
                canAttach = editing == null && attachments.size + preparing < LIMITS.maxCount.toInt(),
                canSend = if (editing != null) draft.text.isNotBlank() else (draft.text.isNotBlank() || attachments.isNotEmpty()) && preparing == 0,
                placeholder = if (channel?.frozen == true) "Chat closed" else if (thread != null) "Reply in thread" else "Message ${channel?.title ?: ""}",
                onValueChange = {
                    val wasEmpty = draft.text.isEmpty()
                    draft = it
                    if (wasEmpty != it.text.isEmpty()) scope.launch { state.core.setTyping(channelUrl, it.text.isNotEmpty()) }
                },
                onAttach = { picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
                canGif = editing == null,
                onGif = { showGifs = true },
                onSend = {
                    val text = encodeMentions(draft.text.trim(), mentionMap)
                    val target = editing
                    val files = if (target == null) attachments.toList() else emptyList()
                    draft = TextFieldValue("")
                    editing = null
                    mentionMap.clear()
                    if (target == null) attachments.clear()
                    files.forEach { LocalPreviews.put(it.name, it.preview) }
                    scope.launch {
                        state.core.setTyping(channelUrl, false)
                        if (target != null) {
                            runCatching { state.core.editMessage(channelUrl, target.id, text) }
                        } else {
                            runCatching {
                                state.core.sendMessage(channelUrl, text, files.map { OutgoingFile(it.name, "image/jpeg", it.bytes) }, parentId)
                            }
                        }
                    }
                },
            )
        }
    }

    viewer?.let { ImageViewer(it) { viewer = null } }
    if (showGifs) {
        GifPicker(state, onDismiss = { showGifs = false }) { gif ->
            showGifs = false
            scope.launch { runCatching { state.core.sendMessage(channelUrl, gif.markdown, emptyList(), parentId) } }
        }
    }
    if (showDetails) {
        PeopleSheet(
            title = "${channel?.title ?: ""} · ${members.size + 1} members",
            people = members,
            onDismiss = { showDetails = false },
            onPick = { member ->
                showDetails = false
                scope.launch {
                    runCatching { state.core.openDirect(member.uid) }.onSuccess { url -> if (url != channelUrl) onOpenChannel(url) }
                }
            },
        )
    }
    LaunchedEffect(initialUnread, messages.size) {
        if (initialUnread > 8 && messages.size >= initialUnread) listState.scrollToItem(initialUnread - 1)
    }

    actionsFor?.let { message ->
        ModalBottomSheet(onDismissRequest = { actionsFor = null }) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                QUICK_REACTIONS.forEach { key ->
                    Text(
                        key,
                        style = MaterialTheme.typography.headlineSmall,
                        modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable {
                            actionsFor = null
                            scope.launch { runCatching { state.core.toggleReaction(channelUrl, message.id, key) } }
                        }.padding(8.dp),
                    )
                }
            }
            if (thread == null) {
                ListItem(headlineContent = { Text("Reply in thread") }, modifier = Modifier.clickable {
                    actionsFor = null
                    onOpenThread(message)
                })
            }
            if (message.text.isNotBlank()) {
                ListItem(headlineContent = { Text("Copy text") }, modifier = Modifier.clickable {
                    actionsFor = null
                    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("message", message.text))
                })
            }
            if (message.isMine && message.kind == "MESG") {
                ListItem(headlineContent = { Text("Edit") }, modifier = Modifier.clickable {
                    actionsFor = null
                    editing = message
                    val (raw, map) = decodeMentions(message)
                    mentionMap.clear()
                    mentionMap.putAll(map)
                    draft = TextFieldValue(raw, TextRange(raw.length))
                })
            }
            if (message.isMine) {
                ListItem(
                    headlineContent = { Text("Delete", color = MaterialTheme.colorScheme.error) },
                    modifier = Modifier.clickable {
                        actionsFor = null
                        scope.launch { runCatching { state.core.deleteMessage(channelUrl, message.id) } }
                    },
                )
            }
            Spacer(Modifier.size(24.dp))
        }
    }
}

private fun currentMentionQuery(value: TextFieldValue): String? {
    val before = value.text.substring(0, value.selection.start.coerceIn(0, value.text.length))
    val at = before.lastIndexOf('@')
    if (at < 0 || (at > 0 && !before[at - 1].isWhitespace())) return null
    val query = before.substring(at + 1)
    return if (query.length <= 30 && '\n' !in query) query else null
}

private fun insertMention(value: TextFieldValue, name: String): Pair<String, Int> {
    val cursor = value.selection.start.coerceIn(0, value.text.length)
    val before = value.text.substring(0, cursor)
    val at = before.lastIndexOf('@')
    val text = value.text.substring(0, at) + "@$name " + value.text.substring(cursor)
    return text to (at + name.length + 2)
}

private fun encodeMentions(text: String, map: Map<String, String>): String =
    map.entries.sortedByDescending { it.key.length }.fold(text) { acc, (name, uid) ->
        acc.replace("@$name", "@mention(type=user&id=$uid)")
    }

private fun decodeMentions(message: MessageItem): Pair<String, Map<String, String>> {
    val map = mutableMapOf<String, String>()
    val text = message.spans.joinToString("") { span ->
        val uid = span.mentionUserId
        if (uid != null && span.text.startsWith("@") && uid.isNotEmpty()) {
            map[span.text.removePrefix("@")] = uid
        }
        span.text
    }
    return text to map
}

@Composable
private fun MentionSuggestions(members: List<MemberItem>, draft: TextFieldValue, onPick: (MemberItem) -> Unit) {
    val query = currentMentionQuery(draft) ?: return
    val matches = members.filter { it.name.contains(query, ignoreCase = true) }.take(6)
    if (matches.isEmpty()) return
    Surface(tonalElevation = 3.dp) {
        LazyColumn(Modifier.fillMaxWidth().heightIn(max = 220.dp)) {
            items(matches, key = { it.uid }) { m ->
                Row(
                    Modifier.fillMaxWidth().clickable { onPick(m) }.padding(horizontal = 12.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Avatar(m.name, m.avatarUrl, size = 28.dp, online = m.isOnline)
                    Spacer(Modifier.width(10.dp))
                    Text(m.name)
                }
            }
        }
    }
}

@Composable
private fun UnreadDivider() {
    Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        HorizontalDivider(Modifier.weight(1f), color = MaterialTheme.colorScheme.error)
        Text(
            "New messages",
            color = MaterialTheme.colorScheme.error,
            style = MaterialTheme.typography.labelSmall,
            modifier = Modifier.padding(horizontal = 8.dp),
        )
        HorizontalDivider(Modifier.weight(1f), color = MaterialTheme.colorScheme.error)
    }
}

@Composable
private fun DaySeparator(ms: Long) {
    Box(Modifier.fillMaxWidth().padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
        Text(
            dayLabel(ms),
            style = MaterialTheme.typography.labelSmall,
            modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceVariant)
                .padding(horizontal = 10.dp, vertical = 3.dp),
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun MessageRow(
    message: MessageItem,
    grouped: Boolean,
    onTap: () -> Unit,
    onLongPress: () -> Unit,
    onReaction: (String) -> Unit,
    onReplies: () -> Unit,
    onImage: (String) -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().combinedClickable(onClick = onTap, onLongClick = onLongPress)
            .padding(start = 12.dp, end = 12.dp, top = if (grouped) 0.dp else 8.dp),
    ) {
        if (grouped) {
            Spacer(Modifier.width(40.dp))
        } else {
            Avatar(message.senderName, message.senderAvatar, size = 36.dp)
            Spacer(Modifier.width(4.dp))
        }
        Spacer(Modifier.width(8.dp))
        Column(Modifier.weight(1f)) {
            if (!grouped) {
                Row(verticalAlignment = Alignment.Bottom) {
                    Text(message.senderName, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.bodyMedium)
                    Spacer(Modifier.width(6.dp))
                    Text(clockTime(message.createdAt), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline)
                    if (message.updatedAt > message.createdAt + 1000 && message.kind == "MESG") {
                        Text(" · edited", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline)
                    }
                }
            }
            if (message.kind == "FILE") FileContent(message, onImage)
            if (message.spans.isNotEmpty()) Text(rich(message), style = MaterialTheme.typography.bodyLarge)
            message.images.forEach { url -> ChatImage(url) { onImage(url) } }
            if (message.reactions.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.padding(top = 2.dp)) {
                    message.reactions.forEach { r ->
                        Text(
                            "${r.key} ${r.count}",
                            style = MaterialTheme.typography.labelMedium,
                            modifier = Modifier.clip(RoundedCornerShape(10.dp))
                                .background(if (r.mine) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant)
                                .clickable { onReaction(r.key) }
                                .padding(horizontal = 6.dp, vertical = 2.dp),
                        )
                    }
                }
            }
            if (message.replyCount > 0u) {
                Text(
                    "${message.replyCount} ${if (message.replyCount == 1uL) "reply" else "replies"}",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.clickable(onClick = onReplies).padding(vertical = 2.dp),
                )
            }
            when (message.state) {
                SendState.PENDING -> Text("Sending…", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline)
                SendState.FAILED -> Text("Not sent. Tap to retry", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.error)
                SendState.SENT -> Unit
            }
        }
    }
}

@Composable
private fun FileContent(message: MessageItem, onImage: (String) -> Unit) {
    if (message.fileType.startsWith("image/")) {
        if (message.fileUrl.isBlank()) {
            ChatImage(LocalPreviews.key(message.fileName))
        } else {
            ChatImage(message.fileUrl) { onImage(message.fileUrl) }
        }
    } else {
        Text("📎 ${message.fileName.ifBlank { "File" }}", color = MaterialTheme.colorScheme.primary)
    }
}

@Composable
private fun rich(message: MessageItem): AnnotatedString {
    val primary = MaterialTheme.colorScheme.primary
    val codeBg = MaterialTheme.colorScheme.surfaceVariant
    return buildAnnotatedString {
        message.spans.forEach { span ->
            val style = SpanStyle(
                fontWeight = if (span.bold || span.mentionUserId != null) FontWeight.SemiBold else null,
                fontStyle = if (span.italic) FontStyle.Italic else null,
                textDecoration = if (span.strike) TextDecoration.LineThrough else null,
                fontFamily = if (span.code) FontFamily.Monospace else null,
                background = when {
                    span.code -> codeBg
                    span.highlight -> Color(0x66FFEB3B)
                    else -> Color.Unspecified
                },
                color = if (span.mentionUserId != null || span.link != null) primary else Color.Unspecified,
            )
            val link = span.link
            if (link != null) {
                withLink(LinkAnnotation.Url(link, TextLinkStyles(style))) { append(span.text) }
            } else {
                withStyle(style) { append(span.text) }
            }
        }
    }
}

@Composable
private fun AttachmentStrip(attachments: List<PendingAttachment>, preparing: Int, onRemove: (PendingAttachment) -> Unit) {
    if (attachments.isEmpty() && preparing == 0) return
    LazyRow(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        contentPadding = PaddingValues(start = 8.dp, end = 8.dp, top = 8.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        items(attachments, key = { it.name }) { attachment ->
            Box(Modifier.size(64.dp)) {
                Image(
                    attachment.preview,
                    null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(8.dp)),
                )
                Box(
                    Modifier.align(Alignment.TopEnd).padding(2.dp).size(22.dp).clip(CircleShape)
                        .background(Color.Black.copy(alpha = 0.6f)).clickable { onRemove(attachment) },
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.Filled.Close, "Remove", tint = Color.White, modifier = Modifier.size(14.dp)) }
            }
        }
        items(preparing) {
            Box(
                Modifier.size(64.dp).clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.surfaceVariant),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator(Modifier.size(20.dp)) }
        }
    }
}

@Composable
private fun Composer(
    value: TextFieldValue,
    enabled: Boolean,
    canAttach: Boolean,
    canSend: Boolean,
    placeholder: String,
    onValueChange: (TextFieldValue) -> Unit,
    onAttach: () -> Unit,
    canGif: Boolean,
    onGif: () -> Unit,
    onSend: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().navigationBarsPadding().padding(8.dp), verticalAlignment = Alignment.Bottom) {
        IconButton(onClick = onAttach, enabled = enabled && canAttach) {
            Icon(Icons.Filled.AddPhotoAlternate, "Attach images")
        }
        IconButton(onClick = onGif, enabled = enabled && canGif) {
            Icon(Icons.Filled.Gif, "Send a GIF")
        }
        OutlinedTextField(
            value = value,
            onValueChange = onValueChange,
            enabled = enabled,
            placeholder = { Text(if (enabled) placeholder else "Loading…", maxLines = 1) },
            maxLines = 6,
            shape = RoundedCornerShape(20.dp),
            modifier = Modifier.weight(1f),
        )
        IconButton(onClick = onSend, enabled = enabled && canSend) {
            Icon(Icons.AutoMirrored.Filled.Send, "Send")
        }
    }
}
