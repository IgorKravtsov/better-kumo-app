package com.kravtsov.kumowatch.wear

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

class ReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val url = intent.getStringExtra(EXTRA_CHANNEL_URL) ?: return
        val id = intent.getIntExtra(EXTRA_NOTIFICATION_ID, 0)
        val token = intent.getLongExtra(EXTRA_TOKEN, 0)
        val text = (RemoteInput.getResultsFromIntent(intent)?.getCharSequence(KEY_TEXT)?.toString()
            ?: intent.getStringExtra(EXTRA_TEXT))?.trim()
        if (text.isNullOrEmpty()) return
        val pending = goAsync()
        val state = context.wearState
        state.scope.launch {
            var handedOver = false
            // goAsync gives a broadcast about 10 s.
            val sent = withTimeoutOrNull(9_000) {
                runCatching { state.sendWhenOnline(url, text) { handedOver = true } }.isSuccess
            } == true
            val manager = context.getSystemService(NotificationManager::class.java)
            // Checked and acted on under the lock PhoneAlertService posts with: a newer message in this chat may
            // replace the alert that was answered at any moment.
            synchronized(ALERTS) {
                val current = manager.activeNotifications.any { it.id == id && it.notification.extras.getLong(EXTRA_TOKEN) == token }
                if (sent) {
                    if (current) manager.cancel(id)
                } else {
                    val shownId = if (current) id else id + 3
                    manager.notify(shownId, failure(context, url, text, shownId, handedOver))
                }
            }
        }.invokeOnCompletion { pending.finish() }
    }

    private fun failure(context: Context, url: String, text: String, shownId: Int, handedOver: Boolean): Notification {
        val token = System.nanoTime()
        val failed = Notification.Builder(context, WearApplication.CHANNEL_ALERTS)
            .setSmallIcon(android.R.drawable.stat_notify_error)
            .setAutoCancel(true)
            .addExtras(Bundle().apply { putLong(EXTRA_TOKEN, token) })
            .setContentIntent(MainActivity.intent(context, shownId, channelUrl = url))
        if (handedOver) {
            // The core may still deliver it, so sending it again could post it twice.
            return failed.setContentTitle("Reply not confirmed").setContentText("Open the chat to check “$text”").build()
        }
        val retry = PendingIntent.getBroadcast(
            context, shownId,
            Intent(context, ReplyReceiver::class.java)
                // Its own identity, so another failed reply's PendingIntent can't replace this text.
                .setData(Uri.parse("kumo-reply:$token"))
                .putExtra(EXTRA_CHANNEL_URL, url)
                .putExtra(EXTRA_TEXT, text)
                .putExtra(EXTRA_NOTIFICATION_ID, shownId)
                .putExtra(EXTRA_TOKEN, token),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return failed.setContentTitle("Reply not sent").setContentText("“$text”")
            .addAction(Notification.Action.Builder(null, "Send again", retry).build())
            .build()
    }

    companion object {
        const val KEY_TEXT = "reply"
        const val EXTRA_CHANNEL_URL = "channel_url"
        const val EXTRA_NOTIFICATION_ID = "notification_id"
        const val EXTRA_TOKEN = "token"
        private const val EXTRA_TEXT = "text"
    }
}
