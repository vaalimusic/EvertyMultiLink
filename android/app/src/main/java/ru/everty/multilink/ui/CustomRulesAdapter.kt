package ru.everty.multilink.ui

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.core.content.ContextCompat
import androidx.recyclerview.widget.RecyclerView
import ru.everty.multilink.R
import ru.everty.multilink.model.CustomRule

class CustomRulesAdapter(
    private var rules: List<CustomRule>,
    private val onDeleteClick: (CustomRule) -> Unit
) : RecyclerView.Adapter<CustomRulesAdapter.RuleViewHolder>() {

    fun updateRules(newRules: List<CustomRule>) {
        rules = newRules
        notifyDataSetChanged()
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): RuleViewHolder {
        val view = LayoutInflater.from(parent.context).inflate(R.layout.item_custom_rule, parent, false)
        return RuleViewHolder(view)
    }

    override fun onBindViewHolder(holder: RuleViewHolder, position: Int) {
        holder.bind(rules[position])
    }

    override fun getItemCount(): Int = rules.size

    inner class RuleViewHolder(itemView: View) : RecyclerView.ViewHolder(itemView) {
        private val tvActionBadge: TextView = itemView.findViewById(R.id.tvRuleActionBadge)
        private val tvPattern: TextView = itemView.findViewById(R.id.tvRulePattern)
        private val tvType: TextView = itemView.findViewById(R.id.tvRuleType)
        private val btnDelete: ImageView = itemView.findViewById(R.id.btnDeleteRule)

        init {
            itemView.setOnFocusChangeListener { view, hasFocus ->
                val scale = if (hasFocus) 1.02f else 1.0f
                view.animate().scaleX(scale).scaleY(scale).setDuration(140).start()
            }

            btnDelete.setOnClickListener {
                val pos = bindingAdapterPosition
                if (pos != RecyclerView.NO_POSITION) {
                    onDeleteClick(rules[pos])
                }
            }
        }

        fun bind(rule: CustomRule) {
            tvPattern.text = rule.pattern
            tvType.text = rule.type
            tvActionBadge.text = rule.action
            if (rule.action == "DIRECT") {
                tvActionBadge.setTextColor(ContextCompat.getColor(itemView.context, R.color.accent_green))
            } else {
                tvActionBadge.setTextColor(ContextCompat.getColor(itemView.context, R.color.accent_cyan))
            }
        }
    }
}
