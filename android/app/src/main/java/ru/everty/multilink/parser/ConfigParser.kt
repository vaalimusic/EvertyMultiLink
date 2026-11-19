package ru.everty.multilink.parser

import android.net.Uri
import android.util.Base64
import ru.everty.multilink.model.VpnNode
import java.net.URLDecoder
import java.nio.charset.StandardCharsets
import java.util.UUID

object ConfigParser {

    fun isSupportedScheme(s: String): Boolean {
        val l = s.lowercase()
        return l.startsWith("vless://") || l.startsWith("hysteria2://") || l.startsWith("hy2://")
            || l.startsWith("trojan://") || l.startsWith("ss://") || l.startsWith("vmess://")
    }

    fun parse(rawInput: String): List<VpnNode> {
        val trimmed = rawInput.trim().removePrefix("\uFEFF").trim()
        if (trimmed.isEmpty() || trimmed.length > 4 * 1024 * 1024) return emptyList()
        if (trimmed.startsWith("everty-lan://")) {
            return runCatching { listOf(ExtraNodes.lan(trimmed)) }.getOrDefault(emptyList())
        }
        if (trimmed.startsWith("{") && !trimmed.contains("\"v\":")) {
            return runCatching { ExtraNodes.xray(org.json.JSONObject(trimmed)) }.getOrDefault(emptyList())
        }
        if (trimmed.lineSequence().take(20).any { it.trim() == "[Interface]" }) {
            return runCatching { listOf(ExtraNodes.wireguard(trimmed)) }.getOrDefault(emptyList())
        }

        // 1. Direct multi-line or single URI
        val lines = trimmed.lines().map { it.trim() }.filter { it.isNotEmpty() }
        val directNodes = lines.mapNotNull { parseUri(it) }
        if (directNodes.isNotEmpty()) return directNodes

        // 2. Try Base64 decoding
        try {
            val cleaned = trimmed.replace("\n", "").replace("\r", "").replace(" ", "")
            val decodedBytes = Base64.decode(cleaned, Base64.DEFAULT)
            val decodedStr = String(decodedBytes, StandardCharsets.UTF_8).trim()
            if (decodedStr.lines().any { isSupportedScheme(it.trim()) }) {
                return parse(decodedStr)
            }
        } catch (_: Exception) {}

        // 3. Fallback: single URI parse
        parseUri(trimmed)?.let { return listOf(it) }

        return emptyList()
    }

    fun parseUri(uriString: String): VpnNode? {
        val trimmed = uriString.trim()
        return when {
            trimmed.startsWith("vless://", ignoreCase = true) -> parseVlessUri(trimmed)
            trimmed.startsWith("hysteria2://", ignoreCase = true) || trimmed.startsWith("hy2://", ignoreCase = true) -> parseHysteria2Uri(trimmed)
            trimmed.startsWith("trojan://", ignoreCase = true) -> parseTrojanUri(trimmed)
            trimmed.startsWith("ss://", ignoreCase = true) -> parseShadowsocksUri(trimmed)
            trimmed.startsWith("vmess://", ignoreCase = true) -> parseVmessUri(trimmed)
            else -> null
        }
    }

    fun parseVlessUri(uriString: String): VpnNode? {
        return try {
            val uri = Uri.parse(uriString)
            if (uri.scheme?.lowercase() != "vless") return null

            val userInfo = uri.userInfo ?: ""
            val host = uri.host ?: return null
            val port = if (uri.port > 0) uri.port else 443
            val fragment = uri.fragment?.let {
                try { URLDecoder.decode(it, "UTF-8") } catch (_: Exception) { it }
            } ?: "$host:$port"

            val security = uri.getQueryParameter("security") ?: "reality"
            val sni = uri.getQueryParameter("sni") ?: uri.getQueryParameter("serverName") ?: host
            val pbk = uri.getQueryParameter("pbk") ?: uri.getQueryParameter("publicKey") ?: ""
            val sid = uri.getQueryParameter("sid") ?: uri.getQueryParameter("shortId") ?: ""
            val spx = uri.getQueryParameter("spx") ?: uri.getQueryParameter("spiderX") ?: ""
            val flow = uri.getQueryParameter("flow") ?: ""
            val type = uri.getQueryParameter("type") ?: "tcp"
            val path = uri.getQueryParameter("path") ?: ""

            var outbound: Map<String, Any>? = null
            if (type == "xhttp") {
                val xhttpOpts = mutableMapOf<String, Any>("path" to (if (path.isEmpty()) "/" else path))
                uri.getQueryParameter("host")?.let { xhttpOpts["headers"] = mapOf("Host" to it) }
                uri.getQueryParameter("mode")?.let { xhttpOpts["mode"] = it }
                uri.getQueryParameter("extra")?.let { xhttpOpts["extra"] = it }
                val out = mutableMapOf<String, Any>(
                    "type" to "vless",
                    "server" to host,
                    "port" to port,
                    "uuid" to userInfo,
                    "network" to "xhttp",
                    "udp" to true,
                    "tls" to true,
                    "client-fingerprint" to "chrome",
                    "xhttp-opts" to xhttpOpts
                )
                if (sni.isNotEmpty()) out["servername"] = sni
                if (flow.isNotEmpty()) out["flow"] = flow
                if (security == "reality" && pbk.isNotEmpty()) {
                    val ro = mutableMapOf<String, Any>("public-key" to pbk)
                    if (sid.isNotEmpty()) ro["short-id"] = sid
                    out["reality-opts"] = ro
                }
                outbound = out
            }

            VpnNode(
                id = UUID.randomUUID().toString(),
                name = fragment,
                server = host,
                port = port,
                uuid = userInfo,
                security = security,
                sni = sni,
                pbk = pbk,
                sid = sid,
                spx = spx,
                flow = flow,
                type = type,
                path = path,
                rawUri = uriString,
                outbound = outbound
            )
        } catch (e: Exception) {
            null
        }
    }

