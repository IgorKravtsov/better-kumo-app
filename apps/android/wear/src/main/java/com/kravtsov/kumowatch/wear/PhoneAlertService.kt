package com.kravtsov.kumowatch.wear

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import com.google.android.gms.tasks.Task
import com.google.android.gms.tasks.Tasks
import com.google.android.gms.wearable.MessageEvent
import com.google.android.gms.wearable.WearableListenerService
import com.kravtsov.kumowatch.WatchLink

class PhoneAlertService : WearableListenerService() {
    override fun onMessageReceived(event: MessageEvent) {
        if (event.path == WatchLink.PATH_CALL_STATE) WatchLink.CallState.fromBytes(event.data)?.let { wearState.onPhoneCall(it) }
    }

    override fun onRequest(nodeId: String, path: String, data: ByteArray): Task<ByteArray>? {
        if (path != WatchLink.PATH_ALERT) return null
        val alert = WatchLink.Alert.fromBytes(data)
        val shown = alert != null && wearState.signedIn && canNotify() && runCatching { show(alert!!) }.isSuccess
        return Tasks.forResult(if (shown) WatchLink.ALERT_SHOWN else ByteArray(0))
    }

    private fun canNotify(): Boolean {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        val manager = getSystemService(NotificationManager::class.java)
        return manager.areNotificationsEnabled() &&
            manager.getNotificationChannel(WearApplication.CHANNEL_ALERTS)?.importance != NotificationManager.IMPORTANCE_NONE
    }

    private fun show(alert: WatchLink.Alert) {
        // A nudge and a knock from one person are separate alerts, as on the phone.
        val id = (alert.channelUrl ?: "${alert.kind}:${alert.personId ?: alert.title}").hashCode()
        val person = alert.personId
        val nudge = alert.kind == "NUDGE"
        val token = System.currentTimeMillis()
        val builder = Notification.Builder(this, WearApplication.CHANNEL_ALERTS)
            .addExtras(Bundle().apply { putLong(ReplyReceiver.EXTRA_TOKEN, token) })
            .setSmallIcon(if (nudge) android.R.drawable.stat_sys_phone_call else android.R.drawable.sym_action_chat)
            .setContentTitle(alert.title)
            .setContentText(alert.body)
            .setStyle(Notification.BigTextStyle().bigText(alert.body))
            .setAutoCancel(true)
            .setCategory(if (nudge) Notification.CATEGORY_CALL else Notification.CATEGORY_MESSAGE)
            .setContentIntent(MainActivity.intent(this, id, channelUrl = alert.channelUrl))
        // Same as the phone: only a nudge offers Join; a knock comes from someone outside the room.
        if (nudge && person != null) {
            builder.addAction(
                Notification.Action.Builder(null, "Join", MainActivity.intent(this, id + 1, joinPerson = person, notificationId = id))
                    .build(),
            )
        }
        alert.channelUrl?.let { url ->
            val reply = PendingIntent.getBroadcast(
                this, id + 2,
                Intent(this, ReplyReceiver::class.java)
                    .putExtra(ReplyReceiver.EXTRA_CHANNEL_URL, url)
                    .putExtra(ReplyReceiver.EXTRA_NOTIFICATION_ID, id)
                    .putExtra(ReplyReceiver.EXTRA_TOKEN, token),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
            )
            val input = RemoteInput.Builder(ReplyReceiver.KEY_TEXT)
                .setLabel("Reply")
                .setChoices(QUICK_REPLIES.toTypedArray())
                .build()
            builder.addAction(Notification.Action.Builder(null, "Reply", reply).addRemoteInput(input).build())
        }
        getSystemService(NotificationManager::class.java).notify(id, builder.build())
    }
}

val QUICK_REPLIES = listOf("👍", "On my way", "Joining now", "Can't talk now", "Later")
