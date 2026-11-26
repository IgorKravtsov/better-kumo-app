package com.kravtsov.kumowatch

import android.app.Notification
import android.app.PendingIntent
import android.app.Person
import android.content.BroadcastReceiver
import android.content.Context
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder

class CallService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    private fun promote() {
        Relay.ensureChannels(this)
        val state = (application as KumoApplication).state
        val builder = Notification.Builder(this, Relay.CHANNEL_SERVICE)
            .setSmallIcon(android.R.drawable.stat_sys_phone_call)
            .setContentTitle("In a Kumospace room")
            .setContentText("Tap to return to the call")
            .setOngoing(true)
            .setContentIntent(Relay.openAppIntent(this, null, 2))
        // Core-Telecom expects a CallStyle notification for its calls.
        if (state.telecom.active && android.os.Build.VERSION.SDK_INT >= 31) {
            val hangUp = PendingIntent.getBroadcast(
                this, 0, Intent(this, HangUpReceiver::class.java), PendingIntent.FLAG_IMMUTABLE,
            )
            builder.setStyle(Notification.CallStyle.forOngoingCall(Person.Builder().setName("Kumospace").build(), hangUp))
                .setCategory(Notification.CATEGORY_CALL)
        }
        val notification = builder.build()
        if (android.os.Build.VERSION.SDK_INT < 29) {
            startForeground(2, notification)
            promoted = true
            if (!state.onCallServicePromoted()) stopSelf()
            return
        }
        val micAllowed = checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
            android.content.pm.PackageManager.PERMISSION_GRANTED
        val type = when {
            !micAllowed -> ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            state.telecom.active ->
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL
            else -> ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
        }
        runCatching { startForeground(2, notification, type) }
            .onFailure {
                // A refused re-promotion keeps the running service and its working call.
                if (promoted) return@onFailure
                startForeground(2, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
                if (micAllowed) state.microphoneBlocked()
            }
        promoted = true
        if (!state.onCallServicePromoted()) stopSelf()
    }

    private var promoted = false

    override fun onDestroy() {
        (application as KumoApplication).state.onCallServiceStopped()
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        promote()
        return START_NOT_STICKY
    }
}

class HangUpReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        (context.applicationContext as KumoApplication).state.leaveCall()
    }
}
