package ru.everty.multilink.model

import android.content.Context
import android.content.SharedPreferences
import com.google.gson.Gson
import com.google.gson.reflect.TypeToken

data class SyncResult(
    val added: Int,
    val updated: Int,
    val removed: Int,
    val total: Int
)

class AppSettings(context: Context) {

    private val prefs: SharedPreferences = context.getSharedPreferences("everty_settings", Context.MODE_PRIVATE)
    private val gson = Gson()

    companion object {
        private const val KEY_NODES = "vpn_nodes"
        private const val KEY_SELECTED_NODE_ID = "selected_node_id"
        private const val KEY_FORCED_NODE_ID = "forced_node_id"
        private const val KEY_POLICY = "routing_policy"
        private const val KEY_SPLIT_RU = "split_ru_direct"
        private const val KEY_SUBSCRIPTION_URL = "last_subscription_url"
        private const val KEY_LAST_SUB_UPDATE = "last_sub_update_time"
        private const val KEY_SUBSCRIPTIONS = "subscriptions_list"
        private const val KEY_AUTO_UPDATE_ON_START = "auto_update_on_start"
        private const val KEY_CUSTOM_RULES = "custom_routing_rules"
        private const val KEY_PER_APP_ENABLED = "per_app_vpn_enabled"
        private const val KEY_PER_APP_PACKAGES = "per_app_vpn_packages"
        private const val KEY_PER_APP_MODE = "per_app_vpn_mode"
        private const val KEY_ISOLATE_LOCAL_PROXY = "isolate_local_proxy"
    }

    var selectedNodeId: String?
        get() = prefs.getString(KEY_SELECTED_NODE_ID, null)
        set(value) = prefs.edit().putString(KEY_SELECTED_NODE_ID, value).apply()

    var forcedNodeId: String?
        get() = prefs.getString(KEY_FORCED_NODE_ID, null)
        set(value) = prefs.edit().putString(KEY_FORCED_NODE_ID, value).apply()

    var policy: Policy
        get() {
            val name = prefs.getString(KEY_POLICY, Policy.SMART.name) ?: Policy.SMART.name
            return try { Policy.valueOf(name) } catch (e: Exception) { Policy.SMART }
        }
        set(value) = prefs.edit().putString(KEY_POLICY, value.name).apply()

    var isSplitRuEnabled: Boolean
        get() = prefs.getBoolean(KEY_SPLIT_RU, true)
        set(value) = prefs.edit().putBoolean(KEY_SPLIT_RU, value).apply()

    var lanGatewayKey: String?
        get() = prefs.getString("lan_gateway_key", null)
        set(value) = prefs.edit().putString("lan_gateway_key", value).apply()

    var lastSubscriptionUrl: String?
        get() = prefs.getString(KEY_SUBSCRIPTION_URL, null)
        set(value) = prefs.edit().putString(KEY_SUBSCRIPTION_URL, value).apply()

    var lastSubscriptionUpdateTime: Long
        get() = prefs.getLong(KEY_LAST_SUB_UPDATE, 0L)
        set(value) = prefs.edit().putLong(KEY_LAST_SUB_UPDATE, value).apply()

    var isAutoUpdateOnStartEnabled: Boolean
        get() = prefs.getBoolean(KEY_AUTO_UPDATE_ON_START, true)
        set(value) = prefs.edit().putBoolean(KEY_AUTO_UPDATE_ON_START, value).apply()

    var isPerAppVpnEnabled: Boolean
        get() = prefs.getBoolean(KEY_PER_APP_ENABLED, false)
        set(value) = prefs.edit().putBoolean(KEY_PER_APP_ENABLED, value).apply()

    var perAppVpnMode: String
        get() = prefs.getString(KEY_PER_APP_MODE, "INCLUDE") ?: "INCLUDE"
        set(value) = prefs.edit().putString(KEY_PER_APP_MODE, value).apply()

    var perAppVpnPackages: Set<String>
        get() = prefs.getStringSet(KEY_PER_APP_PACKAGES, emptySet()) ?: emptySet()
        set(value) = prefs.edit().putStringSet(KEY_PER_APP_PACKAGES, value).apply()

