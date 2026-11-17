package com.kravtsov.kumowatch.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import com.kravtsov.kumowatch.AppState
import uniffi.kumo_ffi.SpaceInfo

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SpacesScreen(state: AppState, onDone: (() -> Unit)? = null) {
    var spaces by remember { mutableStateOf<List<SpaceInfo>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        runCatching { state.spaces() }.onSuccess { spaces = it }.onFailure { error = it.message }
    }
    Scaffold(topBar = { TopAppBar(title = { Text("Choose a space") }) }) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            val list = spaces
            when {
                error != null -> Text(error ?: "", Modifier.align(Alignment.Center))
                list == null -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                else -> LazyColumn {
                    items(list, key = { it.uid }) { space ->
                        ListItem(
                            leadingContent = { Avatar(space.displayName, "") },
                            headlineContent = { Text(space.displayName) },
                            supportingContent = { Text("${space.name} · ${space.memberCount} members") },
                            modifier = Modifier.clickable {
                                state.selectSpace(space)
                                onDone?.invoke()
                            },
                        )
                    }
                }
            }
        }
    }
}
