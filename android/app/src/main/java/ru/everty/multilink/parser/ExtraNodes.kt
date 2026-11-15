package ru.everty.multilink.parser

import android.net.Uri
import android.util.Base64
import ru.everty.multilink.model.VpnNode
import java.util.UUID
import org.json.JSONObject

internal object ExtraNodes {
    const val LAN_PORT = 18992
    const val LAN_CIPHER = "chacha20-ietf-poly1305"

    fun privateIpv4(host: String): Boolean {
        val parts = host.split('.').map { it.toIntOrNull() ?: return false }
        if (parts.size != 4 || parts.any { it !in 0..255 }) return false
        return parts[0] == 10 || (parts[0] == 172 && parts[1] in 16..31) ||
            (parts[0] == 192 && parts[1] == 168)
    }

    fun lan(input: String): VpnNode {
        val uri = Uri.parse(input)
        val address = requireNotNull(uri.host)
        val key = requireNotNull(uri.encodedUserInfo)
        require(privateIpv4(address) && uri.port == LAN_PORT &&
            key.matches(Regex("[a-fA-F0-9]{32}")) && uri.path.isNullOrEmpty() &&
            uri.query == null && uri.fragment == null)
        return VpnNode(UUID.randomUUID().toString(), "LAN · $address", address, LAN_PORT, key,
            security = "lan", rawUri = input, outbound = mapOf("type" to "ss", "server" to address,
                "port" to LAN_PORT, "cipher" to LAN_CIPHER, "password" to key, "udp" to true))
    }

    fun xray(input: JSONObject): List<VpnNode> {
        val outbounds = input.getJSONArray("outbounds")
        val result = mutableListOf<VpnNode>()
        for (i in 0 until outbounds.length()) {
            val entry = outbounds.getJSONObject(i)
            if (entry.optString("protocol") != "vless") continue
            val stream = entry.optJSONObject("streamSettings") ?: JSONObject()
            val network = when (val value = stream.optString("network", "tcp")) {
                "tcp", "raw" -> "tcp"
                "ws", "grpc" -> value
                else -> error("Unsupported transport")
            }
            require(listOf("rawSettings", "tcpSettings").none {
                stream.optJSONObject(it)?.optJSONObject("header")?.optString("type", "none")
                    ?.let { value -> value != "none" } == true
            })
            val security = stream.optString("security", "none")
            require(security in setOf("none", "tls", "reality"))
            val tls = stream.optJSONObject("${security}Settings") ?: JSONObject()
            require(!tls.optBoolean("allowInsecure") && tls.optString("mldsa65Verify").isEmpty())
            val peers = entry.getJSONObject("settings").getJSONArray("vnext")
            for (p in 0 until peers.length()) {
                val peer = peers.getJSONObject(p)
                val host = peer.getString("address")
                require(host.matches(Regex("[a-zA-Z0-9.:-]+")))
                val port = peer.getInt("port")
                require(port in 1..65535)
                val users = peer.getJSONArray("users")
                for (u in 0 until users.length()) {
                    val user = users.getJSONObject(u)
                    val id = UUID.fromString(user.getString("id")).toString()
                    require(user.optString("encryption", "none") == "none")
                    val name = entry.optString("tag").ifBlank { "VLESS · $host" }
                    val config = mutableMapOf<String, Any>("type" to "vless", "server" to host, "port" to port,
                        "uuid" to id, "network" to network, "udp" to true)
                    user.optString("flow").takeIf(String::isNotEmpty)?.let { config["flow"] = it }
                    if (security != "none") {
                        config["tls"] = true
                        tls.optString("serverName").takeIf(String::isNotEmpty)?.let { config["servername"] = it }
                        tls.optString("fingerprint").takeIf(String::isNotEmpty)?.let { config["client-fingerprint"] = it }
                    }
                    if (security == "reality") {
                        val publicKey = tls.optString("publicKey").ifBlank { tls.getString("password") }
                        require(publicKey.isNotBlank())
                        config["reality-opts"] = mapOf("public-key" to publicKey, "short-id" to tls.optString("shortId"))
                    }
                    if (network == "ws") {
                        val ws = stream.optJSONObject("wsSettings") ?: JSONObject()
                        val options = mutableMapOf<String, Any>("path" to ws.optString("path", "/"))
                        val header = ws.optString("host").ifBlank { ws.optJSONObject("headers")?.optString("Host").orEmpty() }
                        if (header.isNotEmpty()) options["headers"] = mapOf("Host" to header)
                        config["ws-opts"] = options
                    }
                    if (network == "grpc") {
                        val grpc = stream.optJSONObject("grpcSettings") ?: JSONObject()
                        require(!grpc.optBoolean("multiMode"))
                        config["grpc-opts"] = mapOf("grpc-service-name" to grpc.optString("serviceName"))
                    }
                    result += VpnNode(UUID.randomUUID().toString(), name, host, port, id,
                        security = security, type = network, outbound = config)
                    require(result.size <= 2000)
                }
            }
        }
        return result
    }

