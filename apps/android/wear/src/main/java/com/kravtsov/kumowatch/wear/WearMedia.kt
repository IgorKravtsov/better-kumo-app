package com.kravtsov.kumowatch.wear

import android.content.Context
import android.os.Handler
import android.os.Looper
import co.daily.CallClient
import co.daily.CallClientListener
import co.daily.model.CallState
import co.daily.model.MediaState
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
import uniffi.kumo_ffi.CallAudioLevel
import uniffi.kumo_ffi.CallMedia
import uniffi.kumo_ffi.CallMediaParticipant
import uniffi.kumo_ffi.CallMediaSink

/** Audio only: no camera and no video subscriptions, to keep the watch's bandwidth and battery for voice. */
class WearMedia(private val context: Context) : CallMedia {
    private val main = Handler(Looper.getMainLooper())
    private var client: CallClient? = null
    private var attempt: ULong = 0u
    private var sink: CallMediaSink? = null

    override fun join(attempt: ULong, url: String, token: String, mic: Boolean, camera: Boolean, sink: CallMediaSink) = onMain {
        release()
        val c = CallClient(context)
        c.addListener(Listener(c))
        client = c
        this.attempt = attempt
        this.sink = sink
        c.updateSubscriptionProfiles(mapOf(SubscriptionProfile("base") to media(Unsubscribed()))) {}
        val inputs = InputSettingsUpdate(
            camera = CameraInputSettingsUpdate(isEnabled = Disable()),
            microphone = MicrophoneInputSettingsUpdate(isEnabled = if (mic) Enable() else Disable()),
        )
        c.join(url, MeetingToken(token), ClientSettingsUpdate(inputSettings = inputs)) { result ->
            if (result.isError) {
                sink.joinFailed("join", result.error?.msg.orEmpty())
            } else {
                routeAudio(c)
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
        if (attempt == this.attempt) client?.setInputsEnabled(false, mic) {}
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

    private fun media(audio: co.daily.settings.Update<SubscriptionState>) = MediaSubscriptionSettingsUpdate(
        microphone = AudioSubscriptionSettingsUpdate(subscriptionState = audio),
        camera = VideoSubscriptionSettingsUpdate(subscriptionState = Unsubscribed()),
        screenAudio = AudioSubscriptionSettingsUpdate(subscriptionState = audio),
        screenVideo = VideoSubscriptionSettingsUpdate(subscriptionState = Unsubscribed()),
    )

    private fun routeAudio(c: CallClient) {
        val bluetooth = c.availableDevices().audio.firstOrNull { it.deviceId.contains("BLUETOOTH", ignoreCase = true) }
        c.setAudioDevice(bluetooth?.deviceId ?: "SPEAKERPHONE") {}
    }

    private fun release() {
        client?.let { c ->
            c.leave {}
            c.release()
        }
        client = null
        sink = null
    }

    private fun publish() {
        val c = client ?: return
        sink?.participants(
            c.participants().all.values.map { p ->
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

        override fun onAvailableDevicesUpdated(availableDevices: co.daily.model.AvailableDevices) {
            if (current != null) routeAudio(owner)
        }

        override fun onRemoteParticipantsAudioLevel(participantsAudioLevel: Map<ParticipantId, Float>) {
            current?.audioLevels(participantsAudioLevel.map { CallAudioLevel(it.key.uuid.toString(), it.value) })
        }

        override fun onLocalAudioLevel(audioLevel: Float) {
            val local = owner.participants().local.id.uuid.toString()
            current?.audioLevels(listOf(CallAudioLevel(local, audioLevel)))
        }

        override fun onError(message: String) {
            current?.error(message)
        }
    }
}
