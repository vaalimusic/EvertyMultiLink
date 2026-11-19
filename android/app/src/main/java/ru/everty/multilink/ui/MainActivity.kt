package ru.everty.multilink.ui

import android.app.Activity
import android.app.UiModeManager
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipboardManager
import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import androidx.core.app.ActivityCompat
import android.content.Intent
import android.content.IntentFilter
import android.content.res.Configuration
import android.net.VpnService
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.LayoutInflater
import android.widget.ImageView
import android.widget.TextView
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import androidx.viewpager2.widget.ViewPager2
import com.google.android.material.button.MaterialButton
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import ru.everty.multilink.R
import ru.everty.multilink.databinding.ActivityMainBinding
import ru.everty.multilink.model.AppSettings
import ru.everty.multilink.model.Subscription
import ru.everty.multilink.model.VpnNode
import ru.everty.multilink.parser.ConfigParser
import ru.everty.multilink.qr.QrUtils
import ru.everty.multilink.service.EvertyVpnService
import ru.everty.multilink.sync.TvLanClient
import ru.everty.multilink.sync.TvLanSyncServer

class MainActivity : AppCompatActivity() {

    private lateinit var binding: ActivityMainBinding
    private lateinit var settings: AppSettings
    private lateinit var pagerAdapter: MainPagerAdapter
    private val mainHandler = Handler(Looper.getMainLooper())
    private var isTvDevice = false
    private var lanServer: TvLanSyncServer? = null

