package com.localnotebook.agent.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Image
import androidx.compose.material.icons.filled.Link
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.sp
import android.content.Intent
import androidx.core.content.FileProvider
import java.io.File
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.localnotebook.agent.core.L10n
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import kotlin.math.max

const val MAX_ATTACHED_IMAGES = 6
private const val MAX_EDGE = 1568
private const val MAX_INPUT_BYTES = 40 * 1024 * 1024
private const val MAX_FILE_BYTES = 200 * 1024 * 1024

/**
 * Something attached to the next message: a picture (downscaled, with a thumbnail), a one-off
 * reference file (sent as is; the agent reads it on demand) or a web link.
 */
class AttachedImage(
    val name: String,
    val base64: String,
    val thumb: ImageBitmap?,
    val kind: String = "image",
    val url: String? = null,
    val size: Long = 0,
)

/**
 * Puts the attachments produced by [ImageAttachState.toJson] into a request as the core expects
 * them: `images`, `files` and `links`.
 */
fun JSONObject.putAttachments(items: JSONArray?): JSONObject {
    if (items == null) return this
    val images = JSONArray()
    val files = JSONArray()
    val links = JSONArray()
    for (i in 0 until items.length()) {
        val o = items.optJSONObject(i) ?: continue
        when (o.optString("kind", "image")) {
            "file" -> files.put(JSONObject().put("name", o.optString("name")).put("data_base64", o.optString("data_base64")))
            "link" -> links.put(o.optString("url"))
            else -> images.put(JSONObject().put("name", o.optString("name")).put("data_base64", o.optString("data_base64")))
        }
    }
    if (images.length() > 0) put("images", images)
    if (files.length() > 0) put("files", files)
    if (links.length() > 0) put("links", links)
    return this
}

/**
 * Pictures attached to one text input (chat, Studio request, outline revision, lecture question).
 * Pictures are decoded, rotated by their EXIF orientation, downscaled and re-encoded here so the
 * request stays small; the core downscales again defensively.
 */
class ImageAttachState {
    val items = mutableStateListOf<AttachedImage>()
    val isFull get() = items.size >= MAX_ATTACHED_IMAGES
    val hasItems get() = items.isNotEmpty()

    fun clear() = items.clear()
    fun remove(i: AttachedImage) = items.remove(i)

    /** Everything attached, each with its `kind` (see [putAttachments]); null when empty. */
    fun toJson(): JSONArray? {
        if (items.isEmpty()) return null
        val arr = JSONArray()
        items.forEach {
            val o = JSONObject().put("kind", it.kind).put("name", it.name)
            if (it.kind == "link") o.put("url", it.url) else o.put("data_base64", it.base64)
            arr.put(o)
        }
        return arr
    }

