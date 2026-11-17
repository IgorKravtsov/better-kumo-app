package com.kravtsov.kumowatch.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.util.LruCache
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.BrokenImage
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImagePainter
import coil3.compose.rememberAsyncImagePainter
import coil3.request.ImageRequest
import java.io.ByteArrayOutputStream
import uniffi.kumo_ffi.AttachmentLimits

private const val PREVIEW_PIXELS = 800
private const val FULL_PIXELS = 4096
private const val LOCAL_SCHEME = "local:"

object LocalPreviews {
    private val cache = object : LruCache<String, ImageBitmap>(32 * 1024 * 1024) {
        override fun sizeOf(key: String, value: ImageBitmap): Int = value.width * value.height * 4
    }

    fun key(name: String): String = LOCAL_SCHEME + name

    fun put(name: String, image: ImageBitmap) {
        cache.put(key(name), image)
    }

    fun get(url: String): ImageBitmap? = cache.get(url)
}

private fun stableKey(url: String): String {
    val uri = Uri.parse(url)
    if (uri.getQueryParameter("auth") == null) return url
    val kept = uri.buildUpon().clearQuery()
    uri.queryParameterNames.filter { it != "auth" }.forEach { name ->
        uri.getQueryParameters(name).forEach { kept.appendQueryParameter(name, it) }
    }
    return kept.build().toString()
}

fun chatImageRequest(context: Context, url: String, full: Boolean): ImageRequest {
    val key = stableKey(url)
    val preview = "p|$key"
    return ImageRequest.Builder(context)
        .data(url)
        .diskCacheKey(key)
        .memoryCacheKey(if (full) "f|$key" else preview)
        .apply { if (full) placeholderMemoryCacheKey(preview) }
        .size(if (full) FULL_PIXELS else PREVIEW_PIXELS)
        .build()
}

private fun fitted(size: Size, maxSide: Dp, pixelsPerDp: Float): DpSize {
    if (size.width <= 0f || size.height <= 0f) return DpSize(maxSide, maxSide)
    val width = size.width / pixelsPerDp
    val height = size.height / pixelsPerDp
    val scale = minOf(1f, maxSide.value / maxOf(width, height))
    return DpSize(maxOf(40f, width * scale).dp, maxOf(40f, height * scale).dp)
}

@Composable
fun ChatImage(url: String, maxSide: Dp = 260.dp, onClick: (() -> Unit)? = null) {
    val shape = RoundedCornerShape(8.dp)
    val tap = if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier
    val pixelsPerDp = LocalDensity.current.density
    if (url.startsWith(LOCAL_SCHEME)) {
        val local = LocalPreviews.get(url)
        if (local != null) {
            val size = fitted(Size(local.width.toFloat(), local.height.toFloat()), maxSide, pixelsPerDp)
            Image(local, null, contentScale = ContentScale.Crop, modifier = Modifier.padding(vertical = 4.dp).size(size).clip(shape))
        } else {
            ImageLoading(maxSide)
        }
        return
    }
    val context = LocalContext.current
    val request = remember(url) { chatImageRequest(context, url, full = false) }
    val painter = rememberAsyncImagePainter(request)
    val state by painter.state.collectAsState()
    when (val current = state) {
        is AsyncImagePainter.State.Success -> {
            val size = fitted(current.painter.intrinsicSize, maxSide, pixelsPerDp)
            Image(painter, null, contentScale = ContentScale.Crop, modifier = Modifier.padding(vertical = 4.dp).size(size).clip(shape).then(tap))
        }
        is AsyncImagePainter.State.Error -> ImageUnavailable()
        else -> ImageLoading(maxSide)
    }
}

@Composable
private fun ImageLoading(maxSide: Dp) {
    Box(
        Modifier.padding(vertical = 4.dp).size(maxSide * 0.8f, maxSide * 0.6f).clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant),
        contentAlignment = Alignment.Center,
    ) { CircularProgressIndicator(Modifier.size(20.dp)) }
}

@Composable
private fun ImageUnavailable() {
    Row(
        Modifier.padding(vertical = 4.dp).clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Outlined.BrokenImage, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.outline)
        Spacer(Modifier.width(6.dp))
        Text("Image unavailable", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.outline)
    }
}

fun compressImage(context: Context, uri: Uri, limits: AttachmentLimits): Bitmap? {
    val maxSide = limits.imageMaxSide.toInt()
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
    var sample = 1
    while (bounds.outWidth / (sample * 2) >= maxSide && bounds.outHeight / (sample * 2) >= maxSide) sample *= 2
    val decoded = context.contentResolver.openInputStream(uri)?.use {
        BitmapFactory.decodeStream(it, null, BitmapFactory.Options().apply { inSampleSize = sample })
    } ?: return null
    val rotation = context.contentResolver.openInputStream(uri)?.use {
        val orientation = runCatching {
            ExifInterface(it).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)
        }.getOrDefault(ExifInterface.ORIENTATION_NORMAL)
        when (orientation) {
            ExifInterface.ORIENTATION_ROTATE_90 -> 90
            ExifInterface.ORIENTATION_ROTATE_180 -> 180
            ExifInterface.ORIENTATION_ROTATE_270 -> 270
            else -> 0
        }
    } ?: 0
    val scale = minOf(1f, maxSide.toFloat() / maxOf(decoded.width, decoded.height))
    if (scale >= 1f && rotation == 0) return decoded
    val matrix = Matrix().apply {
        postScale(scale, scale)
        postRotate(rotation.toFloat())
    }
    return Bitmap.createBitmap(decoded, 0, 0, decoded.width, decoded.height, matrix, true)
}

fun jpegBytes(bitmap: Bitmap, limits: AttachmentLimits): ByteArray =
    ByteArrayOutputStream().use { out ->
        bitmap.compress(Bitmap.CompressFormat.JPEG, limits.jpegQuality.toInt(), out)
        out.toByteArray()
    }

fun thumbnail(bitmap: Bitmap): ImageBitmap {
    val scale = minOf(1f, PREVIEW_PIXELS.toFloat() / maxOf(bitmap.width, bitmap.height))
    val scaled = if (scale < 1f) {
        Bitmap.createScaledBitmap(bitmap, (bitmap.width * scale).toInt().coerceAtLeast(1), (bitmap.height * scale).toInt().coerceAtLeast(1), true)
    } else bitmap
    return scaled.asImageBitmap()
}
