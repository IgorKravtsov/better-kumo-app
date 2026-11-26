package com.kravtsov.kumowatch.wear

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import com.google.android.gms.wearable.CapabilityClient
import com.google.android.gms.wearable.Wearable
import com.kravtsov.kumowatch.KeystoreStorage
import com.kravtsov.kumowatch.WatchLink
import com.kravtsov.kumowatch.toWatchState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.kumo_ffi.AppEvent
import uniffi.kumo_ffi.AppListener
import uniffi.kumo_ffi.CallFailure
import uniffi.kumo_ffi.CallPhase
import uniffi.kumo_ffi.CallView
import uniffi.kumo_ffi.ChannelItem
import uniffi.kumo_ffi.ClientPlatform
import uniffi.kumo_ffi.ConnectionStatus
import uniffi.kumo_ffi.KumoApp
import uniffi.kumo_ffi.RelayPairing
import uniffi.kumo_ffi.RelayStorage
import uniffi.kumo_ffi.SpaceInfo
import uniffi.kumo_ffi.SpaceView
import uniffi.kumo_ffi.UserInfo

enum class WearScreen { SIGN_IN, HOME }

data class WearCall(val state: WatchLink.CallState, val onPhone: Boolean)

private object NoRelay : RelayStorage {
    override fun load(): RelayPairing? = null
    override fun save(pairing: RelayPairing) = Unit
    override fun clear() = Unit
    override fun saveMirroring(enabled: Boolean) = Unit
}

private fun CallPhase.active() = this != CallPhase.Idle && this !is CallPhase.Failed

class WearState(private val context: Context) : AppListener {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val core = KumoApp(context.filesDir.absolutePath, KeystoreStorage(context), NoRelay, WearMedia(context), this).also {
        it.setDevice(ClientPlatform.ANDROID, java.util.TimeZone.getDefault().id)
    }

    private val _screen = MutableStateFlow(WearScreen.SIGN_IN)
    val screen: StateFlow<WearScreen> = _screen.asStateFlow()

    private val _user = MutableStateFlow<UserInfo?>(null)
    val user: StateFlow<UserInfo?> = _user.asStateFlow()

    private val _call = MutableStateFlow(core.call())

    private val _space = MutableStateFlow<SpaceView?>(null)
    val space: StateFlow<SpaceView?> = _space.asStateFlow()

    private val _channels = MutableStateFlow<List<ChannelItem>>(emptyList())
    val channels: StateFlow<List<ChannelItem>> = _channels.asStateFlow()

    private val _connection = MutableStateFlow(ConnectionStatus.OFFLINE)
    val connection: StateFlow<ConnectionStatus> = _connection.asStateFlow()

    private val _messageChanges = MutableSharedFlow<String>(extraBufferCapacity = 64)
    val messageChanges: SharedFlow<String> = _messageChanges.asSharedFlow()

    private val _notices = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val notices: SharedFlow<String> = _notices.asSharedFlow()

    private val prefs = context.getSharedPreferences("wear", Context.MODE_PRIVATE)
    private val _callsOnPhone = MutableStateFlow(prefs.getBoolean(KEY_CALLS_ON_PHONE, false))
    val callsOnPhone: StateFlow<Boolean> = _callsOnPhone.asStateFlow()

    private val _phoneCall = MutableStateFlow(WatchLink.CallState.NONE)
    private var phoneEpoch = 0L
    private var phoneSeq = 0L

    val shownCall: StateFlow<WearCall> = combine(_call, _space, _phoneCall, _callsOnPhone) { local, space, phone, onPhone ->
        val mine = local.toWatchState(space)
        when {
            mine.active -> WearCall(mine, onPhone = false)
            phone.active || onPhone -> WearCall(phone, onPhone = true)
            else -> WearCall(mine, onPhone = false)
        }
    }.stateIn(scope, SharingStarted.Eagerly, WearCall(WatchLink.CallState.NONE, onPhone = false))

    enum class Busy { JOINING, MOVING }

    private val _busy = MutableStateFlow<Busy?>(null)
    val busy: StateFlow<Busy?> = _busy.asStateFlow()

