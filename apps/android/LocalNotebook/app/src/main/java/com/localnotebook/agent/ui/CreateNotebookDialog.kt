package com.localnotebook.agent.ui

import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.localnotebook.agent.core.L10n

/** New-notebook dialog: name, a note that details are AI-generated, and optional source files. */
@Composable
fun CreateNotebookDialog(
    en: Boolean,
    onDismiss: () -> Unit,
    onCreate: (title: String, files: List<Uri>) -> Unit,
) {
    var name by remember { mutableStateOf("") }
    val files = remember { mutableStateListOf<Uri>() }
    val ctx = LocalContext.current
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        for (u in uris) if (u !in files) files.add(u)
    }
    fun displayName(uri: Uri): String {
        var n = uri.lastPathSegment ?: "file"
        ctx.contentResolver.query(uri, null, null, null, null)?.use { c ->
            val idx = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (idx >= 0 && c.moveToFirst()) n = c.getString(idx) ?: n
        }
        return n
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(L10n.t("nb.new", en)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text(L10n.t("nb.dlg_name", en)) },
                    placeholder = { Text(L10n.t("nb.dlg_name_ph", en)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    L10n.t("nb.dlg_ai_hint", en),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(L10n.t("nb.dlg_sources", en), style = MaterialTheme.typography.labelLarge)
                OutlinedButton(onClick = { picker.launch(arrayOf("*/*")) }) { Text(L10n.t("nb.dlg_add_files", en)) }
                files.toList().forEach { uri ->
                    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                        Text(
                            displayName(uri),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.weight(1f),
                        )
                        TextButton(onClick = { files.remove(uri) }) { Text("\u2715") }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(enabled = name.isNotBlank(), onClick = { onCreate(name, files.toList()) }) {
                Text(L10n.t("common.create", en))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(L10n.t("common.cancel", en)) } },
    )
}
