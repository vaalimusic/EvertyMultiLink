package ru.everty.multilink.core

import android.net.IpPrefix
import android.net.VpnService
import android.os.Build
import android.util.Log
import java.net.InetAddress

internal object LanRoutes {
    private const val TAG = "LanRoutes"
    private val ipv4 = listOf("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16",
        "169.254.0.0/16", "224.0.0.0/4", "255.255.255.255/32")
    private val ipv6 = listOf("fc00::/7", "fe80::/10", "ff00::/8")

    fun addTo(builder: VpnService.Builder) {
        for ((root, exclusions) in listOf("0.0.0.0/0" to ipv4, "::/0" to ipv6)) {
            try {
                val excluded = exclusions.map(::parse)
                if (Build.VERSION.SDK_INT >= 33) {
                    builder.addRoute(parse(root))
                    excluded.forEach(builder::excludeRoute)
                } else {
                    // Older Android only accepts inclusive routes. Split prefixes
                    // until each remaining prefix is fully outside the exclusions.
                    addOutside(builder, parse(root), excluded)
                }
            } catch (e: Throwable) {
                Log.w(TAG, "Failed adding routes for $root: ${e.message}")
            }
        }
    }

    private fun addOutside(builder: VpnService.Builder, prefix: IpPrefix, excluded: List<IpPrefix>) {
        try {
            if (excluded.any { it.contains(prefix.address) && it.prefixLength <= prefix.prefixLength }) return
            val inside = excluded.filter { prefix.contains(it.address) }
            if (inside.isEmpty()) {
                builder.addRoute(prefix.address, prefix.prefixLength)
                return
            }
            val right = prefix.address.address
            val bit = prefix.prefixLength
            if (bit >= right.size * 8) return

            val left = IpPrefix(InetAddress.getByAddress(right), bit + 1)
            right[bit / 8] = (right[bit / 8].toInt() or (0x80 ushr (bit % 8))).toByte()
            addOutside(builder, left, inside)
            addOutside(builder, IpPrefix(InetAddress.getByAddress(right), bit + 1), inside)
        } catch (e: Throwable) {
            Log.w(TAG, "addOutside failed for prefix $prefix: ${e.message}")
        }
    }

    private fun parse(cidr: String): IpPrefix {
        val (address, length) = cidr.split('/')
        return IpPrefix(InetAddress.getByName(address), length.toInt())
    }
}
