package ru.everty.multilink.model

data class VpnNode(
    val id: String,
    var name: String,
    val server: String,
    val port: Int,
    val uuid: String,
    val security: String = "reality",
    val sni: String = "",
    val pbk: String = "",
    val sid: String = "",
    val spx: String = "",
    val flow: String = "",
    val type: String = "tcp",
    val path: String = "",
    var pingMs: Long = -1,
    var isEnabled: Boolean = true,
    var isSelected: Boolean = false,
    val rawUri: String = "",
    var subscriptionId: String? = null
) {
    fun getDisplayDetails(): String {
        return "${security.uppercase()} • ${type.uppercase()} • $server:$port"
    }

    fun isRussianNode(): Boolean {
        if (name.contains("🇷🇺")) return true
        val lower = name.lowercase()
        val words = lower.split(Regex("[^a-zA-Z0-9а-яА-ЯёЁ]+")).filter { it.isNotEmpty() }
        val ruKeywords = setOf(
            "russia", "россия", "российск", "moscow", "москва", "msk", "мск",
            "spb", "спб", "питер", "петербург", "petersburg", "novosibirsk",
            "новосибирск", "ekaterinburg", "екатеринбург", "kazan", "казань"
        )
        if (words.any { it in ruKeywords }) return true
        if (words.contains("ru") || words.contains("rus")) return true
        for (w in words) {
            val cl = w.trim('-')
            if (cl.startsWith("ru") && cl.length >= 3 && cl.substring(2).all { it.isDigit() }) return true
            if (cl.endsWith("ru") && cl.length >= 3 && cl.substring(0, cl.length - 2).all { it.isDigit() }) return true
        }
        val s = server.lowercase()
        if (s.endsWith(".ru") || s.endsWith(".рф") || s.endsWith(".su") || s.endsWith(".xn--p1ai")) return true
        return false
    }
}

enum class Policy(val label: String) {
    SMART("Умный выбор"),
    LATENCY("Быстрый (Мин. RTT)"),
    BALANCE("Сбалансированный"),
    STABLE("Резервный (Fallback)")
}

data class Subscription(
    val id: String,
    var name: String,
    var url: String,
    var updatedAt: Long = 0L,
    var nodesCount: Int = 0,
    var expireTimestamp: Long = 0L,
    var uploadBytes: Long = 0L,
    var downloadBytes: Long = 0L,
    var totalBytes: Long = 0L
) {
    fun getRemainingDaysText(): String? {
        if (expireTimestamp <= 0) return null
        val nowSec = System.currentTimeMillis() / 1000
        val diffSec = expireTimestamp - nowSec
        if (diffSec <= 0) return "⚠️ Истекла"
        val days = (diffSec / 86400).toInt()
        val suffix = when {
            days % 100 in 11..19 -> "дней"
            days % 10 == 1 -> "день"
            days % 10 in 2..4 -> "дня"
            else -> "дней"
        }
        return if (days == 0) "⏳ < 1 дня" else "⏳ $days $suffix"
    }

    fun getExpireFormattedDate(): String? {
        if (expireTimestamp <= 0) return null
        val sdf = java.text.SimpleDateFormat("dd.MM.yyyy", java.util.Locale.getDefault())
        return sdf.format(java.util.Date(expireTimestamp * 1000))
    }
}