    private var foreground = false
    private var sending = 0
    private var micOnJoin = false
    private var micAfterSwitch = false
    private var censusHeld = false
    private var officeOpen = false
    private var joinGeneration = 0
    private var operation: Job? = null
    private var lease: Job? = null
    private var callServiceRunning = false
    /** The join generation the user muted during: its pending unmute is dropped. */
    private var mutedGeneration = -1
    /** The mic the moved call should have at cutover; the mute button can change it during the move. */
    private var moveMic = false
    private var movePhoneJoinSent = false
    /** Bumped on sign-out, so a load that started before it can't publish the old account's data. */
    private var session = 0
    private val reloadChannels = reloader(100) {
        val s = session
        val channels = withContext(Dispatchers.IO) { core.channels() }
        if (s == session && signedIn) _channels.value = channels
    }
    private val reloadSpace = reloader(150) {
        val s = session
        val space = withContext(Dispatchers.IO) { core.spaceView() }
        if (s == session && signedIn) _space.value = space
    }

    init {
        refreshSession(start = false)
    }

    override fun onEvent(event: AppEvent) {
        // Never inline: a local join's first CallChanged fires before the core has installed its session.
        scope.launch(Dispatchers.Main) {
            when (event) {
                is AppEvent.ChannelsChanged -> reloadChannels()
                is AppEvent.MessagesChanged -> _messageChanges.tryEmit(event.channelUrl)
                is AppEvent.Connection -> _connection.value = event.status
                is AppEvent.SpaceChanged -> reloadSpace()
                is AppEvent.CallChanged -> onCallChanged()
                is AppEvent.SpaceError -> onSpaceError(event.code, event.message)
                else -> Unit
            }
        }
    }

    /** Coalesces bursts of events; one arriving during a load runs it again. */
    private fun reloader(delayMs: Long, load: suspend () -> Unit): () -> Unit {
        val requests = Channel<Unit>(Channel.CONFLATED)
        scope.launch {
            for (r in requests) {
                delay(delayMs)
                load()
            }
        }
        return { requests.trySend(Unit) }
    }

    private fun onCallChanged() {
        val view = core.call()
        _call.value = view
        val active = view.phase.active()
        if (active && micOnJoin && (!micAfterSwitch || view.phase == CallPhase.Joining)) {
            micOnJoin = false
            micAfterSwitch = false
            core.setMic(true)
        }
        if (!active) micOnJoin = false
        // Rust stops the media after its retries but keeps the Office it wanted for the call.
        (view.phase as? CallPhase.Failed)?.let { failed ->
            _notices.tryEmit(
                when (failed.reason) {
                    CallFailure.TakenOver -> "The call moved to another device"
                    else -> "The call couldn't connect"
                },
            )
            core.leaveCall()
        }
        if (active != callServiceRunning) {
            callServiceRunning = active
            val service = Intent(context, CallService::class.java)
            if (active) {
                runCatching { context.startForegroundService(service) }.onFailure { microphoneRefused() }
            } else {
                // Stopping it before it went foreground crashes the app; it then stops itself instead.
                if (servicePromoted) {
                    servicePromoted = false
                    context.stopService(service)
                }
            }
        }
        // Also after microphoneRefused, which already cleared callServiceRunning.
        if (!active) stopIfIdle()
    }

    private var servicePromoted = false

    /** A call may end before Android delivers the pending foreground-service start. */
    fun onCallServicePromoted(): Boolean {
        servicePromoted = callServiceRunning
        return callServiceRunning
    }

    fun onCallServiceStopped() {
        servicePromoted = false
    }

    /** Android refused the call's foreground service: without it the call would go silent in the background. */
    fun microphoneRefused() {
        callServiceRunning = false
        core.leaveCall()
        _notices.tryEmit("Android didn't allow the microphone. Open Better Kumo and join again.")
    }

