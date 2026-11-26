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
import uniffi.kumo_ffi.PersonJoin
import uniffi.kumo_ffi.SpaceView
import uniffi.kumo_ffi.AppListener
import uniffi.kumo_ffi.CallPhase
import uniffi.kumo_ffi.CallView
import uniffi.kumo_ffi.ChannelItem
import uniffi.kumo_ffi.ConnectionStatus
import uniffi.kumo_ffi.DesktopSnapshot
import uniffi.kumo_ffi.KumoApp
import uniffi.kumo_ffi.NotificationDelivery
import uniffi.kumo_ffi.MyStatus
import uniffi.kumo_ffi.NotificationKinds
import uniffi.kumo_ffi.NotificationProfile
import uniffi.kumo_ffi.RelayStatus
import uniffi.kumo_ffi.SpaceInfo
import uniffi.kumo_ffi.UserInfo

sealed interface Screen {
    data object SignIn : Screen
    data object PickSpace : Screen
    data object Chats : Screen
}

enum class OfficeRequest { OFFICE, DESKTOP, CALL }

class AppState(private val context: Context) : AppListener {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val media = DailyMedia(context.applicationContext) { telecom.active }
    val core = KumoApp(context.filesDir.absolutePath, KeystoreStorage(context), RelayPairingStorage(context), media, this).also {
        it.setDevice(ClientPlatform.ANDROID, java.util.TimeZone.getDefault().id)
    }
    val relay = core.relayClient()

    private val _delivery = MutableStateFlow(core.notificationDelivery())
    val delivery: StateFlow<NotificationDelivery> = _delivery.asStateFlow()

    private val _notificationProfile = MutableStateFlow(core.notificationProfile())
    val notificationProfile: StateFlow<NotificationProfile> = _notificationProfile.asStateFlow()

    private val _myStatus = MutableStateFlow(core.myStatus())
    val myStatus: StateFlow<MyStatus> = _myStatus.asStateFlow()

    private val _desktop = MutableStateFlow(DesktopSnapshot(null, null))
    val desktop: StateFlow<DesktopSnapshot> = _desktop.asStateFlow()

    private val _call = MutableStateFlow(core.call())
    val call: StateFlow<CallView> = _call.asStateFlow()
    private var callServiceRunning = false

    fun joinDesktopCall() {
        phoneJoin()
        _joinFailed.value = false
        core.handoffCall()
    }

    fun toggleMic() = core.setMic(!_call.value.micOn)

    fun toggleCamera() = core.setCamera(!_call.value.cameraOn)

    val telecom: TelecomCalls = TelecomCalls(context, scope, onActive = { updateCallService() }, onHungUp = { leaveCall() })
    val watchCalls = WatchCalls(context, this)

    /** The call was started from the watch, which can't unlock the phone when Android refuses the microphone. */
    private var fromWatch = false

    /** Outlives [fromWatch]: the core ends a rejected join before it reports the room error. */
    private var lastJoinFromWatch = false
    private var micOnJoin = false

    /** Answered from a live call: the mic waits for the media switch (Joining), so a rejected move never unmutes. */
    private var micAfterSwitch = false
    private var callActive = false

    fun beginWatchJoin() {
        fromWatch = true
        lastJoinFromWatch = true
        micOnJoin = false
        _joinFailed.value = false
    }

    private fun phoneJoin() {
        micOnJoin = false
        lastJoinFromWatch = false
        watchCalls.invalidateJoin()
    }

    /** Called once the join was sent, so a failed lookup never unmutes the call the user is already in. */
    fun turnMicOnAfterJoin() {
        val view = core.call()
        // A nudge from someone in this room switches no media, so there is no Joining to wait for.
        val sameRoom = lastJoin?.second?.let { it == runCatching { core.spaceView().selfZoneId }.getOrNull() } == true
        if (view.phase == CallPhase.Joined && sameRoom) {
            core.setMic(true)
        } else if (view.phase == CallPhase.Joined) {
            micAfterSwitch = true
            micOnJoin = true
        } else if (view.phase != CallPhase.Idle && view.phase !is CallPhase.Failed) {
            core.setMic(true)
        } else {
            micAfterSwitch = false
            micOnJoin = true
        }
    }

