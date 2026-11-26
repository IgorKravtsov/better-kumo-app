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
import androidx.activity.result.contract.ActivityResultContracts
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import com.kravtsov.kumowatch.wear.ui.WearApp

sealed interface WearRequest {
    data object ShowCall : WearRequest
    /** The join the call screen was opened for didn't start or was rejected. */
    data object JoinFailed : WearRequest
    data class OpenChat(val channelUrl: String) : WearRequest
}

class MainActivity : ComponentActivity() {
    private val requests = MutableStateFlow<WearRequest?>(null)
    private var asking = false
    private var joinAfterPermission: String? = null
    private val permissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        asking = false
        joinAfterPermission?.let(::joinFromNotification)
        joinAfterPermission = null
    }

    private fun ask(missing: Array<String>) {
        if (asking) return
        asking = true
        permissions.launch(missing)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The permission result can reach a recreated activity.
        asking = savedInstanceState?.getBoolean(KEY_ASKING) == true
        joinAfterPermission = savedInstanceState?.getString(KEY_JOIN_AFTER_PERMISSION)
        if (joinAfterPermission != null) requests.value = WearRequest.ShowCall
        val missing = listOfNotNull(
            Manifest.permission.RECORD_AUDIO,
            Manifest.permission.POST_NOTIFICATIONS.takeIf { Build.VERSION.SDK_INT >= 33 },
        ).filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) ask(missing.toTypedArray())
        val state = wearState
        lifecycleScope.launch {
            state.notices.collect { Toast.makeText(this@MainActivity, it, Toast.LENGTH_SHORT).show() }
        }
        // Notices first, and only fresh launches: extras survive recreation and launches from Recents.
        if (savedInstanceState == null && intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY == 0) handle(intent)
        setContent { WearApp(state, requests) }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putBoolean(KEY_ASKING, asking)
        outState.putString(KEY_JOIN_AFTER_PERMISSION, joinAfterPermission)
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
        // Only the notifications' non-exported alias may act on these; MainActivity itself is exported.
        val fromNotification = intent.component?.className == NOTIFICATION_ALIAS
        val join = intent.getStringExtra(EXTRA_JOIN_PERSON)?.takeIf { fromNotification }
        val channel = intent.getStringExtra(EXTRA_CHANNEL_URL)?.takeIf { fromNotification }
        val showCall = intent.getBooleanExtra(EXTRA_SHOW_CALL, false) && fromNotification
        intent.getIntExtra(EXTRA_NOTIFICATION_ID, 0).takeIf { it != 0 && fromNotification }
            ?.let { getSystemService(NotificationManager::class.java).cancel(it) }
        listOf(EXTRA_DEBUG_SIGN_IN_LINK, EXTRA_JOIN_PERSON, EXTRA_CHANNEL_URL, EXTRA_SHOW_CALL, EXTRA_NOTIFICATION_ID)
            .forEach(intent::removeExtra)
        if (!state.signedIn) return
        when {
            join != null -> {
                requests.value = WearRequest.ShowCall
                // A watch call needs the microphone, which the permission prompt may be about to grant.
                if (!state.callsOnPhone.value && checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                    joinAfterPermission = join
                    ask(arrayOf(Manifest.permission.RECORD_AUDIO))
                } else {
                    joinFromNotification(join)
                }
            }
            showCall -> requests.value = WearRequest.ShowCall
            channel != null -> requests.value = WearRequest.OpenChat(channel)
        }
    }

    private fun joinFromNotification(person: String) {
        val state = wearState
        // Rejected because another join runs: the call screen is that join's, and it closes when that one ends.
        val another = state.busy.value != null
        state.scope.launch {
            // A join rejected before the call screen saw it busy would leave that screen waiting.
            if (!state.joinPerson(person, mic = true) && !another) requests.value = WearRequest.JoinFailed
        }
    }

    companion object {
        const val EXTRA_SHOW_CALL = "show_call"
        const val EXTRA_CHANNEL_URL = "channel_url"
        const val EXTRA_JOIN_PERSON = "join_person"
        const val EXTRA_NOTIFICATION_ID = "notification_id"
        const val EXTRA_DEBUG_SIGN_IN_LINK = "debug_sign_in_link"
        private const val NOTIFICATION_ALIAS = "com.kravtsov.kumowatch.wear.FromNotification"
        private const val KEY_ASKING = "asking"
        private const val KEY_JOIN_AFTER_PERMISSION = "join_after_permission"

        fun intent(
            context: Context,
            requestCode: Int,
            channelUrl: String? = null,
            joinPerson: String? = null,
            notificationId: Int = 0,
            showCall: Boolean = false,
        ): PendingIntent {
            val intent = Intent()
                .setClassName(context, NOTIFICATION_ALIAS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            if (showCall) intent.putExtra(EXTRA_SHOW_CALL, true)
            channelUrl?.let { intent.putExtra(EXTRA_CHANNEL_URL, it) }
            joinPerson?.let { intent.putExtra(EXTRA_JOIN_PERSON, it) }
            if (notificationId != 0) intent.putExtra(EXTRA_NOTIFICATION_ID, notificationId)
            return PendingIntent.getActivity(
                context, requestCode, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }
    }
}