    private fun onSpaceError(code: String, message: String) {
        micOnJoin = false
        micAfterSwitch = false
        // A rejected waiting join leaves the Office wanted in the core; leaving releases it.
        if (core.call().phase == CallPhase.Idle) core.leaveCall()
        _notices.tryEmit(
            when (code) {
                "ROOM_PASSWORD_REQUIRED", "ROOM_PASSWORD_INCORRECT" -> "This room needs a password. Join it from the phone."
                "ZONE_CLOSED", "ROOM_CLOSED" -> "This room is closed"
                "FULL_CAPACITY" -> "This room is full"
                else -> message.ifBlank { code }
            },
        )
    }

    private fun refreshSession(start: Boolean) {
        val user = core.currentUser()
        _user.value = user
        if (user == null || core.selectedSpace() == null) {
            _screen.value = WearScreen.SIGN_IN
            return
        }
        _channels.value = core.channels()
        if (start) core.start()
        _screen.value = WearScreen.HOME
    }

    private fun stopIfIdle() {
        if (!foreground && sending == 0 && !_call.value.phase.active()) runCatching { core.stop() }
    }

    val signedIn: Boolean get() = _screen.value == WearScreen.HOME

    suspend fun signInFromPhone(): Result<Unit> = try {
        val nodes = Wearable.getCapabilityClient(context)
            .getCapability(WatchLink.CAPABILITY_PHONE, CapabilityClient.FILTER_REACHABLE).await().nodes
        val phone = nodes.firstOrNull { it.isNearby } ?: nodes.firstOrNull()
            ?: error("Phone not reachable. Install Better Kumo on the phone and keep it nearby.")
        val reply = withTimeout(30_000) {
            Wearable.getMessageClient(context).sendRequest(phone.id, WatchLink.PATH_SIGN_IN, ByteArray(0)).await()
        }
        val signIn = WatchLink.SignIn.fromBytes(reply) ?: error("Sign in on the phone first, then try again.")
        signInWithLink(signIn.link, signIn.space)
    } catch (e: TimeoutCancellationException) {
        Result.failure(Exception("The phone didn't answer. Open Better Kumo on the phone and try again."))
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Result.failure(e)
    }

    suspend fun signInWithLink(link: String, phoneSpace: WatchLink.Space?): Result<Unit> = runCatching {
        core.signInWithLink(link)
        val space = phoneSpace?.let { SpaceInfo(it.name, it.uid, it.displayName, it.memberCount.toULong()) }
            ?: core.defaultSpace()
            ?: error("No Kumospace space on this account.")
        core.selectSpace(space)
        refreshSession(start = true)
        keepPhoneLease()
        // The app may have gone to the background while the phone answered.
        stopIfIdle()
    }

    fun signOut() {
        joinGeneration++
        session++
        operation?.cancel()
        lease?.cancel()
        core.leaveCall()
        runCatching { core.signOut() }
        _channels.value = emptyList()
        _space.value = null
        _phoneCall.value = WatchLink.CallState.NONE
        refreshSession(start = false)
    }

    fun onForeground(foreground: Boolean) {
        this.foreground = foreground
        if (foreground && signedIn) core.start()
        core.setForeground(foreground)
        if (foreground) {
            if (officeOpen) runCatching { core.spaceOpen() }
            keepPhoneLease()
        } else {
            lease?.cancel()
            if (officeOpen && !_call.value.phase.active()) runCatching { core.spaceClose() }
            stopIfIdle()
        }
    }

    fun setCallsOnPhone(onPhone: Boolean) {
        _callsOnPhone.value = onPhone
        prefs.edit().putBoolean(KEY_CALLS_ON_PHONE, onPhone).apply()
        keepPhoneLease()
    }

    /** While the app is open in phone mode the phone pushes its call; every command renews that lease. */
    private fun keepPhoneLease() {
        lease?.cancel()
        if (!foreground || !signedIn) return
        lease = scope.launch {
            while (isActive) {
                if (_callsOnPhone.value || _phoneCall.value.active) phone(WatchLink.CallCommand(WatchLink.CallCommand.STATE))
                delay(30_000)
            }
        }
    }