    fun parseHysteria2Uri(uriString: String): VpnNode? {
        return try {
            val uri = Uri.parse(uriString)
            val host = uri.host ?: return null
            val port = if (uri.port > 0) uri.port else 443
            val password = uri.userInfo ?: ""
            val fragment = uri.fragment?.let {
                try { URLDecoder.decode(it, "UTF-8") } catch (_: Exception) { it }
            } ?: "$host:$port"
            val sni = uri.getQueryParameter("sni") ?: host
            val obfs = uri.getQueryParameter("obfs") ?: ""
            val obfsPassword = uri.getQueryParameter("obfs-password") ?: ""
            val alpn = uri.getQueryParameter("alpn") ?: "h3"
            val insecure = uri.getQueryParameter("insecure") == "1" || uri.getQueryParameter("allowInsecure") == "1"

            val outboundMap = mutableMapOf<String, Any>(
                "type" to "hysteria2",
                "server" to host,
                "port" to port,
                "password" to password,
                "sni" to sni,
                "skip-cert-verify" to insecure
            )
            if (obfs.isNotEmpty()) outboundMap["obfs"] = obfs
            if (obfsPassword.isNotEmpty()) outboundMap["obfs-password"] = obfsPassword
            if (alpn.isNotEmpty()) outboundMap["alpn"] = alpn.split(",")

            VpnNode(
                id = UUID.randomUUID().toString(),
                name = fragment,
                server = host,
                port = port,
                uuid = password,
                security = "tls",
                sni = sni,
                type = "hysteria2",
                rawUri = uriString,
                outbound = outboundMap
            )
        } catch (_: Exception) {
            null
        }
    }

    fun parseTrojanUri(uriString: String): VpnNode? {
        return try {
            val uri = Uri.parse(uriString)
            val host = uri.host ?: return null
            val port = if (uri.port > 0) uri.port else 443
            val password = uri.userInfo ?: return null
            val fragment = uri.fragment?.let {
                try { URLDecoder.decode(it, "UTF-8") } catch (_: Exception) { it }
            } ?: "$host:$port"
            val sni = uri.getQueryParameter("sni") ?: host
            val net = uri.getQueryParameter("type") ?: "tcp"
            val path = uri.getQueryParameter("path") ?: ""
            val insecure = uri.getQueryParameter("allowInsecure") == "1" || uri.getQueryParameter("insecure") == "1"

            val outboundMap = mutableMapOf<String, Any>(
                "type" to "trojan",
                "server" to host,
                "port" to port,
                "password" to password,
                "sni" to sni,
                "udp" to true,
                "skip-cert-verify" to insecure
            )
            if (net == "ws") {
                outboundMap["network"] = "ws"
                val wsOpts = mutableMapOf<String, Any>("path" to (if (path.isEmpty()) "/" else path))
                uri.getQueryParameter("host")?.let { wsOpts["headers"] = mapOf("Host" to it) }
                outboundMap["ws-opts"] = wsOpts
            } else if (net == "grpc") {
                outboundMap["network"] = "grpc"
                outboundMap["grpc-opts"] = mapOf("grpc-service-name" to (uri.getQueryParameter("serviceName") ?: ""))
            }

            VpnNode(
                id = UUID.randomUUID().toString(),
                name = fragment,
                server = host,
                port = port,
                uuid = password,
                security = "tls",
                sni = sni,
                type = net,
                path = path,
                rawUri = uriString,
                outbound = outboundMap
            )
        } catch (_: Exception) {
            null
        }
    }

