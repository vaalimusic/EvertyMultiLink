package ru.everty.multilink.ui

import android.graphics.drawable.Drawable
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.CheckBox
import android.widget.ImageView
import android.widget.TextView
import androidx.recyclerview.widget.RecyclerView
import ru.everty.multilink.R

data class AppInfoItem(
    val name: String,
    val packageName: String,
    val icon: Drawable?,
    var isSelected: Boolean,
    val isSystem: Boolean = false
)

class AppSelectAdapter(
    private var allApps: List<AppInfoItem>,
    private val onToggle: (AppInfoItem) -> Unit
) : RecyclerView.Adapter<AppSelectAdapter.AppViewHolder>() {

    private var filteredApps: List<AppInfoItem> = allApps
    private var currentQuery: String = ""
    private var showSystem: Boolean = false

    fun setApps(apps: List<AppInfoItem>, showSystemApps: Boolean = showSystem) {
        allApps = apps
        showSystem = showSystemApps
        applyFilter()
    }

    fun setShowSystem(show: Boolean) {
        showSystem = show
        applyFilter()
    }

    fun filter(query: String) {
        currentQuery = query
        applyFilter()
    }

    private fun applyFilter() {
        val q = currentQuery.trim()
        filteredApps = allApps.filter { item ->
            val matchesSystem = showSystem || !item.isSystem || item.isSelected
            val matchesQuery = q.isEmpty() ||
                item.name.contains(q, ignoreCase = true) ||
                item.packageName.contains(q, ignoreCase = true)
            matchesSystem && matchesQuery
        }
        notifyDataSetChanged()
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): AppViewHolder {
        val view = LayoutInflater.from(parent.context).inflate(R.layout.item_app_select, parent, false)
        return AppViewHolder(view)
    }

    override fun onBindViewHolder(holder: AppViewHolder, position: Int) {
        holder.bind(filteredApps[position])
    }

    override fun getItemCount(): Int = filteredApps.size

    inner class AppViewHolder(itemView: View) : RecyclerView.ViewHolder(itemView) {
        private val ivIcon: ImageView = itemView.findViewById(R.id.ivAppIcon)
        private val tvName: TextView = itemView.findViewById(R.id.tvAppName)
        private val tvPkg: TextView = itemView.findViewById(R.id.tvAppPackage)
        private val cbSelected: CheckBox = itemView.findViewById(R.id.cbAppSelected)

        init {
            itemView.setOnClickListener {
                val pos = bindingAdapterPosition
                if (pos != RecyclerView.NO_POSITION) {
                    val item = filteredApps[pos]
                    item.isSelected = !item.isSelected
                    cbSelected.isChecked = item.isSelected
                    onToggle(item)
                }
            }

            itemView.setOnFocusChangeListener { view, hasFocus ->
                val scale = if (hasFocus) 1.02f else 1.0f
                view.animate().scaleX(scale).scaleY(scale).setDuration(140).start()
            }
        }

        fun bind(item: AppInfoItem) {
            tvName.text = item.name
            tvPkg.text = item.packageName
            cbSelected.isChecked = item.isSelected
            if (item.icon != null) {
                ivIcon.setImageDrawable(item.icon)
            } else {
                ivIcon.setImageResource(R.mipmap.ic_launcher)
            }
        }
    }
}