    /**
     * Pushes and replies race; only the newest state of one phone app process counts. A reply ([fresh]) from
     * another process comes from the live one, even if its wall-clock epoch is smaller.
     */
    fun onPhoneCall(state: WatchLink.CallState, fresh: Boolean = false) {
        scope.launch {
            if (!signedIn) return@launch
            val older = state.epoch < phoneEpoch || state.epoch == phoneEpoch && state.seq <= phoneSeq
            if (older && !(fresh && state.epoch != phoneEpoch)) return@launch
            phoneEpoch = state.epoch
            phoneSeq = state.seq
            _phoneCall.value = state
            state.error?.let { _notices.tryEmit(it) }
        }
    }

    private suspend fun phone(command: WatchLink.CallCommand): Boolean {
        val quiet = command.action == WatchLink.CallCommand.STATE
        val reply = try {
            val nodes = Wearable.getCapabilityClient(context)
                .getCapability(WatchLink.CAPABILITY_PHONE, CapabilityClient.FILTER_REACHABLE).await().nodes
            val phone = nodes.firstOrNull { it.isNearby } ?: nodes.firstOrNull()
            if (phone == null) {
                if (!quiet) _notices.tryEmit("Phone not reachable")
                return false
            }
            // The phone may wait for Telecom and for the office census before it answers.
            val bytes = withTimeout(30_000) {
                Wearable.getMessageClient(context).sendRequest(phone.id, WatchLink.PATH_CALL_COMMAND, command.toBytes()).await()
            }
            WatchLink.CallState.fromBytes(bytes)
        } catch (e: TimeoutCancellationException) {
            if (!quiet) _notices.tryEmit("The phone didn't answer")
            return false
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (!quiet) _notices.tryEmit("Phone not reachable")
            return false
        }
        if (reply == null) {
            if (!quiet) _notices.tryEmit("Update Better Kumo on the phone")
            return false
        }
        onPhoneCall(reply, fresh = true)
        return reply.error == null
    }

    fun openOffice() {
        officeOpen = true
        runCatching { core.spaceOpen() }
        reloadSpace()
    }

    fun closeOffice() {
        officeOpen = false
        runCatching { core.spaceClose() }
    }

    /** Another session of this account in a room (usually the desktop app); a watch call there too would echo. */
    fun otherDeviceRoom(view: SpaceView? = _space.value, inCall: Boolean = _call.value.phase.active()): String? {
        if (view == null || !view.connected) return null
        // Until this connection has its own entity, the core takes the account's avatar for it, so selfZoneId can
        // be the desktop's room: it identifies the watch only while the watch is in a call.
        val ownZone = view.selfZoneId.takeIf { inCall }
        return view.floors.flatMap { it.rooms }
            .firstOrNull { room -> room.zoneId != ownZone && room.people.any { it.isSelf } }?.name
    }

    private suspend fun warnIfElsewhere() {
        val inCall = _call.value.phase.active()
        var view = withContext(Dispatchers.IO) { runCatching { core.spaceView() }.getOrNull() }
        if (view?.connected != true) {
            // Held until the join was sent (releaseCensus), so the join reuses this connection.
            if (!officeOpen) censusHeld = true
            runCatching { core.spaceOpen() }
            view = withTimeoutOrNull(4_000) {
                while (true) {
                    val next = withContext(Dispatchers.IO) { runCatching { core.spaceView() }.getOrNull() }
                    if (next?.connected == true) return@withTimeoutOrNull next
                    delay(250)
                }
                @Suppress("UNREACHABLE_CODE") null
            }
        }
        otherDeviceRoom(view, inCall)?.let { _notices.tryEmit("You're also in $it on another device") }
    }

    private fun releaseCensus() {
        if (!censusHeld) return
        censusHeld = false
        if (!officeOpen) runCatching { core.spaceClose() }
    }

