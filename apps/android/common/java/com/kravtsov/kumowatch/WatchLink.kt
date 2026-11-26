package com.kravtsov.kumowatch

import org.json.JSONArray
import org.json.JSONObject

/**
 * Data Layer contract; it only connects apps with the same application ID and signing certificate. Alerts, call
 * commands and call states carry the Kumospace account uid: the watch and the phone must be signed in to the same one.
 */
object WatchLink {
    const val CAPABILITY_PHONE = "kumo_phone"
    const val CAPABILITY_WATCH = "kumo_wear"

    const val PATH_SIGN_IN = "/kumo/sign-in"

    const val ACCOUNT_MISMATCH =
        "The phone is signed in to another Kumospace account. Sign out on the watch and sign in from the phone."

    const val PATH_ALERT = "/kumo/alert"
    val ALERT_SHOWN = byteArrayOf(1)

    const val PATH_CALL_COMMAND = "/kumo/call/command"

    /** Any command renews the lease for these pushes. */
    const val PATH_CALL_STATE = "/kumo/call/state"

    data class Space(val name: String, val uid: String, val displayName: String, val memberCount: Long)

    data class SignIn(val link: String, val space: Space?) {
        fun toBytes(): ByteArray = JSONObject()
            .put("link", link)
            .apply {
                space?.let {
                    put(
                        "space",
                        JSONObject().put("name", it.name).put("uid", it.uid)
                            .put("displayName", it.displayName).put("memberCount", it.memberCount),
                    )
                }
            }
            .toString().toByteArray()

        companion object {
            fun fromBytes(bytes: ByteArray): SignIn? = runCatching {
                val json = JSONObject(String(bytes))
                val space = json.optJSONObject("space")?.let {
                    Space(it.getString("name"), it.getString("uid"), it.optString("displayName"), it.optLong("memberCount"))
                }
                SignIn(json.getString("link"), space)
            }.getOrNull()
        }
    }

    data class Alert(
        val kind: String,
        val title: String,
        val body: String,
        val channelUrl: String?,
        val personId: String?,
        val account: String?,
    ) {
        fun toBytes(): ByteArray = JSONObject()
            .putOpt("account", account)
            .put("kind", kind)
            .put("title", title)
            .put("body", body)
            .putOpt("channelUrl", channelUrl)
            .putOpt("personId", personId)
            .toString().toByteArray()

        companion object {
            fun fromBytes(bytes: ByteArray): Alert? = runCatching {
                val json = JSONObject(String(bytes))
                Alert(
                    kind = json.getString("kind"),
                    title = json.optString("title"),
                    body = json.optString("body"),
                    channelUrl = json.optString("channelUrl").ifEmpty { null },
                    personId = json.optString("personId").ifEmpty { null },
                    account = json.optString("account").ifEmpty { null },
                )
            }.getOrNull()
        }
    }

    data class CallCommand(
        val action: String,
        val floorId: String? = null,
        val zoneId: String? = null,
        val personId: String? = null,
        val mic: Boolean = false,
        val account: String? = null,
    ) {
        fun toBytes(): ByteArray = JSONObject()
            .put("action", action)
            .putOpt("account", account)
            .putOpt("floorId", floorId)
            .putOpt("zoneId", zoneId)
            .putOpt("personId", personId)
            .put("mic", mic)
            .toString().toByteArray()

        companion object {
            const val STATE = "state"
            const val JOIN_ROOM = "join_room"
            const val JOIN_PERSON = "join_person"
            const val MIC = "mic"
            const val LEAVE = "leave"

            fun fromBytes(bytes: ByteArray): CallCommand? = runCatching {
                val json = JSONObject(String(bytes))
                CallCommand(
                    action = json.getString("action"),
                    floorId = json.optString("floorId").ifEmpty { null },
                    zoneId = json.optString("zoneId").ifEmpty { null },
                    personId = json.optString("personId").ifEmpty { null },
                    mic = json.optBoolean("mic"),
                    account = json.optString("account").ifEmpty { null },
                )
            }.getOrNull()
        }
    }

    data class CallPerson(val key: String, val name: String, val isLocal: Boolean, val micOn: Boolean, val speaking: Boolean)

    /** Replies and pushes travel separately, so the watch keeps only the newest ([epoch], [seq]). */
    data class CallState(
        val phase: String,
        val status: String,
        val roomName: String,
        val floorId: String?,
        val zoneId: String?,
        val movable: Boolean,
        val micOn: Boolean,
        val people: List<CallPerson>,
        val error: String? = null,
        val epoch: Long = 0,
        val seq: Long = 0,
        val account: String? = null,
    ) {
        val active: Boolean get() = phase != IDLE && phase != FAILED
        val joined: Boolean get() = phase == JOINED

        fun toBytes(): ByteArray = JSONObject()
            .put("phase", phase)
            .put("status", status)
            .put("roomName", roomName)
            .putOpt("floorId", floorId)
            .putOpt("zoneId", zoneId)
            .put("movable", movable)
            .put("micOn", micOn)
            .putOpt("error", error)
            .put("epoch", epoch)
            .put("seq", seq)
            .putOpt("account", account)
            .put(
                "people",
                JSONArray(
                    people.map {
                        JSONObject().put("key", it.key).put("name", it.name).put("isLocal", it.isLocal)
                            .put("micOn", it.micOn).put("speaking", it.speaking)
                    },
                ),
            )
            .toString().toByteArray()

        companion object {
            const val IDLE = "idle"
            const val JOINED = "joined"
            const val FAILED = "failed"
            val NONE = CallState(IDLE, "Not in a call", "", null, null, false, false, emptyList())

            fun fromBytes(bytes: ByteArray): CallState? = runCatching {
                val json = JSONObject(String(bytes))
                val people = json.optJSONArray("people") ?: JSONArray()
                CallState(
                    phase = json.getString("phase"),
                    status = json.optString("status"),
                    roomName = json.optString("roomName"),
                    floorId = json.optString("floorId").ifEmpty { null },
                    zoneId = json.optString("zoneId").ifEmpty { null },
                    movable = json.optBoolean("movable"),
                    micOn = json.optBoolean("micOn"),
                    people = (0 until people.length()).map {
                        val p = people.getJSONObject(it)
                        CallPerson(
                            p.optString("key"), p.optString("name"), p.optBoolean("isLocal"),
                            p.optBoolean("micOn"), p.optBoolean("speaking"),
                        )
                    },
                    error = json.optString("error").ifEmpty { null },
                    epoch = json.optLong("epoch"),
                    seq = json.optLong("seq"),
                    account = json.optString("account").ifEmpty { null },
                )
            }.getOrNull()
        }
    }
}
