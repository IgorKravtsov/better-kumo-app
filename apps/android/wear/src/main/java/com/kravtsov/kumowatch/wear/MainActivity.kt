package com.kravtsov.kumowatch.wear

import android.Manifest
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import com.kravtsov.kumowatch.wear.ui.WearApp

sealed interface WearRequest {
    data object ShowCall : WearRequest
    data class OpenChat(val channelUrl: String) : WearRequest
}

class MainActivity : ComponentActivity() {
    private val requests = MutableStateFlow<WearRequest?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val missing = listOfNotNull(
            Manifest.permission.RECORD_AUDIO,
            Manifest.permission.POST_NOTIFICATIONS.takeIf { Build.VERSION.SDK_INT >= 33 },
        ).filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) requestPermissions(missing.toTypedArray(), 1)
        val state = wearState
        lifecycleScope.launch {
            state.notices.collect { Toast.makeText(this@MainActivity, it, Toast.LENGTH_SHORT).show() }
        }
        // Notices first, and only fresh launches: extras survive recreation and launches from Recents.
        if (savedInstanceState == null && intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY == 0) handle(intent)
        setContent { WearApp(state, requests) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun onStart() {
        super.onStart()
        wearState.onForeground(true)
    }

    override fun onStop() {
        wearState.onForeground(false)
        super.onStop()
    }

    private fun handle(intent: Intent) {
        val state = wearState
        // Emulators have no paired phone to sign in from.
        val debugLink = intent.getStringExtra(EXTRA_DEBUG_SIGN_IN_LINK)
        if (debugLink != null && applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            lifecycleScope.launch {
                state.signInWithLink(debugLink, null)
                    .onFailure { Toast.makeText(this@MainActivity, it.message ?: "Sign-in failed", Toast.LENGTH_LONG).show() }
            }
        }
        // Only the notification's non-exported alias may start a join; MainActivity itself is exported.
        val join = intent.getStringExtra(EXTRA_JOIN_PERSON)?.takeIf { intent.component?.className == JOIN_ALIAS }
        val channel = intent.getStringExtra(EXTRA_CHANNEL_URL)
        val showCall = intent.getBooleanExtra(EXTRA_SHOW_CALL, false)
        intent.getIntExtra(EXTRA_NOTIFICATION_ID, 0).takeIf { it != 0 }
            ?.let { getSystemService(NotificationManager::class.java).cancel(it) }
        listOf(EXTRA_DEBUG_SIGN_IN_LINK, EXTRA_JOIN_PERSON, EXTRA_CHANNEL_URL, EXTRA_SHOW_CALL, EXTRA_NOTIFICATION_ID)
            .forEach(intent::removeExtra)
        if (!state.signedIn) return
        when {
            join != null -> {
                requests.value = WearRequest.ShowCall
                state.scope.launch { state.joinPerson(join, mic = true) }
            }
            showCall -> requests.value = WearRequest.ShowCall
            channel != null -> requests.value = WearRequest.OpenChat(channel)
        }
    }

    companion object {
        const val EXTRA_SHOW_CALL = "show_call"
        const val EXTRA_CHANNEL_URL = "channel_url"
        const val EXTRA_JOIN_PERSON = "join_person"
        const val EXTRA_NOTIFICATION_ID = "notification_id"
        const val EXTRA_DEBUG_SIGN_IN_LINK = "debug_sign_in_link"
        private const val JOIN_ALIAS = "com.kravtsov.kumowatch.wear.JoinFromNotification"

        fun intent(
            context: Context,
            requestCode: Int,
            channelUrl: String? = null,
            joinPerson: String? = null,
            notificationId: Int = 0,
        ): PendingIntent {
            val intent = Intent()
                .setClassName(context, if (joinPerson != null) JOIN_ALIAS else MainActivity::class.java.name)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            channelUrl?.let { intent.putExtra(EXTRA_CHANNEL_URL, it) }
            joinPerson?.let { intent.putExtra(EXTRA_JOIN_PERSON, it) }
            if (notificationId != 0) intent.putExtra(EXTRA_NOTIFICATION_ID, notificationId)
            return PendingIntent.getActivity(
                context, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }
    }
}