    fun wireguard(input: String): VpnNode {
        val sections = mutableMapOf<String, MutableMap<String, String>>()
        var current: MutableMap<String, String>? = null
        for (raw in input.lineSequence()) {
            val line = raw.substringBefore('#').substringBefore(';').trim()
            if (line.isEmpty()) continue
            if (line == "[Interface]" || line == "[Peer]") {
                val next = mutableMapOf<String, String>()
                require(sections.put(line, next) == null)
                current = next
            } else {
                val parts = line.split('=', limit = 2)
                require(parts.size == 2)
                require(requireNotNull(current).put(parts[0].trim().lowercase(), parts[1].trim()) == null)
            }
        }
        val local = sections.getValue("[Interface]")
        val peer = sections.getValue("[Peer]")
        fun key(value: String): String {
            require(Base64.decode(value, Base64.DEFAULT).size == 32)
            return value
        }
        val endpoint = peer.getValue("endpoint")
        val address = endpoint.substringBeforeLast(':').removeSurrounding("[", "]")
        require(address.matches(Regex("[a-zA-Z0-9.:-]+")))
        val port = endpoint.substringAfterLast(':').toInt()
        require(port in 1..65535)
        fun ip(value: String): Boolean = android.system.Os.inet_pton(
            if (':' in value) android.system.OsConstants.AF_INET6 else android.system.OsConstants.AF_INET,
            value
        ) != null
        fun cidr(value: String): Boolean {
            val host = value.substringBefore('/')
            val prefix = value.substringAfter('/', "").toIntOrNull() ?: return false
            return ip(host) && prefix in 0..(if (':' in host) 128 else 32)
        }
        val allowed = peer.getValue("allowedips").split(',').map(String::trim)
        require(allowed.all(::cidr))
        val privateKey = key(local.getValue("privatekey"))
        val proxy = mutableMapOf<String, Any>("type" to "wireguard", "server" to address,
            "port" to port, "udp" to true, "private-key" to privateKey, "allowed-ips" to allowed,
            "public-key" to key(peer.getValue("publickey")))
        local.getValue("address").split(',').map(String::trim).forEach { value ->
            require(cidr(value))
            val host = value.substringBefore('/')
            require(proxy.put(if (':' in host) "ipv6" else "ip", host) == null)
        }
        peer["presharedkey"]?.let { proxy["pre-shared-key"] = key(it) }
        local["mtu"]?.let { proxy["mtu"] = it.toInt().also { v -> require(v in 1..65535) } }
        local["dns"]?.let {
            val servers = it.split(',').map(String::trim)
            require(servers.all(::ip))
            proxy["dns"] = servers
            proxy["remote-dns-resolve"] = true
        }
        peer["persistentkeepalive"]?.takeUnless { it.equals("off", true) }?.let {
            val low = it.substringBefore('-').trim().toInt()
            val high = it.substringAfter('-', it).trim().toInt()
            require(low in 0..high && high <= 65535)
            proxy["persistent-keepalive"] = low + (high - low) / 2
        }
        val v3 = mapOf("headerprotectionkey" to "header-protection-key",
            "contentpaddingaddition" to "content-padding-addition", "rekeyaftertime" to "rekey-after-time",
            "rekeytimeout" to "rekey-timeout", "rejectaftertime" to "reject-after-time",
            "keepalivetimeout" to "keepalive-timeout", "maxhandshakeattempts" to "max-handshake-attempts",
            "randomtrailers" to "random-trailers", "disablecookies" to "disable-cookies")
        val options = mutableMapOf<String, Any>()
        for ((name, value) in local) {
            when (name) {
                "address", "dns", "privatekey", "mtu", "listenport" -> Unit
                "jc", "jmin", "jmax", "s1", "s2", "s3", "s4", "itime" ->
                    options[name] = value.toLong().also { require(it in 0..4294967295) }
                "h1", "h2", "h3", "h4", "i1", "i2", "i3", "i4", "i5", "j1", "j2", "j3" ->
                    options[name] = value
                "randomtrailers", "disablecookies" -> options[v3.getValue(name)] =
                    when (value) { "true", "1" -> true; "false", "0" -> false; else -> error("invalid flag") }
                else -> options[v3.getValue(name)] = value
            }
        }
        require(peer.keys.all { it in setOf("endpoint", "publickey", "presharedkey", "allowedips", "persistentkeepalive") })
        val format = if (options.isEmpty()) "wireguard" else "amneziawg"
        if (options.isNotEmpty()) {
            if (local.keys.any(v3::containsKey)) options["version"] = 3
            proxy["amnezia-wg-option"] = options
        }
        return VpnNode(UUID.randomUUID().toString(), "$format · $address", address, port, privateKey,
            security = format, outbound = proxy)
    }
}
