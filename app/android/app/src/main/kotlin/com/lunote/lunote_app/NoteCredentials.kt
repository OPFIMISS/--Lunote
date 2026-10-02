package com.lunote.lunote_app

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.json.JSONObject
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Passwords never enter preferences unencrypted; access requires local system authentication. */
class NoteCredentials(private val activity: Activity) {
    private val preferences = activity.getSharedPreferences("note_credentials", Context.MODE_PRIVATE)
    private val alias = "lunote.notes.credentials.v1"
    private val requestCode = 4310
    private var pending: MethodChannel.Result? = null
    private var operation: (() -> String?)? = null
    private var cancellation: CancellationSignal? = null
    private var credentialFallback = false

    fun handle(call: MethodCall, result: MethodChannel.Result) {
        val id = call.argument<String>("noteId").orEmpty()
        val tag = call.argument<String>("tag").orEmpty()
        if (id.isEmpty() || id.length > 128 || tag.length > 512) {
            result.error("invalid_note", "笔记认证参数非法", null)
            return
        }
        val name = digest(id)
        when (call.method) {
            "noteCredentialAvailable" -> {
                val stored = preferences.getString(name, null)
                val matches = try { stored != null && JSONObject(stored).optString("tag") == tag } catch (_: Exception) { false }
                result.success(matches)
            }
            "forgetNoteCredential" -> {
                preferences.edit().remove(name).commit()
                result.success(true)
            }
            "rememberNoteCredential" -> {
                val password = call.argument<String>("password").orEmpty()
                if (password.isEmpty() || password.length > 1024) { result.error("invalid_password", "密码非法", null); return }
                authenticate(result) {
                    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                    cipher.init(Cipher.ENCRYPT_MODE, key())
                    cipher.updateAAD("$id\n$tag".toByteArray(Charsets.UTF_8))
                    val encrypted = cipher.doFinal(password.toByteArray(Charsets.UTF_8))
                    val stored = JSONObject().put("tag", tag)
                        .put("iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
                        .put("ciphertext", Base64.encodeToString(encrypted, Base64.NO_WRAP))
                    if (!preferences.edit().putString(name, stored.toString()).commit()) throw IllegalStateException("保存失败")
                    "remembered"
                }
            }
            "unlockNoteCredential" -> {
                val stored = preferences.getString(name, null)
                if (stored == null) { result.success(null); return }
                authenticate(result) {
                    val record = JSONObject(stored)
                    if (record.getString("tag") != tag) throw IllegalStateException("笔记密码已更新，请重新输入密码")
                    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                    cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(record.getString("iv"), Base64.NO_WRAP)))
                    cipher.updateAAD("$id\n$tag".toByteArray(Charsets.UTF_8))
                    String(cipher.doFinal(Base64.decode(record.getString("ciphertext"), Base64.NO_WRAP)), Charsets.UTF_8)
                }
            }
            else -> result.notImplemented()
        }
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        val builder = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setUserAuthenticationRequired(true)
        if (Build.VERSION.SDK_INT >= 30) {
            builder.setUserAuthenticationParameters(30, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL)
        } else {
            @Suppress("DEPRECATION")
            builder.setUserAuthenticationValidityDurationSeconds(30)
        }
        if (Build.VERSION.SDK_INT >= 24) builder.setInvalidatedByBiometricEnrollment(false)
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(builder.build()); generateKey()
        }
    }

    private fun authenticate(result: MethodChannel.Result, action: () -> String?) {
        if (pending != null) { result.error("auth_busy", "已有认证正在进行", null); return }
        val keyguard = activity.getSystemService(Context.KEYGUARD_SERVICE) as KeyguardManager
        if (!keyguard.isDeviceSecure) { result.error("no_screen_lock", "请先设置系统锁屏密码", null); return }
        try { key() } catch (error: Exception) { result.error("keystore", "系统安全密钥不可用：${error.message}", null); return }
        pending = result; operation = action; credentialFallback = false
        if (Build.VERSION.SDK_INT < 28) { openCredential(); return }
        cancellation = CancellationSignal()
        val builder = BiometricPrompt.Builder(activity).setTitle("解锁月笺笔记")
        if (Build.VERSION.SDK_INT >= 30) {
            builder.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL)
        } else {
            builder.setNegativeButton("锁屏密码", activity.mainExecutor) { _, _ -> openCredential() }
        }
        builder.build().authenticate(cancellation!!, activity.mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) { complete(true) }
            override fun onAuthenticationError(code: Int, message: CharSequence) {
                if (credentialFallback) return
                if (code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED || code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED) complete(false)
                else openCredential()
            }
        })
    }

    private fun openCredential() {
        if (pending == null || credentialFallback) return
        credentialFallback = true
        val manager = activity.getSystemService(Context.KEYGUARD_SERVICE) as KeyguardManager
        @Suppress("DEPRECATION")
        val intent = manager.createConfirmDeviceCredentialIntent("解锁月笺笔记", "验证此设备的锁屏密码")
        if (intent == null) { complete(false); return }
        try { activity.startActivityForResult(intent, requestCode) } catch (_: Exception) { complete(false) }
    }

    fun onActivityResult(code: Int, result: Int, data: Intent?): Boolean {
        if (code != requestCode) return false
        complete(result == Activity.RESULT_OK)
        return true
    }

    private fun complete(authenticated: Boolean) {
        val result = pending ?: return
        val action = operation
        pending = null; operation = null; cancellation = null; credentialFallback = false
        if (!authenticated) { result.success(null); return }
        try { result.success(action?.invoke()) } catch (error: Exception) {
            result.error("note_auth_failed", "本机解锁失败，请使用笔记密码：${error.message}", null)
        }
    }

    fun dispose() { cancellation?.cancel(); complete(false) }
    private fun digest(value: String) = MessageDigest.getInstance("SHA-256").digest(value.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
}
