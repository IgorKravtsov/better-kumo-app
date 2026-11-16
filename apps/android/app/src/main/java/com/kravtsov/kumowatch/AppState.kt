package com.kravtsov.kumowatch

import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.kumo_ffi.AlertItem
import uniffi.kumo_ffi.AppEvent
import uniffi.kumo_ffi.ClientPlatform
import uniffi.kumo_ffi.SpaceView
import uniffi.kumo_ffi.AppListener
import uniffi.kumo_ffi.ChannelItem
import uniffi.kumo_ffi.ConnectionStatus
import uniffi.kumo_ffi.KumoApp
import uniffi.kumo_ffi.SpaceInfo
import uniffi.kumo_ffi.UserInfo

sealed interface Screen {
    data object SignIn : Screen
    data object PickSpace : Screen
    data object Chats : Screen
}

class AppState(context: Context) : AppListener {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val core = KumoApp(context.filesDir.absolutePath, KeystoreStorage(context), this).also {
        it.setDevice(ClientPlatform.ANDROID, java.util.TimeZone.getDefault().id)
    }
    val call = CallManager(context.applicationContext, this)
    val relay = RelayClient(context.applicationContext)

    suspend fun joinDesktopCall() {
        relay.media(audio = false, video = false)
        val desktopCall = relay.handoff()
        call.joinRemote(desktopCall) { relay.handoff() }
    }

    private val _spaceView = MutableStateFlow<SpaceView?>(null)
    val spaceView: StateFlow<SpaceView?> = _spaceView.asStateFlow()

    private val _notices = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val notices: SharedFlow<String> = _notices.asSharedFlow()

    private var spaceReload: Job? = null
    private var lastJoin: Pair<String, String>? = null

    private val _passwordPrompt = MutableStateFlow<Pair<String, String>?>(null)
    val passwordPrompt: StateFlow<Pair<String, String>?> = _passwordPrompt.asStateFlow()

    private val _screen = MutableStateFlow<Screen>(Screen.SignIn)
    val screen: StateFlow<Screen> = _screen.asStateFlow()

    private val _user = MutableStateFlow<UserInfo?>(null)
    val user: StateFlow<UserInfo?> = _user.asStateFlow()

    private val _space = MutableStateFlow<SpaceInfo?>(null)
    val space: StateFlow<SpaceInfo?> = _space.asStateFlow()

    private val _channels = MutableStateFlow<List<ChannelItem>>(emptyList())
    val channels: StateFlow<List<ChannelItem>> = _channels.asStateFlow()

    private val _connection = MutableStateFlow(ConnectionStatus.OFFLINE)
    val connection: StateFlow<ConnectionStatus> = _connection.asStateFlow()

    private val _typing = MutableStateFlow<Map<String, List<String>>>(emptyMap())
    val typing: StateFlow<Map<String, List<String>>> = _typing.asStateFlow()

    private val _messageChanges = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val messageChanges: SharedFlow<String> = _messageChanges.asSharedFlow()

    private val _alerts = MutableSharedFlow<AlertItem>(extraBufferCapacity = 64)
    val alerts: SharedFlow<AlertItem> = _alerts.asSharedFlow()

    private val _relayEnabled = MutableStateFlow(Relay.isEnabled(context))
    val relayEnabled: StateFlow<Boolean> = _relayEnabled.asStateFlow()

    private val _openRequest = MutableStateFlow<String?>(null)
    val openRequest: StateFlow<String?> = _openRequest.asStateFlow()

    private val _errors = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val errors: SharedFlow<String> = _errors.asSharedFlow()

    private var channelsReload: Job? = null

    init {
        refreshSession()
        if (Relay.isEnabled(context) && _screen.value == Screen.Chats) {
            runCatching { context.startForegroundService(android.content.Intent(context, RelayService::class.java)) }
        }
    }

    override fun onEvent(event: AppEvent) {
        scope.launch {
            when (event) {
                is AppEvent.ChannelsChanged -> scheduleChannelsReload()
                is AppEvent.MessagesChanged -> _messageChanges.tryEmit(event.channelUrl)
                is AppEvent.Connection -> _connection.value = event.status
                is AppEvent.Typing -> _typing.value = _typing.value + (event.channelUrl to event.userNames)
                is AppEvent.Error -> _errors.tryEmit(event.message)
                is AppEvent.Alert -> _alerts.tryEmit(event.alert)
                is AppEvent.SpaceChanged -> scheduleSpaceReload()
                is AppEvent.CallChanged -> {
                    scheduleSpaceReload()
                    call.onCallInfo(core.callInfo())
                }
                is AppEvent.Notice -> Unit
                is AppEvent.SpaceError -> onSpaceError(event.code, event.message)
            }
        }
    }

