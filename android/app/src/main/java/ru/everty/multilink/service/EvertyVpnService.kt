package ru.everty.multilink.service

import android.app.Notification
import android.app.PendingIntent
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.core.app.NotificationCompat
import ru.everty.multilink.EvertyApp
import ru.everty.multilink.R
import ru.everty.multilink.core.MihomoConfigBuilder
import ru.everty.multilink.model.AppSettings
import ru.everty.multilink.ui.MainActivity
import java.io.BufferedReader
import java.io.File
import java.io.InputStreamReader
import java.util.concurrent.atomic.AtomicBoolean

class EvertyVpnService : VpnService() {

    companion object {
        private const val TAG = "EvertyVpnService"
        const val ACTION_CONNECT = "ru.everty.multilink.CONNECT"
        const val ACTION_DISCONNECT = "ru.everty.multilink.DISCONNECT"
        const val ACTION_STATE_CHANGED = "ru.everty.multilink.VPN_STATE_CHANGED"
        const val EXTRA_STATE = "vpn_state"
        const val STATE_DISCONNECTED = 0
        const val STATE_CONNECTING = 1
        const val STATE_CONNECTED = 2

        var currentState = STATE_DISCONNECTED
            private set
    }

    private var vpnInterface: ParcelFileDescriptor? = null
    private val isRunning = AtomicBoolean(false)
    private var workerThread: Thread? = null
    private var mihomoProcess: Process? = null
    private val isDisconnecting = AtomicBoolean(false)
    private var connectivityManager: android.net.ConnectivityManager? = null
    private var networkCallback: android.net.ConnectivityManager.NetworkCallback? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_DISCONNECT -> {
                disconnect()
                stopSelf()
            }
            ACTION_CONNECT -> {
                connect()
            }
        }
        return START_NOT_STICKY
    }

    private fun connect() {
        isDisconnecting.set(false)
        val settings = AppSettings(this)
        val selectedNode = settings.getSelectedNode()
        if (selectedNode == null || !selectedNode.isEnabled) {
            Log.w(TAG, "Cannot connect: no active node selected")
            broadcastState(STATE_DISCONNECTED)
            stopSelf()
            return
        }

        // Clean up any stale process or tun2socks session before connecting
        runCatching {
            mihomoProcess?.let {
                it.destroy()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    it.destroyForcibly()
                }
            }
        }
        mihomoProcess = null
        runCatching { Tun2Socks.stop() }

        broadcastState(STATE_CONNECTING)
        startForeground(1001, buildNotification("Подключение к ${selectedNode.name}…", false))

        workerThread = Thread {
            try {
                // 1. Prepare Mihomo configuration
                val mihomoDir = File(filesDir, "mihomo").apply { mkdirs() }
                val configFile = MihomoConfigBuilder.generateConfig(
                    outputDir = mihomoDir,
                    settings = settings
                )
                Log.d(TAG, "Generated Mihomo config at ${configFile.absolutePath}")

                // 2. Locate native binaries
                val nativeLibDir = applicationInfo.nativeLibraryDir
                val mihomoBin = File(nativeLibDir, "libmihomo.so")
                val actualMihomo = if (mihomoBin.exists()) mihomoBin.absolutePath else "/data/local/tmp/mihomo"

                // 3. Start Mihomo Core
                Log.d(TAG, "Starting Mihomo process: $actualMihomo -d ${mihomoDir.absolutePath}")
                val mihomoPb = ProcessBuilder(actualMihomo, "-d", mihomoDir.absolutePath)
                    .redirectErrorStream(true)
                val proc = mihomoPb.start()
                mihomoProcess = proc

                // Pipe Mihomo logs to Android logcat
                Thread {
                    try {
                        BufferedReader(InputStreamReader(proc.inputStream)).use { reader ->
                            var line: String?
                            while (reader.readLine().also { line = it } != null) {
                                Log.d("MihomoCore", line ?: "")
                            }
                        }
                    } catch (_: Exception) {}
                }.start()

                // Wait for Mihomo to bind 127.0.0.1:7890
                Thread.sleep(600)

                // 4. Establish Android VPN TUN interface
                val sessionTitle = settings.getForcedNode()?.name ?: "MultiLink: ${settings.policy.label}"
                val builder = Builder()
                    .setSession("Everty: $sessionTitle")
                    .addAddress("172.19.0.1", 30)
                    .addDnsServer("1.1.1.1")
                    .addDnsServer("8.8.8.8")
                    .setMtu(1400)
                    .addRoute("0.0.0.0", 0)

                // Intercept IPv6 to prevent cellular ISP leak
                try {
                    builder.addAddress("2001:db8:1234::1", 64)
                    builder.addRoute("::", 0)
                } catch (e: Exception) {
                    Log.w(TAG, "IPv6 route setup skipped: ${e.message}")
                }

                // Per-App Split Tunneling or default self-exclusion
                if (settings.isPerAppVpnEnabled && settings.perAppVpnPackages.isNotEmpty()) {
                    if (settings.perAppVpnMode == "INCLUDE") {
                        for (pkg in settings.perAppVpnPackages) {
                            try {
                                builder.addAllowedApplication(pkg)
                            } catch (e: Exception) {
                                Log.w(TAG, "addAllowedApplication failed for $pkg: ${e.message}")
                            }
                        }
                    } else {
                        for (pkg in settings.perAppVpnPackages) {
                            try {
                                builder.addDisallowedApplication(pkg)
                            } catch (e: Exception) {
                                Log.w(TAG, "addDisallowedApplication failed for $pkg: ${e.message}")
                            }
                        }
                        try {
                            builder.addDisallowedApplication(packageName)
                        } catch (_: Exception) {}
                    }
                } else {
                    // Prevent VPN loopback: Disallow this app so Mihomo's proxy traffic goes out directly
                    try {
                        builder.addDisallowedApplication(packageName)
                    } catch (e: Exception) {
                        Log.w(TAG, "addDisallowedApplication failed: ${e.message}")
                    }
                }

                vpnInterface = builder.establish()

                if (vpnInterface != null) {
                    val pfd = vpnInterface!!
                    val fd = ParcelFileDescriptor.dup(pfd.fileDescriptor).detachFd()
                    pfd.close()
                    vpnInterface = null
                    isRunning.set(true)

                    Log.d(TAG, "VPN TUN established successfully (fd=$fd), starting Tun2Socks...")

                    // 5. Start Tun2Socks in-process via JNI (MTU 1400 avoids mobile carrier packet fragmentation)
                    Tun2Socks.start(fd, "socks5://127.0.0.1:7890", 1400)

                    // 6. Monitor network changes (Wi-Fi <-> Mobile Data) for smooth handover
                    try {
                        connectivityManager = getSystemService(android.net.ConnectivityManager::class.java)
                        val cb = object : android.net.ConnectivityManager.NetworkCallback() {
                            override fun onAvailable(network: android.net.Network) {
                                setUnderlyingNetworks(null)
                            }
                            override fun onLost(network: android.net.Network) {
                                setUnderlyingNetworks(null)
                            }
                            override fun onCapabilitiesChanged(network: android.net.Network, networkCapabilities: android.net.NetworkCapabilities) {
                                setUnderlyingNetworks(null)
                            }
                        }
                        networkCallback = cb
                        connectivityManager?.registerDefaultNetworkCallback(cb)
                    } catch (e: Exception) {
                        Log.w(TAG, "Network callback registration skipped: ${e.message}")
                    }

                    broadcastState(STATE_CONNECTED)
                    startForeground(1001, buildNotification("Защищено через ${selectedNode.name}", true))
                    Log.d(TAG, "VPN Connected and Tun2Socks running.")

                    // Keep running while monitoring Mihomo process health
                    while (isRunning.get()) {
                        try {
                            Thread.sleep(1000)
                            val p = mihomoProcess
                            if (p != null && !p.isAlive) {
                                Log.e(TAG, "Mihomo process terminated unexpectedly (exit=${runCatching { p.exitValue() }.getOrNull()})")
                                if (isRunning.get()) {
                                    disconnect()
                                    stopSelf()
                                }
                                break
                            }
                        } catch (_: InterruptedException) {
                            break
                        }
                    }
                } else {
                    Log.e(TAG, "Failed to establish VPN interface (null)")
                    broadcastState(STATE_DISCONNECTED)
                    stopSelf()
                }
            } catch (e: InterruptedException) {
                // Service intentionally stopped
            } catch (e: Exception) {
                Log.e(TAG, "Error starting VPN: ${e.message}", e)
                broadcastState(STATE_DISCONNECTED)
                stopSelf()
            }
        }.apply { start() }
    }

    private fun disconnect() {
        if (isDisconnecting.getAndSet(true)) {
            Log.d(TAG, "Already disconnecting, skipping duplicate disconnect")
            return
        }
        Log.d(TAG, "Disconnecting VPN...")
        isRunning.set(false)
        workerThread?.interrupt()
        workerThread = null

        networkCallback?.let {
            runCatching { connectivityManager?.unregisterNetworkCallback(it) }
        }
        networkCallback = null
        connectivityManager = null

        runCatching { Tun2Socks.stop() }.onFailure { Log.e(TAG, "Error stopping Tun2Socks: ${it.message}") }
        runCatching {
            mihomoProcess?.let {
                it.destroy()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    it.destroyForcibly()
                }
            }
        }
        mihomoProcess = null

        runCatching { vpnInterface?.close() }
        vpnInterface = null

        broadcastState(STATE_DISCONNECTED)
        runCatching { stopForeground(STOP_FOREGROUND_REMOVE) }
        val nm = getSystemService(android.app.NotificationManager::class.java)
        runCatching { nm?.cancel(1001) }
        Log.d(TAG, "VPN Disconnected cleanly.")
    }

    private fun broadcastState(state: Int) {
        currentState = state
        val intent = Intent(ACTION_STATE_CHANGED).apply {
            putExtra(EXTRA_STATE, state)
            setPackage(packageName)
        }
        sendBroadcast(intent)
    }

    private fun buildNotification(text: String, isConnected: Boolean): Notification {
        val launchIntent = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val pendingIntent = PendingIntent.getActivity(
            this, 0, launchIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val disconnectIntent = Intent(this, EvertyVpnService::class.java).apply {
            action = ACTION_DISCONNECT
        }
        val disconnectPending = PendingIntent.getService(
            this, 1, disconnectIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val builder = NotificationCompat.Builder(this, EvertyApp.VPN_CHANNEL_ID)
            .setContentTitle("Everty MultiLink")
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_qs_everty)
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)

        if (isConnected) {
            builder.addAction(android.R.drawable.ic_menu_close_clear_cancel, "Отключить", disconnectPending)
        }

        return builder.build()
    }

    override fun onDestroy() {
        disconnect()
        super.onDestroy()
    }
}
