package com.kravtsov.kumowatch.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Apartment
import androidx.compose.material.icons.filled.Forum
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.kravtsov.kumowatch.AppState
import com.kravtsov.kumowatch.OfficeRequest
import com.kravtsov.kumowatch.Screen
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull

@Composable
fun RootScreen(state: AppState, pendingLink: MutableStateFlow<String?>) {
    val screen by state.screen.collectAsState()
    var signInError by remember { mutableStateOf<String?>(null) }
    var signingIn by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        pendingLink.filterNotNull().collect { value ->
            pendingLink.value = null
            signingIn = true
            signInError = state.signInWithLink(value).exceptionOrNull()?.message
            signingIn = false
        }
    }

    val prompt by state.passwordPrompt.collectAsState()
    var password by remember { mutableStateOf("") }
    val context = androidx.compose.ui.platform.LocalContext.current
    LaunchedEffect(Unit) {
        state.notices.collect { android.widget.Toast.makeText(context, it, android.widget.Toast.LENGTH_SHORT).show() }
    }
    if (prompt != null) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { state.submitPassword(null) },
            title = { Text("Room password") },
            text = {
                androidx.compose.material3.OutlinedTextField(
                    value = password,
                    onValueChange = { password = it },
                    singleLine = true,
                    visualTransformation = androidx.compose.ui.text.input.PasswordVisualTransformation(),
                )
            },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    state.submitPassword(password)
                    password = ""
                }) { Text("Join") }
            },
            dismissButton = {
                androidx.compose.material3.TextButton(onClick = { state.submitPassword(null) }) { Text("Cancel") }
            },
        )
    }

    when (screen) {
        Screen.SignIn -> SignInScreen(state, signingIn, signInError) { signInError = it }
        Screen.PickSpace -> SpacesScreen(state)
        Screen.Chats -> ChatsNavigation(state)
    }
}

@Composable
private fun ChatsNavigation(state: AppState) {
    var openChannel by remember { mutableStateOf<String?>(null) }
    var showProfile by remember { mutableStateOf(false) }
    var showCall by remember { mutableStateOf(false) }
    var showDesktop by remember { mutableStateOf(false) }
    var showNearby by remember { mutableStateOf(false) }
    var openThread by remember { mutableStateOf<uniffi.kumo_ffi.MessageItem?>(null) }
    var tab by rememberSaveable { mutableStateOf(1) }
    val openRequest by state.openRequest.collectAsState()
    LaunchedEffect(openRequest) {
        openRequest?.let {
            openChannel = it
            showProfile = false
            tab = 0
            state.consumeOpenRequest()
        }
    }
    val officeRequest by state.officeRequest.collectAsState()
    LaunchedEffect(officeRequest) {
        val request = officeRequest ?: return@LaunchedEffect
        openChannel = null
        openThread = null
        showProfile = false
        showNearby = false
        tab = 1
        when (request) {
            OfficeRequest.OFFICE -> Unit
            OfficeRequest.DESKTOP -> showDesktop = true
            OfficeRequest.CALL -> showCall = true
        }
        state.consumeOfficeRequest()
    }
    val current = openChannel
    when {
        showCall -> {
            BackHandler { showCall = false }
            CallScreen(state) { showCall = false }
        }
        showDesktop && showNearby -> {
            BackHandler { showNearby = false }
            NearbyChatScreen(state) { showNearby = false }
        }
        showDesktop -> {
            BackHandler { showDesktop = false }
            DesktopScreen(state, onOpenCall = { showCall = true }, onOpenNearby = { showNearby = true }) { showDesktop = false }
        }
        current != null && openThread != null -> {
            BackHandler { openThread = null }
            ChannelScreen(state, current, thread = openThread) { openThread = null }
        }
        current != null -> {
            BackHandler { openChannel = null }
            ChannelScreen(state, current, onOpenThread = { openThread = it }, onOpenChannel = {
                openThread = null
                openChannel = it
            }) {
                openChannel = null
                openThread = null
            }
        }
        showProfile -> {
            BackHandler { showProfile = false }
            ProfileScreen(state) { showProfile = false }
        }
        else -> Scaffold(
            bottomBar = {
                NavigationBar {
                    NavigationBarItem(
                        selected = tab == 1,
                        onClick = { tab = 1 },
                        icon = { Icon(Icons.Filled.Apartment, null) },
                        label = { Text("Office") },
                    )
                    NavigationBarItem(
                        selected = tab == 0,
                        onClick = { tab = 0 },
                        icon = { Icon(Icons.Filled.Forum, null) },
                        label = { Text("Chat") },
                    )
                }
            },
        ) { padding ->
            Box(Modifier.padding(bottom = padding.calculateBottomPadding())) {
                if (tab == 0) {
                    ChatListScreen(state, onOpen = { openChannel = it }, onProfile = { showProfile = true })
                } else {
                    Column(Modifier.statusBarsPadding()) {
                        OfficeScreen(state, onOpenCall = { showCall = true }, onOpenDesktop = { showDesktop = true })
                    }
                }
            }
        }
    }
}
