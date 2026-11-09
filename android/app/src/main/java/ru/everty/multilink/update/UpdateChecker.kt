package ru.everty.multilink.update

import com.google.gson.JsonParser
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.TimeUnit

object UpdateChecker {
    private const val GITHUB_API_URL = "https://api.github.com/repos/vaalimusic/EvertyMultiLink/releases/latest"

    data class ReleaseInfo(val tagName: String, val htmlUrl: String, val apkUrl: String?)

    fun getAppVersion(context: android.content.Context): String {
        return try {
            val pInfo = context.packageManager.getPackageInfo(context.packageName, 0)
            pInfo.versionName ?: "1.1.2"
        } catch (_: Exception) {
            "1.1.2"
        }
    }

    fun isNewerVersion(remote: String, current: String): Boolean {
        val parse = { v: String ->
            v.trimStart('v', 'V')
                .split('.')
                .map { part -> part.takeWhile { it.isDigit() }.toIntOrNull() ?: 0 }
        }
        val rParts = parse(remote)
        val cParts = parse(current)
        val maxLen = maxOf(rParts.size, cParts.size)
        for (i in 0 until maxLen) {
            val r = rParts.getOrElse(i) { 0 }
            val c = cParts.getOrElse(i) { 0 }
            if (r > c) return true
            if (r < c) return false
        }
        return false
    }

    fun checkLatestReleaseSync(currentVersion: String): Result<ReleaseInfo?> {
        return try {
            val client = OkHttpClient.Builder()
                .connectTimeout(6, TimeUnit.SECONDS)
                .readTimeout(8, TimeUnit.SECONDS)
                .build()
            val request = Request.Builder()
                .url(GITHUB_API_URL)
                .header("User-Agent", "EvertyMultiLink-Android")
                .header("Accept", "application/vnd.github.v3+json")
                .build()
            val response = client.newCall(request).execute()
            if (!response.isSuccessful) {
                return Result.failure(Exception("HTTP ${response.code}"))
            }
            val body = response.body?.string() ?: return Result.success(null)
            val json = JsonParser.parseString(body).asJsonObject
            val tag = json.get("tag_name")?.asString ?: ""
            val htmlUrl = json.get("html_url")?.asString ?: "https://github.com/vaalimusic/EvertyMultiLink/releases"

            var apkUrl: String? = null
            if (json.has("assets") && json.get("assets").isJsonArray) {
                json.getAsJsonArray("assets").forEach { element ->
                    val asset = element.asJsonObject
                    val name = asset.get("name")?.asString ?: ""
                    if (name.endsWith(".apk", ignoreCase = true)) {
                        apkUrl = asset.get("browser_download_url")?.asString
                    }
                }
            }

            if (tag.isNotEmpty() && isNewerVersion(tag, currentVersion)) {
                Result.success(ReleaseInfo(tag, htmlUrl, apkUrl))
            } else {
                Result.success(null)
            }
        } catch (e: Exception) {
            Result.failure(e)
        }
    }
}
