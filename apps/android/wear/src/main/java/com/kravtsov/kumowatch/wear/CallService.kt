package com.kravtsov.kumowatch.wear

import android.app.Notification
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder

class CallService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val open = MainActivity.intent(this, 0, showCall = true)
        val notification = Notification.Builder(this, WearApplication.CHANNEL_CALL)
            .setSmallIcon(android.R.drawable.stat_sys_phone_call)
            .setContentTitle("In a Kumospace room")
            .setContentText("Tap to return to the call")
            .setCategory(Notification.CATEGORY_CALL)
            .setOngoing(true)
            .setContentIntent(open)
            .build()
        runCatching { startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE) }
            .onSuccess { if (!wearState.onCallServicePromoted()) stopSelf() }
            .onFailure {
                // Stopping a service started with startForegroundService before it went foreground crashes the
                // app, and shortService needs no permission.
                if (android.os.Build.VERSION.SDK_INT >= 34) {
                    runCatching { startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SHORT_SERVICE) }
                }
                wearState.microphoneRefused()
                stopSelf()
            }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_NOT_STICKY

    override fun onDestroy() {
        wearState.onCallServiceStopped()
        super.onDestroy()
    }

    private companion object {
        const val ID = 2
    }
}
