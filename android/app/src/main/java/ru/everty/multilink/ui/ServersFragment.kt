package ru.everty.multilink.ui

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.EditText
import android.widget.TextView
import android.widget.Toast
import androidx.core.content.ContextCompat
import androidx.activity.result.contract.ActivityResultContracts
import androidx.fragment.app.Fragment
import androidx.recyclerview.widget.LinearLayoutManager
import com.google.android.material.button.MaterialButton
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import ru.everty.multilink.R
import ru.everty.multilink.databinding.FragmentServersBinding
import ru.everty.multilink.model.AppSettings
import ru.everty.multilink.model.Subscription
import ru.everty.multilink.model.VpnNode
import ru.everty.multilink.parser.ConfigParser
import java.net.InetSocketAddress
import java.net.Socket

class ServersFragment : Fragment() {

    private var _binding: FragmentServersBinding? = null
    private val binding get() = _binding!!

    private lateinit var settings: AppSettings
    private lateinit var subsAdapter: SubscriptionsAdapter
    private lateinit var adapter: NodesAdapter
    private val configFilePicker = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null && isAdded) {
            val nodes = runCatching {
                requireContext().contentResolver.openInputStream(uri)?.use { input ->
                    val bytes = java.io.ByteArrayOutputStream()
                    val buffer = ByteArray(8192)
                    while (true) {
                        val size = input.read(buffer)
                        if (size < 0) break
                        require(bytes.size() + size <= 4 * 1024 * 1024)
                        bytes.write(buffer, 0, size)
                    }
                    ConfigParser.parse(bytes.toString("UTF-8"))
                }.orEmpty()
            }.getOrDefault(emptyList())
            if (nodes.isEmpty()) {
                Toast.makeText(requireContext(), "Не удалось прочитать файл конфигурации", Toast.LENGTH_LONG).show()
            } else {
                settings.addOrUpdateNodes(nodes)
                if (_binding != null) refreshList()
                (activity as? MainActivity)?.notifyNodesChanged()
                Toast.makeText(requireContext(), "Добавлено ${nodes.size} узлов", Toast.LENGTH_SHORT).show()
            }
        }
    }

    override fun onCreateView(
        inflater: LayoutInflater, container: ViewGroup?,
        savedInstanceState: Bundle?
    ): View {
        _binding = FragmentServersBinding.inflate(inflater, container, false)
        return binding.root
    }

    override fun onViewCreated(view: View, savedInstanceState: Bundle?) {
        super.onViewCreated(view, savedInstanceState)
        settings = AppSettings(requireContext())

        setupRecyclerView()
        setupListeners()
        refreshList()
    }

    private fun setupRecyclerView() {
        subsAdapter = SubscriptionsAdapter(
            subscriptions = emptyList(),
            onSubscriptionClick = { sub ->
                showEditSubscriptionDialog(sub)
            },
            onSubscriptionLongClick = { sub ->
                showEditSubscriptionDialog(sub)
            },
            onRefreshClick = { sub ->
                (activity as? MainActivity)?.fetchSubscription(sub.url, subId = sub.id, subName = sub.name, isManual = true)
            }
        )
        binding.rvSubscriptions.layoutManager = LinearLayoutManager(requireContext())
        binding.rvSubscriptions.adapter = subsAdapter

        adapter = NodesAdapter(
            nodes = emptyList(),
            forcedNodeId = settings.forcedNodeId,
            onNodeClicked = { node ->
                if (settings.forcedNodeId == node.id) {
                    settings.forcedNodeId = null
                    Toast.makeText(requireContext(), "Фиксация снята: активен Multi-Link", Toast.LENGTH_SHORT).show()
                } else {
                    settings.forcedNodeId = node.id
                    settings.selectNode(node.id)
                    Toast.makeText(requireContext(), "Сервер зафиксирован: ${node.name}", Toast.LENGTH_SHORT).show()
                }
                refreshList()
                (activity as? MainActivity)?.notifyNodesChanged()
            },
            onNodeLongClick = { node ->
                showNodeActionDialog(node)
            }
        )

        binding.rvNodes.layoutManager = LinearLayoutManager(requireContext())
        binding.rvNodes.adapter = adapter
    }

    private fun setupListeners() {
        binding.btnUpdateAllSubs.setOnClickListener {
            (activity as? MainActivity)?.fetchAllSubscriptions()
        }

        binding.btnAddSubscription.setOnClickListener {
            showAddSubscriptionDialog()
        }
    }

    fun refreshList() {
        if (_binding == null) return

        val subs = settings.getSubscriptions()
        binding.tvSubscriptionsHeader.text = "Подписки (${subs.size})"
        subsAdapter.updateData(subs)

        val nodes = settings.getNodes()
        val enabledCount = nodes.count { it.isEnabled }
        binding.tvNodesHeader.text = "Узлы для Multi-Link ($enabledCount / ${nodes.size})"
        adapter.updateNodes(nodes, settings.forcedNodeId)

        // Run latency pings in background
        measureLatencies(nodes)
    }

    private fun measureLatencies(nodes: List<VpnNode>) {
        Thread {
            val uiHandler = Handler(Looper.getMainLooper())
            val nodesToTest = nodes.filter { it.isEnabled }.take(15)

            for (node in nodesToTest) {
                val ping = testTcpPing(node.server, node.port)
                if (ping > 0) {
                    node.pingMs = ping
                    uiHandler.post {
                        if (_binding != null) {
                            adapter.notifyDataSetChanged()
                        }
                    }
                }
            }
        }.start()
    }

    private fun testTcpPing(host: String, port: Int): Long {
        return try {
            val start = System.currentTimeMillis()
            Socket().use { socket ->
                socket.connect(InetSocketAddress(host, port), 2500)
            }
            System.currentTimeMillis() - start
        } catch (_: Exception) {
            -1L
        }
    }

    private fun showAddSubscriptionDialog() {
        val dialogView = LayoutInflater.from(requireContext()).inflate(R.layout.dialog_add_config, null)
        val etInput = dialogView.findViewById<EditText>(R.id.etConfigInput)
        val btnPaste = dialogView.findViewById<MaterialButton>(R.id.btnPasteClipboard)
        val btnScan = dialogView.findViewById<MaterialButton>(R.id.btnScanFromDialog)
        val btnSave = dialogView.findViewById<MaterialButton>(R.id.btnSaveConfig)

        val dialog = MaterialAlertDialogBuilder(requireContext())
            .setView(dialogView)
            .setNeutralButton("Открыть файл .conf") { _, _ -> configFilePicker.launch(arrayOf("*/*")) }
            .create()

        btnPaste.setOnClickListener {
            val clipboard = requireContext().getSystemService(android.content.Context.CLIPBOARD_SERVICE) as? android.content.ClipboardManager
            val item = clipboard?.primaryClip?.getItemAt(0)
            val text = item?.text?.toString() ?: ""
            if (text.isNotEmpty()) {
                etInput.setText(text)
            }
        }

        btnScan.setOnClickListener {
            dialog.dismiss()
            (activity as? MainActivity)?.startQrScan()
        }

        btnSave.setOnClickListener {
            val text = etInput.text.toString().trim()
            if (text.isEmpty()) {
                Toast.makeText(requireContext(), "Введите ссылку", Toast.LENGTH_SHORT).show()
                return@setOnClickListener
            }

            if (text.startsWith("http://") || text.startsWith("https://")) {
                dialog.dismiss()
                (activity as? MainActivity)?.fetchSubscription(text, isManual = true)
                Toast.makeText(requireContext(), "Обнаружена ссылка подписки — выполняется загрузка…", Toast.LENGTH_SHORT).show()
            } else if (text.startsWith("everty-lan://")) {
                dialog.dismiss()
                (activity as? MainActivity)?.handleScannedQr(text)
            } else {
                val nodes = ConfigParser.parse(text)
                if (nodes.isNotEmpty()) {
                    settings.addOrUpdateNodes(nodes)
                    dialog.dismiss()
                    refreshList()
                    (activity as? MainActivity)?.notifyNodesChanged()
                    val msg = if (nodes.size == 1) "Обнаружен одиночный узел — добавлен в список серверов" else "Добавлено ${nodes.size} узлов в список серверов"
                    Toast.makeText(requireContext(), msg, Toast.LENGTH_SHORT).show()
                } else {
                    Toast.makeText(requireContext(), "Не удалось прочитать конфигурацию (ожидается ссылка или узел)", Toast.LENGTH_SHORT).show()
                }
            }
        }

        dialog.show()
    }

    private fun showEditSubscriptionDialog(sub: Subscription) {
        val dialogView = LayoutInflater.from(requireContext()).inflate(R.layout.dialog_edit_subscription, null)
        val etName = dialogView.findViewById<EditText>(R.id.etEditSubName)
        val etUrl = dialogView.findViewById<EditText>(R.id.etEditSubUrl)
        val btnRefresh = dialogView.findViewById<MaterialButton>(R.id.btnEditSubRefresh)
        val btnShare = dialogView.findViewById<MaterialButton>(R.id.btnEditSubShare)
        val btnSave = dialogView.findViewById<MaterialButton>(R.id.btnEditSubSave)
        val btnDelete = dialogView.findViewById<MaterialButton>(R.id.btnEditSubDelete)
        val btnCancel = dialogView.findViewById<MaterialButton>(R.id.btnEditSubCancel)

        val tvExpire = dialogView.findViewById<TextView>(R.id.tvEditSubExpire)
        etName.setText(sub.name)
        etUrl.setText(sub.url)

        val expireText = sub.getRemainingDaysText()
        if (!expireText.isNullOrBlank()) {
            tvExpire.visibility = View.VISIBLE
            val cleanExpire = expireText.removePrefix("⏳ ")
            val dateStr = sub.getExpireFormattedDate()
            tvExpire.text = if (dateStr != null) {
                "⏳ Срок действия: $cleanExpire (до $dateStr)"
            } else {
                "⏳ Срок действия: $cleanExpire"
            }
            if (expireText.contains("Истекла") || expireText.contains("< 1 дня")) {
                tvExpire.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_red))
            } else {
                tvExpire.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_gold))
            }
        } else {
            tvExpire.visibility = View.GONE
        }

        val dialog = MaterialAlertDialogBuilder(requireContext())
            .setView(dialogView)
            .create()

        btnRefresh.setOnClickListener {
            dialog.dismiss()
            (activity as? MainActivity)?.fetchSubscription(sub.url, subId = sub.id, subName = sub.name, isManual = true)
        }

        btnShare.setOnClickListener {
            dialog.dismiss()
            (activity as? MainActivity)?.let { act ->
                act.showSingleSubscriptionQr(sub)
            }
        }

        btnSave.setOnClickListener {
            val newName = etName.text.toString().trim()
            val newUrl = etUrl.text.toString().trim()
            if (newUrl.isEmpty()) {
                Toast.makeText(requireContext(), "URL не может быть пустым", Toast.LENGTH_SHORT).show()
                return@setOnClickListener
            }
            sub.name = if (newName.isNotEmpty()) newName else sub.name
            sub.url = newUrl
            settings.addOrUpdateSubscription(sub)
            dialog.dismiss()
            refreshList()
            Toast.makeText(requireContext(), "Подписка обновлена", Toast.LENGTH_SHORT).show()
        }

        btnDelete.setOnClickListener {
            MaterialAlertDialogBuilder(requireContext())
                .setTitle("Удалить подписку?")
                .setMessage("Будут удалены все ${sub.nodesCount} серверов подписки \"${sub.name}\".")
                .setPositiveButton("Удалить") { _, _ ->
                    settings.deleteSubscription(sub.id)
                    dialog.dismiss()
                    refreshList()
                    (activity as? MainActivity)?.notifyNodesChanged()
                    Toast.makeText(requireContext(), "Подписка \"${sub.name}\" удалена", Toast.LENGTH_SHORT).show()
                }
                .setNegativeButton("Отмена", null)
                .show()
        }

        btnCancel.setOnClickListener {
            dialog.dismiss()
        }

        dialog.show()
    }

    private fun showNodeActionDialog(node: VpnNode) {
        val dialogView = LayoutInflater.from(requireContext()).inflate(R.layout.dialog_node_actions, null)
        val tvTitle = dialogView.findViewById<TextView>(R.id.tvActionNodeTitle)
        val tvDetails = dialogView.findViewById<TextView>(R.id.tvActionNodeDetails)
        val btnPin = dialogView.findViewById<MaterialButton>(R.id.btnPinNode)
        val btnToggle = dialogView.findViewById<MaterialButton>(R.id.btnToggleNodeActive)
        val btnShare = dialogView.findViewById<MaterialButton>(R.id.btnShareNodeQr)
        val btnDelete = dialogView.findViewById<MaterialButton>(R.id.btnDeleteNode)
        val btnCancel = dialogView.findViewById<MaterialButton>(R.id.btnCancelNodeAction)

        tvTitle.text = node.name
        tvDetails.text = node.getDisplayDetails()

        val isPinned = (settings.forcedNodeId == node.id)
        if (isPinned) {
            btnPin.text = "🔄 Снять принудительную фиксацию (Вернуть Multi-Link)"
            btnPin.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_gold))
        } else {
            btnPin.text = "📌 Принудительно зафиксировать этот сервер"
            btnPin.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_mint))
        }

        if (node.isEnabled) {
            btnToggle.text = "⏸️ Сделать неактивным (Выключить)"
            btnToggle.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_gold))
        } else {
            btnToggle.text = "▶️ Сделать активным (Включить)"
            btnToggle.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_mint))
        }

        val dialog = MaterialAlertDialogBuilder(requireContext())
            .setView(dialogView)
            .create()

        btnPin.setOnClickListener {
            if (isPinned) {
                settings.forcedNodeId = null
                Toast.makeText(requireContext(), "Фиксация снята: активен Multi-Link", Toast.LENGTH_SHORT).show()
            } else {
                settings.forcedNodeId = node.id
                settings.selectNode(node.id)
                Toast.makeText(requireContext(), "Сервер зафиксирован: ${node.name}", Toast.LENGTH_SHORT).show()
            }
            dialog.dismiss()
            refreshList()
            (activity as? MainActivity)?.notifyNodesChanged()
        }

        btnToggle.setOnClickListener {
            val all = settings.getNodes()
            val target = all.find { it.id == node.id }
            if (target != null) {
                target.isEnabled = !target.isEnabled
                settings.saveNodes(all)
                refreshList()
                (activity as? MainActivity)?.notifyNodesChanged()
                val statusText = if (target.isEnabled) "активирован" else "отключен"
                Toast.makeText(requireContext(), "Узел $statusText", Toast.LENGTH_SHORT).show()
            }
            dialog.dismiss()
        }

        btnShare.setOnClickListener {
            dialog.dismiss()
            (activity as? MainActivity)?.showShareQrDialog(node)
        }

        btnDelete.setOnClickListener {
            MaterialAlertDialogBuilder(requireContext())
                .setTitle("Удалить сервер?")
                .setMessage("Вы уверены, что хотите удалить \"${node.name}\"?")
                .setPositiveButton("Удалить") { _, _ ->
                    val all = settings.getNodes().toMutableList()
                    all.removeAll { it.id == node.id }
                    settings.saveNodes(all)
                    if (settings.forcedNodeId == node.id) {
                        settings.forcedNodeId = null
                    }
                    dialog.dismiss()
                    refreshList()
                    (activity as? MainActivity)?.notifyNodesChanged()
                    Toast.makeText(requireContext(), "Сервер удален", Toast.LENGTH_SHORT).show()
                }
                .setNegativeButton("Отмена", null)
                .show()
        }

        btnCancel.setOnClickListener {
            dialog.dismiss()
        }

        dialog.show()
    }

    override fun onDestroyView() {
        super.onDestroyView()
        _binding = null
    }
}
