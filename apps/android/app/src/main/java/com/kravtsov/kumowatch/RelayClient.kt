package com.kravtsov.kumowatch

import android.content.Context
import android.net.Uri
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

data class DeskZone(val id: String, val name: String, val x: Double, val y: Double, val w: Double, val h: Double, val open: Boolean)
data class DeskPerson(
    val id: String, val uid: String, val name: String, val avatar: String,
    val x: Double, val y: Double, val zoneId: String?, val self: Boolean, val availability: String,
    val idle: Boolean = false, val message: String = "",
)
data class DeskCall(val url: String, val token: String, val selfId: String, val peers: Set<String>, val roomName: String)
data class DeskFloor(val id: String, val name: String)
data class DeskRect(val x: Double, val y: Double, val w: Double, val h: Double) {
    fun contains(px: Double, py: Double, margin: Double = 0.0) =
        px >= x - margin && px <= x + w + margin && py >= y - margin && py <= y + h + margin
}

fun DeskState.focus(): DeskRect {
    val full = DeskRect(0.0, 0.0, width, height)
    val zone = zones.firstOrNull { it.id == selfZone }
    val me = people.firstOrNull { it.self }
    val (cx, cy, baseW, baseH) = when {
        zone != null -> listOf(zone.x + zone.w / 2, zone.y + zone.h / 2, zone.w * 1.5, zone.h * 1.5)
        me != null -> listOf(me.x, me.y, 1400.0, 900.0)
        else -> return full
    }
    val w = maxOf(baseW, baseH * 1.6).coerceAtMost(width)
    val h = (w / 1.6).coerceAtMost(height)
    val x = (cx - w / 2).coerceIn(0.0, maxOf(0.0, width - w))
    val y = (cy - h / 2).coerceIn(0.0, maxOf(0.0, height - h))
    return DeskRect(x, y, w, h)
}

data class DeskState(
    val floorId: String?, val width: Double, val height: Double, val floors: List<DeskFloor>,
    val selfZone: String?, val availability: String, val selfMessage: String, val zones: List<DeskZone>, val people: List<DeskPerson>,
    val audio: Boolean, val video: Boolean,
)

class RelayClient(context: Context) {
    private val prefs = context.getSharedPreferences("kumo_relay_link", Context.MODE_PRIVATE)
    private val storage = KeystoreStorage(context, "relay")

    val url: String? get() = prefs.getString("url", null)
    private val secret: String? get() = storage.load()
    val configured: Boolean get() = url != null && secret != null

    fun configure(link: Uri): Boolean {
        val url = link.getQueryParameter("url") ?: return false
        val secret = link.getQueryParameter("secret") ?: return false
        prefs.edit().putString("url", url).apply()
        storage.save(secret)
        return true
    }

    fun forget() {
        prefs.edit().remove("url").apply()
        storage.clear()
    }

    private suspend fun call(path: String, method: String, body: JSONObject? = null): JSONObject? = withContext(Dispatchers.IO) {
        val base = url ?: error("Relay is not set up")
        val conn = URL(base.trimEnd('/') + path).openConnection() as HttpURLConnection
        conn.requestMethod = method
        conn.connectTimeout = 8000
        conn.readTimeout = 10000
        conn.setRequestProperty("Authorization", "Bearer ${secret ?: error("Relay is not set up")}")
        if (body != null) {
            conn.doOutput = true
            conn.setRequestProperty("Content-Type", "application/json")
            conn.outputStream.use { it.write(body.toString().toByteArray()) }
        }
        val code = conn.responseCode
        val text = (if (code in 200..299) conn.inputStream else conn.errorStream)?.bufferedReader()?.readText().orEmpty()
        conn.disconnect()
        if (code !in 200..299) error(if (code == 401) "Wrong relay secret" else text.ifBlank { "Relay returned $code" })
        if (text.isBlank()) null else JSONObject(text)
    }

    suspend fun status(): JSONObject? = call("/v1/status", "GET")
    suspend fun setEnabled(enabled: Boolean): JSONObject? = call("/v1/enabled", "PUT", JSONObject().put("enabled", enabled))
    suspend fun desktop(): DeskState = parse(call("/v1/desktop", "GET")!!)
    suspend fun move(x: Double, y: Double) = parse(call("/v1/desktop/move", "POST", JSONObject().put("x", x).put("y", y))!!)
    suspend fun zone(id: String) = parse(call("/v1/desktop/zone", "POST", JSONObject().put("zone_id", id))!!)
    suspend fun floor(id: String) = parse(call("/v1/desktop/floor", "POST", JSONObject().put("floor_id", id))!!)
    suspend fun follow(id: String?) = parse(call("/v1/desktop/follow", "POST", JSONObject().put("user_id", id ?: JSONObject.NULL))!!)
    suspend fun status(availability: String, message: String) =
        parse(call("/v1/desktop/status", "POST", JSONObject().put("availability", availability).put("message", message))!!)
    suspend fun media(audio: Boolean? = null, video: Boolean? = null) = parse(
        call("/v1/desktop/media", "POST", JSONObject().put("audio", audio ?: JSONObject.NULL).put("video", video ?: JSONObject.NULL))!!,
    )
    suspend fun desktopCall(): DeskCall = deskCall(call("/v1/desktop/call", "GET"))

    suspend fun handoff(): DeskCall = deskCall(call("/v1/desktop/handoff", "POST", JSONObject()))

    suspend fun reclaim() { call("/v1/desktop/reclaim", "POST", JSONObject()) }

    private fun deskCall(json: JSONObject?): DeskCall {
        val o = json ?: error("No call data")
        val peers = o.optJSONArray("peers")
        return DeskCall(
            url = o.getString("url"),
            token = o.getString("token"),
            selfId = o.getString("selfId"),
            peers = if (peers == null) emptySet() else (0 until peers.length()).map { peers.getString(it) }.toSet(),
            roomName = o.optString("roomName"),
        )
    }

    suspend fun home() = parse(call("/v1/desktop/home", "POST", JSONObject())!!)

    suspend fun nudge(id: String) { call("/v1/desktop/nudge", "POST", JSONObject().put("user_id", id)) }

    private fun parse(o: JSONObject): DeskState {
        fun <T> JSONArray?.list(f: (JSONObject) -> T): List<T> =
            if (this == null) emptyList() else (0 until length()).map { f(getJSONObject(it)) }
        val self = o.optJSONObject("self")
        return DeskState(
            floorId = o.optString("floorId").ifBlank { null },
            width = o.optDouble("width", 0.0),
            height = o.optDouble("height", 0.0),
            floors = o.optJSONArray("floors").list { DeskFloor(it.getString("id"), it.optString("name")) },
            selfZone = self?.optString("zoneId")?.takeIf { it.isNotBlank() && it != "null" },
            availability = self?.optString("availability").orEmpty(),
            selfMessage = self?.optString("message").orEmpty(),
            zones = o.optJSONArray("zones").list {
                DeskZone(it.getString("id"), it.optString("name"), it.optDouble("x"), it.optDouble("y"), it.optDouble("w"), it.optDouble("h"), it.optBoolean("open", true))
            },
            people = o.optJSONArray("people").list {
                DeskPerson(
                    it.getString("id"), it.optString("uid"), it.optString("name"), it.optString("avatar"),
                    it.optDouble("x"), it.optDouble("y"), it.optString("zoneId").takeIf { z -> z.isNotBlank() && z != "null" },
                    it.optBoolean("self"), it.optString("availability"),
                    it.optBoolean("idle"), it.optString("message"),
                )
            },
            audio = o.optBoolean("audio"),
            video = o.optBoolean("video"),
        )
    }
}