    /** Adds picked files as they are (pictures among them become pictures). */
    suspend fun addFiles(context: Context, uris: List<Uri>, en: Boolean): String? {
        var warn: String? = null
        for (uri in uris) {
            if (items.size >= MAX_ATTACHED_IMAGES) {
                warn = String.format(L10n.t("ctx.too_many", en), MAX_ATTACHED_IMAGES)
                break
            }
            val mime = context.contentResolver.getType(uri).orEmpty()
            if (mime.startsWith("image/")) {
                addUris(context, listOf(uri), en)?.let { warn = it }
                continue
            }
            val loaded = withContext(Dispatchers.IO) {
                runCatching {
                    val resolver = context.contentResolver
                    val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                        if (c.moveToFirst()) c.getString(0) else null
                    } ?: "file"
                    val raw = resolver.openInputStream(uri)?.use { it.readBytes() } ?: return@runCatching null
                    if (raw.isEmpty() || raw.size > MAX_FILE_BYTES) return@runCatching null
                    AttachedImage(name, Base64.encodeToString(raw, Base64.NO_WRAP), null, kind = "file", size = raw.size.toLong())
                }.getOrNull()
            }
            if (loaded == null) warn = L10n.t("img.read_failed", en) else items.add(loaded)
        }
        return warn
    }

    fun addLink(url: String) {
        if (items.size >= MAX_ATTACHED_IMAGES) return
        val u = Uri.parse(url)
        val label = (u.host ?: url) + (u.path ?: "").trimEnd('/')
        items.add(AttachedImage(label, "", null, kind = "link", url = url))
    }

    /** Adds the picked pictures; returns a message for the user when some could not be added. */
    suspend fun addUris(context: Context, uris: List<Uri>, en: Boolean): String? {
        var warn: String? = null
        for (uri in uris) {
            if (items.size >= MAX_ATTACHED_IMAGES) {
                warn = String.format(L10n.t("img.too_many", en), MAX_ATTACHED_IMAGES)
                break
            }
            val loaded = withContext(Dispatchers.IO) { runCatching { load(context, uri) }.getOrNull() }
            if (loaded == null) {
                warn = L10n.t("img.read_failed", en)
                continue
            }
            items.add(loaded)
        }
        return warn
    }

    private fun load(context: Context, uri: Uri): AttachedImage? {
        val resolver = context.contentResolver
        val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        } ?: "image"
        val raw = resolver.openInputStream(uri)?.use { it.readBytes() } ?: return null
        if (raw.isEmpty() || raw.size > MAX_INPUT_BYTES) return null

        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(raw, 0, raw.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (max(bounds.outWidth, bounds.outHeight) / (sample * 2) >= MAX_EDGE) sample *= 2
        var bmp = BitmapFactory.decodeByteArray(raw, 0, raw.size, BitmapFactory.Options().apply { inSampleSize = sample }) ?: return null

        val rotation = runCatching {
            when (ExifInterface(ByteArrayInputStream(raw)).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
                ExifInterface.ORIENTATION_ROTATE_90 -> 90f
                ExifInterface.ORIENTATION_ROTATE_180 -> 180f
                ExifInterface.ORIENTATION_ROTATE_270 -> 270f
                else -> 0f
            }
        }.getOrDefault(0f)
        val longest = max(bmp.width, bmp.height)
        val scale = if (longest > MAX_EDGE) MAX_EDGE.toFloat() / longest else 1f
        if (rotation != 0f || scale != 1f) {
            val m = Matrix()
            if (rotation != 0f) m.postRotate(rotation)
            if (scale != 1f) m.postScale(scale, scale)
            bmp = Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, m, true)
        }
        val out = ByteArrayOutputStream()
        if (bmp.hasAlpha()) bmp.compress(Bitmap.CompressFormat.PNG, 100, out) else bmp.compress(Bitmap.CompressFormat.JPEG, 88, out)
        val thumbScale = 256f / max(bmp.width, bmp.height)
        val thumb = if (thumbScale < 1f) Bitmap.createScaledBitmap(bmp, (bmp.width * thumbScale).toInt().coerceAtLeast(1), (bmp.height * thumbScale).toInt().coerceAtLeast(1), true) else bmp
        return AttachedImage(name, Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP), thumb.asImageBitmap())
    }
}

@Composable
fun rememberImageAttachState(): ImageAttachState = remember { ImageAttachState() }

/** Returns a function that opens the system photo picker and adds the chosen pictures to [state]. */
@Composable
fun rememberImagePicker(state: ImageAttachState, en: Boolean): () -> Unit {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(MAX_ATTACHED_IMAGES)) { uris ->
        if (uris.isNotEmpty()) {
            scope.launch {
                state.addUris(context, uris, en)?.let { Toast.makeText(context, it, Toast.LENGTH_SHORT).show() }
            }
        }
    }
    return { launcher.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }
}

