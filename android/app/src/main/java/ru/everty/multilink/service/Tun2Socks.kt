package ru.everty.multilink.service

import android.util.Log

object Tun2Socks {
    private const val TAG = "Tun2Socks"
    private var isLoaded = false

    init {
        try {
            System.loadLibrary("tun2socks")
            isLoaded = true
            Log.d(TAG, "libtun2socks.so loaded successfully")
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to load libtun2socks.so: ${e.message}", e)
        }
    }

    @JvmStatic
    external fun start(fd: Int, proxy: String, mtu: Int): Int

    @JvmStatic
    external fun stop(): Int

    fun safeStart(fd: Int, proxy: String, mtu: Int): Int {
        if (!isLoaded) {
            Log.e(TAG, "Cannot start: native tun2socks library is not loaded")
            return -1
        }
        return try {
            start(fd, proxy, mtu)
        } catch (t: Throwable) {
            Log.e(TAG, "tun2socks native start threw: ${t.message}", t)
            -1
        }
    }

    fun safeStop(): Int {
        if (!isLoaded) return 0
        return try {
            stop()
        } catch (t: Throwable) {
            Log.e(TAG, "tun2socks native stop threw: ${t.message}", t)
            -1
        }
    }
}