    private fun scheduleChannelsReload() {
        if (channelsReload?.isActive == true) return
        channelsReload = scope.launch {
            delay(50)
            _channels.value = withContext(Dispatchers.IO) { core.channels() }
        }
    }

    private fun scheduleSpaceReload() {
        if (spaceReload?.isActive == true) return
        spaceReload = scope.launch {
            delay(100)
            _spaceView.value = withContext(Dispatchers.IO) { core.spaceView() }
            call.onPeersChanged()
        }
    }

    fun knownUids(): Set<String> {
        val view = _spaceView.value ?: return emptySet()
        return (view.floors.flatMap { f -> f.rooms.flatMap { it.people } + f.commonArea } + view.away)
            .map { it.uid }.toSet()
    }

    fun person(uid: String): uniffi.kumo_ffi.PersonItem? {
        val view = _spaceView.value ?: return null
        return (view.floors.flatMap { f -> f.rooms.flatMap { it.people } + f.commonArea } + view.away)
            .firstOrNull { it.uid == uid }
    }

    fun personByKey(key: String): uniffi.kumo_ffi.PersonItem? {
        val view = _spaceView.value ?: return null
        return (view.floors.flatMap { f -> f.rooms.flatMap { it.people } + f.commonArea } + view.away)
            .firstOrNull { it.key == key }
    }

    fun openSpace() {
        runCatching { core.spaceOpen() }
        scheduleSpaceReload()
    }

    fun closeSpace() {
        runCatching { core.spaceClose() }
    }

    private fun onSpaceError(code: String, message: String) {
        when (code) {
            "ROOM_PASSWORD_REQUIRED", "ROOM_PASSWORD_INCORRECT" -> {
                _passwordPrompt.value = lastJoin
                if (code == "ROOM_PASSWORD_INCORRECT") _notices.tryEmit("Wrong room password")
            }
            else -> {
                if (call.state.value.phase == CallPhase.Waiting) call.leave()
                _notices.tryEmit(
                    when (code) {
                        "ZONE_CLOSED", "ROOM_CLOSED" -> "This room is closed"
                        "FULL_CAPACITY" -> "This room is full"
                        else -> message.ifBlank { code }
                    },
                )
            }
        }
    }

    fun submitPassword(password: String?) {
        val target = _passwordPrompt.value ?: return
        _passwordPrompt.value = null
        if (password.isNullOrBlank()) {
            call.leave()
            return
        }
        runCatching { core.joinRoomWithPassword(target.first, target.second, password) }
    }

    fun joinRoom(floorId: String, zoneId: String) {
        lastJoin = floorId to zoneId
        call.expectCall()
        runCatching { core.joinRoom(floorId, zoneId) }.onFailure { _notices.tryEmit(it.message ?: "Cannot join") }
    }

    fun joinPerson(uid: String) {
        call.expectCall()
        runCatching { core.joinPerson(uid) }.onFailure { _notices.tryEmit(it.message ?: "Cannot join") }
    }

    fun leaveCall() {
        val remote = call.isRemote
        call.leave()
        if (!remote) runCatching { core.leaveCall() }
    }

    private fun refreshSession() {
        val user = core.currentUser()
        _user.value = user
        if (user == null) {
            _screen.value = Screen.SignIn
            return
        }
        val space = core.selectedSpace()
        _space.value = space
        if (space == null) {
            _screen.value = Screen.PickSpace
            return
        }
        _channels.value = core.channels()
        core.start()
        _screen.value = Screen.Chats
    }

    suspend fun signInWithLink(link: String): Result<Unit> = scope.async {
        runCatching {
            core.signInWithLink(link)
            refreshSession()
            if (_screen.value == Screen.PickSpace) {
                core.defaultSpace()?.let { selectSpace(it) }
            }
        }
    }.await()

    suspend fun spaces(): List<SpaceInfo> = core.spaces()

    fun selectSpace(space: SpaceInfo) {
        core.selectSpace(space)
        _space.value = space
        _channels.value = core.channels()
        _screen.value = Screen.Chats
    }

    suspend fun transferLink(): Result<String> = runCatching { core.transferLink() }

    fun signOut() {
        core.signOut()
        _channels.value = emptyList()
        refreshSession()
    }

    fun onRelayChanged(enabled: Boolean) {
        _relayEnabled.value = enabled
    }

    fun requestOpen(channelUrl: String?) {
        _openRequest.value = channelUrl
    }

    fun consumeOpenRequest() {
        _openRequest.value = null
    }

    fun onForeground() {
        if (_screen.value == Screen.Chats) core.start()
    }
}
