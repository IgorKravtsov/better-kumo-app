package com.kravtsov.kumowatch

import android.app.Notification
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder

class CallService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        Relay.ensureChannels(this)
        val notification = Notification.Builder(this, Relay.CHANNEL_SERVICE)
            .setSmallIcon(android.R.drawable.stat_sys_phone_call)
            .setContentTitle("In a Kumospace room")
            .setContentText("Tap to return to the call")
            .setOngoing(true)
            .setContentIntent(Relay.openAppIntent(this, null, 2))
            .build()
        val type = if (checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
            android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
        } else {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        }
        runCatching { startForeground(2, notification, type) }
            .onFailure { startForeground(2, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE) }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_NOT_STICKY
}
