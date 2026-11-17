package com.kravtsov.kumowatch

import android.app.Application

class KumoApplication : Application() {
    lateinit var state: AppState
        private set

    override fun onCreate() {
        super.onCreate()
        state = AppState(this)
    }
}
