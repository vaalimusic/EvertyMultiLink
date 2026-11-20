package ru.everty.multilink.ui

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.button.MaterialButton
import ru.everty.multilink.R
import ru.everty.multilink.model.Subscription
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

class SubscriptionsAdapter(
    private var subscriptions: List<Subscription>,
    private val onSubscriptionClick: (Subscription) -> Unit,
    private val onSubscriptionLongClick: (Subscription) -> Unit,
    private val onRefreshClick: (Subscription) -> Unit
) : RecyclerView.Adapter<SubscriptionsAdapter.SubViewHolder>() {

    fun updateData(newSubs: List<Subscription>) {
        subscriptions = newSubs
        notifyDataSetChanged()
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): SubViewHolder {
        val view = LayoutInflater.from(parent.context).inflate(R.layout.item_subscription, parent, false)
        return SubViewHolder(view)
    }

    override fun onBindViewHolder(holder: SubViewHolder, position: Int) {
        holder.bind(subscriptions[position])
    }

    override fun getItemCount(): Int = subscriptions.size

    inner class SubViewHolder(itemView: View) : RecyclerView.ViewHolder(itemView) {
        private val tvSubName: TextView = itemView.findViewById(R.id.tvSubName)
        private val tvSubCount: TextView = itemView.findViewById(R.id.tvSubCount)
        private val tvSubExpire: TextView = itemView.findViewById(R.id.tvSubExpire)
        private val tvSubUrl: TextView = itemView.findViewById(R.id.tvSubUrl)
        private val tvSubUpdated: TextView = itemView.findViewById(R.id.tvSubUpdated)
        private val btnSubRefresh: MaterialButton = itemView.findViewById(R.id.btnSubRefresh)

        init {
            itemView.setOnClickListener {
                val pos = bindingAdapterPosition
                if (pos != RecyclerView.NO_POSITION) {
                    onSubscriptionClick(subscriptions[pos])
                }
            }

            itemView.setOnLongClickListener {
                val pos = bindingAdapterPosition
                if (pos != RecyclerView.NO_POSITION) {
                    onSubscriptionLongClick(subscriptions[pos])
                    true
                } else false
            }

            btnSubRefresh.setOnClickListener {
                val pos = bindingAdapterPosition
                if (pos != RecyclerView.NO_POSITION) {
                    onRefreshClick(subscriptions[pos])
                }
            }

            // TV focus animation
            itemView.setOnFocusChangeListener { view, hasFocus ->
                if (hasFocus) {
                    view.animate().scaleX(1.02f).scaleY(1.02f).setDuration(150).start()
                } else {
                    view.animate().scaleX(1.0f).scaleY(1.0f).setDuration(150).start()
                }
            }
        }

        fun bind(sub: Subscription) {
            tvSubName.text = sub.name
            tvSubCount.text = "${sub.nodesCount} узлов"
            tvSubUrl.text = sub.url
            if (sub.updatedAt > 0) {
                val sdf = SimpleDateFormat("dd.MM HH:mm", Locale.getDefault())
                tvSubUpdated.text = "Синхр: ${sdf.format(Date(sub.updatedAt))}"
            } else {
                tvSubUpdated.text = "Не синхронизировано"
            }

            val expireText = sub.getRemainingDaysText()
            if (!expireText.isNullOrBlank()) {
                tvSubExpire.visibility = View.VISIBLE
                tvSubExpire.text = expireText
                if (expireText.contains("Истекла") || expireText.contains("< 1 дня")) {
                    tvSubExpire.setTextColor(androidx.core.content.ContextCompat.getColor(itemView.context, R.color.accent_red))
                } else {
                    tvSubExpire.setTextColor(androidx.core.content.ContextCompat.getColor(itemView.context, R.color.accent_gold))
                }
            } else {
                tvSubExpire.visibility = View.GONE
            }
        }
    }
}
