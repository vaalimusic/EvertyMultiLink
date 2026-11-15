package ru.everty.multilink.core

import ru.everty.multilink.model.AppSettings
import ru.everty.multilink.model.Policy
import ru.everty.multilink.model.VpnNode
import java.io.File
import com.google.gson.Gson
import ru.everty.multilink.parser.ExtraNodes

object MihomoConfigBuilder {
    const val VPN_DNS = "198.19.255.53"

    fun generateConfig(
        outputDir: File,
        settings: AppSettings,
        upstreamDns: List<String>,
        controllerSecret: String,
        policy: Policy = settings.policy,
        diagnostic: Boolean = false,
        lanAddress: String? = null
    ): File {
        outputDir.mkdirs()
        val configFile = File(outputDir, "config.yaml")

        val allNodes = settings.getNodes()
        val enabledNodes = allNodes.filter { it.isEnabled }
        val forcedNode = settings.getForcedNode()
        val gateway = settings.lanGatewayKey

        val sb = StringBuilder()
        sb.appendLine("mixed-port: 7890")
        sb.appendLine("allow-lan: false")
        sb.appendLine("mode: rule")
        sb.appendLine(if (diagnostic) "log-level: info" else "log-level: warning")
        sb.appendLine("ipv6: false")
        sb.appendLine("external-controller: 127.0.0.1:9090")
        sb.appendLine("secret: '$controllerSecret'")
        sb.appendLine("sniffer:")
        sb.appendLine("  enable: true")
        sb.appendLine("  override-destination: false")
        sb.appendLine("  sniff:")
        sb.appendLine("    HTTP: {ports: [80, 8080-8880]}")
        sb.appendLine("    TLS: {ports: [443, 8443]}")
        sb.appendLine("    QUIC: {ports: [443, 8443]}")
        sb.appendLine()

        sb.appendLine("dns:")
        sb.appendLine("  enable: true")
        sb.appendLine("  listen: 127.0.0.1:1053")
        sb.appendLine("  enhanced-mode: fake-ip")
        sb.appendLine("  fake-ip-filter: ['localhost', '+.local', '+.lan', '+.home.arpa', '+.in-addr.arpa', '+.ip6.arpa']")
        sb.appendLine("  nameserver:")
        sb.appendLine("    - 1.1.1.1")
        sb.appendLine("    - 8.8.8.8")
        if (upstreamDns.isNotEmpty()) {
            sb.appendLine("  direct-nameserver:")
            upstreamDns.forEach { sb.appendLine("    - '$it'") }
            sb.appendLine("  nameserver-policy:")
            sb.appendLine("    '+.lan,+.home.arpa,+.in-addr.arpa,+.ip6.arpa':")
            upstreamDns.forEach { sb.appendLine("      - '$it'") }
        }
        sb.appendLine()

        sb.appendLine("proxies:")
        sb.appendLine("  - {name: VPN_DNS, type: dns}")

        val nodesToInclude = if (forcedNode != null && forcedNode.isEnabled) {
            listOf(forcedNode)
        } else if (enabledNodes.isNotEmpty()) {
            enabledNodes
        } else {
            allNodes.take(1)
        }
        if (gateway != null) {
            require(lanAddress != null && ExtraNodes.privateIpv4(lanAddress) && gateway.matches(Regex("[0-9a-f]{32}"))) {
                "Для LAN-хоста нужна Wi-Fi или Ethernet сеть с частным IPv4"
            }
            require(nodesToInclude.none { it.security == "lan" }) {
                "Отключите LAN-серверы перед включением LAN-хоста"
            }
        }

        val proxyNames = mutableListOf<String>()

        for ((index, node) in nodesToInclude.withIndex()) {
            val safeName = "NODE_$index"
            proxyNames.add(safeName)

            node.outbound?.let {
                sb.appendLine("  - " + Gson().toJson(it + ("name" to safeName)))
            }
            if (node.outbound != null) continue
            sb.appendLine("  - name: \"$safeName\"")
            sb.appendLine("    type: vless")
            sb.appendLine("    server: \"${node.server}\"")
            sb.appendLine("    port: ${node.port}")
            sb.appendLine("    uuid: \"${node.uuid}\"")
            sb.appendLine("    network: \"${if (node.type.isEmpty()) "tcp" else node.type}\"")
            sb.appendLine("    udp: true")
            sb.appendLine("    tls: true")

            if (node.flow.isNotEmpty()) {
                sb.appendLine("    flow: \"${node.flow}\"")
            }
            if (node.sni.isNotEmpty()) {
                sb.appendLine("    servername: \"${node.sni}\"")
            }
            sb.appendLine("    client-fingerprint: chrome")

            if (node.security == "reality" && node.pbk.isNotEmpty()) {
                sb.appendLine("    reality-opts:")
                sb.appendLine("      public-key: \"${node.pbk}\"")
                if (node.sid.isNotEmpty()) {
                    sb.appendLine("      short-id: \"${node.sid}\"")
                }
            }

            if (node.path.isNotEmpty()) {
                sb.appendLine("    ws-opts:")
                sb.appendLine("      path: \"${node.path}\"")
            }
        }
        sb.appendLine()

        val mainProxyTarget: String
        if (forcedNode != null && proxyNames.isNotEmpty()) {
            mainProxyTarget = proxyNames[0]
        } else if (proxyNames.size > 1) {
            mainProxyTarget = "MULTI_LINK"
            sb.appendLine("proxy-groups:")
            sb.appendLine("  - name: MULTI_LINK")
            when (policy) {
                Policy.BALANCE -> {
                    sb.appendLine("    type: load-balance")
                    sb.appendLine("    strategy: round-robin")
                    sb.appendLine("    url: 'http://cp.cloudflare.com/generate_204'")
                    sb.appendLine("    interval: 300")
                    sb.appendLine("    lazy: true")
                }
                Policy.STABLE -> {
                    sb.appendLine("    type: fallback")
                    sb.appendLine("    url: 'http://cp.cloudflare.com/generate_204'")
                    sb.appendLine("    interval: 300")
                    sb.appendLine("    lazy: true")
                }
                Policy.SMART, Policy.LATENCY -> {
                    sb.appendLine("    type: url-test")
                    sb.appendLine("    url: 'http://cp.cloudflare.com/generate_204'")
                    sb.appendLine("    interval: 300")
                    sb.appendLine("    tolerance: 50")
                    sb.appendLine("    lazy: true")
                }
            }
            sb.appendLine("    proxies:")
            val autoProxies = if (forcedNode != null) {
                proxyNames
            } else {
                val nonRu = nodesToInclude.mapIndexed { idx, n -> Pair(proxyNames[idx], n) }.filter { !it.second.isRussianNode() }
                if (nonRu.isNotEmpty()) nonRu.map { it.first } else proxyNames
            }
            for (pName in autoProxies) {
                sb.appendLine("      - \"$pName\"")
            }
            sb.appendLine()
        } else if (proxyNames.isNotEmpty()) {
            mainProxyTarget = proxyNames[0]
        } else {
            mainProxyTarget = "DIRECT"
        }

        if (settings.isSplitRuEnabled) {
            sb.appendLine("rule-providers:")
            sb.appendLine("  RU-SITES: {type: file, behavior: domain, format: mrs, path: ./category-ru.mrs}")
        }
        if (gateway != null) {
            val listener = mapOf("name" to "everty-lan", "type" to "shadowsocks",
                "listen" to lanAddress, "port" to ExtraNodes.LAN_PORT, "cipher" to ExtraNodes.LAN_CIPHER,
                "password" to gateway, "udp" to true)
            sb.appendLine("listeners:")
            sb.appendLine("  - " + Gson().toJson(listener))
        }
        sb.appendLine("rules:")
        if (gateway != null) {
            for (cidr in listOf("127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16",
                    "169.254.0.0/16", "::1/128", "fc00::/7", "fe80::/10")) {
                sb.appendLine("  - AND,((IN-NAME,everty-lan),(IP-CIDR,$cidr)),REJECT")
            }
        }
        sb.appendLine("  - AND,((IP-CIDR,$VPN_DNS/32,no-resolve),(DST-PORT,53)),VPN_DNS")
        // User rules take precedence over the default country split.
        for (rule in settings.getCustomRules()) {
            sb.appendLine("  - ${rule.toMihomoRule(mainProxyTarget)}")
        }
        if (settings.isSplitRuEnabled) {
            sb.appendLine("  - DOMAIN-SUFFIX,ru,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,xn--p1ai,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,su,DIRECT")
            sb.appendLine("  - RULE-SET,RU-SITES,DIRECT")
        }
        sb.appendLine("  - MATCH,$mainProxyTarget")

        configFile.writeText(sb.toString())
        return configFile
    }
}
