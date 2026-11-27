package com.localnotebook.agent.core

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.util.concurrent.atomic.AtomicLong

data class ApiEvent(
    val event: String,
    val data: JSONObject,
)

class CoreCallException(
    val code: String,
    override val message: String,
) : Exception(message)

class CoreClient(private val context: Context) {
    private var handle: Long = 0L

    /** UI language for messages raised here; kept in step with the preferences by the view model. */
    @Volatile
    var english: Boolean = UiPreferences().isEnglish
    private val reqSeq = AtomicLong(1L)
    private val _events = MutableSharedFlow<ApiEvent>(extraBufferCapacity = 64)
    val events: SharedFlow<ApiEvent> = _events

    private val encryptedPrefs: SharedPreferences? by lazy {
        runCatching {
            val masterKey = MasterKey.Builder(context)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()
            EncryptedSharedPreferences.create(
                context,
                "lna_android_secrets",
                masterKey,
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
            )
        }.getOrNull()
    }

    suspend fun open(): Boolean = withContext(Dispatchers.IO) {
        if (!NativeBridge.isLoaded) return@withContext false
        if (handle != 0L) return@withContext true
        val dataDir = File(context.filesDir, "LocalNotebook").apply { mkdirs() }
        handle = NativeBridge.nativeOpen(
            dataDir.absolutePath,
            object : CoreEventListener {
                override fun onCoreEvent(eventJson: String) {
                    runCatching {
                        val obj = JSONObject(eventJson)
                        val name = obj.optString("event", "")
                        val data = obj.optJSONObject("data") ?: JSONObject()
                        if (name.isNotEmpty()) {
                            _events.tryEmit(ApiEvent(name, data))
                        }
                    }
                }
            },
        )
        if (handle != 0L) {
            hydrateSecrets()
            true
        } else {
            false
        }
    }

    private fun hydrateSecrets() {
        val prefs = encryptedPrefs ?: return
        for ((k, v) in prefs.all) {
            val secret = v as? String ?: continue
            if (secret.isNotEmpty()) {
                val req = JSONObject()
                    .put("id", "sec_${reqSeq.getAndIncrement()}")
                    .put("method", "secret.set")
                    .put("params", JSONObject().put("keychain_ref", k).put("secret", secret))
                NativeBridge.nativeCall(handle, req.toString())
            }
        }
    }

    fun persistSecret(keyRef: String, secret: String) {
        encryptedPrefs?.edit()?.putString(keyRef, secret)?.apply()
    }

    fun clearPersistedSecret(keyRef: String) {
        encryptedPrefs?.edit()?.remove(keyRef)?.apply()
    }

    suspend fun call(method: String, params: JSONObject = JSONObject()): JSONObject =
        withContext(Dispatchers.IO) {
            if (handle == 0L) {
                throw CoreCallException(
                    "CORE_UNAVAILABLE",
                    L10n.t("core.unavailable", english),
                )
            }
            val req = JSONObject()
                .put("id", "and_${reqSeq.getAndIncrement()}")
                .put("method", method)
                .put("params", params)
            val raw = NativeBridge.nativeCall(handle, req.toString())
            val parsed = JSONObject(raw)
            val err = parsed.optJSONObject("error")
            if (err != null) {
                throw CoreCallException(
                    code = err.optString("code", "INTERNAL"),
                    message = err.optString("message", L10n.t("error.unknown", english)),
                )
            }
            parsed.optJSONObject("result") ?: JSONObject()
        }

    fun close() {
        if (handle != 0L) {
            val h = handle
            handle = 0L
            NativeBridge.nativeClose(h)
        }
    }
}