    private fun hasMicrophone(): Boolean {
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) return true
        _notices.tryEmit("Allow the microphone for Better Kumo in the watch's settings")
        return false
    }

    suspend fun joinRoom(floorId: String, zoneId: String): Boolean = busyWhile(Busy.JOINING) {
        if (_callsOnPhone.value) {
            phone(WatchLink.CallCommand(WatchLink.CallCommand.JOIN_ROOM, floorId = floorId, zoneId = zoneId))
        } else {
            joinRoomHere(floorId, zoneId, mic = false, warn = true)
        }
    }

    private suspend fun joinRoomHere(floorId: String, zoneId: String, mic: Boolean, warn: Boolean): Boolean {
        if (!hasMicrophone()) return false
        core.start()
        micOnJoin = false
        val generation = ++joinGeneration
        try {
            if (warn) warnIfElsewhere()
            if (generation != joinGeneration) return false
            return runCatching { core.joinRoom(floorId, zoneId) }
                .onSuccess { if (mic && mutedGeneration != generation) turnMicOnAfterJoin(zoneId) }
                .onFailure { _notices.tryEmit(it.message ?: "Cannot join") }
                .isSuccess
        } finally {
            releaseCensus()
        }
    }

    /** From a live call the mic waits for the media switch (Joining), so a rejected move never unmutes it. */
    private fun turnMicOnAfterJoin(targetZone: String?) {
        val phase = core.call().phase
        val sameRoom = targetZone != null && targetZone == runCatching { core.spaceView().selfZoneId }.getOrNull()
        when {
            phase == CallPhase.Joined && sameRoom -> core.setMic(true)
            phase == CallPhase.Joined -> {
                micAfterSwitch = true
                micOnJoin = true
            }
            phase.active() -> core.setMic(true)
            else -> {
                micAfterSwitch = false
                micOnJoin = true
            }
        }
    }

    suspend fun joinPerson(personId: String, mic: Boolean): Boolean = busyWhile(Busy.JOINING) {
        if (_callsOnPhone.value) {
            return@busyWhile phone(WatchLink.CallCommand(WatchLink.CallCommand.JOIN_PERSON, personId = personId, mic = mic))
        }
        if (!hasMicrophone()) return@busyWhile false
        core.start()
        micOnJoin = false
        val generation = ++joinGeneration
        var target: String? = null
        val joined = try {
            warnIfElsewhere()
            if (generation != joinGeneration) return@busyWhile false
            core.joinPersonSoon(personId)?.also { target = it.zoneId } != null
        } catch (e: CancellationException) {
            // A lookup cancelled between retries (Office closed) keeps the Office the core wanted for it.
            if (core.call().phase == CallPhase.Idle) core.leaveCall()
            throw e
        } catch (e: Exception) {
            _notices.tryEmit(e.message ?: "Cannot join")
            return@busyWhile false
        } finally {
            releaseCensus()
        }
        if (generation != joinGeneration) {
            if (joined) core.leaveCall()
            return@busyWhile false
        }
        if (!joined) {
            _notices.tryEmit("They are not in a room you can join")
            return@busyWhile false
        }
        if (mic && mutedGeneration != generation) turnMicOnAfterJoin(target)
        true
    }

    fun toggleMic() {
        val shown = shownCall.value
        val on = !shown.state.micOn
        if (_busy.value == Busy.MOVING) moveMic = on
        if (shown.onPhone) {
            scope.launch { phone(WatchLink.CallCommand(WatchLink.CallCommand.MIC, mic = on)) }
        } else {
            if (!on) {
                // An explicit mute wins over the unmute a nudge's Join still has pending.
                micOnJoin = false
                micAfterSwitch = false
                mutedGeneration = joinGeneration
            }
            core.setMic(on)
        }
    }

    fun leaveCall() {
        joinGeneration++
        micOnJoin = false
        operation?.cancel()
        // A move ends the phone side only once it has asked the phone to join; before that the phone call isn't ours.
        if (movePhoneJoinSent || shownCall.value.onPhone || _callsOnPhone.value) {
            scope.launch { phone(WatchLink.CallCommand(WatchLink.CallCommand.LEAVE)) }
        }
        core.leaveCall()
    }

    /** Joining the destination before leaving the source keeps the source call if the move fails. */
    fun moveCall() {
        if (_busy.value != null) return
        scope.launch { busyWhile(Busy.MOVING) { move() } }
    }

    private suspend fun move(): Boolean {
        val shown = shownCall.value
        val room = shown.state
        val floor = room.floorId
        val zone = room.zoneId
        if (!room.movable || floor == null || zone == null) {
            _notices.tryEmit("This call can't be moved")
            return false
        }
        // The destination joins muted and takes the mic once the source has left, so both never transmit.
        moveMic = room.micOn
        if (shown.onPhone) {
            if (!joinRoomHere(floor, zone, mic = false, warn = false)) return false
            if (!arrived { val v = core.call(); if (!v.phase.active()) false else if (v.phase == CallPhase.Joined && localZone() == zone) true else null }) {
                core.leaveCall()
                _notices.tryEmit("Couldn't join on the watch. The call stays on the phone.")
                return false
            }
            if (phone(WatchLink.CallCommand(WatchLink.CallCommand.LEAVE))) {
                if (moveMic) core.setMic(true)
            } else {
                _notices.tryEmit("Leave the call on the phone too")
            }
            setCallsOnPhone(false)
        } else {
            // Only to an idle phone: any call it then has is this move's, so a failed move may end it, and a
            // joined phone call can't carry a live mic into the room.
            if (!phone(WatchLink.CallCommand(WatchLink.CallCommand.STATE))) {
                _notices.tryEmit("Phone not reachable. The call stays on the watch.")
                return false
            }
            if (_phoneCall.value.active) {
                _notices.tryEmit("The phone is already in a call")
                return false
            }
            try {
                movePhoneJoinSent = true
                // The reply comes after the phone ran the join; then it must reach the room's media.
                val ok = phone(WatchLink.CallCommand(WatchLink.CallCommand.JOIN_ROOM, floorId = floor, zoneId = zone, mic = false)) &&
                    arrived {
                        val p = _phoneCall.value
                        when {
                            p.joined && p.zoneId == zone -> true
                            p.error != null || p.phase == WatchLink.CallState.FAILED -> false
                            else -> null
                        }
                    }
                if (!ok) {
                    phone(WatchLink.CallCommand(WatchLink.CallCommand.LEAVE))
                    _notices.tryEmit("Couldn't join on the phone. The call stays on the watch.")
                    return false
                }
            } finally {
                movePhoneJoinSent = false
            }
            core.leaveCall()
            if (moveMic) phone(WatchLink.CallCommand(WatchLink.CallCommand.MIC, mic = true))
            setCallsOnPhone(true)
        }
        return true
    }

    private fun localZone(): String? = runCatching { core.spaceView().selfZoneId }.getOrNull()

    private suspend fun arrived(check: suspend () -> Boolean?): Boolean =
        withTimeoutOrNull(MOVE_TIMEOUT_MS) {
            while (true) {
                check()?.let { return@withTimeoutOrNull it }
                delay(300)
            }
            @Suppress("UNREACHABLE_CODE") false
        } ?: false

    private suspend fun busyWhile(kind: Busy, block: suspend () -> Boolean): Boolean {
        if (_busy.value != null) {
            _notices.tryEmit(if (_busy.value == Busy.MOVING) "Moving the call…" else "Already joining…")
            return false
        }
        _busy.value = kind
        operation = kotlin.coroutines.coroutineContext[Job]
        return try {
            block()
        } catch (e: CancellationException) {
            false
        } finally {
            _busy.value = null
            operation = null
        }
    }

    fun personName(uid: String): String? = core.personName(uid)

    /** [onSending] runs once the core has the message: from then on it may reach the chat even if this fails. */
    suspend fun sendWhenOnline(channelUrl: String, text: String, onSending: () -> Unit = {}) {
        if (!signedIn) error("Not signed in")
        sending++
        try {
            core.start()
            withTimeout(6_000) { connection.first { it == ConnectionStatus.ONLINE } }
            onSending()
            core.sendMessage(channelUrl, text, emptyList(), null)
        } finally {
            sending--
            stopIfIdle()
        }
    }

    private companion object {
        const val KEY_CALLS_ON_PHONE = "calls_on_phone"
        const val MOVE_TIMEOUT_MS = 20_000L
    }
}
