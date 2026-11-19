package com.kravtsov.kumowatch

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import uniffi.kumo_ffi.NotificationDelivery

object Relay {
    const val CHANNEL_ALERTS = "kumo_alerts"
    const val CHANNEL_SERVICE = "kumo_relay"
    const val EXTRA_CHANNEL_URL = "channel_url"
    private const val PREFS = "kumo_relay"
    private const val KEY_ENABLED = "enabled"

    fun isEnabled(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_ENABLED, false)

    fun setEnabled(context: Context, enabled: Boolean) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ENABLED, enabled).apply()
        applyService(context)
        (context.applicationContext as KumoApplication).state.onRelayChanged(enabled)
        RelayWidget.updateAll(context)
    }

    fun applyService(context: Context) {
        val state = (context.applicationContext as KumoApplication).state
        val intent = Intent(context, RelayService::class.java)
        if (isEnabled(context) && state.delivery.value == NotificationDelivery.PHONE) {
            runCatching { context.startForegroundService(intent) }
        } else {
            context.stopService(intent)
        }
    }

    fun ensureChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ALERTS, "Messages and nudges", NotificationManager.IMPORTANCE_HIGH),
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_SERVICE, "Mirroring status", NotificationManager.IMPORTANCE_MIN),
        )
    }

    fun openAppIntent(context: Context, channelUrl: String?, requestCode: Int): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        channelUrl?.let { intent.putExtra(EXTRA_CHANNEL_URL, it) }
        return PendingIntent.getActivity(
            context, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    fun personIntent(context: Context, personId: String, via: String?, requestCode: Int): PendingIntent {
        val link = android.net.Uri.Builder()
            .scheme("kumowatch")
            .authority("person")
            .appendQueryParameter("id", personId)
            .apply { via?.let { appendQueryParameter("via", it) } }
            .build()
        val intent = Intent(Intent.ACTION_VIEW, link, context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        return PendingIntent.getActivity(
            context, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    fun serviceNotification(context: Context): Notification =
        Notification.Builder(context, CHANNEL_SERVICE)
            .setSmallIcon(android.R.drawable.ic_popup_reminder)
            .setContentTitle("Mirroring Kumospace notifications")
            .setContentText("Messages, mentions and nudges arrive here")
            .setOngoing(true)
            .setContentIntent(openAppIntent(context, null, 0))
            .build()
}
