package ru.everty.multilink.ui

import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.CheckBox
import android.widget.EditText
import android.widget.ProgressBar
import android.widget.Toast
import androidx.core.widget.doAfterTextChanged
import androidx.fragment.app.Fragment
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.button.MaterialButton
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import ru.everty.multilink.R
import ru.everty.multilink.databinding.FragmentRoutingBinding
import ru.everty.multilink.model.AppSettings

class RoutingFragment : Fragment() {

    private var _binding: FragmentRoutingBinding? = null
    private val binding get() = _binding!!

    private lateinit var settings: AppSettings
    private lateinit var rulesAdapter: CustomRulesAdapter

    override fun onCreateView(
        inflater: LayoutInflater, container: ViewGroup?,
        savedInstanceState: Bundle?
    ): View {
        _binding = FragmentRoutingBinding.inflate(inflater, container, false)
        return binding.root
    }

    override fun onViewCreated(view: View, savedInstanceState: Bundle?) {
        super.onViewCreated(view, savedInstanceState)
        settings = AppSettings(requireContext())

        setupSplitRu()
        setupCustomRules()
        setupPerAppVpn()
        setupAutoUpdateAndProjectLinks()
    }

    private fun setupAutoUpdateAndProjectLinks() {
        binding.switchAutoUpdateSub.isChecked = settings.isAutoUpdateOnStartEnabled
        binding.switchAutoUpdateSub.setOnCheckedChangeListener { _, isChecked ->
            settings.isAutoUpdateOnStartEnabled = isChecked
        }

        // Cute pulsating heart animation for Boosty
        val pulseAnim = android.view.animation.ScaleAnimation(
            1.0f, 1.22f, 1.0f, 1.22f,
            android.view.animation.Animation.RELATIVE_TO_SELF, 0.5f,
            android.view.animation.Animation.RELATIVE_TO_SELF, 0.5f
        ).apply {
            duration = 850
            repeatMode = android.view.animation.Animation.REVERSE
            repeatCount = android.view.animation.Animation.INFINITE
            interpolator = android.view.animation.AccelerateDecelerateInterpolator()
        }
        binding.ivBoostyHeart.startAnimation(pulseAnim)

        binding.cardBoostyDonate.setOnClickListener {
            openUrl("https://boosty.to/vaali/donate")
        }

        binding.cardOpenSourceLink.setOnClickListener {
            openUrl("https://desk.everty.ru/opensource")
        }

        binding.cardGithubLink.setOnClickListener {
            openUrl("https://github.com/vaalimusic/EvertyMultiLink")
        }

        setupAppUpdates()
    }

    private fun setupAppUpdates() {
        val currentVersion = ru.everty.multilink.update.UpdateChecker.getAppVersion(requireContext())
        binding.tvAppUpdateDesc.text = getString(R.string.update_desc, currentVersion)

        var latestRelease: ru.everty.multilink.update.UpdateChecker.ReleaseInfo? = null

        fun doCheck(showFeedback: Boolean) {
            binding.tvAppUpdateDesc.text = getString(R.string.update_checking)
            binding.btnAppUpdateAction.text = "…"
            Thread {
                val res = ru.everty.multilink.update.UpdateChecker.checkLatestReleaseSync(currentVersion)
                activity?.runOnUiThread {
                    if (_binding == null || !isAdded) return@runOnUiThread
                    val release = res.getOrNull()
                    latestRelease = release
                    if (release != null) {
                        binding.tvAppUpdateDesc.text = getString(R.string.update_available, release.tagName)
                        binding.btnAppUpdateAction.text = getString(R.string.btn_update)
                        binding.btnAppUpdateAction.setTextColor(androidx.core.content.ContextCompat.getColor(requireContext(), R.color.accent_mint))
                        if (showFeedback) {
                            val targetUrl = release.apkUrl ?: release.htmlUrl
                            openUrl(targetUrl)
                        }
                    } else {
                        binding.tvAppUpdateDesc.text = getString(R.string.update_latest)
                        binding.btnAppUpdateAction.text = getString(R.string.btn_check_update)
                        if (showFeedback) {
                            android.widget.Toast.makeText(requireContext(), R.string.update_latest, android.widget.Toast.LENGTH_SHORT).show()
                        }
                    }
                }
            }.start()
        }

        binding.cardAppUpdate.setOnClickListener {
            if (latestRelease != null) {
                val targetUrl = latestRelease!!.apkUrl ?: latestRelease!!.htmlUrl
                openUrl(targetUrl)
            } else {
                doCheck(true)
            }
        }
        binding.btnAppUpdateAction.setOnClickListener {
            if (latestRelease != null) {
                val targetUrl = latestRelease!!.apkUrl ?: latestRelease!!.htmlUrl
                openUrl(targetUrl)
            } else {
                doCheck(true)
            }
        }

        doCheck(false)
    }

