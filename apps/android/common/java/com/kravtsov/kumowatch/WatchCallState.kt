package com.kravtsov.kumowatch

import uniffi.kumo_ffi.CallPhase
import uniffi.kumo_ffi.CallSource
import uniffi.kumo_ffi.CallView
import uniffi.kumo_ffi.SpaceView
import uniffi.kumo_ffi.callFailureLabel

/** [space] is this connection's census: its own zone is the call's room only for an Office call. */
fun CallView.toWatchState(space: SpaceView?, error: String? = null): WatchLink.CallState {
    val office = source != CallSource.DESKTOP && phase != CallPhase.Idle
    val zone = space?.selfZoneId?.takeIf { office }
    val floor = zone?.let { z -> space.floors.firstOrNull { f -> f.rooms.any { it.zoneId == z } }?.id }
    return WatchLink.CallState(
        phase = when (phase) {
            CallPhase.Idle -> WatchLink.CallState.IDLE
            CallPhase.Waiting -> "waiting"
            CallPhase.Joining -> "joining"
            CallPhase.Joined -> WatchLink.CallState.JOINED
            is CallPhase.Failed -> WatchLink.CallState.FAILED
        },
        status = when (val p = phase) {
            CallPhase.Idle -> "Not in a call"
            CallPhase.Waiting -> "Waiting…"
            CallPhase.Joining -> "Joining…"
            CallPhase.Joined -> "Connected"
            is CallPhase.Failed -> callFailureLabel(p.reason)
        },
        roomName = roomName,
        floorId = floor,
        zoneId = zone,
        movable = phase == CallPhase.Joined && floor != null,
        micOn = micOn,
        people = tiles.filter { it.inCall }.map { WatchLink.CallPerson(it.key, it.name, it.isLocal, it.micOn, it.speaking) },
        error = error,
    )
}
