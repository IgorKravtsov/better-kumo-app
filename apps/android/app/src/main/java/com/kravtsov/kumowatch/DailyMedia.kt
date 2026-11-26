package com.kravtsov.kumowatch

import android.content.Context
import android.os.Handler
import android.os.Looper
import co.daily.CallClient
import co.daily.CallClientListener
import co.daily.model.AudioDeviceType
import co.daily.model.AvailableDevices
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
import uniffi.kumo_ffi.CallAudioLevel
import uniffi.kumo_ffi.CallMedia
import uniffi.kumo_ffi.CallMediaParticipant
import uniffi.kumo_ffi.CallMediaSink

data class AudioRoute(val type: AudioDeviceType, val id: String, val label: String)

data class CallTracks(val camera: MediaStreamTrack?, val screen: MediaStreamTrack?)

class DailyMedia(private val context: Context) : CallMedia {
    private val main = Handler(Looper.getMainLooper())
    private var client: CallClient? = null
    private var attempt: ULong = 0u
    private var sink: CallMediaSink? = null
    private val _tracks = MutableStateFlow<Map<String, CallTracks>>(emptyMap())
    val tracks: StateFlow<Map<String, CallTracks>> = _tracks.asStateFlow()
    private val _audioRoutes = MutableStateFlow<List<AudioRoute>>(emptyList())
    val audioRoutes: StateFlow<List<AudioRoute>> = _audioRoutes.asStateFlow()
    private val _audioRoute = MutableStateFlow<AudioDeviceType?>(null)
    val audioRoute: StateFlow<AudioDeviceType?> = _audioRoute.asStateFlow()
    private var chosenRoute: AudioDeviceType? = null
    private var desiredRoute: AudioDeviceType? = null
    private var routingClient: CallClient? = null

    override fun join(attempt: ULong, url: String, token: String, mic: Boolean, camera: Boolean, sink: CallMediaSink) = onMain {
        release()
        val c = CallClient(context)
        val listener = Listener(c)
        c.addListener(listener)
        client = c
        this.attempt = attempt
        this.sink = sink
        c.updateSubscriptionProfiles(
            mapOf(SubscriptionProfile("base") to media(Unsubscribed())),
        ) {}
        val inputs = InputSettingsUpdate(
            camera = CameraInputSettingsUpdate(isEnabled = if (camera) Enable() else Disable()),
            microphone = MicrophoneInputSettingsUpdate(isEnabled = if (mic) Enable() else Disable()),
        )
        c.join(url, MeetingToken(token), ClientSettingsUpdate(inputSettings = inputs)) { result ->
            if (result.isError) {
                sink.joinFailed("join", result.error?.msg.orEmpty())
            } else {
                if (c === client) routeAudio()
                c.startRemoteParticipantsAudioLevelObserver(200L) {}
                c.startLocalAudioLevelObserver(200L) {}
                sink.joined()
                if (c === client) publish()
            }
        }
    }

    override fun leave(attempt: ULong) = onMain {
        if (attempt == this.attempt) release()
    }

    override fun setInputs(attempt: ULong, mic: Boolean, camera: Boolean) = onMain {
        if (attempt == this.attempt) client?.setInputsEnabled(camera, mic) {}
    }

    override fun subscribe(attempt: ULong, participantIds: List<String>) = onMain {
        val c = client ?: return@onMain
        if (attempt != this.attempt) return@onMain
        val wanted = participantIds.toSet()
        val updates = c.participants().all.values.filter { !it.info.isLocal }.associate { p ->
            val state: co.daily.settings.Update<SubscriptionState> =
                if (p.id.uuid.toString() in wanted) Subscribed() else Unsubscribed()
            p.id to SubscriptionSettingsUpdate(media = media(state))
        }
        if (updates.isNotEmpty()) c.updateSubscriptionsForParticipants(updates) {}
    }

    fun refreshAudioRoutes() = onMain { routeAudio() }

    fun setAudioRoute(type: AudioDeviceType) = onMain {
        chosenRoute = type
        routeAudio()
    }

    private fun routeAudio(devices: AvailableDevices? = null) {
        val c = client ?: return
        val available = devices ?: c.availableDevices()
        val routes = AudioDeviceType.entries.mapNotNull { t ->
            available.findAudioDevice(t)?.let { AudioRoute(t, it.deviceId, it.label) }
        }
        val previous = _audioRoutes.value.map { it.type }.toSet()
        // A headset that connects mid-call takes over, as it does for a phone call.
        val fresh = listOf(AudioDeviceType.WIRED, AudioDeviceType.BLUETOOTH).firstOrNull { t -> t !in previous && routes.any { it.type == t } }
        if (previous.isNotEmpty() && fresh != null) chosenRoute = fresh
        val types = routes.map { it.type }.toSet()
        if (chosenRoute !in types) chosenRoute = null
        _audioRoutes.value = routes
        desiredRoute = chosenRoute ?: autoRoute()
        applyRoute()
    }

