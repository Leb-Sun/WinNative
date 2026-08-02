package com.winlator.cmod.app.shell

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.AltRoute
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.winlator.cmod.R
import com.winlator.cmod.shared.ui.nav.DialogPaneNav
import com.winlator.cmod.shared.ui.nav.LocalPaneNav
import com.winlator.cmod.shared.ui.nav.PaneNavRegistry
import com.winlator.cmod.shared.ui.nav.paneNavItem
import java.util.Date

/** A single Steam beta branch entry from appinfo depots.branches. */
internal data class StoreBetaBranchItem(
    val name: String,
    val buildId: Long,
    val timeUpdated: Date?,
    val pwdRequired: Boolean,
)

private val WsBg = Color(0xFF12121B)
private val WsBorder = Color(0xFF2A2A3A)
private val WsAccent = Color(0xFF1A9FFF)
private val WsAccentGlow = Color(0xFF58A6FF)
private val WsTextPrimary = Color(0xFFF0F4FF)
private val WsTextSecondary = Color(0xFF93A6BC)
private val WsScrim = Color(0xFF000000)
private val WsLocked = Color(0xFF505060)

/**
 * Steam beta-branch picker — a Workshop-shaped modal listing the game's PICS
 * depots.branches entries. Password-protected branches are shown disabled;
 * this app has no beta-password flow.
 *
 * Stateless: data and callbacks are hoisted to the BetaBranchesDialog wrapper.
 */
@Composable
internal fun StoreBetaBranchScreen(
    gameTitle: String,
    branches: List<StoreBetaBranchItem>,
    selectedBranch: StoreBetaBranchItem?,
    onSelect: (StoreBetaBranchItem) -> Unit,
    onClose: () -> Unit,
) {
    val registry = remember { PaneNavRegistry() }
    CompositionLocalProvider(LocalPaneNav provides registry) {
        DialogPaneNav(registry, onDismiss = onClose)
        BoxWithConstraints(
            modifier =
                Modifier
                    .fillMaxSize()
                    .background(WsScrim.copy(alpha = 0.6f))
                    .windowInsetsPadding(WindowInsets.navigationBars),
            contentAlignment = Alignment.Center,
        ) {
            val dialogWidth = (maxWidth - 32.dp).coerceAtMost(560.dp)
            val dialogMaxHeight = (maxHeight - 48.dp).coerceIn(220.dp, 640.dp)
            Surface(
                modifier =
                    Modifier
                        .widthIn(min = 320.dp, max = dialogWidth)
                        .fillMaxWidth()
                        .heightIn(max = dialogMaxHeight),
                shape = RoundedCornerShape(14.dp),
                color = WsBg,
                border = BorderStroke(1.dp, WsBorder),
                tonalElevation = 8.dp,
            ) {
                Column(Modifier.fillMaxWidth()) {
                    BetaBranchHeader(
                        gameTitle = gameTitle,
                        branchCount = branches.size,
                        onClose = onClose,
                    )
                    HorizontalDivider(color = WsBorder, thickness = 0.5.dp)
                    LazyColumn(
                        modifier = Modifier.fillMaxWidth().weight(1f, fill = false),
                        contentPadding = PaddingValues(vertical = 4.dp),
                    ) {
                        itemsIndexed(branches) { index, branch ->
                            BetaBranchPickerRow(
                                branch = branch,
                                selected = branch == selectedBranch,
                                onClick = { onSelect(branch) },
                            )
                            if (index < branches.lastIndex) {
                                HorizontalDivider(
                                    color = Color.White.copy(alpha = 0.06f),
                                    thickness = 1.dp,
                                    modifier = Modifier.padding(horizontal = 14.dp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun BetaBranchHeader(
    gameTitle: String,
    branchCount: Int,
    onClose: () -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(
            Modifier
                .size(34.dp)
                .clip(RoundedCornerShape(9.dp))
                .background(WsAccent.copy(alpha = 0.16f)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                Icons.AutoMirrored.Outlined.AltRoute,
                contentDescription = null,
                tint = WsAccentGlow,
                modifier = Modifier.size(19.dp),
            )
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Text(
                stringResource(R.string.store_game_beta_branch).uppercase(),
                color = WsTextSecondary,
                fontSize = 9.sp,
                fontWeight = FontWeight.Bold,
                letterSpacing = 0.9.sp,
            )
            Text(
                gameTitle,
                style = MaterialTheme.typography.titleSmall,
                color = WsTextPrimary,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        val branchCountLabel =
            androidx.compose.ui.res.pluralStringResource(
                R.plurals.store_game_beta_branch_count,
                branchCount,
                branchCount,
            )
        Surface(
            modifier = Modifier.semantics { contentDescription = branchCountLabel },
            color = WsAccent.copy(alpha = 0.14f),
            shape = RoundedCornerShape(7.dp),
        ) {
            Text(
                branchCount.toString(),
                color = WsAccentGlow,
                fontSize = 11.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(horizontal = 9.dp, vertical = 3.dp),
            )
        }
        IconButton(
            onClick = onClose,
            modifier = Modifier.size(36.dp).paneNavItem(onActivate = onClose),
        ) {
            Icon(
                Icons.Outlined.Close,
                contentDescription = stringResource(R.string.common_ui_close),
                tint = WsTextSecondary,
                modifier = Modifier.size(20.dp),
            )
        }
    }
}

@Composable
private fun BetaBranchPickerRow(
    branch: StoreBetaBranchItem,
    selected: Boolean,
    onClick: () -> Unit,
) {
    val locked = branch.pwdRequired
    Row(
        modifier =
            Modifier
                .fillMaxWidth()
                .then(
                    if (locked) {
                        Modifier
                    } else {
                        Modifier
                            .paneNavItem(cornerRadius = 8.dp, onActivate = onClick)
                            .clickable(onClick = onClick)
                    },
                )
                .alpha(if (locked) 0.45f else 1f)
                .padding(horizontal = 14.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(11.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            val displayName =
                if (branch.name.equals("public", ignoreCase = true)) {
                    stringResource(R.string.store_game_beta_branch_default, branch.name)
                } else {
                    branch.name
                }
            Text(
                displayName,
                color = if (selected) WsAccentGlow else WsTextPrimary,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            val dateStr =
                remember(branch.timeUpdated) {
                    branch.timeUpdated
                        ?.let { java.text.DateFormat.getDateInstance(java.text.DateFormat.MEDIUM).format(it) }
                        ?: "—"
                }
            Text(
                stringResource(R.string.store_game_beta_branch_build, branch.buildId, dateStr),
                color = WsTextSecondary,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        when {
            locked -> Icon(
                Icons.Outlined.Lock,
                contentDescription = stringResource(R.string.store_game_beta_branch_locked),
                tint = WsLocked,
                modifier = Modifier.size(17.dp),
            )
            selected -> Icon(
                Icons.Outlined.Check,
                contentDescription = null,
                tint = WsAccentGlow,
                modifier = Modifier.size(18.dp),
            )
        }
    }
}
