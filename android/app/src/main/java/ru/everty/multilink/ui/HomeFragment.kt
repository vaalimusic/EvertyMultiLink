package ru.everty.multilink.ui

import android.content.res.ColorStateList
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import androidx.core.content.ContextCompat
import androidx.fragment.app.Fragment
import ru.everty.multilink.R
import ru.everty.multilink.databinding.FragmentHomeBinding
import ru.everty.multilink.model.AppSettings
import ru.everty.multilink.model.Policy
import ru.everty.multilink.service.EvertyVpnService

class HomeFragment : Fragment() {

    private var _binding: FragmentHomeBinding? = null
    private val binding get() = _binding!!

    private lateinit var settings: AppSettings
    private val timerHandler = Handler(Looper.getMainLooper())
    private var connectedStartTime: Long = 0L

    private val timerRunnable = object : Runnable {
        override fun run() {
            if (EvertyVpnService.currentState == EvertyVpnService.STATE_CONNECTED) {
                val elapsed = (System.currentTimeMillis() - connectedStartTime) / 1000
                val hours = elapsed / 3600
                val minutes = (elapsed % 3600) / 60
                val seconds = elapsed % 60
                binding.tvConnectionTimer.text = String.format("%02d:%02d:%02d", hours, minutes, seconds)
                timerHandler.postDelayed(this, 1000)
            }
        }
    }

    override fun onCreateView(
        inflater: LayoutInflater, container: ViewGroup?,
        savedInstanceState: Bundle?
    ): View {
        _binding = FragmentHomeBinding.inflate(inflater, container, false)
        return binding.root
    }

    override fun onViewCreated(view: View, savedInstanceState: Bundle?) {
        super.onViewCreated(view, savedInstanceState)
        settings = AppSettings(requireContext())

        setupListeners()
        updateModeDisplay()
        updateVpnState(EvertyVpnService.currentState)
    }

    private fun setupListeners() {
        binding.frameConnectCircle.setOnClickListener {
            (activity as? MainActivity)?.toggleVpn()
        }

        binding.btnHomeLanSync.setOnClickListener {
            (activity as? MainActivity)?.showLanSyncDialog()
        }

        binding.btnHomeShareQr.setOnClickListener {
            (activity as? MainActivity)?.showShareQrDialog()
        }

        binding.btnHomeScanQr.setOnClickListener {
            (activity as? MainActivity)?.startQrScan()
        }

        binding.btnModeSmart.setOnClickListener {
            settings.policy = Policy.SMART
            updateModeDisplay()
        }

        binding.btnModeFastest.setOnClickListener {
            settings.policy = Policy.LATENCY
            updateModeDisplay()
        }

        binding.btnModeBalance.setOnClickListener {
            settings.policy = Policy.BALANCE
            updateModeDisplay()
        }

        binding.btnResetPin.setOnClickListener {
            settings.forcedNodeId = null
            updateModeDisplay()
            (activity as? MainActivity)?.notifyNodesChanged()
        }
    }

    fun updateModeDisplay() {
        if (_binding == null || !::settings.isInitialized) return

        val forcedNode = settings.getForcedNode()
        val allNodes = settings.getNodes()
        val enabledNodes = allNodes.filter { it.isEnabled }
        binding.badgeActiveNodes.text = "${enabledNodes.size} / ${allNodes.size} узлов"

        if (forcedNode != null && forcedNode.isEnabled) {
            binding.layoutPinnedServer.visibility = View.VISIBLE
            binding.tvPinnedServerName.text = "📌 Зафиксирован: ${forcedNode.name}"
            binding.tvModeTitle.text = "📌 Одиночный режим (Сервер зафиксирован)"
            binding.tvModeDescription.text = "Весь защищённый трафик принудительно направляется только через этот сервер."
        } else {
            binding.layoutPinnedServer.visibility = View.GONE
            binding.tvModeTitle.text = "⚡ Режим: Multi-Link (${settings.policy.label})"
            binding.tvModeDescription.text = "Автоматическая балансировка и выбор наилучшего маршрута без привязки к одному серверу."
        }

        val primaryBg = ContextCompat.getDrawable(requireContext(), R.drawable.btn_primary_selector)
        val secondaryBg = ContextCompat.getDrawable(requireContext(), R.drawable.btn_secondary_selector)

        binding.btnModeSmart.background = if (settings.policy == Policy.SMART) primaryBg else secondaryBg
        binding.btnModeFastest.background = if (settings.policy == Policy.LATENCY) primaryBg else secondaryBg
        binding.btnModeBalance.background = if (settings.policy == Policy.BALANCE) primaryBg else secondaryBg
    }

    fun updateVpnState(state: Int) {
        if (_binding == null) return

        when (state) {
            EvertyVpnService.STATE_CONNECTED -> {
                binding.frameConnectCircle.setBackgroundResource(R.drawable.btn_connect_circle_on)
                binding.ivPowerIcon.imageTintList = ColorStateList.valueOf(ContextCompat.getColor(requireContext(), R.color.accent_green))
                binding.tvConnectAction.text = "ОТКЛЮЧИТЬ"
                binding.tvConnectHint.text = "Нажмите для разъединения"
                binding.tvConnectionStatus.text = "● ЗАЩИЩЕНО"
                binding.tvConnectionStatus.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_green))
                binding.tvConnectionTimer.visibility = View.VISIBLE
                binding.tvCurrentIp.text = "Трафик защищен • Multi-Link активен"

                if (connectedStartTime == 0L) {
                    connectedStartTime = System.currentTimeMillis()
                    timerHandler.post(timerRunnable)
                }
            }
            EvertyVpnService.STATE_CONNECTING -> {
                binding.frameConnectCircle.setBackgroundResource(R.drawable.btn_connect_circle_connecting)
                binding.ivPowerIcon.imageTintList = ColorStateList.valueOf(ContextCompat.getColor(requireContext(), R.color.accent_orange))
                binding.tvConnectAction.text = "ПОДКЛЮЧЕНИЕ…"
                binding.tvConnectHint.text = "Установка соединения"
                binding.tvConnectionStatus.text = "● ПОДКЛЮЧЕНИЕ…"
                binding.tvConnectionStatus.setTextColor(ContextCompat.getColor(requireContext(), R.color.accent_orange))
            }
            else -> {
                binding.frameConnectCircle.setBackgroundResource(R.drawable.btn_connect_circle_off)
                binding.ivPowerIcon.imageTintList = ColorStateList.valueOf(ContextCompat.getColor(requireContext(), R.color.accent_cyan))
                binding.tvConnectAction.text = "ПОДКЛЮЧИТЬ"
                binding.tvConnectHint.text = "Нажмите для старта"
                binding.tvConnectionStatus.text = "● ОТКЛЮЧЕНО"
                binding.tvConnectionStatus.setTextColor(ContextCompat.getColor(requireContext(), R.color.text_muted))
                binding.tvConnectionTimer.visibility = View.GONE
                binding.tvCurrentIp.text = "Прямой интернет"

                connectedStartTime = 0L
                timerHandler.removeCallbacks(timerRunnable)
            }
        }
    }

    override fun onResume() {
        super.onResume()
        updateModeDisplay()
        updateVpnState(EvertyVpnService.currentState)
    }

    override fun onDestroyView() {
        timerHandler.removeCallbacks(timerRunnable)
        super.onDestroyView()
        _binding = null
    }
}