/** The "+" button: a "添加上下文" menu with picture, file and web-link entries. */
@Composable
fun AttachImageButton(state: ImageAttachState, en: Boolean, enabled: Boolean = true) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val pickImages = rememberImagePicker(state, en)
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) {
            scope.launch {
                state.addFiles(context, uris, en)?.let { Toast.makeText(context, it, Toast.LENGTH_SHORT).show() }
            }
        }
    }
    var menu by remember { mutableStateOf(false) }
    var askLink by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { menu = true }, enabled = enabled && !state.isFull) {
            Icon(Icons.Default.Add, contentDescription = L10n.t("ctx.add", en), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, shape = RoundedCornerShape(14.dp)) {
            Text(
                L10n.t("ctx.add", en),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
            )
            DropdownMenuItem(
                text = { Text(L10n.t("ctx.image", en)) },
                leadingIcon = { Icon(Icons.Default.Image, contentDescription = null) },
                onClick = { menu = false; pickImages() },
            )
            DropdownMenuItem(
                text = { Text(L10n.t("ctx.file", en)) },
                leadingIcon = { Icon(Icons.Default.AttachFile, contentDescription = null) },
                // the core checks the format (same list as adding a source) and says what is wrong
                onClick = { menu = false; pickFiles.launch(arrayOf("*/*")) },
            )
            DropdownMenuItem(
                text = { Text(L10n.t("ctx.link", en)) },
                leadingIcon = { Icon(Icons.Default.Link, contentDescription = null) },
                onClick = { menu = false; askLink = true },
            )
        }
    }
    if (askLink) {
        var url by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { askLink = false },
            title = { Text(L10n.t("ctx.link_title", en)) },
            text = {
                OutlinedTextField(value = url, onValueChange = { url = it }, singleLine = true, placeholder = { Text("https://") })
            },
            confirmButton = {
                TextButton(onClick = {
                    val u = url.trim()
                    if (u.startsWith("http://") || u.startsWith("https://")) {
                        state.addLink(u)
                        askLink = false
                    } else {
                        Toast.makeText(context, L10n.t("ctx.link_bad", en), Toast.LENGTH_SHORT).show()
                    }
                }) { Text(L10n.t("ctx.link_add", en)) }
            },
            dismissButton = { TextButton(onClick = { askLink = false }) { Text(L10n.t("common.close", en)) } },
        )
    }
}

private fun humanSize(b: Long): String =
    if (b <= 0) "" else if (b < 1024 * 1024) "${maxOf(1, b / 1024)} KB" else String.format("%.1f MB", b / 1024.0 / 1024.0)

