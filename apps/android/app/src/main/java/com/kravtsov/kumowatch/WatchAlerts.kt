package com.kravtsov.kumowatch

import android.content.Context
import com.google.android.gms.wearable.CapabilityClient
import com.google.android.gms.wearable.Wearable
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.joinAll
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.kumo_ffi.AlertItem

object WatchAlerts {
    private var watches: Pair<Long, Set<String>>? = null

    /** Bounded: the phone's own notification waits for it. */
    /** [account]: the phone's Kumospace uid; a watch signed in to another account doesn't show it. */
    suspend fun forward(context: Context, alert: AlertItem, account: String): Boolean = withTimeoutOrNull(1_500) {
        val nodes = reachableWatches(context)
        if (nodes.isEmpty()) return@withTimeoutOrNull false
        val bytes = WatchLink.Alert(alert.kind.name, alert.title, alert.body, alert.channelUrl, alert.personId, account).toBytes()
        val messages = Wearable.getMessageClient(context)
        val shown = CompletableDeferred<Boolean>()
        coroutineScope {
            val sends = nodes.map { node ->
                launch {
                    val reply = runCatching { messages.sendRequest(node, WatchLink.PATH_ALERT, bytes).await() }.getOrNull()
                    if (reply?.contentEquals(WatchLink.ALERT_SHOWN) == true) shown.complete(true)
                }
            }
            // The first watch that shows it is enough.
            launch {
                sends.joinAll()
                shown.complete(false)
            }
            shown.await().also { coroutineContext.cancelChildren() }
        }
    } ?: false

    private suspend fun reachableWatches(context: Context): Set<String> {
        val now = System.currentTimeMillis()
        watches?.let { (at, nodes) -> if (now - at < if (nodes.isEmpty()) 10_000 else 60_000) return nodes }
        val nodes = try {
            Wearable.getCapabilityClient(context)
                .getCapability(WatchLink.CAPABILITY_WATCH, CapabilityClient.FILTER_REACHABLE).await()
                .nodes.map { it.id }.toSet()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // A failed lookup isn't "no watches": look again next time.
            return emptySet()
        }
        watches = now to nodes
        return nodes
    }
}
