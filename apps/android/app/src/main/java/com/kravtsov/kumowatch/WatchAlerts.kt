package com.kravtsov.kumowatch

import android.content.Context
import com.google.android.gms.wearable.CapabilityClient
import com.google.android.gms.wearable.Wearable
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.kumo_ffi.AlertItem

object WatchAlerts {
    private var watches: Pair<Long, Set<String>>? = null

    /** Bounded: the phone's own notification waits for it. */
    suspend fun forward(context: Context, alert: AlertItem): Boolean = withTimeoutOrNull(1_500) {
        val nodes = reachableWatches(context)
        if (nodes.isEmpty()) return@withTimeoutOrNull false
        val bytes = WatchLink.Alert(alert.kind.name, alert.title, alert.body, alert.channelUrl, alert.personId).toBytes()
        val messages = Wearable.getMessageClient(context)
        coroutineScope {
            nodes.map { node ->
                async {
                    runCatching { messages.sendRequest(node, WatchLink.PATH_ALERT, bytes).await() }
                        .getOrNull()?.contentEquals(WatchLink.ALERT_SHOWN) == true
                }
            }.awaitAll().any { it }
        }
    } ?: false

    private suspend fun reachableWatches(context: Context): Set<String> {
        val now = System.currentTimeMillis()
        watches?.let { (at, nodes) -> if (now - at < 60_000) return nodes }
        val nodes = runCatching {
            Wearable.getCapabilityClient(context)
                .getCapability(WatchLink.CAPABILITY_WATCH, CapabilityClient.FILTER_REACHABLE).await()
                .nodes.map { it.id }.toSet()
        }.getOrDefault(emptySet())
        watches = now to nodes
        return nodes
    }
}
