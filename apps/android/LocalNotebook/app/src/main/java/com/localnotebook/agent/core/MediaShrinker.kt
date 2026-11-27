package com.localnotebook.agent.core

import android.content.Context
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.media3.common.Effect
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.effect.Presentation
import androidx.media3.transformer.AudioEncoderSettings
import androidx.media3.transformer.Composition
import androidx.media3.transformer.DefaultEncoderFactory
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.Effects
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.Transformer
import androidx.media3.transformer.VideoEncoderSettings
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * On-device "trade quality for capacity" for media the core cannot downscale (Android has no ffmpeg).
 * Re-encodes with Media3 Transformer, stepping down until the output fits the limit.
 */
object MediaShrinker {
    /** MiMo takes audio/video as Base64 inline, at most 50 MB encoded, i.e. about 35 MB raw. */
    const val MIMO_LIMIT_BYTES: Long = 35L * 1024 * 1024

    private const val VIDEO_AUDIO_BPS = 64_000
    private const val MIN_VIDEO_BPS = 100_000

    /** Target height to video bitrate cap, tried in order. */
    private val VIDEO_STEPS = listOf(480 to 1_000_000, 360 to 600_000, 240 to 300_000)
    private val AUDIO_BPS_STEPS = listOf(64_000, 32_000, 16_000)

    // Mirrors lna-ingest formats.rs so the Kotlin side classifies files like the core does.
    private val AUDIO_EXTENSIONS = setOf(
        "aac", "aif", "aifc", "aiff", "amr", "au", "m4a", "mp3", "ogg", "opus", "ra", "snd", "wav", "wma",
    )
    private val VIDEO_EXTENSIONS = setOf("3g2", "3gp", "avi", "mp4", "mpeg")

    /** true for video, false for audio, null when the name is neither. */
    fun mediaKind(fileName: String): Boolean? {
        val ext = fileName.substringAfterLast('.', "").lowercase()
        return when (ext) {
            in VIDEO_EXTENSIONS -> true
            in AUDIO_EXTENSIONS -> false
            else -> null
        }
    }

    /**
     * Returns a re-encoded copy in cacheDir when [source] is larger than [limitBytes], or null when it
     * already fits. Each failed attempt's temp output is deleted. Throws [IllegalStateException] with a
     * localized message when even the smallest setting is still over the limit.
     */
    suspend fun shrinkIfNeeded(
        context: Context,
        source: File,
        isVideo: Boolean,
        limitBytes: Long,
        en: Boolean,
    ): File? {
        if (source.length() <= limitBytes) return null
        val probe = probe(source)

        if (isVideo) {
            // A bitrate budget from the duration keeps long clips from needing several full re-encodes.
            val budgetBps: Long? = probe.durationMs?.takeIf { it > 0 }?.let { limitBytes * 8_000L * 9 / 10 / it }
            for ((height, capBps) in VIDEO_STEPS) {
                val videoBps = budgetBps
                    ?.let { minOf(capBps.toLong(), maxOf(it - VIDEO_AUDIO_BPS, MIN_VIDEO_BPS.toLong())).toInt() }
                    ?: capBps
                // Never upscale: skip the scale when the source is already at or below the target height.
                val scaleHeight = height.takeIf { probe.height == null || it < probe.height }
                val out = newOutput(context, "mp4")
                try {
                    export(context, source, out, isVideo = true, scaleHeight, videoBps, VIDEO_AUDIO_BPS)
                } catch (e: Throwable) {
                    out.delete()
                    throw e
                }
                if (out.length() <= limitBytes) return out
                out.delete()
            }
        } else {
            for (audioBps in AUDIO_BPS_STEPS) {
                val out = newOutput(context, "m4a")
                try {
                    export(context, source, out, isVideo = false, null, 0, audioBps)
                } catch (e: Throwable) {
                    out.delete()
                    throw e
                }
                if (out.length() <= limitBytes) return out
                out.delete()
            }
        }
        throw IllegalStateException(L10n.t("model.shrink_failed", en))
    }

    private suspend fun export(
        context: Context,
        source: File,
        out: File,
        isVideo: Boolean,
        scaleHeight: Int?,
        videoBps: Int,
        audioBps: Int,
    ) {
        // Transformer needs a thread with a Looper, and start/cancel must run on it.
        withContext(Dispatchers.Main) {
            suspendCancellableCoroutine<Unit> { cont ->
                val encoderFactory = DefaultEncoderFactory.Builder(context)
                    .setRequestedAudioEncoderSettings(AudioEncoderSettings.Builder().setBitrate(audioBps).build())
                    .apply {
                        if (isVideo) {
                            setRequestedVideoEncoderSettings(
                                VideoEncoderSettings.Builder().setBitrate(videoBps).build(),
                            )
                        }
                    }
                    .build()

                val transformer = Transformer.Builder(context)
                    .setAudioMimeType(MimeTypes.AUDIO_AAC)
                    .apply { if (isVideo) setVideoMimeType(MimeTypes.VIDEO_H264) }
                    .setEncoderFactory(encoderFactory)
                    .addListener(object : Transformer.Listener {
                        override fun onCompleted(composition: Composition, exportResult: ExportResult) {
                            if (cont.isActive) cont.resume(Unit)
                        }

                        override fun onError(
                            composition: Composition,
                            exportResult: ExportResult,
                            exportException: ExportException,
                        ) {
                            if (cont.isActive) {
                                cont.resumeWithException(
                                    IllegalStateException("${exportException.errorCodeName}: ${exportException.message}", exportException),
                                )
                            }
                        }
                    })
                    .build()

                cont.invokeOnCancellation {
                    Handler(Looper.getMainLooper()).post { transformer.cancel() }
                }

                val builder = EditedMediaItem.Builder(MediaItem.fromUri(Uri.fromFile(source)))
                if (isVideo) {
                    val videoEffects: List<Effect> =
                        if (scaleHeight != null) listOf(Presentation.createForHeight(scaleHeight)) else emptyList()
                    builder.setEffects(Effects(emptyList<AudioProcessor>(), videoEffects))
                } else {
                    builder.setRemoveVideo(true)
                }
                transformer.start(builder.build(), out.absolutePath)
            }
        }
    }

    private data class Probe(val height: Int?, val durationMs: Long?)

    private fun probe(file: File): Probe {
        val mmr = MediaMetadataRetriever()
        val result = runCatching {
            mmr.setDataSource(file.absolutePath)
            Probe(
                height = mmr.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)?.toIntOrNull(),
                durationMs = mmr.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull(),
            )
        }.getOrElse { Probe(null, null) }
        mmr.release()
        return result
    }

    private fun newOutput(context: Context, ext: String): File =
        File(context.cacheDir, "shrink_${System.nanoTime()}.$ext")
}