    fun watchJoinFailed() {
        micOnJoin = false
        if (!core.call().phase.let { it != CallPhase.Idle && it !is CallPhase.Failed }) {
            fromWatch = false
            telecom.end()
            updateCallService()
        }
    }

    fun microphoneBlocked() {
        if (!fromWatch) return
        watchCalls.invalidateJoin(leave = true)
        core.leaveCall()
        telecom.end()
        fromWatch = false
        updateCallService()
        watchCalls.report("Unlock the phone to use its microphone, or switch calls to the watch")
    }

    fun inForeground(): Boolean {
        val info = android.app.ActivityManager.RunningAppProcessInfo()
        android.app.ActivityManager.getMyMemoryState(info)
        return info.importance <= android.app.ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    }

    private fun onCallChanged() {
        val view = core.call()
        _call.value = view
        if (view.phase != CallPhase.Idle) _joinFailed.value = false
        val active = view.phase != CallPhase.Idle && view.phase !is CallPhase.Failed
        if (active && micOnJoin && (!micAfterSwitch || view.phase == CallPhase.Joining)) {
            micOnJoin = false
            micAfterSwitch = false
            core.setMic(true)
        }
        watchCalls.publish(fromWatch)
        if (callActive && !active) {
            micOnJoin = false
            fromWatch = false
            telecom.end()
        }
        callActive = active
        updateCallService()
    }

    /** The service also runs while Telecom holds a watch call that is still joining: Telecom wants it within 5 s. */
    private fun updateCallService() {
        val wanted = callActive || telecom.active
        val service = android.content.Intent(context, CallService::class.java)
        if (wanted && callServiceRunning && telecom.active != serviceHasTelecom) {
            serviceHasTelecom = telecom.active
            runCatching { context.startService(service) }
            return
        }
        if (wanted == callServiceRunning) return
        callServiceRunning = wanted
        serviceHasTelecom = telecom.active
        if (wanted) {
            runCatching { context.startForegroundService(service) }.onFailure {
                callServiceRunning = false
                microphoneBlocked()
            }
        } else if (servicePromoted) {
            servicePromoted = false
            context.stopService(service)
        }
        // Otherwise the service stops itself once promoted: stopping it earlier crashes the app.
    }

    private var serviceHasTelecom = false
    private var servicePromoted = false

    /** A call may end before Android delivers the pending foreground-service start. */
    fun onCallServicePromoted(): Boolean {
        servicePromoted = callServiceRunning
        return callServiceRunning
    }

    fun onCallServiceStopped() {
        servicePromoted = false
    }

    private val _spaceView = MutableStateFlow<SpaceView?>(null)
    val spaceView: StateFlow<SpaceView?> = _spaceView.asStateFlow()

    private val _notices = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val notices: SharedFlow<String> = _notices.asSharedFlow()
    private val _joinFailed = MutableStateFlow(false)
    /** State, not an event: a call screen that opens after the rejection must still close. */
    val joinFailed: StateFlow<Boolean> = _joinFailed.asStateFlow()

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

    private val _officeRequest = MutableStateFlow<OfficeRequest?>(null)
    val officeRequest: StateFlow<OfficeRequest?> = _officeRequest.asStateFlow()

    private val _errors = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val errors: SharedFlow<String> = _errors.asSharedFlow()

    private var channelsReload: Job? = null

