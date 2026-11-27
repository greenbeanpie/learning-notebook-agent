package com.localnotebook.agent.ui

import android.annotation.SuppressLint
import android.content.Context
import android.media.MediaPlayer
import android.os.Bundle
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.localnotebook.agent.core.CoreClient
import com.localnotebook.agent.core.L10n
import com.localnotebook.agent.core.StudioArtifactItem
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.Locale
import kotlin.coroutines.resume

/** One spoken sentence of the lecture (display-only slides contribute an empty one). */
private data class LecSentence(
    val slide: Int,
    val slideId: String,
    val id: String,
    val text: String,
    val cues: JSONArray,
    val lastOfSlide: Boolean,
    val checkpoint: JSONObject?,
)

private data class QaEntry(
    val question: String,
    val images: List<String> = emptyList(),
    val answer: String = "",
    val speak: String = "",
    val pending: Boolean = true,
    val error: String = "",
    val suggestKind: String = "",
    val suggestReason: String = "",
)

private fun buildSequence(payload: JSONObject): List<LecSentence> {
    val out = mutableListOf<LecSentence>()
    val slides = payload.optJSONArray("slides") ?: return out
    for (si in 0 until slides.length()) {
        val s = slides.optJSONObject(si) ?: continue
        val sid = s.optString("id")
        val cp = s.optJSONObject("checkpoint")
        val narr = s.optJSONArray("narration") ?: JSONArray()
        val spoken = (0 until narr.length()).mapNotNull { narr.optJSONObject(it) }.filter { it.optString("text").isNotBlank() }
        if (spoken.isEmpty()) {
            out += LecSentence(si, sid, "$sid.0", "", JSONArray(), true, cp)
            continue
        }
        spoken.forEachIndexed { ni, n ->
            out += LecSentence(
                si, sid, n.optString("id").ifEmpty { "$sid.${ni + 1}" }, n.optString("text"),
                n.optJSONArray("cues") ?: JSONArray(), ni == spoken.lastIndex, cp,
            )
        }
    }
    return out
}

/** Same naming as the core's lecture::slide_file_id. */
private fun slideFileId(s: JSONObject, idx: Int): String {
    val id = s.optString("id").filter { it.isLetterOrDigit() && it.code < 128 || it == '-' || it == '_' || it == '.' }
    return id.ifEmpty { "slide${idx + 1}" }
}

/** Payload JSON with each slide's figure SVG inlined as figure_svg. */
private fun payloadWithFigures(payload: JSONObject, outputDir: String): JSONObject {
    val copy = JSONObject(payload.toString())
    if (outputDir.isEmpty()) return copy
    val slides = copy.optJSONArray("slides") ?: return copy
    for (i in 0 until slides.length()) {
        val s = slides.optJSONObject(i) ?: continue
        if (s.optJSONObject("figure") == null) continue
        val f = File(outputDir, "figures/${slideFileId(s, i)}.svg")
        if (f.isFile) runCatching { s.put("figure_svg", f.readText()) }
    }
    return copy
}

private fun jsStr(s: String): String = JSONObject.quote(s)

