package ru.everty.multilink.parser

import android.net.Uri
import android.util.Base64
import ru.everty.multilink.model.VpnNode
import java.net.URLDecoder
import java.nio.charset.StandardCharsets
import java.util.UUID

object ConfigParser {

    fun parse(rawInput: String): List<VpnNode> {
        val trimmed = rawInput.trim().removePrefix("\uFEFF").trim()
        if (trimmed.isEmpty() || trimmed.length > 4 * 1024 * 1024) return emptyList()
        if (trimmed.startsWith("everty-lan://")) {
            return runCatching { listOf(ExtraNodes.lan(trimmed)) }.getOrDefault(emptyList())
        }
        if (trimmed.startsWith("{")) {
            return runCatching { ExtraNodes.xray(org.json.JSONObject(trimmed)) }.getOrDefault(emptyList())
        }
        if (trimmed.lineSequence().take(20).any { it.trim() == "[Interface]" }) {
            return runCatching { listOf(ExtraNodes.wireguard(trimmed)) }.getOrDefault(emptyList())
        }

        // 1. Direct vless:// or multi-line vless://
        if (trimmed.contains("vless://")) {
            val nodes = mutableListOf<VpnNode>()
            trimmed.lines().forEach { line ->
                val l = line.trim()
                if (l.startsWith("vless://")) {
                    parseVlessUri(l)?.let { nodes.add(it) }
                }
            }
            if (nodes.isNotEmpty()) return nodes
        }

        // 2. Try Base64 decoding
        try {
            val cleaned = trimmed.replace("\n", "").replace("\r", "").replace(" ", "")
            val decodedBytes = Base64.decode(cleaned, Base64.DEFAULT)
            val decodedStr = String(decodedBytes, StandardCharsets.UTF_8).trim()
            if (decodedStr.contains("vless://")) {
                return parse(decodedStr)
            }
        } catch (_: Exception) {}

        // 3. Fallback: single URI parse
        if (trimmed.startsWith("vless://")) {
            parseVlessUri(trimmed)?.let { return listOf(it) }
        }

        return emptyList()
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
                rawUri = uriString
            )
        } catch (e: Exception) {
            null
        }
    }
}
