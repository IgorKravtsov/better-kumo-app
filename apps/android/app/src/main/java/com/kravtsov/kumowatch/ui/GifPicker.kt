package com.kravtsov.kumowatch.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.staggeredgrid.LazyVerticalStaggeredGrid
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridCells
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridItemSpan
import androidx.compose.foundation.lazy.staggeredgrid.items
import androidx.compose.foundation.lazy.staggeredgrid.rememberLazyStaggeredGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import com.kravtsov.kumowatch.AppState
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.kumo_ffi.GifItem

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GifPicker(state: AppState, onDismiss: () -> Unit, onPick: (GifItem) -> Unit) {
    var query by remember { mutableStateOf("") }
    val gifs = remember { mutableStateListOf<GifItem>() }
    var nextOffset by remember { mutableStateOf(0u) }
    var loading by remember { mutableStateOf(false) }
    var reachedEnd by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    // Bumped on every new query so late pages of an older query are dropped.
    var generation by remember { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    val grid = rememberLazyStaggeredGridState()

    suspend fun fetch(mine: Int) {
        val page = runCatching { state.core.searchGifs(query, nextOffset) }
        if (mine != generation) return
        loading = false
        page.onSuccess { result ->
            val known = gifs.map { it.id }.toSet()
            gifs.addAll(result.gifs.filter { it.id !in known })
            nextOffset = result.nextOffset
            reachedEnd = result.reachedEnd
        }.onFailure {
            failed = true
            reachedEnd = true
        }
    }

    LaunchedEffect(query) {
        generation++
        val mine = generation
        gifs.clear()
        nextOffset = 0u
        reachedEnd = false
        failed = false
        loading = true
        grid.scrollToItem(0)
        if (query.isNotBlank()) delay(350)
        fetch(mine)
    }

    val nearEnd by remember {
        derivedStateOf {
            val last = grid.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1
            gifs.isNotEmpty() && last >= gifs.size - 4
        }
    }
    LaunchedEffect(nearEnd, loading, reachedEnd) {
        if (!nearEnd || loading || reachedEnd) return@LaunchedEffect
        loading = true
        val mine = generation
        scope.launch { fetch(mine) }
    }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.9f)) {
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                singleLine = true,
                leadingIcon = { Icon(Icons.Filled.Search, null) },
                placeholder = { Text("Search GIPHY") },
                shape = RoundedCornerShape(20.dp),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
            )
            LazyVerticalStaggeredGrid(
                columns = StaggeredGridCells.Fixed(2),
                state = grid,
                contentPadding = PaddingValues(8.dp),
                verticalItemSpacing = 6.dp,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.weight(1f),
            ) {
                items(gifs, key = { it.id }) { gif -> GifCell(gif) { onPick(gif) } }
                item(span = StaggeredGridItemSpan.FullLine) {
                    Box(Modifier.fillMaxWidth().padding(12.dp), contentAlignment = Alignment.Center) {
                        when {
                            loading -> CircularProgressIndicator(Modifier.size(24.dp))
                            failed && gifs.isEmpty() -> Text("Couldn't load GIFs", color = MaterialTheme.colorScheme.outline)
                            gifs.isEmpty() && query.isNotBlank() -> Text("No GIFs found", color = MaterialTheme.colorScheme.outline)
                            else -> Text("Powered by GIPHY", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun GifCell(gif: GifItem, onClick: () -> Unit) {
    val context = LocalContext.current
    val ratio = if (gif.previewWidth > 0u && gif.previewHeight > 0u) gif.previewWidth.toFloat() / gif.previewHeight.toFloat() else 1f
    val request = remember(gif.previewUrl) { chatImageRequest(context, gif.previewUrl, full = false) }
    AsyncImage(
        model = request,
        contentDescription = gif.title.ifBlank { "GIF" },
        contentScale = ContentScale.Crop,
        modifier = Modifier.fillMaxWidth().aspectRatio(ratio).clip(RoundedCornerShape(6.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant).clickable(onClick = onClick),
    )
}
