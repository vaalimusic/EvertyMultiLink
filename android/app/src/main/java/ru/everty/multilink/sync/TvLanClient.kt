package ru.everty.multilink.sync

import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.nio.charset.StandardCharsets
import java.util.concurrent.TimeUnit

object TvLanClient {

    private val client = OkHttpClient.Builder()
        .connectTimeout(5, TimeUnit.SECONDS)
        .readTimeout(8, TimeUnit.SECONDS)
        .build()

    fun sendConfigToTv(
        tvUrl: String,
        pin: String,
        config: String,
        subName: String? = null,
        subExpire: Long = 0L,
        disabledSignatures: List<String> = emptyList(),
        disabledNames: List<String> = emptyList(),
        callback: (Boolean, String) -> Unit
    ) {
        Thread {
            try {
                // If url has query pin or bare url, normalize
                val base = if (tvUrl.contains("?")) tvUrl.substringBefore("?") else tvUrl
                val targetUrl = if (base.endsWith("/")) "${base}api/import" else "$base/api/import"

                val json = JSONObject().apply {
                    put("config", config)
                    put("pin", pin)
                    if (!subName.isNullOrBlank()) put("subName", subName)
                    if (subExpire > 0L) put("subExpire", subExpire)
                    if (disabledSignatures.isNotEmpty()) {
                        val arr = JSONArray()
                        for (sig in disabledSignatures) arr.put(sig)
                        put("disabledSignatures", arr)
                    }
                    if (disabledNames.isNotEmpty()) {
                        val arr = JSONArray()
                        for (name in disabledNames) arr.put(name)
                        put("disabledNames", arr)
                    }
                }

                val body = json.toString().toRequestBody("application/json; charset=utf-8".toMediaType())
                val request = Request.Builder()
                    .url(targetUrl)
                    .post(body)
                    .build()

                client.newCall(request).execute().use { response ->
                    val respStr = response.body?.string() ?: ""
                    if (response.isSuccessful) {
                        val respJson = JSONObject(respStr)
                        val ok = respJson.optBoolean("ok", false)
                        val count = respJson.optInt("count", 0)
                        if (ok) {
                            callback(true, respJson.optString("message", "Успешно отправлено на устройство ($count узлов)"))
                        } else {
                            callback(false, respJson.optString("error", "Ошибка передачи"))
                        }
                    } else {
                        callback(false, "Код ошибки HTTP: ${response.code}")
                    }
                }
            } catch (e: Exception) {
                callback(false, e.localizedMessage ?: "Сбой соединения")
            }
        }.start()
    }

    fun listenForTvBroadcast(onTvDiscovered: (String, Int, String) -> Unit): DatagramSocket? {
        return try {
            val socket = DatagramSocket(8898)
            socket.broadcast = true
            Thread {
                val buffer = ByteArray(1024)
                while (!socket.isClosed) {
                    try {
                        val packet = DatagramPacket(buffer, buffer.size)
                        socket.receive(packet)
                        val msg = String(packet.data, 0, packet.length, StandardCharsets.UTF_8).trim()
                        if (msg.startsWith("EVERTY_TV:")) {
                            val parts = msg.split(":")
                            if (parts.size >= 4) {
                                val ip = parts[1]
                                val port = parts[2].toIntOrNull() ?: 8899
                                val pin = parts[3]
                                onTvDiscovered(ip, port, pin)
                            }
                        }
                    } catch (_: Exception) {
                        break
                    }
                }
            }.start()
            socket
        } catch (_: Exception) {
            null
        }
    }
}
