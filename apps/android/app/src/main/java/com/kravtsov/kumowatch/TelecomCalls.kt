package com.kravtsov.kumowatch

import android.content.Context
import android.net.Uri
import android.telecom.DisconnectCause
import androidx.core.telecom.CallAttributesCompat
import androidx.core.telecom.CallControlResult
import androidx.core.telecom.CallControlScope
import androidx.core.telecom.CallEndpointCompat
import androidx.core.telecom.CallsManager
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/** A locked phone may start the microphone only for an app with an ongoing system call (to be verified on a device). */
class TelecomCalls(
    private val context: Context,
    private val scope: CoroutineScope,
    private val onActive: () -> Unit,
    private val onHungUp: () -> Unit,
    private val onMuted: (Boolean) -> Unit,
) {
    private val calls by lazy {
        CallsManager(context).also { it.registerAppWithTelecom(CallsManager.CAPABILITY_BASELINE) }
    }
    private var job: Job? = null
    private var ready: CompletableDeferred<Boolean>? = null
    private var ended: CompletableDeferred<Unit>? = null

    var active = false
        private set

    /** Concurrent starts share one attempt. */
    suspend fun start(label: String): Boolean {
        if (active) return true
        ready?.let { return withTimeoutOrNull(START_TIMEOUT_MS) { it.await() } ?: false }
        val attemptReady = CompletableDeferred<Boolean>().also { ready = it }
        val attemptEnded = CompletableDeferred<Unit>().also { ended = it }
        job = scope.launch {
            try {
                calls.addCall(
                    // No hold capability: Daily can't pause, so another call (a SIM call) ends this one instead.
                    CallAttributesCompat(
                        label,
                        Uri.fromParts("sip", "kumospace", null),
                        CallAttributesCompat.DIRECTION_OUTGOING,
                        CallAttributesCompat.CALL_TYPE_AUDIO_CALL,
                        0,
                    ),
                    onAnswer = {},
                    onDisconnect = {
                        if (!attemptEnded.isCompleted) {
                            onHungUp()
                            end()
                        }
                    },
                    onSetActive = {},
                    onSetInactive = {
                        if (!attemptEnded.isCompleted) {
                            onHungUp()
                            end()
                        }
                    },
                ) {
                    launch { run(attemptReady, attemptEnded) }
                }
            } catch (e: Exception) {
                if (e is kotlinx.coroutines.CancellationException) throw e
            } finally {
                attemptReady.complete(false)
                if (ended === attemptEnded) {
                    val wasActive = active
                    active = false
                    ready = null
                    ended = null
                    // Telecom dropped the call without onDisconnect.
                    if (wasActive) onHungUp()
                }
            }
        }
        val ok = withTimeoutOrNull(START_TIMEOUT_MS) { attemptReady.await() } ?: false
        // A stale attempt must not end a newer call.
        if (!ok && ended === attemptEnded) end()
        return ok
    }

    private suspend fun CallControlScope.run(ready: CompletableDeferred<Boolean>, ended: CompletableDeferred<Unit>) {
        val ok = setActive() is CallControlResult.Success && !ended.isCompleted
        if (ok) {
            active = true
            onActive()
        }
        ready.complete(ok)
        if (ok) {
            // Headsets are left to Telecom (the watch also offers hands-free audio); only a pocketed earpiece is replaced.
            val routing = launch {
                if (currentCallEndpoint.first().type == CallEndpointCompat.TYPE_EARPIECE) {
                    availableEndpoints.first().firstOrNull { it.type == CallEndpointCompat.TYPE_SPEAKER }?.let { requestEndpointChange(it) }
                }
            }
            // A system mute (a headset button, the call notification) mutes Kumospace too.
            val muting = launch { isMuted.collect { onMuted(it) } }
            ended.await()
            routing.cancel()
            muting.cancel()
        }
        disconnect(DisconnectCause(DisconnectCause.LOCAL))
    }

    fun end() {
        val current = ended ?: return
        current.complete(Unit)
        ready?.complete(false)
        active = false
        ready = null
        ended = null
        val attempt = job
        scope.launch {
            // Telecom may never finish the disconnect.
            if (withTimeoutOrNull(5_000) { attempt?.join() } == null) attempt?.cancel()
        }
    }

    private companion object {
        const val START_TIMEOUT_MS = 5_000L
    }
}
