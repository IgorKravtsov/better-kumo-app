package com.kravtsov.kumowatch

import android.app.Notification
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import uniffi.kumo_ffi.AlertItem
import uniffi.kumo_ffi.AlertKind

class RelayService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        Relay.ensureChannels(this)
        startForeground(1, Relay.serviceNotification(this), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        val state = (application as KumoApplication).state
        val enabled = runCatching { state.core.setRelayEnabled(true) }.isSuccess
        if (!enabled) {
            stopSelf()
            return
        }
        scope.launch {
            // Each alert may wait for the watch: one in flight per notification, and only the newest waits behind it.
            val pending = mutableMapOf<Int, AlertItem>()
            val showing = mutableSetOf<Int>()
            state.alerts.collect { alert ->
                val id = (alert.channelUrl ?: alert.title).hashCode()
                pending[id] = alert
                if (showing.add(id)) {
                    launch {
                        try {
                            while (true) show(pending.remove(id) ?: break, id)
                        } finally {
                            showing.remove(id)
                        }
                    }
                }
            }
        }
    }

    private suspend fun show(alert: AlertItem, id: Int) {
        val account = runCatching { (application as KumoApplication).state.core.currentUser()?.uid }.getOrNull()
        val onWatch = account != null && WatchAlerts.forward(this@RelayService, alert, account)
        val manager = getSystemService(NotificationManager::class.java)
        val notification = Notification.Builder(this@RelayService, Relay.CHANNEL_ALERTS)
            .setSmallIcon(
                if (alert.kind == AlertKind.NUDGE || alert.kind == AlertKind.KNOCK) {
                    android.R.drawable.ic_lock_idle_alarm
                } else {
                    android.R.drawable.sym_action_chat
                },
            )
            .setContentTitle(alert.title)
            .setContentText(alert.body)
            .setStyle(Notification.BigTextStyle().bigText(alert.body))
            .setAutoCancel(true)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setContentIntent(Relay.openAppIntent(this@RelayService, alert.channelUrl, id))
            .setLocalOnly(onWatch)
            .apply {
                val person = alert.personId
                if (alert.kind == AlertKind.NUDGE && person != null) {
                    setContentIntent(Relay.personIntent(this@RelayService, person, null, id))
                    addAction(
                        Notification.Action.Builder(
                            null,
                            "Go to them on desktop",
                            Relay.personIntent(this@RelayService, person, "desktop", id + 1),
                        ).build(),
                    )
                    addAction(
                        Notification.Action.Builder(
                            null,
                            "Join them on phone",
                            Relay.personIntent(this@RelayService, person, "phone", id + 2),
                        ).build(),
                    )
                }
            }
            .build()
        manager.notify(id, notification)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        scope.cancel()
        runCatching {
            (application as KumoApplication).state.core.setRelayEnabled(false)
        }
        super.onDestroy()
    }
}