    private val vpnPermissionLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            startVpnService(EvertyVpnService.ACTION_CONNECT)
        } else {
            Toast.makeText(this, "Разрешение на VPN отклонено", Toast.LENGTH_SHORT).show()
        }
    }

    private val qrScannerLauncher = registerForActivityResult(ScanContract()) { result ->
        if (result.contents != null) {
            handleScannedQr(result.contents)
        }
    }

    private val vpnStateReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val state = intent?.getIntExtra(EvertyVpnService.EXTRA_STATE, EvertyVpnService.STATE_DISCONNECTED)
                ?: EvertyVpnService.STATE_DISCONNECTED
            pagerAdapter.homeFragment.updateVpnState(state)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)

        settings = AppSettings(this)
        detectDeviceMode()
        setupViewPagerAndNavigation()
        handleIntent(intent)

        checkNotificationPermission()

        // Automatic subscription check & sync on launch
        checkSubscriptionOnStart()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIntent(intent)
    }

    private fun detectDeviceMode() {
        val uiModeManager = getSystemService(Context.UI_MODE_SERVICE) as? UiModeManager
        val isTelevision = uiModeManager?.currentModeType == Configuration.UI_MODE_TYPE_TELEVISION
        val isTvFeature = packageManager.hasSystemFeature("android.software.leanback")
        isTvDevice = isTelevision || isTvFeature
    }

    private fun setupViewPagerAndNavigation() {
        pagerAdapter = MainPagerAdapter(this)
        binding.viewPager.adapter = pagerAdapter
        binding.viewPager.offscreenPageLimit = 2

        // Bottom Navigation (Phone, Tablet, TV D-Pad)
        binding.bottomNav.setOnItemSelectedListener { item ->
            when (item.itemId) {
                R.id.nav_home -> binding.viewPager.setCurrentItem(0, true)
                R.id.nav_servers -> binding.viewPager.setCurrentItem(1, true)
                R.id.nav_routing -> binding.viewPager.setCurrentItem(2, true)
            }
            true
        }

        // Sync ViewPager swipe with BottomNav selection
        binding.viewPager.registerOnPageChangeCallback(object : ViewPager2.OnPageChangeCallback() {
            override fun onPageSelected(position: Int) {
                when (position) {
                    0 -> binding.bottomNav.selectedItemId = R.id.nav_home
                    1 -> binding.bottomNav.selectedItemId = R.id.nav_servers
                    2 -> binding.bottomNav.selectedItemId = R.id.nav_routing
                }
            }
        })
    }

    private fun checkSubscriptionOnStart() {
        if (!settings.isAutoUpdateOnStartEnabled) return
        val subs = settings.getSubscriptions()
        if (subs.isNotEmpty()) {
            for (sub in subs) {
                fetchSubscription(sub.url, subId = sub.id, subName = sub.name, isManual = false)
            }
        } else {
            val subUrl = settings.lastSubscriptionUrl
            if (!subUrl.isNullOrEmpty()) {
                fetchSubscription(subUrl, isManual = false)
            }
        }
    }

    fun toggleVpn() {
        if (EvertyVpnService.currentState == EvertyVpnService.STATE_CONNECTED ||
            EvertyVpnService.currentState == EvertyVpnService.STATE_CONNECTING) {
            startVpnService(EvertyVpnService.ACTION_DISCONNECT)
        } else {
            val prepareIntent = VpnService.prepare(this)
            if (prepareIntent != null) {
                vpnPermissionLauncher.launch(prepareIntent)
            } else {
                startVpnService(EvertyVpnService.ACTION_CONNECT)
            }
        }
    }

    private fun startVpnService(action: String) {
        val intent = Intent(this, EvertyVpnService::class.java).apply {
            this.action = action
        }
        if (action == EvertyVpnService.ACTION_CONNECT && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(intent)
        } else {
            startService(intent)
        }
    }

    private fun checkNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 102)
            }
        }
    }

    fun startQrScan() {
        val options = ScanOptions().apply {
            setPrompt("Наведите камеру на QR-код подписки или ТВ")
            setBeepEnabled(true)
            setOrientationLocked(false)
        }
        qrScannerLauncher.launch(options)
    }

    fun showLanMenu() {
        MaterialAlertDialogBuilder(this).setTitle("Локальная сеть")
            .setItems(arrayOf("Обмен подписками", "LAN-хост Everty", "Добавить LAN-сервер")) { _, choice ->
                when (choice) {
                    0 -> showLanSyncDialog()
                    1 -> showLanGatewayDialog()
                    else -> {
                        val field = android.widget.EditText(this).apply { hint = "everty-lan://…" }
                        MaterialAlertDialogBuilder(this).setTitle("Ключ или ссылка LAN")
                            .setView(field).setNegativeButton("Отмена", null)
                            .setPositiveButton("Подключить") { _, _ -> handleScannedQr(field.text.toString()) }
                            .show()
                    }
                }
            }.show()
    }

    private fun showLanGatewayDialog() {
        val key = settings.lanGatewayKey
        val address = ru.everty.multilink.core.LanGateway.address(this)
        val link = if (key != null && address != null) ru.everty.multilink.core.LanGateway.link(key, address) else null
        val builder = MaterialAlertDialogBuilder(this)
            .setTitle("LAN-хост Everty")
            .setMessage("Доступ по ключу из той же Wi-Fi или Ethernet сети. Хост применяет свои правила к трафику устройств. Изменение вступит в силу при следующем подключении.")
            .setNegativeButton("Закрыть", null)
        if (EvertyVpnService.currentState == EvertyVpnService.STATE_DISCONNECTED) {
            builder.setNeutralButton(if (key == null) "Включить" else "Выключить") { _, _ ->
                settings.lanGatewayKey = if (key == null) java.util.UUID.randomUUID().toString().replace("-", "") else null
                showLanGatewayDialog()
            }
        }
        if (link != null) {
            builder.setPositiveButton("Копировать ссылку") { _, _ ->
                getSystemService(android.content.ClipboardManager::class.java)
                    .setPrimaryClip(android.content.ClipData.newPlainText("Everty LAN", link))
            }
            if (EvertyVpnService.currentState == EvertyVpnService.STATE_CONNECTED) {
                builder.setNeutralButton("Копировать ключ") { _, _ ->
                    getSystemService(android.content.ClipboardManager::class.java).setPrimaryClip(
                        android.content.ClipData.newPlainText("Everty LAN", "everty-lan://$key"))
                }
            }
        }
        builder.show()
    }

    fun showLanSyncDialog() {
        val dialogView = LayoutInflater.from(this).inflate(R.layout.dialog_lan_sync, null)
        val tvTitle = dialogView.findViewById<TextView>(R.id.tvLanTitle)
        val tvInstructions = dialogView.findViewById<TextView>(R.id.tvLanInstructions)
        val ivLanQr = dialogView.findViewById<ImageView>(R.id.ivLanQr)
        val tvLanUrl = dialogView.findViewById<TextView>(R.id.tvLanUrl)
        val tvLanPin = dialogView.findViewById<TextView>(R.id.tvLanPin)
        val tvLanStatus = dialogView.findViewById<TextView>(R.id.tvLanStatus)
        val btnClose = dialogView.findViewById<MaterialButton>(R.id.btnLanClose)

        tvTitle.text = getString(R.string.lan_sync_title)
        tvInstructions.text = getString(R.string.lan_sync_desc)

        val dialog = MaterialAlertDialogBuilder(this)
            .setView(dialogView)
            .setCancelable(true)
            .create()

        lanServer?.stop()
        lanServer = TvLanSyncServer(this) { importedConfig, count, subName, subExpire, disabledSigs, disabledNames ->
            val rawNodes = ConfigParser.parse(importedConfig)
            val nodes = rawNodes.map { node ->
                val sig = "${node.server}:${node.port}:${node.uuid}"
                val isExplicitlyDisabled = (sig in disabledSigs) || (node.name in disabledNames)
                if (isExplicitlyDisabled) node.copy(isEnabled = false) else node
            }
            if (nodes.isNotEmpty()) {
                val targetId = java.util.UUID.randomUUID().toString()
                val targetName = subName ?: "Получено по Wi-Fi"
                settings.addOrUpdateSubscription(
                    Subscription(
                        id = targetId,
                        name = targetName,
                        url = importedConfig,
                        updatedAt = System.currentTimeMillis(),
                        nodesCount = nodes.size,
                        expireTimestamp = subExpire
                    )
                )
                val syncResult = settings.syncSubscriptionNodes(nodes, subId = targetId)
                notifyNodesChanged()
                val disabledCount = nodes.count { !it.isEnabled }
                val extra = if (disabledCount > 0) " (выключено $disabledCount серверов)" else ""
                tvLanStatus.text = "✅ Получено ${syncResult.total} узлов$extra!"
                tvLanStatus.setTextColor(ContextCompat.getColor(this, R.color.accent_mint))
                Toast.makeText(this, "Подписка успешно получена по локальной сети!", Toast.LENGTH_LONG).show()
            } else if (importedConfig.startsWith("http://") || importedConfig.startsWith("https://")) {
                fetchSubscription(
                    importedConfig,
                    subName = subName,
                    isManual = true,
                    disabledSignatures = disabledSigs,
                    disabledNames = disabledNames
                )
            }
        }

        if (lanServer?.start() == true) {
            val syncUrl = lanServer!!.getSyncUrl()
            tvLanUrl.text = syncUrl
            tvLanPin.text = lanServer!!.pinCode

            val qrBitmap = QrUtils.generateQrBitmap(syncUrl, 500)
            ivLanQr.setImageBitmap(qrBitmap)
        } else {
            tvLanStatus.text = "Не удалось запустить локальный сервер Wi-Fi"
        }

        btnClose.setOnClickListener {
            dialog.dismiss()
        }

        dialog.setOnDismissListener {
            lanServer?.stop()
            lanServer = null
        }

        dialog.show()
    }

    fun showSingleSubscriptionQr(sub: Subscription) {
        val allNodes = settings.getNodes().filter { it.subscriptionId == sub.id || it.subscriptionId == null }
        val disabledNames = allNodes.filter { !it.isEnabled }.map { it.name }
        val qrContent = if (disabledNames.isNotEmpty()) {
            val encUrl = java.net.URLEncoder.encode(sub.url, "UTF-8")
            val encName = java.net.URLEncoder.encode(sub.name, "UTF-8")
            val encDisabled = java.net.URLEncoder.encode(disabledNames.joinToString(","), "UTF-8")
            "everty://sync?url=$encUrl&name=$encName&disabled=$encDisabled"
        } else {
            sub.url
        }

        val extraSub = if (disabledNames.isNotEmpty()) " (${disabledNames.size} выкл)" else ""
        displayQrCode(
            title = "Поделиться подпиской",
            subtitle = "${sub.name} (${sub.nodesCount} серверов$extraSub)",
            content = qrContent
        )
    }

    fun showShareQrDialog(customNode: VpnNode? = null) {
        if (customNode != null) {
            val content = customNode.rawUri?.takeIf { it.isNotEmpty() }
                ?: "vless://${customNode.uuid}@${customNode.server}:${customNode.port}?security=${customNode.security}&sni=${customNode.sni}#${customNode.name}"
            displayQrCode(
                title = "Поделиться сервером",
                subtitle = customNode.name,
                content = content
            )
            return
        }

        val subs = settings.getSubscriptions()
        val nodes = settings.getNodes()

        if (subs.isNotEmpty() && nodes.isNotEmpty()) {
            showShareChoiceDialog(subs, nodes)
        } else if (subs.isNotEmpty()) {
            showSingleSubscriptionQr(subs.first())
        } else if (nodes.size == 1) {
            val node = nodes.first()
            val content = node.rawUri?.takeIf { it.isNotEmpty() }
                ?: "vless://${node.uuid}@${node.server}:${node.port}?security=${node.security}&sni=${node.sni}#${node.name}"
            displayQrCode(
                title = "Поделиться сервером",
                subtitle = node.name,
                content = content
            )
        } else if (nodes.size > 1) {
            showSingleServerSelectDialog(nodes)
        } else {
            Toast.makeText(this, "Нет доступных серверов или подписок для экспорта", Toast.LENGTH_SHORT).show()
        }
    }

    private fun showShareChoiceDialog(subs: List<Subscription>, nodes: List<VpnNode>) {
        val dialogView = LayoutInflater.from(this).inflate(R.layout.dialog_share_choice, null)
        val btnSub = dialogView.findViewById<MaterialButton>(R.id.btnShareSubscription)
        val btnServer = dialogView.findViewById<MaterialButton>(R.id.btnShareSingleServer)
        val btnCancel = dialogView.findViewById<MaterialButton>(R.id.btnCancelShareChoice)

        if (subs.size == 1) {
            btnSub.text = "🌐 Поделиться подпиской (${subs.first().nodesCount} узлов)"
        } else {
            btnSub.text = "🌐 Поделиться подпиской (${subs.size} подписок)"
        }

        val dialog = MaterialAlertDialogBuilder(this)
            .setView(dialogView)
            .setCancelable(true)
            .create()

        btnSub.setOnClickListener {
            dialog.dismiss()
            if (subs.size == 1) {
                showSingleSubscriptionQr(subs.first())
            } else {
                val names = subs.map { "${it.name} (${it.nodesCount} серверов)" }.toTypedArray()
                MaterialAlertDialogBuilder(this)
                    .setTitle("Выберите подписку для QR")
                    .setItems(names) { _, which ->
                        showSingleSubscriptionQr(subs[which])
                    }
                    .setNegativeButton("Отмена", null)
                    .show()
            }
        }

        btnServer.setOnClickListener {
            dialog.dismiss()
            showSingleServerSelectDialog(nodes)
        }

        btnCancel.setOnClickListener {
            dialog.dismiss()
        }

        dialog.show()
    }

    private fun showSingleServerSelectDialog(nodes: List<VpnNode>) {
        val names = nodes.map { it.name }.toTypedArray()
        MaterialAlertDialogBuilder(this)
            .setTitle("Выберите сервер для QR-кода")
            .setItems(names) { _, which ->
                val node = nodes[which]
                val content = node.rawUri?.takeIf { it.isNotEmpty() }
                    ?: "vless://${node.uuid}@${node.server}:${node.port}?security=${node.security}&sni=${node.sni}#${node.name}"
                displayQrCode(
                    title = "Поделиться сервером",
                    subtitle = node.name,
                    content = content
                )
            }
            .setNegativeButton("Отмена", null)
            .show()
    }

    private fun displayQrCode(title: String, subtitle: String, content: String) {
        val dialogView = LayoutInflater.from(this).inflate(R.layout.dialog_qr_display, null)
        val ivShareQr = dialogView.findViewById<ImageView>(R.id.ivShareQr)
        val tvQrTitle = dialogView.findViewById<TextView>(R.id.tvQrTitle)
        val tvQrSubtitle = dialogView.findViewById<TextView>(R.id.tvQrSubtitle)
        val tvQrDetails = dialogView.findViewById<TextView>(R.id.tvQrDetails)
        val btnCopy = dialogView.findViewById<MaterialButton>(R.id.btnQrCopy)
        val btnClose = dialogView.findViewById<MaterialButton>(R.id.btnQrClose)

        tvQrTitle.text = title
        tvQrSubtitle.text = subtitle
        tvQrDetails.text = content

        val qrBitmap = QrUtils.generateQrBitmap(content, 500)
        ivShareQr.setImageBitmap(qrBitmap)

        val dialog = MaterialAlertDialogBuilder(this)
            .setView(dialogView)
            .setCancelable(true)
            .create()

        btnCopy.setOnClickListener {
            val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val clip = ClipData.newPlainText("Everty Link", content)
            clipboard.setPrimaryClip(clip)
            Toast.makeText(this, "Ссылка скопирована в буфер", Toast.LENGTH_SHORT).show()
        }

        btnClose.setOnClickListener {
            dialog.dismiss()
        }

        dialog.show()
    }

    fun fetchSubscription(
        url: String,
        subId: String? = null,
        subName: String? = null,
        isManual: Boolean = true,
        disabledSignatures: List<String> = emptyList(),
        disabledNames: List<String> = emptyList()
    ) {
        if (isManual) {
            Toast.makeText(this, "Синхронизация подписки…", Toast.LENGTH_SHORT).show()
        }
        settings.lastSubscriptionUrl = url

        Thread {
            try {
                val cleanUrl = if (url.contains("#")) url.substringBefore("#") else url
                val client = okhttp3.OkHttpClient()
                val request = okhttp3.Request.Builder()
                    .url(cleanUrl)
                    .header("User-Agent", "EvertyMultiLink/1.0 (v2rayng/clash-meta)")
                    .build()
                client.newCall(request).execute().use { response ->
                    val body = response.body?.string() ?: ""
                    val rawNodes = ConfigParser.parse(body)
                    val nodes = rawNodes.map { node ->
                        val sig = "${node.server}:${node.port}:${node.uuid}"
                        if (sig in disabledSignatures || node.name in disabledNames) {
                            node.copy(isEnabled = false)
                        } else {
                            node
                        }
                    }

                    // Parse subscription-userinfo header: upload=...; download=...; total=...; expire=...
                    val subUserInfo = response.header("subscription-userinfo")
                        ?: response.header("Subscription-Userinfo")
                        ?: response.header("Subscription-UserInfo")

                    var expireSec = 0L
                    var uploadBytes = 0L
                    var downloadBytes = 0L
                    var totalBytes = 0L

                    if (!subUserInfo.isNullOrBlank()) {
                        val parts = subUserInfo.split(";")
                        for (part in parts) {
                            val kv = part.trim().split("=")
                            if (kv.size == 2) {
                                val k = kv[0].trim().lowercase()
                                val v = kv[1].trim().toLongOrNull() ?: 0L
                                when (k) {
                                    "expire" -> expireSec = v
                                    "upload" -> uploadBytes = v
                                    "download" -> downloadBytes = v
                                    "total" -> totalBytes = v
                                }
                            }
                        }
                    }

                    mainHandler.post {
                        if (nodes.isNotEmpty()) {
                            val currentSubs = settings.getSubscriptions()
                            val targetId = subId ?: currentSubs.find { it.url == url }?.id ?: java.util.UUID.randomUUID().toString()
                            val existingSub = currentSubs.find { it.id == targetId }
                            val targetName = subName
                                ?: existingSub?.name
                                ?: try { java.net.URI(cleanUrl).host ?: "Подписка" } catch (_: Exception) { "Подписка" }

                            val finalExpire = if (expireSec > 0L) expireSec else (existingSub?.expireTimestamp ?: 0L)
                            val finalUpload = if (uploadBytes > 0L) uploadBytes else (existingSub?.uploadBytes ?: 0L)
                            val finalDownload = if (downloadBytes > 0L) downloadBytes else (existingSub?.downloadBytes ?: 0L)
                            val finalTotal = if (totalBytes > 0L) totalBytes else (existingSub?.totalBytes ?: 0L)

                            settings.addOrUpdateSubscription(
                                Subscription(
                                    id = targetId,
                                    name = targetName,
                                    url = url,
                                    updatedAt = System.currentTimeMillis(),
                                    nodesCount = nodes.size,
                                    expireTimestamp = finalExpire,
                                    uploadBytes = finalUpload,
                                    downloadBytes = finalDownload,
                                    totalBytes = finalTotal
                                )
                            )

                            val res = settings.syncSubscriptionNodes(nodes, subId = targetId)
                            notifyNodesChanged()
                            val msg = "Подписка \"$targetName\" обновлена: +${res.added}, всего ${res.total} узлов"
                            Toast.makeText(this, msg, Toast.LENGTH_LONG).show()
                        } else if (isManual) {
                            Toast.makeText(this, "В подписке не найдено узлов", Toast.LENGTH_SHORT).show()
                        }
                    }
                }
            } catch (e: Exception) {
                if (isManual) {
                    mainHandler.post {
                        Toast.makeText(this, "Ошибка загрузки: ${e.message}", Toast.LENGTH_LONG).show()
                    }
                }
            }
        }.start()
    }

    fun fetchAllSubscriptions() {
        val subs = settings.getSubscriptions()
        if (subs.isEmpty()) {
            val legacyUrl = settings.lastSubscriptionUrl
            if (!legacyUrl.isNullOrEmpty()) {
                fetchSubscription(legacyUrl, isManual = true)
            } else {
                Toast.makeText(this, "Нет добавленных подписок", Toast.LENGTH_SHORT).show()
            }
            return
        }
        Toast.makeText(this, "Обновление ${subs.size} подписок…", Toast.LENGTH_SHORT).show()
        for (sub in subs) {
            fetchSubscription(sub.url, subId = sub.id, subName = sub.name, isManual = false)
        }
    }

    fun notifyNodesChanged() {
        pagerAdapter.homeFragment.updateModeDisplay()
        pagerAdapter.serversFragment.refreshList()
    }

    internal fun handleScannedQr(content: String) {
        val trimmed = content.trim()
        if (trimmed.matches(Regex("everty-lan://[0-9a-fA-F]{32}"))) {
            Thread {
                val resolved = runCatching { ru.everty.multilink.core.LanGateway.expand(this, trimmed) }
                runOnUiThread {
                    if (isFinishing || isDestroyed) return@runOnUiThread
                    val link = resolved.getOrNull()
                    if (link != null) handleScannedQr(link)
                    else Toast.makeText(this, "Поиск LAN не дал одного хоста. Используйте полную ссылку с IP.",
                        Toast.LENGTH_LONG).show()
                }
            }.start()
            return
        }
        if (trimmed.startsWith("http://") && trimmed.contains("?pin=")) {
            val pin = trimmed.substringAfter("?pin=")
            val subs = settings.getSubscriptions()
            val allNodes = settings.getNodes()
            val activeNode = settings.getSelectedNode()
            val currentSub = subs.find { it.url == settings.lastSubscriptionUrl } ?: subs.firstOrNull()
            val sharePayload = currentSub?.url ?: activeNode?.rawUri ?: settings.lastSubscriptionUrl ?: ""

            if (sharePayload.isEmpty()) {
                Toast.makeText(this, "Нет активной подписки для передачи на устройство", Toast.LENGTH_LONG).show()
                return
            }

            val subNodes = if (currentSub != null) allNodes.filter { it.subscriptionId == currentSub.id || it.subscriptionId == null } else allNodes
            val disabledSigs = subNodes.filter { !it.isEnabled }.map { "${it.server}:${it.port}:${it.uuid}" }
            val disabledNames = subNodes.filter { !it.isEnabled }.map { it.name }

            val subTitle = currentSub?.name ?: "Everty MultiLink"
            val disabledCount = disabledNames.size
            val infoMsg = if (disabledCount > 0) {
                "Перенести подписку \"$subTitle\" на ТВ/устройство?\n($disabledCount отключённых серверов останутся выключенными)"
            } else {
                "Перенести подписку \"$subTitle\" на экран телевизора?"
            }

            MaterialAlertDialogBuilder(this)
                .setTitle("📺 Найдено устройство")
                .setMessage(infoMsg)
                .setPositiveButton("Передать") { _, _ ->
                    TvLanClient.sendConfigToTv(
                        tvUrl = trimmed,
                        pin = pin,
                        config = sharePayload,
                        subName = currentSub?.name,
                        subExpire = currentSub?.expireTimestamp ?: 0L,
                        disabledSignatures = disabledSigs,
                        disabledNames = disabledNames
                    ) { _, msg ->
                        mainHandler.post {
                            Toast.makeText(this, msg, Toast.LENGTH_LONG).show()
                        }
                    }
                }
                .setNegativeButton("Отмена", null)
                .show()
        } else if (trimmed.startsWith("everty://sync")) {
            try {
                val uri = android.net.Uri.parse(trimmed)
                val subUrl = uri.getQueryParameter("url") ?: ""
                val subName = uri.getQueryParameter("name")
                val disabledParam = uri.getQueryParameter("disabled") ?: ""
                val disabledList = if (disabledParam.isNotEmpty()) disabledParam.split(",") else emptyList()
                if (subUrl.isNotEmpty()) {
                    fetchSubscription(subUrl, subName = subName, isManual = true, disabledNames = disabledList)
                } else {
                    Toast.makeText(this, "Неверная ссылка синхронизации", Toast.LENGTH_SHORT).show()
                }
            } catch (e: Exception) {
                Toast.makeText(this, "Ошибка импорта: ${e.message}", Toast.LENGTH_SHORT).show()
            }
        } else {
            if (trimmed.startsWith("http://") || trimmed.startsWith("https://")) {
                fetchSubscription(trimmed, isManual = true)
            } else {
                val nodes = ConfigParser.parse(trimmed)
                if (nodes.isNotEmpty()) {
                    settings.addOrUpdateNodes(nodes)
                    notifyNodesChanged()
                    Toast.makeText(this, "Добавлено ${nodes.size} узлов", Toast.LENGTH_SHORT).show()
                } else {
                    Toast.makeText(this, "Не удалось распознать конфигурацию", Toast.LENGTH_LONG).show()
                }
            }
        }
    }

    private fun handleIntent(intent: Intent?) {
        val url = intent?.getStringExtra("import_url") ?: intent?.dataString
        if (!url.isNullOrEmpty()) {
            handleScannedQr(url)
        }
    }

    override fun onStart() {
        super.onStart()
        val filter = IntentFilter(EvertyVpnService.ACTION_STATE_CHANGED)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(vpnStateReceiver, filter, RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(vpnStateReceiver, filter)
        }
    }

    override fun onStop() {
        super.onStop()
        try {
            unregisterReceiver(vpnStateReceiver)
        } catch (_: Exception) {}
    }

    override fun onDestroy() {
        lanServer?.stop()
        lanServer = null
        super.onDestroy()
    }
}
