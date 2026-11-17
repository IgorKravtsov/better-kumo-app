package com.kravtsov.kumowatch

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.widget.RemoteViews
import kotlinx.coroutines.launch
import uniffi.kumo_ffi.NotificationDelivery
import uniffi.kumo_ffi.RelayStatus

private enum class WidgetLook(val text: String, val on: Boolean, val icon: Int) {
    PHONE_ON("Kumo alerts: on", true, android.R.drawable.ic_lock_silent_mode_off),
    PHONE_OFF("Kumo alerts: off", false, android.R.drawable.ic_lock_silent_mode),
    NTFY_ON("ntfy alerts: on", true, android.R.drawable.ic_lock_silent_mode_off),
    NTFY_OFF("ntfy alerts: off", false, android.R.drawable.ic_lock_silent_mode),
    CHECKING("ntfy alerts: …", false, android.R.drawable.ic_popup_sync),
    NO_RELAY("ntfy: pair a relay", false, android.R.drawable.stat_notify_error),
    UNREACHABLE("ntfy: relay unreachable", false, android.R.drawable.stat_notify_error),
}

class RelayWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        refresh(context, goAsync())
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == ACTION_TOGGLE) {
            toggle(context, goAsync())
            return
        }
        super.onReceive(context, intent)
    }

    companion object {
        private const val ACTION_TOGGLE = "com.kravtsov.kumowatch.TOGGLE_RELAY"

        fun updateAll(context: Context) = refresh(context, null)

        private fun state(context: Context) = (context.applicationContext as KumoApplication).state

        private fun refresh(context: Context, pending: BroadcastReceiver.PendingResult?) {
            val state = state(context)
            if (state.delivery.value == NotificationDelivery.PHONE) {
                render(context, if (Relay.isEnabled(context)) WidgetLook.PHONE_ON else WidgetLook.PHONE_OFF)
                pending?.finish()
                return
            }
            relayCall(context, pending) { state.relay.status() }
        }

        private fun toggle(context: Context, pending: BroadcastReceiver.PendingResult) {
            val state = state(context)
            if (state.delivery.value == NotificationDelivery.PHONE) {
                Relay.setEnabled(context, !Relay.isEnabled(context))
                pending.finish()
                return
            }
            relayCall(context, pending) { state.relay.setMirroring(!state.relay.status().mirroring) }
        }

        private fun relayCall(
            context: Context,
            pending: BroadcastReceiver.PendingResult?,
            action: suspend () -> RelayStatus,
        ) {
            val state = state(context)
            if (!state.relay.isPaired()) {
                render(context, WidgetLook.NO_RELAY)
                pending?.finish()
                return
            }
            render(context, WidgetLook.CHECKING)
            state.scope.launch {
                val look = runCatching { action() }.fold(
                    onSuccess = { if (it.mirroring) WidgetLook.NTFY_ON else WidgetLook.NTFY_OFF },
                    onFailure = { WidgetLook.UNREACHABLE },
                )
                render(context, look)
                pending?.finish()
            }
        }

        private fun render(context: Context, look: WidgetLook) {
            val manager = AppWidgetManager.getInstance(context)
            val ids = manager.getAppWidgetIds(ComponentName(context, RelayWidget::class.java))
            val toggle = PendingIntent.getBroadcast(
                context, 0,
                Intent(context, RelayWidget::class.java).setAction(ACTION_TOGGLE),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            ids.forEach { id ->
                val views = RemoteViews(context.packageName, R.layout.relay_widget).apply {
                    setTextViewText(R.id.widget_text, look.text)
                    setImageViewResource(R.id.widget_icon, look.icon)
                    setInt(R.id.widget_root, "setBackgroundResource", if (look.on) R.drawable.widget_on else R.drawable.widget_off)
                    setOnClickPendingIntent(R.id.widget_root, toggle)
                }
                manager.updateAppWidget(id, views)
            }
        }
    }
}