    init {
        refreshSession()
        if (Relay.isEnabled(context) && _delivery.value == NotificationDelivery.PHONE && _screen.value == Screen.Chats) {
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
                is AppEvent.SpaceChanged -> {
                    _myStatus.value = core.myStatus()
                    scheduleSpaceReload()
                }
                is AppEvent.CallChanged -> onCallChanged()
                is AppEvent.Notice -> Unit
                is AppEvent.SpaceError -> onSpaceError(event.code, event.message)
                is AppEvent.DesktopChanged -> _desktop.value = relay.desktop()
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

    fun openSpace() {
        runCatching { core.spaceOpen() }
        scheduleSpaceReload()
    }

    fun closeSpace() {
        runCatching { core.spaceClose() }
    }

    private fun onSpaceError(code: String, message: String) {
        micOnJoin = false
        // A rejected waiting join leaves the Office wanted in the core; leaving releases it.
        if (core.call().phase == CallPhase.Idle) core.leaveCall()
        if (lastJoinFromWatch) {
            watchCalls.report(
                when (code) {
                    "ROOM_PASSWORD_REQUIRED", "ROOM_PASSWORD_INCORRECT" -> "This room needs a password. Join it on the phone."
                    "ZONE_CLOSED", "ROOM_CLOSED" -> "This room is closed"
                    "FULL_CAPACITY" -> "This room is full"
                    else -> message.ifBlank { code }
                },
            )
        }
        micAfterSwitch = false
        when (code) {
            "ROOM_PASSWORD_REQUIRED", "ROOM_PASSWORD_INCORRECT" -> {
                _passwordPrompt.value = lastJoin
                if (code == "ROOM_PASSWORD_INCORRECT") _notices.tryEmit("Wrong room password")
            }
            else -> {
                joinEnded()
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
            joinEnded()
            return
        }
        _joinFailed.value = false
        phoneJoin()
        runCatching { core.joinRoomWithPassword(target.first, target.second, password) }
    }

    /** Any space error abandons a waiting call; only one that left no call ends the join. */
    private fun joinEnded() {
        if (core.call().phase == CallPhase.Idle) _joinFailed.value = true
    }

    fun joinRoom(floorId: String, zoneId: String) {
        phoneJoin()
        joinRoomOrError(floorId, zoneId)?.let { _notices.tryEmit(it) }
    }

    fun joinRoomOrError(floorId: String, zoneId: String): String? {
        lastJoin = floorId to zoneId
        _joinFailed.value = false
        return runCatching { core.joinRoom(floorId, zoneId) }.exceptionOrNull()?.let { it.message ?: "Cannot join" }
    }

    /** A watch command can arrive before the office census. */
    suspend fun joinPersonSoonOrError(personId: String): String? {
        _joinFailed.value = false
        return try {
            if (joined(core.joinPersonSoon(personId))) null else "They are not in a room you can join"
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            e.message ?: "Cannot join"
        }
    }

    private fun joined(join: PersonJoin?): Boolean {
        if (join == null) return false
        lastJoin = join.floorId to join.zoneId
        return true
    }

    fun joinPerson(personId: String): Boolean =
        runCatching {
            phoneJoin()
            _joinFailed.value = false
            joined(core.joinPerson(personId))
        }
            .onSuccess { if (!it) _notices.tryEmit("They are not in a room you can join") }
            .onFailure { _notices.tryEmit(it.message ?: "Cannot join") }
            .getOrDefault(false)

    fun openPerson(personId: String, via: String?) {
        when (via) {
            "desktop" -> {
                _officeRequest.value = OfficeRequest.DESKTOP
                scope.launch {
                    runCatching { relay.joinPerson(personId) }
                        .onFailure { _notices.tryEmit(it.message ?: "Cannot reach the desktop app") }
                }
            }
            "phone" -> {
                _officeRequest.value = OfficeRequest.OFFICE
                scope.launch {
                    runCatching {
                        phoneJoin()
                        _joinFailed.value = false
                        joined(core.joinPersonSoon(personId))
                    }
                        .onSuccess { joined ->
                            if (joined) _officeRequest.value = OfficeRequest.CALL
                            else _notices.tryEmit("They are not in a room you can join")
                        }
                        .onFailure { _notices.tryEmit(it.message ?: "Cannot join") }
                }
            }
            else -> _officeRequest.value = OfficeRequest.OFFICE
        }
    }

    fun consumeOfficeRequest() {
        _officeRequest.value = null
    }

    fun leaveCall() {
        watchCalls.invalidateJoin(leave = true)
        core.leaveCall()
        watchJoinFailed()
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

    fun setDelivery(delivery: NotificationDelivery) {
        runCatching { core.setNotificationDelivery(delivery) }
            .onSuccess { _delivery.value = delivery }
            .onFailure { _errors.tryEmit(it.message ?: "Cannot save notification delivery") }
        Relay.applyService(context)
        RelayWidget.updateAll(context)
    }

    suspend fun applyNotificationKinds(status: MyStatus, kinds: NotificationKinds): Result<RelayStatus?> =
        runCatching { core.applyNotificationKinds(status, kinds) }
            .also { _notificationProfile.value = core.notificationProfile() }

    suspend fun syncNotificationProfile(): Result<RelayStatus?> =
        runCatching { core.syncNotificationProfile() }
            .also { _notificationProfile.value = core.notificationProfile() }

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
