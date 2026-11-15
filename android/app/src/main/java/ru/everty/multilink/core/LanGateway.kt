package ru.everty.multilink.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import ru.everty.multilink.parser.ExtraNodes
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketException
import java.net.SocketTimeoutException
import java.util.UUID

internal object LanGateway {
    private const val discoveryPort = 18993
    private const val query = "EVERTY-LAN-1?"
    private const val reply = "EVERTY-LAN-1!"
    fun address(context: Context): String? {
        val manager = context.getSystemService(ConnectivityManager::class.java)
        return manager.allNetworks.asSequence().filter { network ->
            val capabilities = manager.getNetworkCapabilities(network) ?: return@filter false
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
        }.flatMap { manager.getLinkProperties(it)?.linkAddresses.orEmpty().asSequence() }
            .mapNotNull { it.address.hostAddress }.firstOrNull(ExtraNodes::privateIpv4)
    }

    fun link(key: String, address: String): String = "everty-lan://$key@$address:${ExtraNodes.LAN_PORT}"

    fun advertise(address: String): DatagramSocket {
        val socket = DatagramSocket(discoveryPort)
        Thread {
            val bytes = ByteArray(64)
            while (!socket.isClosed) {
                try {
                    val packet = DatagramPacket(bytes, bytes.size)
                    socket.receive(packet)
                    val peer = packet.address.hostAddress.orEmpty()
                    val message = String(bytes, 0, packet.length, Charsets.US_ASCII)
                    if (peer != address && ExtraNodes.privateIpv4(peer) &&
                        message.length == query.length + 32 && message.startsWith(query)
                    ) {
                        val response = (reply + message.removePrefix(query)).toByteArray(Charsets.US_ASCII)
                        socket.send(DatagramPacket(response, response.size, packet.socketAddress))
                    }
                } catch (_: SocketException) {
                    break
                } catch (_: java.io.IOException) {
                    // A Wi-Fi transition may interrupt a datagram.
                }
            }
        }.apply { isDaemon = true; start() }
        return socket
    }

    fun expand(context: Context, input: String): String {
        val key = input.trim().removePrefix("everty-lan://")
        if (key == input.trim() || !key.matches(Regex("[a-fA-F0-9]{32}"))) return input
        val local = address(context) ?: error("Подключитесь к той же Wi-Fi сети")
        val nonce = UUID.randomUUID().toString().replace("-", "")
        val bytes = (query + nonce).toByteArray(Charsets.US_ASCII)
        val found = mutableSetOf<String>()
        DatagramSocket(InetSocketAddress(local, 0)).use { socket ->
            socket.broadcast = true
            socket.soTimeout = 200
            repeat(2) {
                socket.send(DatagramPacket(bytes, bytes.size, InetAddress.getByName("255.255.255.255"), discoveryPort))
                val end = android.os.SystemClock.elapsedRealtime() + 650
                while (android.os.SystemClock.elapsedRealtime() < end) {
                    val response = DatagramPacket(ByteArray(64), 64)
                    try {
                        socket.receive(response)
                    } catch (_: SocketTimeoutException) {
                        continue
                    }
                    val peer = response.address.hostAddress.orEmpty()
                    if (response.port == discoveryPort && peer != local && ExtraNodes.privateIpv4(peer) &&
                        String(response.data, 0, response.length, Charsets.US_ASCII) == reply + nonce
                    ) found.add(peer)
                }
            }
        }
        return link(key, found.singleOrNull() ?: error(
            if (found.isEmpty()) "LAN-хост не найден. Используйте полную ссылку с IP."
            else "Найдено несколько LAN-хостов. Используйте полную ссылку с IP."
        ))
    }
}
