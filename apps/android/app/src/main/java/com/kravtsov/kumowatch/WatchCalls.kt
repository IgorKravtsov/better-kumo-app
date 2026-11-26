package com.kravtsov.kumowatch

import android.content.Context
import android.content.pm.PackageManager
import com.google.android.gms.wearable.CapabilityClient
import com.google.android.gms.wearable.Wearable
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext

/** Runs on the main thread, like [AppState]. */
class WatchCalls(private val context: Context, private val state: AppState) {
    /** A later phone process always has a larger epoch, so the watch can drop a delayed message from an older one. */
    private val epoch = context.getSharedPreferences("watch_calls", Context.MODE_PRIVATE).let { prefs ->
        // Kept increasing even if the clock went back.
        maxOf(System.currentTimeMillis(), prefs.getLong("epoch", 0) + 1).also { prefs.edit().putLong("epoch", it).apply() }
    }
    private var seq = 0L
    private var joinGeneration = 0
    private var leaveGeneration = -1
    private var lookup: Deferred<String?>? = null
    private var lastSent: WatchLink.CallState? = null
    private var publishing: Job? = null
    private var dirty = false
    private var leaseUntil = 0L
    private var watches: Pair<Long, Set<String>>? = null
    private var error: Pair<Long, String>? = null

    suspend fun handle(nodeId: String, command: WatchLink.CallCommand): WatchLink.CallState {
        // The watch must be signed in to the phone's account; no state of this one goes to another.
        val account = account()
        if (account == null || command.account != account) {
            return WatchLink.CallState.NONE.copy(
                error = if (account == null) "Sign in to Better Kumo on the phone" else WatchLink.ACCOUNT_MISMATCH,
                epoch = epoch,
                seq = ++seq,
            )
        }
        leaseUntil = System.currentTimeMillis() + LEASE_MS
        // The reply shows the watch a state the push dedupe never saw.
        lastSent = null
        watches = watches?.let { (at, nodes) -> at to nodes + nodeId }
        when (command.action) {
            WatchLink.CallCommand.JOIN_ROOM, WatchLink.CallCommand.JOIN_PERSON -> {
                invalidateJoin()
                val generation = joinGeneration
                val failure = join(command, generation)
                if (generation != joinGeneration) return snapshot(null)
                if (failure != null) {
                    state.watchJoinFailed()
                    return snapshot(failure)
                }
                if (command.mic) state.turnMicOnAfterJoin()
            }
            WatchLink.CallCommand.MIC -> state.setMic(command.mic)
            WatchLink.CallCommand.LEAVE -> state.leaveCall()
            WatchLink.CallCommand.STATE -> Unit
            // A newer watch app: answering with plain state would read as success.
            else -> return snapshot("Update Better Kumo on the phone")
        }
        return snapshot(null)
    }

    private suspend fun join(command: WatchLink.CallCommand, generation: Int): String? {
        val floor = command.floorId
        val zone = command.zoneId
        val person = command.personId
        if (command.action == WatchLink.CallCommand.JOIN_ROOM && (floor == null || zone == null)) return "No room to join"
        if (command.action == WatchLink.CallCommand.JOIN_PERSON && person == null) return "Nobody to join"
        state.beginWatchJoin()
        if (context.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            return "Allow the microphone for Kumospace on the phone, or switch calls to the watch"
        }
        // Without the system call a locked phone gets no microphone; an unlocked one can still join.
        if (!state.telecom.start("Kumospace") && !state.inForeground()) {
            return "Unlock the phone to use its microphone, or switch calls to the watch"
        }
        if (generation != joinGeneration) return null
        if (person == null) return state.joinRoomOrError(floor!!, zone!!)
        // Cancelling the lookup drops the core's retry loop, so a superseded lookup can't move a newer call.
        val attempt = state.scope.async { state.joinPersonSoonOrError(person) }
        lookup = attempt
        val failure = try {
            attempt.await()
        } catch (e: CancellationException) {
            if (generation == joinGeneration) throw e
            null
        } finally {
            if (lookup === attempt) lookup = null
        }
        if (failure == null && generation != joinGeneration && leaveGeneration == joinGeneration) state.leaveCall()
        return failure
    }

    fun invalidateJoin(leave: Boolean = false) {
        joinGeneration++
        if (leave) leaveGeneration = joinGeneration
        lookup?.cancel()
        lookup = null
    }

    fun publish(fromWatch: Boolean) {
        if (!fromWatch && System.currentTimeMillis() > leaseUntil) {
            lastSent = null
            return
        }
        dirty = true
        if (publishing?.isActive == true) return
        publishing = state.scope.launch {
            while (dirty) {
                dirty = false
                delay(300)
                val nodes = reachableWatches()
                if (nodes.isEmpty()) break
                val next = snapshot(takeError())
                if (!changed(next, leased = System.currentTimeMillis() <= leaseUntil)) continue
                val bytes = next.toBytes()
                val messages = Wearable.getMessageClient(context)
                val sent = nodes.map { node ->
                    runCatching { messages.sendMessage(node, WatchLink.PATH_CALL_STATE, bytes).await() }.isSuccess
                }.all { it }
                // A watch that missed it gets it again with the next change.
                lastSent = next.takeIf { sent }
            }
        }
    }

    /** Without a lease (watch app closed) who is speaking doesn't matter, only the call itself. */
    private fun changed(next: WatchLink.CallState, leased: Boolean): Boolean {
        val last = lastSent ?: return true
        fun WatchLink.CallState.key() = copy(
            seq = 0,
            people = if (leased) people else people.map { it.copy(speaking = false) },
        )
        return next.error != null || next.key() != last.copy(error = null).key()
    }

    fun report(message: String) {
        error = System.currentTimeMillis() to message
        publish(fromWatch = true)
    }

    fun failure(message: String): WatchLink.CallState =
        state.core.call().toWatchState(state.spaceView.value, message).copy(epoch = epoch, seq = ++seq, account = account())

    private fun account(): String? = runCatching { state.core.currentUser()?.uid }.getOrNull()

    private fun takeError(): String? {
        val (at, message) = error ?: return null
        error = null
        return message.takeIf { System.currentTimeMillis() - at < 30_000 }
    }

    private suspend fun snapshot(error: String?): WatchLink.CallState {
        // Fresh: the cached census can still show the room before a move.
        val space = withContext(Dispatchers.IO) { runCatching { state.core.spaceView() }.getOrNull() } ?: state.spaceView.value
        return state.core.call().toWatchState(space, error).copy(epoch = epoch, seq = ++seq, account = account())
    }

    private suspend fun reachableWatches(): Set<String> {
        val now = System.currentTimeMillis()
        watches?.let { (at, nodes) -> if (now - at < 15_000) return nodes }
        val nodes = runCatching {
            Wearable.getCapabilityClient(context)
                .getCapability(WatchLink.CAPABILITY_WATCH, CapabilityClient.FILTER_REACHABLE).await()
                .nodes.map { it.id }.toSet()
        }.getOrDefault(emptySet())
        watches = now to nodes
        return nodes
    }

    private companion object {
        /** The watch renews it every 30 s while it shows a phone call or has calls set to the phone. */
        const val LEASE_MS = 75_000L
    }
}
