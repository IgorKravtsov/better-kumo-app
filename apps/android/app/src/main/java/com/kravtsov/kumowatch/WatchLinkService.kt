package com.kravtsov.kumowatch

import com.google.android.gms.tasks.Task
import com.google.android.gms.tasks.TaskCompletionSource
import com.google.android.gms.wearable.WearableListenerService
import kotlinx.coroutines.launch

class WatchLinkService : WearableListenerService() {
    override fun onRequest(nodeId: String, path: String, data: ByteArray): Task<ByteArray>? {
        val state = (application as KumoApplication).state
        if (path == WatchLink.PATH_CALL_COMMAND) {
            val command = WatchLink.CallCommand.fromBytes(data) ?: return null
            val reply = TaskCompletionSource<ByteArray>()
            state.scope.launch {
                reply.setResult(
                    runCatching { state.watchCalls.handle(nodeId, command) }
                        .getOrElse { state.watchCalls.failure(it.message ?: "The phone could not do that") }
                        .toBytes(),
                )
            }
            return reply.task
        }
        if (path != WatchLink.PATH_SIGN_IN) return null
        val reply = TaskCompletionSource<ByteArray>()
        state.scope.launch {
            val space = state.space.value
            val link = if (state.user.value != null) state.transferLink().getOrNull() else null
            reply.setResult(
                link?.let {
                    WatchLink.SignIn(
                        it,
                        space?.let { s -> WatchLink.Space(s.name, s.uid, s.displayName, s.memberCount.toLong()) },
                    ).toBytes()
                } ?: ByteArray(0),
            )
        }
        return reply.task
    }
}