private fun deckHtml(context: Context, payload: JSONObject, en: Boolean): String {
    val tpl = context.assets.open("deck_player.html").bufferedReader().use { it.readText() }
    fun q(v: String) = JSONObject.quote(v)
    val bridge = "<script>window.chrome={webview:{postMessage:function(m){LnaBridge.post(m)}}};</script>"
    return tpl
        .replaceFirst("<head>", "<head>$bridge")
        .replace("__LANG__", L10n.t("slides.en", en))
        .replace("__LECTURE__", "true")
        .replace("__T_PLAY_JSON__", q(L10n.t("slides.play", en)))
        .replace("__T_PAUSE_JSON__", q(L10n.t("slides.pause", en)))
        .replace("__T_NONOTES_JSON__", q(""))
        .replace("__T_DECK_TITLE_JSON__", q(L10n.t("slides.default_title", en)))
        .replace("__T_SLIDE_PATTERN_JSON__", q(L10n.t("slides.slide_n", en)))
        .replace("__T_KICKER_SUMMARY_JSON__", q(L10n.t("slides.kicker_summary", en)))
        .replace("__T_KICKER_COVER_JSON__", q(L10n.t("slides.kicker_cover", en)))
        .replace("__T_EYE_AGENDA_JSON__", q(L10n.t("slides.eye_agenda", en)))
        .replace("__T_EYE_METRICS_JSON__", q(L10n.t("slides.eye_metrics", en)))
        .replace("__T_EYE_TIMELINE_JSON__", q(L10n.t("slides.eye_timeline", en)))
        .replace("__T_EYE_COMPARE_JSON__", q(L10n.t("slides.eye_compare", en)))
        .replace("__T_EYE_FORMULA_JSON__", q(L10n.t("slides.eye_formula", en)))
        .replace("__T_EYE_INSIGHT_JSON__", q(L10n.t("slides.eye_insight", en)))
        .replace(Regex("__T_[A-Z]+__"), "")
        .replace("__PAYLOAD__", payload.toString().replace("</", "<\\/"))
}