    fun parseShadowsocksUri(uriString: String): VpnNode? {
        return try {
            val raw = uriString.removePrefix("ss://")
            val hashIdx = raw.indexOf('#')
            val body = if (hashIdx >= 0) raw.substring(0, hashIdx) else raw
            val fragment = if (hashIdx >= 0) {
                try { URLDecoder.decode(raw.substring(hashIdx + 1), "UTF-8") } catch (_: Exception) { raw.substring(hashIdx + 1) }
            } else ""

            val atIdx = body.lastIndexOf('@')
            val (userInfo, hostPort) = if (atIdx >= 0) {
                val uPart = body.substring(0, atIdx)
                val decodedUser = try {
                    String(Base64.decode(uPart, Base64.DEFAULT), StandardCharsets.UTF_8)
                } catch (_: Exception) { uPart }
                Pair(decodedUser, body.substring(atIdx + 1))
            } else {
                val decodedAll = String(Base64.decode(body, Base64.DEFAULT), StandardCharsets.UTF_8)
                val at = decodedAll.lastIndexOf('@')
                if (at < 0) return null
                Pair(decodedAll.substring(0, at), decodedAll.substring(at + 1))
            }

            val colonU = userInfo.indexOf(':')
            if (colonU < 0) return null
            val cipher = userInfo.substring(0, colonU)
            val password = userInfo.substring(colonU + 1)

            val colonHp = hostPort.lastIndexOf(':')
            if (colonHp < 0) return null
            val host = hostPort.substring(0, colonHp).trim('[', ']')
            val portPart = hostPort.substring(colonHp + 1)
            val port = portPart.takeWhile { it.isDigit() }.toIntOrNull() ?: return null

            val name = if (fragment.isNotEmpty()) fragment else "$host:$port"
            val outboundMap = mapOf<String, Any>(
                "type" to "ss",
                "server" to host,
                "port" to port,
                "cipher" to cipher,
                "password" to password,
                "udp" to true
            )

            VpnNode(
                id = UUID.randomUUID().toString(),
                name = name,
                server = host,
                port = port,
                uuid = password,
                security = cipher,
                type = "ss",
                rawUri = uriString,
                outbound = outboundMap
            )
        } catch (_: Exception) {
            null
        }
    }

    fun parseVmessUri(uriString: String): VpnNode? {
        return try {
            val b64 = uriString.removePrefix("vmess://").trim()
            val decodedStr = String(Base64.decode(b64, Base64.DEFAULT), StandardCharsets.UTF_8)
            val json = org.json.JSONObject(decodedStr)
            val host = json.getString("add")
            val port = json.getInt("port")
            val uuid = json.getString("id")
            val alterId = json.optInt("aid", 0)
            val net = json.optString("net", "tcp")
            val tls = json.optString("tls", "")
            val sni = json.optString("sni", json.optString("host", host))
            val path = json.optString("path", "")
            val name = json.optString("ps", "$host:$port")

            val outboundMap = mutableMapOf<String, Any>(
                "type" to "vmess",
                "server" to host,
                "port" to port,
                "uuid" to uuid,
                "alterId" to alterId,
                "cipher" to "auto",
                "udp" to true
            )
            if (tls.equals("tls", ignoreCase = true)) {
                outboundMap["tls"] = true
                if (sni.isNotEmpty()) outboundMap["servername"] = sni
            }
            if (net == "ws") {
                outboundMap["network"] = "ws"
                val wsOpts = mutableMapOf<String, Any>("path" to (if (path.isEmpty()) "/" else path))
                val h = json.optString("host", "")
                if (h.isNotEmpty()) wsOpts["headers"] = mapOf("Host" to h)
                outboundMap["ws-opts"] = wsOpts
            } else if (net == "grpc") {
                outboundMap["network"] = "grpc"
                outboundMap["grpc-opts"] = mapOf("grpc-service-name" to path)
            }

            VpnNode(
                id = UUID.randomUUID().toString(),
                name = name,
                server = host,
                port = port,
                uuid = uuid,
                security = if (tls.isNotEmpty()) tls else "none",
                sni = sni,
                type = net,
                path = path,
                rawUri = uriString,
                outbound = outboundMap
            )
        } catch (_: Exception) {
            null
        }
    }
}
