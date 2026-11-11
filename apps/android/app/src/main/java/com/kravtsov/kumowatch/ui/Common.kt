package com.kravtsov.kumowatch.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.compose.AsyncImage
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale

@Composable
fun Avatar(name: String, url: String, size: Dp = 40.dp, online: Boolean = false) {
    Box {
        Box(
            modifier = Modifier.size(size).clip(CircleShape).background(colorFor(name)),
            contentAlignment = Alignment.Center,
        ) {
            Text(initials(name), color = Color.White, fontSize = (size.value * 0.38f).sp)
            if (url.isNotBlank()) {
                AsyncImage(
                    model = url,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.size(size),
                )
            }
        }
        if (online) {
            Box(
                Modifier.align(Alignment.BottomEnd).size(size / 4).clip(CircleShape)
                    .background(MaterialTheme.colorScheme.surface),
                contentAlignment = Alignment.Center,
            ) {
                Box(Modifier.size(size / 4 - 3.dp).clip(CircleShape).background(Color(0xFF2EB67D)))
            }
        }
    }
}

private val palette = listOf(
    Color(0xFF5B6CFF), Color(0xFF00A3A3), Color(0xFFE5793B), Color(0xFFB05BD6),
    Color(0xFF3B8EE5), Color(0xFFD64F6E), Color(0xFF4CA35A), Color(0xFF8C6A4F),
)

fun colorFor(name: String): Color = palette[(name.hashCode() and 0x7fffffff) % palette.size]

fun initials(name: String): String =
    name.split(' ', '.', '_').filter { it.isNotBlank() }.take(2).joinToString("") { it.first().uppercase() }
        .ifEmpty { "?" }

fun shortTime(ms: Long): String {
    if (ms <= 0) return ""
    val now = Calendar.getInstance()
    val then = Calendar.getInstance().apply { timeInMillis = ms }
    val pattern = when {
        sameDay(now, then) -> "HH:mm"
        now.get(Calendar.YEAR) == then.get(Calendar.YEAR) -> "d MMM"
        else -> "d MMM yyyy"
    }
    return SimpleDateFormat(pattern, Locale.getDefault()).format(Date(ms))
}

fun clockTime(ms: Long): String = SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date(ms))

fun dayLabel(ms: Long): String {
    val now = Calendar.getInstance()
    val then = Calendar.getInstance().apply { timeInMillis = ms }
    if (sameDay(now, then)) return "Today"
    now.add(Calendar.DAY_OF_YEAR, -1)
    if (sameDay(now, then)) return "Yesterday"
    return SimpleDateFormat("EEEE, d MMMM yyyy", Locale.getDefault()).format(Date(ms))
}

fun sameDay(a: Long, b: Long): Boolean = sameDay(
    Calendar.getInstance().apply { timeInMillis = a },
    Calendar.getInstance().apply { timeInMillis = b },
)

private fun sameDay(a: Calendar, b: Calendar): Boolean =
    a.get(Calendar.YEAR) == b.get(Calendar.YEAR) && a.get(Calendar.DAY_OF_YEAR) == b.get(Calendar.DAY_OF_YEAR)