    var isLocalProxyIsolated: Boolean
        get() = prefs.getBoolean(KEY_ISOLATE_LOCAL_PROXY, false)
        set(value) = prefs.edit().putBoolean(KEY_ISOLATE_LOCAL_PROXY, value).apply()

    fun toggleAppPackage(packageName: String) {
        val current = perAppVpnPackages.toMutableSet()
        if (current.contains(packageName)) {
            current.remove(packageName)
        } else {
            current.add(packageName)
        }
        perAppVpnPackages = current
    }

    fun getNodes(): List<VpnNode> {
        val json = prefs.getString(KEY_NODES, null) ?: return emptyList()
        val type = object : TypeToken<List<VpnNode>>() {}.type
        return try {
            gson.fromJson(json, type) ?: emptyList()
        } catch (e: Exception) {
            emptyList()
        }
    }

    fun saveNodes(nodes: List<VpnNode>) {
        val json = gson.toJson(nodes)
        prefs.edit().putString(KEY_NODES, json).apply()
    }

    fun getSelectedNode(): VpnNode? {
        val nodes = getNodes()
        val forced = forcedNodeId?.let { fid -> nodes.find { it.id == fid && it.isEnabled } }
        if (forced != null) return forced

        val id = selectedNodeId ?: return nodes.firstOrNull { it.isEnabled }
        return nodes.find { it.id == id && it.isEnabled } ?: nodes.firstOrNull { it.isEnabled }
    }

    fun getForcedNode(): VpnNode? {
        val fid = forcedNodeId ?: return null
        return getNodes().find { it.id == fid }
    }

    fun selectNode(nodeId: String) {
        selectedNodeId = nodeId
        val nodes = getNodes()
        val updated = nodes.map { it.copy(isSelected = (it.id == nodeId)) }
        saveNodes(updated)
    }

    fun getSubscriptions(): List<Subscription> {
        val json = prefs.getString(KEY_SUBSCRIPTIONS, null)
        val list: MutableList<Subscription> = if (!json.isNullOrBlank()) {
            val type = object : TypeToken<List<Subscription>>() {}.type
            try {
                gson.fromJson<List<Subscription>>(json, type)?.toMutableList() ?: mutableListOf()
            } catch (e: Exception) {
                mutableListOf()
            }
        } else {
            mutableListOf()
        }

        if (list.isEmpty() && !lastSubscriptionUrl.isNullOrBlank()) {
            val nodes = getNodes()
            val initial = Subscription(
                id = "sub_default",
                name = "Подписка VLESS",
                url = lastSubscriptionUrl!!,
                updatedAt = lastSubscriptionUpdateTime,
                nodesCount = nodes.size
            )
            list.add(initial)
            saveSubscriptions(list)
            val updatedNodes = nodes.map { it.copy(subscriptionId = it.subscriptionId ?: "sub_default") }
            saveNodes(updatedNodes)
        }

        return list
    }

    fun saveSubscriptions(subscriptions: List<Subscription>) {
        val json = gson.toJson(subscriptions)
        prefs.edit().putString(KEY_SUBSCRIPTIONS, json).apply()
    }

    fun addOrUpdateSubscription(sub: Subscription) {
        val subs = getSubscriptions().toMutableList()
        val index = subs.indexOfFirst { it.id == sub.id || it.url == sub.url }
        if (index >= 0) {
            subs[index] = sub
        } else {
            subs.add(sub)
        }
        saveSubscriptions(subs)
        lastSubscriptionUrl = sub.url
    }

    fun deleteSubscription(subId: String) {
        val subs = getSubscriptions().filter { it.id != subId }
        saveSubscriptions(subs)
        val remainingNodes = getNodes().filter { it.subscriptionId != subId }
        saveNodes(remainingNodes)
        if (forcedNodeId != null && !remainingNodes.any { it.id == forcedNodeId }) {
            forcedNodeId = null
        }
    }

