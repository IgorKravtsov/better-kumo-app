package com.kravtsov.kumowatch.wear.ui

import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.wear.compose.material3.AppScaffold
import androidx.wear.compose.material3.MaterialTheme
import androidx.wear.compose.navigation.SwipeDismissableNavHost
import androidx.wear.compose.navigation.composable
import androidx.wear.compose.navigation.rememberSwipeDismissableNavController
import com.kravtsov.kumowatch.wear.WearRequest
import com.kravtsov.kumowatch.wear.WearScreen
import com.kravtsov.kumowatch.wear.WearState
import kotlinx.coroutines.flow.MutableStateFlow

private object Routes {
    const val HOME = "home"
    const val OFFICE = "office"
    const val CALL = "call"
    const val CHATS = "chats"
    const val CHAT = "chat?url={url}"
    fun chat(url: String) = "chat?url=${Uri.encode(url)}"
}

@Composable
fun WearApp(state: WearState, requests: MutableStateFlow<WearRequest?>) {
    MaterialTheme {
        AppScaffold {
            val screen by state.screen.collectAsState()
            when (screen) {
                WearScreen.SIGN_IN -> SignInScreen(state)
                WearScreen.HOME -> SignedIn(state, requests)
            }
        }
    }
}

@Composable
private fun SignedIn(state: WearState, requests: MutableStateFlow<WearRequest?>) {
    val nav = rememberSwipeDismissableNavController()
    val request by requests.collectAsState()
    LaunchedEffect(request) {
        val r = request ?: return@LaunchedEffect
        requests.value = null
        when (r) {
            WearRequest.ShowCall -> nav.navigate(Routes.CALL) { launchSingleTop = true }
            is WearRequest.OpenChat -> nav.navigate(Routes.chat(r.channelUrl))
        }
    }
    SwipeDismissableNavHost(navController = nav, startDestination = Routes.HOME) {
        composable(Routes.HOME) {
            HomeScreen(
                state,
                onCall = { nav.navigate(Routes.CALL) { launchSingleTop = true } },
                onOffice = { nav.navigate(Routes.OFFICE) },
                onChats = { nav.navigate(Routes.CHATS) },
            )
        }
        composable(Routes.OFFICE) {
            OfficeScreen(state, onJoined = {
                nav.navigate(Routes.CALL) {
                    popUpTo(Routes.HOME)
                    launchSingleTop = true
                }
            })
        }
        composable(Routes.CALL) { CallScreen(state, onEnded = { nav.popBackStack(Routes.HOME, inclusive = false) }) }
        composable(Routes.CHATS) { ChatsScreen(state, onOpen = { nav.navigate(Routes.chat(it)) }) }
        composable(Routes.CHAT) { entry ->
            val url = entry.arguments?.getString("url")?.let(Uri::decode) ?: return@composable
            ChannelScreen(state, url)
        }
    }
}
