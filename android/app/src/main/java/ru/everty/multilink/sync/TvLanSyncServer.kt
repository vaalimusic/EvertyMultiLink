package ru.everty.multilink.sync

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import ru.everty.multilink.parser.ConfigParser
import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.NetworkInterface
import java.net.ServerSocket
import java.net.Socket
import java.nio.charset.StandardCharsets
import java.util.Collections
import java.util.Random
import java.util.concurrent.Executors

class TvLanSyncServer(
    private val context: Context,
    private val onConfigImported: (
        config: String,
        count: Int,
        subName: String?,
        subExpire: Long,
        disabledSignatures: List<String>,
        disabledNames: List<String>
    ) -> Unit
) {
    private var serverSocket: ServerSocket? = null
    private val executor = Executors.newFixedThreadPool(4)
    private val mainHandler = Handler(Looper.getMainLooper())
    private var isRunning = false
    private var broadcastSocket: DatagramSocket? = null
    private var broadcastThread: Thread? = null

    var serverPort: Int = 8899
        private set
    var pinCode: String = ""
        private set
    var localIp: String = "127.0.0.1"
        private set

    fun start(): Boolean {
        if (isRunning) return true
        return try {
            localIp = getLocalWifiIpAddress()
            pinCode = String.format("%04d", Random().nextInt(10000))
            serverSocket = ServerSocket(serverPort)
            isRunning = true

            executor.execute {
                while (isRunning && serverSocket != null && !serverSocket!!.isClosed) {
                    try {
                        val client = serverSocket!!.accept()
                        executor.execute { handleClient(client) }
                    } catch (_: Exception) {
                        break
                    }
                }
            }

            startUdpBeacon()
            true
        } catch (e: Exception) {
            e.printStackTrace()
            false
        }
    }

    fun stop() {
        isRunning = false
        try {
            serverSocket?.close()
        } catch (_: Exception) {}
        serverSocket = null

        try {
            broadcastSocket?.close()
        } catch (_: Exception) {}
        broadcastSocket = null
        broadcastThread?.interrupt()
        runCatching { executor.shutdownNow() }
    }

    fun getSyncUrl(): String {
        return "http://$localIp:$serverPort?pin=$pinCode"
    }

    private fun handleClient(socket: Socket) {
        try {
            val reader = BufferedReader(InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))
            val line = reader.readLine() ?: return
            val parts = line.split(" ")
            if (parts.size < 2) return
            val method = parts[0]
            val path = parts[1]

            var contentLength = 0
            var headerLine: String?
            while (reader.readLine().also { headerLine = it } != null) {
                if (headerLine!!.isEmpty()) break
                if (headerLine!!.lowercase().startsWith("content-length:")) {
                    contentLength = headerLine!!.substring(15).trim().toIntOrNull() ?: 0
                }
            }

            val out = socket.getOutputStream()

            if (method == "GET") {
                val html = getHtmlPage()
                sendResponse(out, 200, "OK", "text/html; charset=UTF-8", html)
            } else if (method == "POST" && path.startsWith("/api/import")) {
                val body = CharArray(contentLength)
                var readTotal = 0
                while (readTotal < contentLength) {
                    val count = reader.read(body, readTotal, contentLength - readTotal)
                    if (count <= 0) break
                    readTotal += count
                }
                val jsonStr = String(body)
                val json = JSONObject(jsonStr)
                val reqPin = json.optString("pin")
                val config = json.optString("config")
                val subName = json.optString("subName").takeIf { it.isNotBlank() }
                val subExpire = json.optLong("subExpire", 0L)
                val disabledSignatures = mutableListOf<String>()
                json.optJSONArray("disabledSignatures")?.let { arr ->
                    for (i in 0 until arr.length()) {
                        disabledSignatures.add(arr.getString(i))
                    }
                }
                val disabledNames = mutableListOf<String>()
                json.optJSONArray("disabledNames")?.let { arr ->
                    for (i in 0 until arr.length()) {
                        disabledNames.add(arr.getString(i))
                    }
                }

                if (reqPin != pinCode && pinCode.isNotEmpty()) {
                    val resp = JSONObject().apply {
                        put("ok", false)
                        put("error", "Неверный ПИН-код")
                    }
                    sendResponse(out, 403, "Forbidden", "application/json", resp.toString())
                } else {
                    var parsedNodes = ConfigParser.parse(config)
                    if (parsedNodes.isEmpty() && (config.startsWith("http://") || config.startsWith("https://"))) {
                        try {
                            val cleanUrl = if (config.contains("#")) config.substringBefore("#") else config
                            val okClient = okhttp3.OkHttpClient()
                            val req = okhttp3.Request.Builder()
                                .url(cleanUrl)
                                .header("User-Agent", "EvertyMultiLink/1.0 (v2rayng)")
                                .build()
                            val subBody = okClient.newCall(req).execute().body?.string() ?: ""
                            parsedNodes = ConfigParser.parse(subBody)
                        } catch (e: Exception) {
                            e.printStackTrace()
                        }
                    }

                    mainHandler.post {
                        onConfigImported(config, parsedNodes.size, subName, subExpire, disabledSignatures, disabledNames)
                    }
                    val resp = JSONObject().apply {
                        put("ok", true)
                        put("count", parsedNodes.size)
                    }
                    sendResponse(out, 200, "OK", "application/json", resp.toString())
                }
            } else {
                sendResponse(out, 404, "Not Found", "text/plain", "Not Found")
            }
        } catch (e: Exception) {
            e.printStackTrace()
        } finally {
            try { socket.close() } catch (_: Exception) {}
        }
    }

    private fun sendResponse(out: OutputStream, code: Int, status: String, contentType: String, content: String) {
        val bytes = content.toByteArray(StandardCharsets.UTF_8)
        val header = "HTTP/1.1 $code $status\r\n" +
                "Content-Type: $contentType\r\n" +
                "Content-Length: ${bytes.size}\r\n" +
                "Access-Control-Allow-Origin: *\r\n" +
                "Connection: close\r\n\r\n"
        out.write(header.toByteArray(StandardCharsets.UTF_8))
        out.write(bytes)
        out.flush()
    }

    private fun startUdpBeacon() {
        broadcastThread = Thread {
            try {
                broadcastSocket = DatagramSocket()
                broadcastSocket?.broadcast = true
                val beaconData = "EVERTY_TV:$localIp:$serverPort:$pinCode".toByteArray(StandardCharsets.UTF_8)
                val packet = DatagramPacket(
                    beaconData,
                    beaconData.size,
                    InetAddress.getByName("255.255.255.255"),
                    8898
                )
                while (isRunning) {
                    try {
                        broadcastSocket?.send(packet)
                        Thread.sleep(2500)
                    } catch (_: Exception) {
                        break
                    }
                }
            } catch (e: Exception) {
                e.printStackTrace()
            }
        }.apply { start() }
    }

    private fun getLocalWifiIpAddress(): String {
        try {
            val interfaces = Collections.list(NetworkInterface.getNetworkInterfaces())
            for (intf in interfaces) {
                if (intf.name.startsWith("dummy") || intf.isLoopback) continue
                val addrs = Collections.list(intf.inetAddresses)
                for (addr in addrs) {
                    if (!addr.isLoopbackAddress && addr.hostAddress.indexOf(':') < 0) {
                        val ip = addr.hostAddress
                        if (ip.startsWith("192.168.") || ip.startsWith("10.") || ip.startsWith("172.")) {
                            return ip
                        }
                    }
                }
            }
        } catch (_: Exception) {}
        return "127.0.0.1"
    }

    private fun getHtmlPage(): String {
        return """
            <!DOCTYPE html>
            <html lang="ru">
            <head>
              <meta charset="UTF-8">
              <meta name="viewport" content="width=device-width, initial-scale=1">
              <title>Everty MultiLink TV Sync</title>
              <style>
                * { box-sizing: border-box; margin: 0; padding: 0; }
                body { background: #0B0F19; color: #F8FAFC; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; display: flex; align-items: center; justify-content: center; min-height: 100vh; padding: 20px; }
                .card { background: #151B28; border: 1px solid #2A354D; border-radius: 20px; padding: 28px; width: 100%; max-width: 480px; box-shadow: 0 10px 30px rgba(0,0,0,0.5); }
                h1 { color: #38BDF8; font-size: 22px; margin-bottom: 8px; text-align: center; }
                p { color: #94A3B8; font-size: 14px; text-align: center; margin-bottom: 20px; line-height: 1.4; }
                label { display: block; font-size: 12px; font-weight: bold; color: #94A3B8; margin-bottom: 6px; text-transform: uppercase; letter-spacing: 0.05em; }
                textarea { width: 100%; height: 130px; background: #0B0F19; border: 1px solid #2A354D; border-radius: 12px; color: #F8FAFC; padding: 12px; font-size: 13px; resize: none; margin-bottom: 16px; outline: none; }
                textarea:focus, input:focus { border-color: #38BDF8; }
                input { width: 100%; height: 46px; background: #0B0F19; border: 1px solid #2A354D; border-radius: 12px; color: #F8FAFC; padding: 10px 14px; font-size: 16px; text-align: center; font-weight: bold; margin-bottom: 20px; outline: none; letter-spacing: 2px; }
                button { width: 100%; height: 52px; background: #2563EB; border: none; border-radius: 14px; color: #fff; font-size: 15px; font-weight: bold; cursor: pointer; transition: background 0.2s; letter-spacing: 0.05em; }
                button:hover { background: #1D4ED8; }
                .status-box { margin-top: 16px; padding: 12px; border-radius: 10px; text-align: center; font-size: 14px; display: none; }
                .success { background: rgba(16, 185, 129, 0.15); color: #10B981; border: 1px solid #10B981; }
                .error { background: rgba(239, 68, 68, 0.15); color: #EF4444; border: 1px solid #EF4444; }
              </style>
            </head>
            <body>
              <div class="card">
                <h1>📺 Everty MultiLink TV</h1>
                <p>Быстрый перенос подписки или VLESS на экран ТВ без мышки и набора с пульта</p>
                
                <label>Конфигурация или ссылка подписки</label>
                <textarea id="config" placeholder="Вставьте vless://... или https://..."></textarea>
                
                <label>ПИН-код подтверждения</label>
                <input type="text" id="pin" value="$pinCode" maxlength="4">
                
                <button onclick="submitConfig()">ОТПРАВИТЬ НА ЭКРАН ТВ</button>
                <div id="status" class="status-box"></div>
              </div>

              <script>
                function submitConfig() {
                  const config = document.getElementById('config').value.trim();
                  const pin = document.getElementById('pin').value.trim();
                  const status = document.getElementById('status');
                  
                  if (!config) {
                    alert('Пожалуйста, вставьте ссылку или vless-конфиг');
                    return;
                  }
                  
                  status.style.display = 'block';
                  status.className = 'status-box';
                  status.style.color = '#38BDF8';
                  status.innerText = 'Отправка на ТВ...';
                  
                  fetch('/api/import', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ config: config, pin: pin })
                  })
                  .then(r => r.json())
                  .then(data => {
                    if (data.ok) {
                      status.className = 'status-box success';
                      status.innerText = '✅ Успешно! Добавлено узлов на ТВ: ' + data.count;
                      document.getElementById('config').value = '';
                    } else {
                      status.className = 'status-box error';
                      status.innerText = '❌ Ошибка: ' + (data.error || 'Неверный ПИН');
                    }
                  })
                  .catch(err => {
                    status.className = 'status-box error';
                    status.innerText = '❌ Ошибка отправки: ' + err.message;
                  });
                }
              </script>
            </body>
            </html>
        """.trimIndent()
    }
}