    private fun autoRoute(): AudioDeviceType {
        val types = _audioRoutes.value.map { it.type }.toSet()
        return listOf(AudioDeviceType.WIRED, AudioDeviceType.BLUETOOTH, AudioDeviceType.SPEAKERPHONE).firstOrNull { it in types }
            ?: AudioDeviceType.SPEAKERPHONE
    }

    private fun applyRoute() {
        val c = client ?: return
        if (routingClient === c) return
        routingClient = c
        stepRoute(c, null)
    }

    private fun stepRoute(c: CallClient, applied: AudioDeviceType?) {
        val wanted = desiredRoute
        val id = _audioRoutes.value.firstOrNull { it.type == wanted }?.id
            ?: if (wanted == AudioDeviceType.SPEAKERPHONE) "SPEAKERPHONE" else null
        if (c !== client || wanted == null || wanted == applied || id == null || c.audioDevice() == id) {
            if (routingClient === c) routingClient = null
            if (c === client) publishRoute(c)
            return
        }
        c.setAudioDevice(id) { result ->
            onMain {
                when {
                    c !== client -> if (routingClient === c) routingClient = null
                    result.isError && desiredRoute == wanted && chosenRoute == wanted -> {
                        chosenRoute = null
                        desiredRoute = autoRoute()
                        stepRoute(c, wanted)
                    }
                    result.isError && desiredRoute == wanted -> {
                        if (routingClient === c) routingClient = null
                        publishRoute(c)
                    }
                    else -> stepRoute(c, wanted)
                }
            }
        }
    }

    private fun publishRoute(c: CallClient) {
        val id = c.audioDevice()
        _audioRoute.value = _audioRoutes.value.firstOrNull { it.id == id }?.type
    }

    private fun media(state: co.daily.settings.Update<SubscriptionState>) = MediaSubscriptionSettingsUpdate(
        microphone = AudioSubscriptionSettingsUpdate(subscriptionState = state),
        camera = VideoSubscriptionSettingsUpdate(subscriptionState = state),
        screenAudio = AudioSubscriptionSettingsUpdate(subscriptionState = state),
        screenVideo = VideoSubscriptionSettingsUpdate(subscriptionState = state),
    )

    private fun release() {
        client?.let { c ->
            c.leave {}
            c.release()
        }
        client = null
        sink = null
        _tracks.value = emptyMap()
        _audioRoutes.value = emptyList()
        _audioRoute.value = null
        chosenRoute = null
        desiredRoute = null
        routingClient = null
    }

    private fun publish() {
        val c = client ?: return
        val all = c.participants().all.values
        _tracks.value = all.associate { p ->
            p.id.uuid.toString() to CallTracks(
                camera = p.media?.camera?.takeIf { it.state == MediaState.playable }?.track,
                screen = p.media?.screenVideo?.takeIf { it.state == MediaState.playable }?.track,
            )
        }
        sink?.participants(
            all.map { p ->
                CallMediaParticipant(
                    id = p.id.uuid.toString(),
                    userId = p.info.userId,
                    userName = p.info.userName,
                    isLocal = p.info.isLocal,
                    micOn = p.media?.microphone?.state == MediaState.playable,
                    hasCamera = p.media?.camera?.state == MediaState.playable,
                    hasScreen = p.media?.screenVideo?.state == MediaState.playable,
                )
            },
        )
    }

    private fun onMain(work: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) work() else main.post(work)
    }

    private inner class Listener(private val owner: CallClient) : CallClientListener {
        private val current: CallMediaSink? get() = if (owner === client) sink else null

        override fun onCallStateUpdated(state: CallState) {
            if (state == CallState.left) current?.left()
        }

        override fun onParticipantJoined(participant: Participant) {
            if (current != null) publish()
        }

        override fun onParticipantUpdated(participant: Participant) {
            if (current != null) publish()
        }

        override fun onParticipantLeft(participant: Participant, reason: ParticipantLeftReason) {
            if (current != null) publish()
        }

        override fun onRemoteParticipantsAudioLevel(participantsAudioLevel: Map<ParticipantId, Float>) {
            current?.audioLevels(participantsAudioLevel.map { CallAudioLevel(it.key.uuid.toString(), it.value) })
        }

        override fun onLocalAudioLevel(audioLevel: Float) {
            val local = owner.participants().local.id.uuid.toString()
            current?.audioLevels(listOf(CallAudioLevel(local, audioLevel)))
        }

        override fun onAvailableDevicesUpdated(availableDevices: AvailableDevices) {
            onMain { if (current != null) routeAudio(availableDevices) }
        }

        override fun onError(message: String) {
            current?.error(message)
        }
    }
}
