package ru.everty.multilink.core

import ru.everty.multilink.model.AppSettings
import ru.everty.multilink.model.Policy
import ru.everty.multilink.model.VpnNode
import java.io.File

object MihomoConfigBuilder {

    fun generateConfig(
        outputDir: File,
        settings: AppSettings
    ): File {
        outputDir.mkdirs()
        val configFile = File(outputDir, "config.yaml")

        val allNodes = settings.getNodes()
        val enabledNodes = allNodes.filter { it.isEnabled }
        val forcedNode = settings.getForcedNode()

        val sb = StringBuilder()
        sb.appendLine("mixed-port: 7890")
        sb.appendLine("allow-lan: false")
        sb.appendLine("mode: rule")
        sb.appendLine("log-level: warning")
        sb.appendLine("ipv6: false")
        sb.appendLine("external-controller: 127.0.0.1:9090")
        sb.appendLine()

        sb.appendLine("dns:")
        sb.appendLine("  enable: true")
        sb.appendLine("  listen: 127.0.0.1:1053")
        sb.appendLine("  enhanced-mode: fake-ip")
        sb.appendLine("  nameserver:")
        sb.appendLine("    - 1.1.1.1")
        sb.appendLine("    - 8.8.8.8")
        sb.appendLine("    - 77.88.8.8")
        sb.appendLine()

        sb.appendLine("proxies:")

        val nodesToInclude = if (forcedNode != null && forcedNode.isEnabled) {
            listOf(forcedNode)
        } else if (enabledNodes.isNotEmpty()) {
            enabledNodes
        } else {
            allNodes.take(1)
        }

        val proxyNames = mutableListOf<String>()

        for ((index, node) in nodesToInclude.withIndex()) {
            val safeName = "NODE_$index"
            proxyNames.add(safeName)

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
            when (settings.policy) {
                Policy.BALANCE -> {
                    sb.appendLine("    type: load-balance")
                    sb.appendLine("    strategy: round-robin")
                    sb.appendLine("    url: 'http://cp.cloudflare.com/generate_204'")
                    sb.appendLine("    interval: 300")
                }
                Policy.STABLE -> {
                    sb.appendLine("    type: fallback")
                    sb.appendLine("    url: 'http://cp.cloudflare.com/generate_204'")
                    sb.appendLine("    interval: 300")
                }
                Policy.SMART, Policy.LATENCY -> {
                    sb.appendLine("    type: url-test")
                    sb.appendLine("    url: 'http://cp.cloudflare.com/generate_204'")
                    sb.appendLine("    interval: 300")
                    sb.appendLine("    tolerance: 50")
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

        sb.appendLine("rules:")
        // 1. Split RU rules
        if (settings.isSplitRuEnabled) {
            sb.appendLine("  - DOMAIN-SUFFIX,ru,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,рф,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,su,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,gosuslugi.ru,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,sber.ru,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,tinkoff.ru,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,yandex.ru,DIRECT")
            sb.appendLine("  - DOMAIN-SUFFIX,vk.com,DIRECT")
            sb.appendLine("  - GEOIP,RU,DIRECT")
        }

        // 2. Custom User Rules
        val customRules = settings.getCustomRules()
        for (cr in customRules) {
            sb.appendLine("  - ${cr.toMihomoRule(mainProxyTarget)}")
        }

        // 3. Fallback
        sb.appendLine("  - MATCH,$mainProxyTarget")

        configFile.writeText(sb.toString())
        return configFile
    }
}
