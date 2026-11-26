package com.kravtsov.kumowatch.wear

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager

class WearApplication : Application() {
    lateinit var state: WearState
        private set

    override fun onCreate() {
        super.onCreate()
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ALERTS, "Messages and nudges", NotificationManager.IMPORTANCE_HIGH),
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_CALL, "Call in progress", NotificationManager.IMPORTANCE_LOW),
        )
        state = WearState(this)
    }

    companion object {
        const val CHANNEL_ALERTS = "kumo_alerts"
        const val CHANNEL_CALL = "kumo_call"
    }
}

val android.content.Context.wearState: WearState get() = (applicationContext as WearApplication).state
