package com.localnotebook.agent.core

interface CoreEventListener {
    fun onCoreEvent(eventJson: String)
}

object NativeBridge {
    @Volatile
    var isLoaded: Boolean = false
        private set

    init {
        try {
            System.loadLibrary("lna_ffi")
            isLoaded = true
        } catch (_: UnsatisfiedLinkError) {
            isLoaded = false
        }
    }

    external fun nativeOpen(dataDir: String, listener: CoreEventListener): Long
    external fun nativeCall(handle: Long, requestJson: String): String
    external fun nativeClose(handle: Long)
}
