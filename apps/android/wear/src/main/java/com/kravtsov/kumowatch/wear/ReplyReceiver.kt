package com.kravtsov.kumowatch.wear

import android.app.Notification
import android.app.NotificationManager
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

class ReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val url = intent.getStringExtra(EXTRA_CHANNEL_URL) ?: return
        val id = intent.getIntExtra(EXTRA_NOTIFICATION_ID, 0)
        val text = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(KEY_TEXT)?.toString()?.trim()
        if (text.isNullOrEmpty()) return
        val pending = goAsync()
        val state = context.wearState
        state.scope.launch {
            // goAsync gives a broadcast about 10 s.
            val sent = withTimeoutOrNull(9_000) { runCatching { state.sendWhenOnline(url, text) }.isSuccess } == true
            val manager = context.getSystemService(NotificationManager::class.java)
            if (sent) {
                manager.cancel(id)
            } else {
                manager.notify(
                    id,
                    Notification.Builder(context, WearApplication.CHANNEL_ALERTS)
                        .setSmallIcon(android.R.drawable.stat_notify_error)
                        .setContentTitle("Reply not sent")
                        .setContentText("Open Better Kumo to send “$text”")
                        .setAutoCancel(true)
                        .setContentIntent(MainActivity.intent(context, id, channelUrl = url))
                        .build(),
                )
            }
            pending.finish()
        }
    }

    companion object {
        const val KEY_TEXT = "reply"
        const val EXTRA_CHANNEL_URL = "channel_url"
        const val EXTRA_NOTIFICATION_ID = "notification_id"
    }
}
