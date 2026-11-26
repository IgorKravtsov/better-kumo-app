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
    // A call screen restored with no call, no join running and none about to start (process death) is stale.
    LaunchedEffect(Unit) {
        val stale = !state.shownCall.value.state.active && state.busy.value == null && requests.value == null
        if (stale && nav.currentDestination?.route == Routes.CALL) nav.popBackStack(Routes.HOME, inclusive = false)
    }
    LaunchedEffect(request) {
        val r = request ?: return@LaunchedEffect
        requests.value = null
        when (r) {
            WearRequest.ShowCall -> nav.navigate(Routes.CALL) { launchSingleTop = true }
            WearRequest.JoinFailed -> if (nav.currentDestination?.route == Routes.CALL && !state.shownCall.value.state.active) {
                nav.popBackStack(Routes.HOME, inclusive = false)
            }
            is WearRequest.OpenChat -> {
                val top = nav.currentBackStackEntry
                val open = top?.destination?.route == Routes.CHAT && top.arguments?.getString("url") == r.channelUrl
                if (!open) nav.navigate(Routes.chat(r.channelUrl))
            }
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
            val url = entry.arguments?.getString("url") ?: return@composable
            ChannelScreen(state, url)
        }
    }
}
