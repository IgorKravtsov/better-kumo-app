package com.kravtsov.kumowatch

import android.content.Context
import android.content.Intent
import android.util.Log
import co.daily.CallClient
import co.daily.CallClientListener
import co.daily.model.CallState
import co.daily.model.MediaState
import co.daily.model.MediaStreamTrack
import co.daily.model.MeetingToken
import co.daily.model.Participant
import co.daily.model.ParticipantId
import co.daily.model.ParticipantLeftReason
import co.daily.settings.CameraInputSettingsUpdate
import co.daily.settings.ClientSettingsUpdate
import co.daily.settings.Disable
import co.daily.settings.Enable
import co.daily.settings.InputSettingsUpdate
import co.daily.settings.MicrophoneInputSettingsUpdate
import co.daily.settings.subscription.AudioSubscriptionSettingsUpdate
import co.daily.settings.subscription.MediaSubscriptionSettingsUpdate
import co.daily.settings.subscription.SubscriptionProfile
import co.daily.settings.subscription.SubscriptionSettingsUpdate
import co.daily.settings.subscription.SubscriptionState
import co.daily.settings.subscription.Subscribed
import co.daily.settings.subscription.Unsubscribed
import co.daily.settings.subscription.VideoSubscriptionSettingsUpdate
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import uniffi.kumo_ffi.CallInfo

data class CallTile(
    val id: ParticipantId,
    val userId: String,
    val name: String,
    val isLocal: Boolean,
    val camera: MediaStreamTrack?,
    val screen: MediaStreamTrack?,
    val micOn: Boolean,
)

enum class CallPhase { Idle, Waiting, Joining, Joined, Failed }

data class CallUiState(
    val phase: CallPhase = CallPhase.Idle,
    val roomName: String = "",
    val tiles: List<CallTile> = emptyList(),
    val activeSpeaker: ParticipantId? = null,
    val speaking: Set<ParticipantId> = emptySet(),
    val micOn: Boolean = false,
    val cameraOn: Boolean = false,
    val error: String? = null,
)

class CallManager(private val context: Context, private val app: AppState) : CallClientListener {
    private var client: CallClient? = null
    private var joinedUrl: String? = null
    private var tokenRetries = 0
    private val _state = MutableStateFlow(CallUiState())
    val state: StateFlow<CallUiState> = _state.asStateFlow()

    fun expectCall() {
        if (_state.value.phase == CallPhase.Idle || _state.value.phase == CallPhase.Failed) {
            _state.value = CallUiState(phase = CallPhase.Waiting)
        }
        app.core.callInfo()?.let { onCallInfo(it) }
    }

    fun onCallInfo(info: CallInfo?) {
        if (info == null) {
            if (_state.value.phase == CallPhase.Joined || _state.value.phase == CallPhase.Joining) leaveClient()
            if (_state.value.phase != CallPhase.Idle) _state.value = _state.value.copy(phase = CallPhase.Waiting)
            return
        }
        _state.value = _state.value.copy(roomName = info.roomName)
        if (_state.value.phase == CallPhase.Idle) return
        if (joinedUrl == info.dailyUrl && client != null) {
            onPeersChanged()
            return
        }
        join(info)
    }

    private fun join(info: CallInfo) {
        leaveClient()
        val c = CallClient(context)
        client = c
        joinedUrl = info.dailyUrl
        c.addListener(this)
        _state.value = _state.value.copy(phase = CallPhase.Joining, error = null)
        context.startForegroundService(Intent(context, CallService::class.java))
        c.updateSubscriptionProfiles(
            mapOf(SubscriptionProfile("base") to MediaSubscriptionSettingsUpdate(
                microphone = AudioSubscriptionSettingsUpdate(subscriptionState = Unsubscribed()),
                camera = VideoSubscriptionSettingsUpdate(subscriptionState = Unsubscribed()),
                screenAudio = AudioSubscriptionSettingsUpdate(subscriptionState = Unsubscribed()),
                screenVideo = VideoSubscriptionSettingsUpdate(subscriptionState = Unsubscribed()),
            )),
        ) {}
        val inputs = InputSettingsUpdate(
            camera = CameraInputSettingsUpdate(isEnabled = if (_state.value.cameraOn) Enable() else Disable()),
            microphone = MicrophoneInputSettingsUpdate(isEnabled = if (_state.value.micOn) Enable() else Disable()),
        )
        c.join(info.dailyUrl, MeetingToken(info.dailyToken), ClientSettingsUpdate(inputSettings = inputs)) { result ->
            if (result.isError) {
                Log.w(TAG, "join failed: ${result.error?.msg}")
                leaveClient()
                if (tokenRetries < 3) {
                    tokenRetries++
                    _state.value = _state.value.copy(phase = CallPhase.Waiting)
                    app.core.refreshCallToken()
                } else {
                    _state.value = _state.value.copy(phase = CallPhase.Failed, error = result.error?.msg)
                }
            } else {
                tokenRetries = 0
                c.setAudioDevice("SPEAKERPHONE") {}
                c.startRemoteParticipantsAudioLevelObserver(200L) {}
                c.startLocalAudioLevelObserver(200L) {}
                _state.value = _state.value.copy(phase = CallPhase.Joined)
                logParticipants()
                onPeersChanged()
                refreshTiles()
            }
        }
    }

