package com.kravtsov.kumowatch

import android.content.Context
import uniffi.kumo_ffi.RelayPairing
import uniffi.kumo_ffi.RelayStorage

class RelayPairingStorage(context: Context) : RelayStorage {
    private val prefs = context.getSharedPreferences("kumo_relay_link", Context.MODE_PRIVATE)
    private val secrets = KeystoreStorage(context, "relay")

    override fun load(): RelayPairing? {
        val url = prefs.getString("url", null) ?: return null
        val secret = secrets.load() ?: return null
        return RelayPairing(url, secret)
    }

    override fun save(pairing: RelayPairing) {
        prefs.edit().putString("url", pairing.url).apply()
        secrets.save(pairing.secret)
    }

    override fun clear() {
        prefs.edit().remove("url").apply()
        secrets.clear()
    }

    override fun saveMirroring(enabled: Boolean) = Unit
}