    fun syncSubscriptionNodes(newNodes: List<VpnNode>, subId: String? = null): SyncResult {
        val current = getNodes()
        var added = 0
        var updated = 0

        val targetSubId = subId ?: getSubscriptions().firstOrNull()?.id ?: "sub_default"
        val taggedNewNodes = newNodes.map { it.copy(subscriptionId = targetSubId) }

        val otherNodes = current.filter { it.subscriptionId != targetSubId }
        val thisSubCurrent = current.filter { it.subscriptionId == targetSubId }

        val mergedThisSub = mutableListOf<VpnNode>()
        val newSignatures = mutableSetOf<String>()

        for (n in taggedNewNodes) {
            val sig = "${n.server}:${n.port}:${n.uuid}"
            newSignatures.add(sig)

            val existing = thisSubCurrent.find { "${it.server}:${it.port}:${it.uuid}" == sig }
            if (existing != null) {
                val nodeUpdated = existing.copy(
                    name = n.name,
                    isEnabled = if (!n.isEnabled) false else existing.isEnabled,
                    security = n.security,
                    sni = n.sni,
                    pbk = n.pbk,
                    sid = n.sid,
                    flow = n.flow,
                    type = n.type,
                    path = n.path,
                    rawUri = n.rawUri,
                    outbound = n.outbound,
                    subscriptionId = targetSubId
                )
                if (existing.name != n.name || existing.sni != n.sni || existing.pbk != n.pbk) {
                    updated++
                }
                mergedThisSub.add(nodeUpdated)
            } else {
                mergedThisSub.add(n)
                added++
            }
        }

        val removed = thisSubCurrent.count { "${it.server}:${it.port}:${it.uuid}" !in newSignatures }
        val allNodes = otherNodes + mergedThisSub

        if (forcedNodeId != null && allNodes.none { it.id == forcedNodeId }) {
            forcedNodeId = null
        }

        saveNodes(allNodes)

        val subs = getSubscriptions().toMutableList()
        val sIdx = subs.indexOfFirst { it.id == targetSubId }
        val now = System.currentTimeMillis()
        if (sIdx >= 0) {
            subs[sIdx].updatedAt = now
            subs[sIdx].nodesCount = mergedThisSub.size
            saveSubscriptions(subs)
        }
        lastSubscriptionUpdateTime = now

        return SyncResult(added = added, updated = updated, removed = removed.coerceAtLeast(0), total = allNodes.size)
    }

    fun addOrUpdateNodes(newNodes: List<VpnNode>) {
        val current = getNodes().toMutableList()
        for (node in newNodes) {
            val index = current.indexOfFirst { it.server == node.server && it.port == node.port &&
                it.uuid == node.uuid && it.security == node.security }
            if (index >= 0) {
                val old = current[index]
                current[index] = node.copy(id = old.id, isEnabled = old.isEnabled,
                    isSelected = old.isSelected, subscriptionId = old.subscriptionId)
            } else current.add(node)
        }
        saveNodes(current)
    }

    fun getCustomRules(): List<CustomRule> {
        val json = prefs.getString(KEY_CUSTOM_RULES, null)
        if (json.isNullOrEmpty()) {
            return listOf(
                CustomRule(pattern = "2ip.ru", type = "DOMAIN-KEYWORD", action = "DIRECT"),
                CustomRule(pattern = "speedtest.net", type = "DOMAIN-KEYWORD", action = "DIRECT")
            )
        }
        val type = object : TypeToken<List<CustomRule>>() {}.type
        return try {
            gson.fromJson(json, type) ?: emptyList()
        } catch (_: Exception) {
            emptyList()
        }
    }

    fun saveCustomRules(rules: List<CustomRule>) {
        val json = gson.toJson(rules)
        prefs.edit().putString(KEY_CUSTOM_RULES, json).apply()
    }

    fun addCustomRule(pattern: String, type: String = "DOMAIN-KEYWORD", action: String = "DIRECT") {
        val current = getCustomRules().toMutableList()
        current.add(0, CustomRule(pattern = pattern.trim(), type = type, action = action))
        saveCustomRules(current)
    }

    fun deleteCustomRule(ruleId: String) {
        val current = getCustomRules().filter { it.id != ruleId }
        saveCustomRules(current)
    }
}