    private fun openUrl(url: String) {
        try {
            val browserIntent = Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url))
            startActivity(browserIntent)
        } catch (_: Exception) {
            Toast.makeText(requireContext(), "Не удалось открыть ссылку", Toast.LENGTH_SHORT).show()
        }
    }

    private fun setupSplitRu() {
        binding.switchSplitRu.isChecked = settings.isSplitRuEnabled
        binding.switchSplitRu.setOnCheckedChangeListener { _, isChecked ->
            settings.isSplitRuEnabled = isChecked
            Toast.makeText(
                requireContext(),
                if (isChecked) "Прямой доступ для РФ включен" else "Прямой доступ для РФ выключен",
                Toast.LENGTH_SHORT
            ).show()
        }
    }

    private fun setupCustomRules() {
        val actions = listOf("DIRECT (Напрямую)", "PROXY (Через VPN)")
        val spinnerAdapter = ArrayAdapter(requireContext(), android.R.layout.simple_spinner_dropdown_item, actions)
        binding.spinnerRuleAction.adapter = spinnerAdapter

        rulesAdapter = CustomRulesAdapter(
            rules = settings.getCustomRules(),
            onDeleteClick = { rule ->
                settings.deleteCustomRule(rule.id)
                refreshRulesList()
            }
        )

        binding.rvCustomRules.layoutManager = LinearLayoutManager(requireContext())
        binding.rvCustomRules.adapter = rulesAdapter

        binding.btnAddRule.setOnClickListener {
            val text = binding.etNewRulePattern.text.toString().trim()
            if (text.isEmpty()) {
                binding.etNewRulePattern.error = "Введите домен или IP"
                return@setOnClickListener
            }

            val isDirect = binding.spinnerRuleAction.selectedItemPosition == 0
            val action = if (isDirect) "DIRECT" else "PROXY"
            val type = when {
                text.contains("/") -> "IP-CIDR"
                text.startsWith(".") -> "DOMAIN-SUFFIX"
                text.contains(".") -> "DOMAIN-KEYWORD"
                else -> "DOMAIN-KEYWORD"
            }

            settings.addCustomRule(text, type, action)
            binding.etNewRulePattern.text.clear()
            refreshRulesList()
            Toast.makeText(requireContext(), "Правило для $text добавлено", Toast.LENGTH_SHORT).show()
        }
    }

    private fun refreshRulesList() {
        rulesAdapter.updateRules(settings.getCustomRules())
    }

    private fun setupPerAppVpn() {
        binding.switchPerAppVpn.isChecked = settings.isPerAppVpnEnabled
        binding.layoutPerAppSettings.visibility = if (settings.isPerAppVpnEnabled) View.VISIBLE else View.GONE

        binding.switchPerAppVpn.setOnCheckedChangeListener { _, isChecked ->
            settings.isPerAppVpnEnabled = isChecked
            binding.layoutPerAppSettings.visibility = if (isChecked) View.VISIBLE else View.GONE
        }

        if (settings.perAppVpnMode == "EXCLUDE") {
            binding.rbPerAppExclude.isChecked = true
        } else {
            binding.rbPerAppInclude.isChecked = true
        }

        binding.rgPerAppMode.setOnCheckedChangeListener { _, checkedId ->
            settings.perAppVpnMode = if (checkedId == R.id.rbPerAppExclude) "EXCLUDE" else "INCLUDE"
            updateSelectAppsButtonText()
        }

        binding.btnSelectApps.setOnClickListener {
            showSelectAppsDialog()
        }

        updateSelectAppsButtonText()
    }

    private fun updateSelectAppsButtonText() {
        val count = settings.perAppVpnPackages.size
        binding.btnSelectApps.text = "📱 Выбрать приложения ($count выбрано)"
    }

    private fun showSelectAppsDialog() {
        val dialogView = LayoutInflater.from(requireContext()).inflate(R.layout.dialog_select_apps, null)
        val etSearch = dialogView.findViewById<EditText>(R.id.etSearchApp)
        val rvApps = dialogView.findViewById<RecyclerView>(R.id.rvAppsList)
        val btnClose = dialogView.findViewById<MaterialButton>(R.id.btnCloseAppsDialog)
        val cbShowSystem = dialogView.findViewById<CheckBox>(R.id.cbShowSystemApps)
        val pbLoading = dialogView.findViewById<ProgressBar>(R.id.pbLoadingApps)

        val selectedSet = settings.perAppVpnPackages.toMutableSet()
        val appsAdapter = AppSelectAdapter(emptyList()) { item ->
            settings.toggleAppPackage(item.packageName)
            updateSelectAppsButtonText()
        }

        rvApps.layoutManager = LinearLayoutManager(requireContext())
        rvApps.adapter = appsAdapter

        etSearch.doAfterTextChanged { text ->
            appsAdapter.filter(text?.toString() ?: "")
        }

        cbShowSystem.setOnCheckedChangeListener { _, isChecked ->
            appsAdapter.setShowSystem(isChecked)
        }

        pbLoading.visibility = View.VISIBLE

        val dialog = MaterialAlertDialogBuilder(requireContext())
            .setView(dialogView)
            .create()

        btnClose.setOnClickListener {
            dialog.dismiss()
        }

        dialog.show()

        val appContext = requireContext().applicationContext
        Thread {
            val pm = appContext.packageManager
            val installedApps: List<ApplicationInfo> = try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    pm.getInstalledApplications(PackageManager.ApplicationInfoFlags.of(0))
                } else {
                    @Suppress("DEPRECATION")
                    pm.getInstalledApplications(0)
                }
            } catch (e: Exception) {
                emptyList()
            }

            val appItems = installedApps.mapNotNull { appInfo ->
                val pkg = appInfo.packageName
                if (pkg == appContext.packageName) return@mapNotNull null

                val launchIntent = pm.getLaunchIntentForPackage(pkg)
                    ?: pm.getLeanbackLaunchIntentForPackage(pkg)
                val isLaunchable = launchIntent != null
                val isSystem = (appInfo.flags and ApplicationInfo.FLAG_SYSTEM) != 0
                    && (appInfo.flags and ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) == 0
                    && !isLaunchable

                val name = try {
                    appInfo.loadLabel(pm).toString()
                } catch (e: Exception) {
                    pkg
                }
                val icon = try {
                    appInfo.loadIcon(pm)
                } catch (e: Exception) {
                    null
                }

                AppInfoItem(
                    name = name,
                    packageName = pkg,
                    icon = icon,
                    isSelected = selectedSet.contains(pkg),
                    isSystem = isSystem
                )
            }.sortedWith(
                compareByDescending<AppInfoItem> { it.isSelected }
                    .thenBy { it.isSystem }
                    .thenBy { it.name.lowercase() }
            )

            activity?.runOnUiThread {
                if (_binding == null || !isAdded) return@runOnUiThread
                pbLoading.visibility = View.GONE
                appsAdapter.setApps(appItems, cbShowSystem.isChecked)
            }
        }.start()
    }

    override fun onResume() {
        super.onResume()
        refreshRulesList()
        updateSelectAppsButtonText()
    }

    override fun onDestroyView() {
        super.onDestroyView()
        _binding = null
    }
}