/**
 * Narrated lecture player: the shared deck page (lecture mode) in a WebView, driven sentence
 * by sentence with Android TextToSpeech or the core's cached cloud voice, plus captions,
 * checkpoints, interruption questions and adaptations of the remaining slides.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun LecturePlayerView(artifact: StudioArtifactItem, core: CoreClient?, en: Boolean) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var payload by remember(artifact.id) { mutableStateOf(artifact.payload) }
    var outputDir by remember(artifact.id) { mutableStateOf(artifact.outputDir) }
    var seq by remember(artifact.id) { mutableStateOf(buildSequence(artifact.payload)) }
    var pos by remember(artifact.id) {
        val saved = artifact.progressState.optString("position")
        mutableIntStateOf(buildSequence(artifact.payload).indexOfFirst { it.id == saved }.coerceAtLeast(0))
    }
    var playing by remember { mutableStateOf(false) }
    var captions by remember { mutableStateOf(true) }
    var rate by remember { mutableFloatStateOf(1f) }
    var engine by remember { mutableStateOf("system") }
    var policy by remember { mutableStateOf("auto") }
    var webReady by remember { mutableStateOf(false) }
    var checkpoint by remember { mutableStateOf<LecSentence?>(null) }
    var showAsk by remember { mutableStateOf(false) }
    var question by remember { mutableStateOf("") }
    val askImages = rememberImageAttachState()
    val qa = remember { mutableStateListOf<QaEntry>() }
    var adapting by remember { mutableStateOf(false) }
    var adaptation by remember { mutableStateOf<JSONObject?>(null) }
    var showAdaptDetails by remember { mutableStateOf(false) }
    var playJob by remember { mutableStateOf<Job?>(null) }
    var status by remember { mutableStateOf("") }

    val webView = remember(artifact.id) { WebView(context) }
    val tts = remember {
        var ref: TextToSpeech? = null
        ref = TextToSpeech(context) { ok ->
            if (ok == TextToSpeech.SUCCESS) ref?.language = if (en) Locale.ENGLISH else Locale.SIMPLIFIED_CHINESE
        }
        ref
    }
    val media = remember { arrayOfNulls<MediaPlayer>(1) }
    val cloudCache = remember { mutableMapOf<String, Deferred<String?>>() }

    fun js(script: String) {
        if (webReady) webView.post { webView.evaluateJavascript(script, null) }
    }

    fun stepBefore(p: Int): Int {
        if (p !in seq.indices) return 0
        var step = 0
        var i = p - 1
        while (i >= 0 && seq[i].slide == seq[p].slide) {
            val c = seq[i].cues
            for (k in 0 until c.length()) {
                val cue = c.optJSONObject(k) ?: continue
                if (cue.optString("type") == "step") step = maxOf(step, cue.optInt("n"))
            }
            i--
        }
        return step
    }

    fun showPosition() {
        if (seq.isEmpty()) return
        val s = seq[pos]
        js("lecture.show(${s.slide},${stepBefore(pos)})")
        js("lecture.caption(${if (captions) jsStr(s.text) else "''"})")
    }

    fun saveProgress() {
        val c = core ?: return
        if (seq.isEmpty()) return
        val id = seq[pos].id
        scope.launch {
            runCatching {
                c.call("studio.save_progress", JSONObject().put("artifact_id", artifact.id).put("state", JSONObject().put("position", id)))
            }
        }
    }

    fun cloudAudio(text: String): Deferred<String?> = cloudCache.getOrPut(text) {
        scope.async(Dispatchers.IO) {
            runCatching { core?.call("tts.synthesize", JSONObject().put("text", text))?.optString("path")?.ifEmpty { null } }.getOrNull()
        }
    }

    suspend fun speakSystem(text: String): Boolean = suspendCancellableCoroutine { cont ->
        val t = tts ?: run { cont.resume(true); return@suspendCancellableCoroutine }
        val uid = "u" + System.nanoTime()
        t.setSpeechRate(rate)
        t.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String?) {}
            override fun onDone(utteranceId: String?) { if (utteranceId == uid && cont.isActive) cont.resume(true) }
            @Deprecated("Deprecated in Java")
            override fun onError(utteranceId: String?) { if (utteranceId == uid && cont.isActive) cont.resume(true) }
            override fun onStop(utteranceId: String?, interrupted: Boolean) { if (utteranceId == uid && cont.isActive) cont.resume(false) }
        })
        t.speak(text, TextToSpeech.QUEUE_FLUSH, Bundle(), uid)
        cont.invokeOnCancellation { t.stop() }
    }

    suspend fun speakFile(path: String): Boolean = suspendCancellableCoroutine { cont ->
        media[0]?.release()
        val mp = MediaPlayer()
        media[0] = mp
        runCatching {
            mp.setDataSource(path)
            mp.prepare()
            mp.playbackParams = mp.playbackParams.setSpeed(rate)
            mp.setOnCompletionListener { if (cont.isActive) cont.resume(true) }
            mp.start()
        }.onFailure { if (cont.isActive) cont.resume(true) }
        cont.invokeOnCancellation { runCatching { mp.stop() } }
    }

    suspend fun speak(text: String): Boolean {
        if (text.isBlank()) { delay(3000); return true }
        if (engine == "cloud") {
            val path = cloudAudio(text).await()
            if (path != null) return speakFile(path)
        }
        return speakSystem(text)
    }

    fun stopAudio() {
        tts?.stop()
        runCatching { media[0]?.pause() }
    }

    fun pause() {
        playJob?.cancel()
        playJob = null
        playing = false
        stopAudio()
        js("lecture.stopLaser()")
        saveProgress()
    }

    fun play() {
        if (seq.isEmpty()) return
        playJob?.cancel()
        checkpoint = null
        playing = true
        var shown = -1
        playJob = scope.launch {
            while (isActive && pos < seq.size) {
                val s = seq[pos]
                if (s.slide != shown) {
                    js("lecture.show(${s.slide},${stepBefore(pos)})")
                    shown = s.slide
                    saveProgress()
                }
                val lasers = mutableListOf<String>()
                for (k in 0 until s.cues.length()) {
                    val c = s.cues.optJSONObject(k) ?: continue
                    when (c.optString("type")) {
                        "step" -> js("lecture.step(${c.optInt("n")})")
                        "laser" -> lasers += c.toString()
                    }
                }
                js("lecture.caption(${if (captions) jsStr(s.text) else "''"})")
                if (engine == "cloud") {
                    seq.getOrNull(pos + 1)?.text?.takeIf { it.isNotBlank() }?.let { cloudAudio(it) }
                    seq.getOrNull(pos + 2)?.text?.takeIf { it.isNotBlank() }?.let { cloudAudio(it) }
                }
                // the page times each pointer cue by where its words fall in the sentence
                if (lasers.isNotEmpty()) js("lecture.lasers([${lasers.joinToString(",")}],${jsStr(s.text)},$rate)")
                val ok = speak(s.text)
                if (!ok || !isActive) return@launch
                if (s.lastOfSlide && s.checkpoint != null) {
                    playing = false
                    checkpoint = s
                    showAsk = true
                    return@launch
                }
                if (pos >= seq.size - 1) {
                    playing = false
                    js("lecture.caption(${jsStr(L10n.t("lec.ended", en))})")
                    saveProgress()
                    return@launch
                }
                pos++
            }
        }
    }

    fun jumpTo(newPos: Int) {
        val was = playing
        pause()
        pos = newPos.coerceIn(0, (seq.size - 1).coerceAtLeast(0))
        showPosition()
        if (was) play()
    }

    fun applyArtifact(art: JSONObject) {
        val p = art.optJSONObject("payload") ?: return
        val curId = seq.getOrNull(pos)?.id
        val curSlide = seq.getOrNull(pos)?.slideId
        payload = p
        art.optString("output_dir").takeIf { it.isNotEmpty() }?.let { outputDir = it }
        seq = buildSequence(p)
        pos = seq.indexOfFirst { it.id == curId }.takeIf { it >= 0 } ?: seq.indexOfFirst { it.slideId == curSlide }.coerceAtLeast(0)
        val slides = payloadWithFigures(p, outputDir).optJSONArray("slides") ?: JSONArray()
        if (seq.isNotEmpty()) js("lecture.setSlides($slides,${seq[pos].slide},${stepBefore(pos)})")
    }

    fun adapt(slideId: String, kind: String, reason: String, q: String) {
        val c = core ?: return
        adapting = true
        scope.launch {
            runCatching {
                c.call(
                    "lecture.adapt",
                    JSONObject().put("artifact_id", artifact.id).put("slide_id", slideId).put("kind", kind).put("reason", reason).put("question", q),
                )
            }.onSuccess { r ->
                r.optJSONObject("artifact")?.let { applyArtifact(it) }
                adaptation = r.optJSONObject("adaptation")
            }.onFailure { status = L10n.t("lec.adapt_failed", en) + ": " + (it.message ?: "") }
            adapting = false
        }
    }

    fun undo(all: Boolean) {
        val c = core ?: return
        scope.launch {
            runCatching {
                val p = JSONObject().put("artifact_id", artifact.id).put("all", all)
                adaptation?.optString("id")?.takeIf { !all && it.isNotEmpty() }?.let { p.put("adaptation_id", it) }
                c.call("lecture.undo", p)
            }.onSuccess { r ->
                r.optJSONObject("artifact")?.let { applyArtifact(it) }
                adaptation = null
            }.onFailure { status = it.message ?: "" }
        }
    }

    fun ask(q: String, allowAdapt: Boolean, images: JSONArray? = null) {
        val c = core ?: return
        if (seq.isEmpty() || q.isBlank()) return
        if (playing) pause()
        val s = seq[pos]
        val idx = qa.size
        qa += QaEntry(question = q)
        val hist = JSONArray()
        qa.takeLast(4).filter { !it.pending }.forEach {
            hist.put(JSONObject().put("role", "user").put("text", it.question))
            hist.put(JSONObject().put("role", "assistant").put("text", it.answer))
        }
        scope.launch {
            runCatching {
                val params = JSONObject().put("artifact_id", artifact.id).put("slide_id", s.slideId).put("sentence_id", s.id).put("question", q).put("history", hist)
                params.putAttachments(images)
                c.call("lecture.ask", params)
            }.onSuccess { r ->
                val ad = r.optJSONObject("adapt") ?: JSONObject()
                val needed = ad.optBoolean("needed") && allowAdapt && policy != "never"
                val stored = r.optJSONArray("attachments") ?: JSONArray()
                qa[idx] = qa[idx].copy(
                    images = (0 until stored.length()).mapNotNull { stored.optJSONObject(it)?.optString("path")?.takeIf { p -> p.isNotEmpty() } },
                    answer = r.optString("answer"), speak = r.optString("speak"), pending = false,
                    suggestKind = if (needed && policy == "ask") ad.optString("kind") else "",
                    suggestReason = ad.optString("reason"),
                )
                if (needed && policy == "auto") adapt(s.slideId, ad.optString("kind"), ad.optString("reason"), q)
            }.onFailure { qa[idx] = qa[idx].copy(pending = false, error = it.message ?: "") }
        }
    }

    // settings + web page
    LaunchedEffect(artifact.id) {
        core?.let { c ->
            runCatching { c.call("tts.get_settings") }.getOrNull()?.let { engine = if (it.optString("engine") == "cloud") "cloud" else "system" }
            runCatching { c.call("settings.get", JSONObject().put("key", "lecture.prefs")) }.getOrNull()
                ?.optJSONObject("value")?.optString("adapt_policy")?.takeIf { it.isNotEmpty() }?.let { policy = it }
        }
        val html = withContext(Dispatchers.IO) { deckHtml(context, payloadWithFigures(payload, outputDir), en) }
        webView.settings.javaScriptEnabled = true
        webView.settings.allowFileAccess = true
        webView.addJavascriptInterface(object {
            @JavascriptInterface
            fun post(msg: String) {
                val type = runCatching { JSONObject(msg).optString("type") }.getOrDefault("")
                if (type == "ready") webView.post { webReady = true; showPosition() }
            }
        }, "LnaBridge")
        webView.webViewClient = WebViewClient()
        webView.loadDataWithBaseURL("file:///android_asset/", html, "text/html", "utf-8", null)
    }
    DisposableEffect(artifact.id) {
        onDispose {
            playJob?.cancel()
            saveProgress()
            tts?.stop()
            tts?.shutdown()
            media[0]?.release()
            webView.destroy()
        }
    }

    Column(modifier = Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        AndroidView(
            factory = { webView },
            modifier = Modifier.fillMaxWidth().weight(if (showAsk) 0.9f else 1f),
        )
        // controls
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(2.dp), modifier = Modifier.fillMaxWidth()) {
            fun slideStart(delta: Int): Int {
                if (seq.isEmpty()) return 0
                val target = (seq[pos].slide + delta).coerceIn(0, seq.last().slide)
                return seq.indexOfFirst { it.slide == target }.coerceAtLeast(0)
            }
            IconButton(onClick = { jumpTo(slideStart(-1)) }) { Icon(Icons.Default.SkipPrevious, L10n.t("lec.prev_slide", en)) }
            IconButton(onClick = { jumpTo(pos - 1) }) { Icon(Icons.Default.FastRewind, L10n.t("lec.prev_sentence", en)) }
            FilledIconButton(onClick = { if (playing) pause() else play() }) {
                Icon(if (playing) Icons.Default.Pause else Icons.Default.PlayArrow, L10n.t(if (playing) "lec.pause" else "lec.play", en))
            }
            IconButton(onClick = { jumpTo(pos + 1) }) { Icon(Icons.Default.FastForward, L10n.t("lec.next_sentence", en)) }
            IconButton(onClick = { jumpTo(slideStart(1)) }) { Icon(Icons.Default.SkipNext, L10n.t("lec.next_slide", en)) }
            val total = payload.optJSONArray("slides")?.length() ?: 0
            Text("${(seq.getOrNull(pos)?.slide ?: 0) + 1}/$total", fontSize = 12.sp, modifier = Modifier.padding(horizontal = 4.dp))
            Spacer(Modifier.weight(1f))
            TextButton(onClick = {
                rate = when (rate) { 1f -> 1.25f; 1.25f -> 1.5f; 1.5f -> 2f; 2f -> 0.75f; else -> 1f }
            }) { Text("${rate}×", fontSize = 12.sp) }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.fillMaxWidth()) {
            FilterChip(selected = captions, onClick = {
                captions = !captions
                seq.getOrNull(pos)?.let { js("lecture.caption(${if (captions) jsStr(it.text) else "''"})") }
            }, label = { Text(L10n.t("lec.captions", en)) })
            Button(onClick = { pause(); showAsk = true }) { Text(L10n.t("lec.ask", en)) }
            var policyMenu by remember { mutableStateOf(false) }
            Box {
                TextButton(onClick = { policyMenu = true }) { Text(L10n.t("lec.adapt_policy", en), fontSize = 12.sp) }
                DropdownMenu(expanded = policyMenu, onDismissRequest = { policyMenu = false }) {
                    listOf("auto" to "lec.policy_auto", "ask" to "lec.policy_ask", "never" to "lec.policy_never").forEach { (k, l) ->
                        DropdownMenuItem(
                            text = { Text((if (policy == k) "✓ " else "") + L10n.t(l, en)) },
                            onClick = {
                                policy = k
                                policyMenu = false
                                scope.launch {
                                    runCatching {
                                        core?.call("settings.set", JSONObject().put("key", "lecture.prefs").put("value", JSONObject().put("adapt_policy", k)))
                                    }
                                }
                            },
                        )
                    }
                }
            }
            if (adapting) {
                CircularProgressIndicator(modifier = Modifier.size(16.dp), strokeWidth = 2.dp)
                Text(L10n.t("lec.adapting", en), fontSize = 11.sp, maxLines = 1)
            } else if (adaptation != null) {
                AssistChip(onClick = { showAdaptDetails = true }, label = { Text(L10n.t("lec.adapted", en), fontSize = 12.sp) })
            }
        }
        if (status.isNotEmpty()) Text(status, fontSize = 11.sp, color = MaterialTheme.colorScheme.error)

        if (showAsk) {
            Surface(shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.fillMaxWidth().weight(1f)) {
                Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(L10n.t("lec.qa_title", en), fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        IconButton(onClick = { showAsk = false }) { Icon(Icons.Default.Close, null) }
                    }
                    checkpoint?.let { cp ->
                        val c = cp.checkpoint ?: JSONObject()
                        val attempt = c.optString("kind") == "attempt"
                        var ans by remember(cp.id) { mutableStateOf("") }
                        Text(L10n.t(if (attempt) "lec.attempt" else "lec.checkpoint", en), fontWeight = FontWeight.SemiBold)
                        Text(c.optString("question"))
                        if (attempt) {
                            OutlinedTextField(value = ans, onValueChange = { ans = it }, label = { Text(L10n.t("lec.your_answer", en)) }, modifier = Modifier.fillMaxWidth())
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (attempt) Button(onClick = {
                                ask(L10n.t("lec.attempt_answer_fmt", en).replace("{0}", c.optString("question")).replace("{1}", ans), false)
                            }) { Text(L10n.t("lec.submit", en)) }
                            OutlinedButton(onClick = {
                                checkpoint = null
                                if (pos < seq.size - 1) pos++
                                play()
                            }) { Text(L10n.t("lec.continue", en)) }
                        }
                    }
                    val listState = rememberLazyListState()
                    LaunchedEffect(qa.size) { if (qa.isNotEmpty()) listState.animateScrollToItem(qa.size - 1) }
                    LazyColumn(state = listState, modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        items(qa.indices.toList()) { i ->
                            val e = qa[i]
                            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                if (e.images.isNotEmpty()) AttachmentGallery(e.images)
                                Text(e.question, fontWeight = FontWeight.Medium)
                                when {
                                    e.pending -> Row(verticalAlignment = Alignment.CenterVertically) {
                                        CircularProgressIndicator(modifier = Modifier.size(14.dp), strokeWidth = 2.dp)
                                        Spacer(Modifier.width(6.dp))
                                        Text(L10n.t("lec.thinking", en), fontSize = 12.sp)
                                    }
                                    e.error.isNotEmpty() -> Text(e.error, color = MaterialTheme.colorScheme.error, fontSize = 12.sp)
                                    else -> {
                                        Text(e.answer, fontSize = 14.sp)
                                        TextButton(onClick = {
                                            pause()
                                            playJob = scope.launch { speak(e.speak.ifEmpty { e.answer }) }
                                        }) { Text(L10n.t("lec.speak_answer", en), fontSize = 12.sp) }
                                        if (e.suggestKind.isNotEmpty()) {
                                            Text("${L10n.t("lec.adapt_suggest", en)}${L10n.t("studio.colon", en)}${L10n.t("lec.kind_${e.suggestKind}", en)}", fontWeight = FontWeight.SemiBold, fontSize = 13.sp)
                                            if (e.suggestReason.isNotEmpty()) Text(e.suggestReason, fontSize = 12.sp)
                                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                                Button(onClick = {
                                                    qa[i] = e.copy(suggestKind = "")
                                                    adapt(seq[pos].slideId, e.suggestKind, e.suggestReason, e.question)
                                                }) { Text(L10n.t("lec.adapt_apply", en)) }
                                                OutlinedButton(onClick = { qa[i] = e.copy(suggestKind = "") }) { Text(L10n.t("lec.adapt_skip", en)) }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    AttachStrip(askImages, en)
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                        AttachImageButton(askImages, en)
                        OutlinedTextField(
                            value = question,
                            onValueChange = { question = it },
                            placeholder = { Text(L10n.t("lec.ask_placeholder", en), fontSize = 13.sp) },
                            modifier = Modifier.weight(1f),
                            maxLines = 3,
                        )
                        Button(onClick = {
                            val q = question.trim().ifEmpty { L10n.t("img.ask_default", en) }
                            val imgs = askImages.toJson()
                            question = ""
                            askImages.clear()
                            ask(q, true, imgs)
                        }, enabled = question.isNotBlank() || askImages.hasItems) { Text(L10n.t("lec.send", en)) }
                    }
                }
            }
        }
    }

    if (showAdaptDetails) {
        val a = adaptation
        AlertDialog(
            onDismissRequest = { showAdaptDetails = false },
            title = { Text(L10n.t("lec.kind_${a?.optString("kind") ?: ""}", en)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    a?.optString("reason")?.takeIf { it.isNotEmpty() }?.let { Text("${L10n.t("lec.reason", en)}${L10n.t("studio.colon", en)}$it") }
                    val ins = a?.optJSONArray("inserted") ?: JSONArray()
                    val insT = a?.optJSONArray("inserted_titles") ?: JSONArray()
                    if (ins.length() > 0) Text(L10n.t("lec.inserted", en), fontWeight = FontWeight.SemiBold)
                    for (k in 0 until ins.length()) {
                        val id = ins.optString(k)
                        TextButton(onClick = {
                            showAdaptDetails = false
                            seq.indexOfFirst { it.slideId == id }.takeIf { it >= 0 }?.let { jumpTo(it) }
                        }) { Text(insT.optString(k, id)) }
                    }
                    val rem = a?.optJSONArray("removed_titles") ?: JSONArray()
                    if (rem.length() > 0) {
                        Text(L10n.t("lec.replaced", en), fontWeight = FontWeight.SemiBold)
                        Text((0 until rem.length()).joinToString(L10n.t("studio.list_sep", en)) { rem.optString(it) })
                    }
                }
            },
            confirmButton = { TextButton(onClick = { showAdaptDetails = false; undo(false) }) { Text(L10n.t("lec.undo", en)) } },
            dismissButton = { TextButton(onClick = { showAdaptDetails = false; undo(true) }) { Text(L10n.t("lec.restore_original", en)) } },
        )
    }
}
