package com.kravtsov.kumowatch

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import kotlinx.coroutines.flow.MutableStateFlow
import com.kravtsov.kumowatch.ui.RootScreen

class MainActivity : ComponentActivity() {
    private val pendingLink = MutableStateFlow<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        handleIntent(intent)
        Relay.ensureChannels(this)
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        val state = (application as KumoApplication).state
        setContent {
            MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
                RootScreen(state, pendingLink)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIntent(intent)
    }

    override fun onStart() {
        super.onStart()
        (application as KumoApplication).state.onForeground()
    }

    private fun handleIntent(intent: Intent?) {
        intent?.getStringExtra(Relay.EXTRA_CHANNEL_URL)?.let {
            (application as KumoApplication).state.requestOpen(it)
        }
        val data = intent?.dataString ?: return
        if (data.startsWith("kumospace://login")) pendingLink.value = data
        if (data.startsWith("kumowatch://chat")) {
            intent.data?.getQueryParameter("channel")?.let { (application as KumoApplication).state.requestOpen(it) }
        }
    }
}