    fun leave() {
        leaveClient()
        _state.value = CallUiState()
    }

    private fun leaveClient() {
        client?.let { c ->
            c.leave {}
            c.release()
        }
        client = null
        joinedUrl = null
        context.stopService(Intent(context, CallService::class.java))
    }

    fun toggleMic() {
        val on = !_state.value.micOn
        _state.value = _state.value.copy(micOn = on)
        client?.setInputsEnabled(null, on) {}
        refreshTiles()
    }

    fun toggleCamera() {
        val on = !_state.value.cameraOn
        _state.value = _state.value.copy(cameraOn = on)
        client?.setInputsEnabled(on, null) {}
        refreshTiles()
    }

    fun onPeersChanged() {
        app.core.callInfo()?.roomName?.takeIf { it.isNotBlank() && it != _state.value.roomName }?.let {
            _state.value = _state.value.copy(roomName = it)
        }
        val c = client ?: return
        val peers = app.core.callPeers().toSet()
        val remotes = c.participants().all.values.filter { !it.info.isLocal }
        val updates = remotes.associate { p ->
            val wanted = p.info.userId in peers
            val state: co.daily.settings.Update<SubscriptionState> =
                if (wanted) Subscribed() else Unsubscribed()
            p.id to SubscriptionSettingsUpdate(
                media = MediaSubscriptionSettingsUpdate(
                    microphone = AudioSubscriptionSettingsUpdate(subscriptionState = state),
                    camera = VideoSubscriptionSettingsUpdate(subscriptionState = state),
                    screenAudio = AudioSubscriptionSettingsUpdate(subscriptionState = state),
                    screenVideo = VideoSubscriptionSettingsUpdate(subscriptionState = state),
                ),
            )
        }
        Log.i(TAG, "peers=${peers.size} subscribed=" + remotes.filter { it.info.userId in peers }.map { it.info.userName })
        if (updates.isNotEmpty()) c.updateSubscriptionsForParticipants(updates) {}
        refreshTiles()
    }


    private fun refreshTiles() {
        val c = client ?: return
        val peers = app.core.callPeers().toSet()
        val tiles = c.participants().all.values
            .filter { it.info.isLocal || it.info.userId in peers }
            .map { p ->
                val key = p.info.userId ?: ""
                CallTile(
                    id = p.id,
                    userId = key,
                    name = app.personByKey(key)?.name ?: p.info.userName ?: "Guest",
                    isLocal = p.info.isLocal,
                    camera = p.media?.camera?.takeIf { it.state == MediaState.playable }?.track,
                    screen = p.media?.screenVideo?.takeIf { it.state == MediaState.playable }?.track,
                    micOn = p.media?.microphone?.state == MediaState.playable,
                )
            }
            .sortedWith(compareBy({ !it.isLocal }, { it.name }))
        _state.value = _state.value.copy(tiles = tiles)
    }

    private fun logParticipants() {
        client?.participants()?.all?.values?.forEach {
            Log.i(TAG, "participant local=${it.info.isLocal} userId=${it.info.userId} userName=${it.info.userName}")
        }
    }

    override fun onCallStateUpdated(state: CallState) {
        if (state == CallState.left && _state.value.phase == CallPhase.Joined) {
            _state.value = _state.value.copy(phase = CallPhase.Waiting)
        }
    }

    override fun onParticipantJoined(participant: Participant) {
        Log.i(TAG, "joined userId=${participant.info.userId} userName=${participant.info.userName}")
        onPeersChanged()
    }

    override fun onParticipantUpdated(participant: Participant) = refreshTiles()

    override fun onParticipantLeft(participant: Participant, reason: ParticipantLeftReason) = refreshTiles()

    override fun onRemoteParticipantsAudioLevel(participantsAudioLevel: Map<ParticipantId, Float>) {
        val visible = _state.value.tiles.filter { !it.isLocal }.map { it.id }.toSet()
        val remote = participantsAudioLevel.filter { it.key in visible && it.value > SPEAKING_LEVEL }.keys
        updateSpeaking(remote, localSpeaking = _state.value.speaking.any { id -> _state.value.tiles.any { it.isLocal && it.id == id } })
    }

    override fun onLocalAudioLevel(audioLevel: Float) {
        val local = _state.value.tiles.firstOrNull { it.isLocal }?.id ?: return
        val remote = _state.value.speaking - local
        updateSpeaking(remote, localSpeaking = _state.value.micOn && audioLevel > SPEAKING_LEVEL)
    }

    private fun updateSpeaking(remote: Set<ParticipantId>, localSpeaking: Boolean) {
        val local = _state.value.tiles.firstOrNull { it.isLocal }?.id
        val speaking = if (localSpeaking && local != null) remote + local else remote
        if (speaking == _state.value.speaking) return
        val loudest = remote.firstOrNull() ?: _state.value.activeSpeaker
        _state.value = _state.value.copy(speaking = speaking, activeSpeaker = loudest)
    }

    override fun onError(message: String) {
        Log.w(TAG, "daily error: $message")
        _state.value = _state.value.copy(error = message)
    }

    private companion object {
        const val TAG = "KumoCall"
        const val SPEAKING_LEVEL = 0.02f
    }
}
