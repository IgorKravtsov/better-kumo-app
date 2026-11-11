package com.kravtsov.kumowatch

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.widget.RemoteViews

class RelayWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        render(context, manager, ids)
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == ACTION_TOGGLE) {
            Relay.setEnabled(context, !Relay.isEnabled(context))
            return
        }
        super.onReceive(context, intent)
    }

    companion object {
        private const val ACTION_TOGGLE = "com.kravtsov.kumowatch.TOGGLE_RELAY"

        fun updateAll(context: Context) {
            val manager = AppWidgetManager.getInstance(context)
            render(context, manager, manager.getAppWidgetIds(ComponentName(context, RelayWidget::class.java)))
        }

        private fun render(context: Context, manager: AppWidgetManager, ids: IntArray) {
            val on = Relay.isEnabled(context)
            val toggle = PendingIntent.getBroadcast(
                context, 0,
                Intent(context, RelayWidget::class.java).setAction(ACTION_TOGGLE),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            ids.forEach { id ->
                val views = RemoteViews(context.packageName, R.layout.relay_widget).apply {
                    setTextViewText(R.id.widget_text, if (on) "Kumo alerts: on" else "Kumo alerts: off")
                    setImageViewResource(
                        R.id.widget_icon,
                        if (on) android.R.drawable.ic_lock_silent_mode_off else android.R.drawable.ic_lock_silent_mode,
                    )
                    setInt(R.id.widget_root, "setBackgroundResource", if (on) R.drawable.widget_on else R.drawable.widget_off)
                    setOnClickPendingIntent(R.id.widget_root, toggle)
                }
                manager.updateAppWidget(id, views)
            }
        }
    }
}