/** A file or link chip: format icon, name and size / address. */
@Composable
fun AttachmentChip(kind: String, name: String, detail: String, modifier: Modifier = Modifier, onClick: (() -> Unit)? = null) {
    val ext = if (kind == "link") "" else name.substringAfterLast('.', "").uppercase()
    Surface(
        shape = RoundedCornerShape(12.dp),
        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
        color = Color.Transparent,
        modifier = modifier.then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 8.dp, end = 12.dp, top = 8.dp, bottom = 8.dp)) {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier.size(36.dp).clip(RoundedCornerShape(9.dp)).background(MaterialTheme.colorScheme.surfaceVariant),
            ) {
                Icon(if (kind == "link") Icons.Default.Link else Icons.Default.Description, contentDescription = null, modifier = Modifier.size(20.dp))
            }
            Column(modifier = Modifier.padding(start = 10.dp).widthIn(max = 180.dp)) {
                Text(name, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    if (ext.isNotEmpty() && detail.isNotEmpty()) "$ext · $detail" else ext.ifEmpty { detail },
                    fontSize = 11.5.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** Opens a stored attachment: a link in the browser, a file with another app. */
fun openAttachment(context: Context, kind: String, path: String, url: String?) {
    runCatching {
        val intent = if (kind == "link") {
            Intent(Intent.ACTION_VIEW, Uri.parse(url))
        } else {
            val uri = FileProvider.getUriForFile(context, context.packageName + ".files", File(path))
            Intent(Intent.ACTION_VIEW).setDataAndType(uri, context.contentResolver.getType(uri) ?: "*/*")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        context.startActivity(Intent.createChooser(intent, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
}

/** Removable thumbnails above an input; nothing is drawn while empty. */
@Composable
fun AttachStrip(state: ImageAttachState, en: Boolean, modifier: Modifier = Modifier) {
    if (state.items.isEmpty()) return
    var preview by remember { mutableStateOf<ImageBitmap?>(null) }
    Row(
        modifier = modifier.horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (img in state.items.toList()) {
            if (img.thumb == null) {
                Box {
                    AttachmentChip(
                        img.kind,
                        img.name,
                        if (img.kind == "link") img.url.orEmpty() else humanSize(img.size),
                        modifier = Modifier.padding(top = 4.dp, end = 10.dp),
                    )
                    Box(
                        contentAlignment = Alignment.Center,
                        modifier = Modifier
                            .align(Alignment.TopEnd)
                            .size(20.dp)
                            .clip(CircleShape)
                            .background(Color(0xD9141418))
                            .clickable { state.remove(img) },
                    ) {
                        Icon(Icons.Default.Close, contentDescription = L10n.t("img.remove", en), tint = Color.White, modifier = Modifier.size(12.dp))
                    }
                }
                continue
            }
            Box(modifier = Modifier.size(64.dp)) {
                Image(
                    bitmap = img.thumb,
                    contentDescription = img.name,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .fillMaxSize()
                        .clip(RoundedCornerShape(12.dp))
                        .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(12.dp))
                        .clickable { preview = img.thumb },
                )
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .padding(3.dp)
                        .size(20.dp)
                        .clip(CircleShape)
                        .background(Color(0xD9141418))
                        .clickable { state.remove(img) },
                ) {
                    Icon(Icons.Default.Close, contentDescription = L10n.t("img.remove", en), tint = Color.White, modifier = Modifier.size(12.dp))
                }
            }
        }
    }
    preview?.let { ImagePreviewDialog(it) { preview = null } }
}

/** Full-screen viewer with pinch-zoom; a tap closes it. */
@Composable
fun ImagePreviewDialog(bitmap: ImageBitmap, onDismiss: () -> Unit) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offsetX by remember { mutableFloatStateOf(0f) }
    var offsetY by remember { mutableFloatStateOf(0f) }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier
                .fillMaxSize()
                .background(Color(0xE6000000))
                .clickable(onClick = onDismiss),
        ) {
            Image(
                bitmap = bitmap,
                contentDescription = null,
                contentScale = ContentScale.Fit,
                modifier = Modifier
                    .fillMaxSize()
                    .pointerInput(Unit) {
                        detectTransformGestures { _, pan, zoom, _ ->
                            scale = (scale * zoom).coerceIn(1f, 6f)
                            if (scale > 1f) {
                                offsetX += pan.x
                                offsetY += pan.y
                            } else {
                                offsetX = 0f
                                offsetY = 0f
                            }
                        }
                    }
                    .graphicsLayer(scaleX = scale, scaleY = scale, translationX = offsetX, translationY = offsetY),
            )
        }
    }
}

/** Thumbnails of the stored pictures of a message; tap to enlarge. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AttachmentGallery(paths: List<String>, modifier: Modifier = Modifier) {
    if (paths.isEmpty()) return
    var preview by remember { mutableStateOf<String?>(null) }
    FlowRow(
        modifier = modifier,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (p in paths) {
            val thumb = remember(p) { decodeFile(p, 360)?.asImageBitmap() }
            if (thumb != null) {
                Image(
                    bitmap = thumb,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .size(110.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(12.dp))
                        .clickable { preview = p },
                )
            }
        }
    }
    preview?.let { p ->
        val full = remember(p) { decodeFile(p, 2048)?.asImageBitmap() }
        if (full != null) ImagePreviewDialog(full) { preview = null } else preview = null
    }
}

/** Decodes a stored picture at roughly [maxEdge] pixels on its longest side. */
fun decodeFile(path: String, maxEdge: Int): Bitmap? = runCatching {
    val b = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeFile(path, b)
    var sample = 1
    while (max(b.outWidth, b.outHeight) / (sample * 2) >= maxEdge) sample *= 2
    BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = sample })
}.getOrNull()
