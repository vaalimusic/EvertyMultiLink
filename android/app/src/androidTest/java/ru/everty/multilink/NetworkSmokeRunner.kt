package ru.everty.multilink

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.net.VpnService
import android.os.Bundle
import android.os.SystemClock
import java.io.File
import ru.everty.multilink.service.EvertyVpnService

/** Test APK entry point. HTTP probes run as the ADB shell UID outside the client. */
class NetworkSmokeRunner : Instrumentation() {
    private var options: Bundle? = null

    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        options = arguments
        start()
    }

    override fun onStart() {
        var started = false
        var stopFile: File? = null
        val result = Bundle()
        try {
            check(VpnService.prepare(targetContext) == null) {
                "Approve the VPN once from the debug app before running the test."
            }
            check(EvertyVpnService.currentState == EvertyVpnService.STATE_DISCONNECTED) {
                "Disconnect MultiLink before starting the test."
            }
            val token = options?.getString("token") ?: "manual"
            require(token.matches(Regex("[a-zA-Z0-9]{1,32}")))
            stopFile = File(targetContext.filesDir, "network-smoke-$token.stop")
            val seconds = options?.getString("windowSeconds")?.toLongOrNull()?.coerceIn(5, 300) ?: 65
            val connect = Intent(targetContext, EvertyVpnService::class.java)
                .setAction(EvertyVpnService.ACTION_CONNECT)
                .putExtra("testPolicy", options?.getString("policy"))
                .putExtra("testDiagnostics", true)
            targetContext.startForegroundService(connect)
            started = true
            val deadline = SystemClock.elapsedRealtime() + 15_000
            while (EvertyVpnService.currentState != EvertyVpnService.STATE_CONNECTED) {
                check(SystemClock.elapsedRealtime() < deadline) { "VPN did not connect within 15 seconds." }
                Thread.sleep(100)
            }
            sendStatus(1, Bundle().apply { putString("multilink", "ready") })
            val end = SystemClock.elapsedRealtime() + seconds * 1000
            while (!stopFile.exists() && SystemClock.elapsedRealtime() < end) {
                check(EvertyVpnService.currentState == EvertyVpnService.STATE_CONNECTED) {
                    "VPN disconnected during the test."
                }
                Thread.sleep(200)
            }
            result.putString("multilink", "completed")
        } catch (error: Exception) {
            result.putString("multilink", error.message ?: "test failed")
        } finally {
            if (started && EvertyVpnService.currentState != EvertyVpnService.STATE_DISCONNECTED) {
                targetContext.startService(Intent(targetContext, EvertyVpnService::class.java)
                    .setAction(EvertyVpnService.ACTION_DISCONNECT))
                val deadline = SystemClock.elapsedRealtime() + 5_000
                while (EvertyVpnService.currentState != EvertyVpnService.STATE_DISCONNECTED &&
                    SystemClock.elapsedRealtime() < deadline
                ) Thread.sleep(100)
                if (EvertyVpnService.currentState != EvertyVpnService.STATE_DISCONNECTED) {
                    result.putString("multilink", "VPN did not stop after the test.")
                }
            }
            stopFile?.delete()
            finish(if (result.getString("multilink") == "completed") Activity.RESULT_OK else Activity.RESULT_CANCELED,
                result)
        }
    }
}
