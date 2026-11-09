package ru.everty.multilink.ui

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.core.content.ContextCompat
import androidx.recyclerview.widget.RecyclerView
import ru.everty.multilink.R
import ru.everty.multilink.model.VpnNode

class NodesAdapter(
    private var nodes: List<VpnNode>,
    private var forcedNodeId: String? = null,
    private val onNodeClicked: (VpnNode) -> Unit,
    private val onNodeLongClick: (VpnNode) -> Unit
) : RecyclerView.Adapter<NodesAdapter.NodeViewHolder>() {

    fun updateNodes(newNodes: List<VpnNode>, newForcedId: String? = forcedNodeId) {
        nodes = newNodes
        forcedNodeId = newForcedId
        notifyDataSetChanged()
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): NodeViewHolder {
        val view = LayoutInflater.from(parent.context).inflate(R.layout.item_node, parent, false)
        return NodeViewHolder(view)
    }

    override fun onBindViewHolder(holder: NodeViewHolder, position: Int) {
        holder.bind(nodes[position])
    }

    override fun getItemCount(): Int = nodes.size

    inner class NodeViewHolder(itemView: View) : RecyclerView.ViewHolder(itemView) {
        private val tvNodeName: TextView = itemView.findViewById(R.id.tvNodeName)
        private val tvNodeDetails: TextView = itemView.findViewById(R.id.tvNodeDetails)
        private val tvNodePing: TextView = itemView.findViewById(R.id.tvNodePing)
        private val tvNodeDisabled: TextView = itemView.findViewById(R.id.tvNodeDisabled)
        private val tvNodePinned: TextView = itemView.findViewById(R.id.tvNodePinned)

        init {
            itemView.setOnClickListener {
                val pos = bindingAdapterPosition
                if (pos != RecyclerView.NO_POSITION) {
                    val node = nodes[pos]
                    if (node.isEnabled) {
                        onNodeClicked(node)
                    }
                }
            }

            itemView.setOnLongClickListener {
                val pos = bindingAdapterPosition
                if (pos != RecyclerView.NO_POSITION) {
                    onNodeLongClick(nodes[pos])
                    true
                } else {
                    false
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

        fun bind(node: VpnNode) {
            tvNodeName.text = node.name
            val details = if (node.isRussianNode()) {
                "${node.getDisplayDetails()} • 🇷🇺 RU (прямой доступ)"
            } else {
                node.getDisplayDetails()
            }
            tvNodeDetails.text = details

            val isPinned = (node.id == forcedNodeId)
            if (isPinned) {
                tvNodePinned.visibility = View.VISIBLE
                tvNodeName.setTextColor(ContextCompat.getColor(itemView.context, R.color.accent_gold))
            } else {
                tvNodePinned.visibility = View.GONE
                tvNodeName.setTextColor(ContextCompat.getColor(itemView.context, R.color.text_primary))
            }

            if (!node.isEnabled) {
                itemView.alpha = 0.45f
                tvNodeDisabled.visibility = View.VISIBLE
                tvNodePing.visibility = View.GONE
            } else {
                itemView.alpha = 1.0f
                tvNodeDisabled.visibility = View.GONE
                tvNodePing.visibility = View.VISIBLE

                if (node.pingMs > 0) {
                    tvNodePing.text = "${node.pingMs} мс"
                    val color = when {
                        node.pingMs < 150 -> ContextCompat.getColor(itemView.context, R.color.accent_mint)
                        node.pingMs < 300 -> ContextCompat.getColor(itemView.context, R.color.accent_gold)
                        else -> ContextCompat.getColor(itemView.context, R.color.accent_red)
                    }
                    tvNodePing.setTextColor(color)
                } else {
                    tvNodePing.text = "тест…"
                    tvNodePing.setTextColor(ContextCompat.getColor(itemView.context, R.color.text_muted))
                }
            }
        }
    }
}
