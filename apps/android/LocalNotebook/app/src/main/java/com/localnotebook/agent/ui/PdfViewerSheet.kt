package com.localnotebook.agent.ui

import android.graphics.Bitmap
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.localnotebook.agent.core.PdfPreviewTarget
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PdfViewerOverlay(
    target: PdfPreviewTarget,
    onClose: () -> Unit,
) {
    var currentPage by remember(target) { mutableIntStateOf((target.initialPage - 1).coerceAtLeast(0)) }
    var pageCount by remember(target) { mutableIntStateOf(0) }
    var bitmap by remember(target) { mutableStateOf<Bitmap?>(null) }

    LaunchedEffect(target.filePath, currentPage) {
        val rendered = withContext(Dispatchers.IO) {
            runCatching {
                val file = File(target.filePath)
                ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { pfd ->
                    PdfRenderer(pfd).use { renderer ->
                        val count = renderer.pageCount
                        val safeIdx = currentPage.coerceIn(0, (count - 1).coerceAtLeast(0))
                        renderer.openPage(safeIdx).use { page ->
                            val scale = 2
                            val bmp = Bitmap.createBitmap(
                                (page.width * scale).coerceAtMost(2400),
                                (page.height * scale).coerceAtMost(3200),
                                Bitmap.Config.ARGB_8888,
                            )
                            bmp.eraseColor(android.graphics.Color.WHITE)
                            page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                            Pair(count, bmp)
                        }
                    }
                }
            }.getOrNull()
        }
        if (rendered != null) {
            pageCount = rendered.first
            bitmap = rendered.second
        }
    }

    Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(modifier = Modifier.fillMaxSize()) {
            TopAppBar(
                title = {
                    Column {
                        Text(target.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium)
                        if (pageCount > 0) {
                            Text("${currentPage + 1} / $pageCount", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                },
                navigationIcon = {
                    IconButton(onClick = onClose) { Icon(Icons.Default.Close, contentDescription = null) }
                },
                actions = {
                    IconButton(onClick = { if (currentPage > 0) currentPage-- }, enabled = currentPage > 0) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = null)
                    }
                    IconButton(onClick = { if (currentPage + 1 < pageCount) currentPage++ }, enabled = currentPage + 1 < pageCount) {
                        Icon(Icons.AutoMirrored.Filled.ArrowForward, contentDescription = null)
                    }
                },
            )
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(Color(0xFF181A1F))
                    .padding(12.dp),
                contentAlignment = Alignment.Center,
            ) {
                val bmp = bitmap
                if (bmp != null) {
                    Image(
                        bitmap = bmp.asImageBitmap(),
                        contentDescription = target.title,
                        contentScale = ContentScale.Fit,
                        modifier = Modifier.fillMaxSize(),
                    )
                } else {
                    CircularProgressIndicator()
                }
            }
        }
    }
}
