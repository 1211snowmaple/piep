package io.github.u1211snowmaple.piep.android

import android.app.Activity
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private const val KEYSTORE = "AndroidKeyStore"
private const val KEY_ALIAS = "piep-connection"
private const val TRANSFORMATION = "AES/GCM/NoPadding"
private const val NONCE_BYTES = 12
private const val TAG_BITS = 128

@InvokeArg
class SecretCipherArgs {
  lateinit var data: String
}

/**
 * サーバーの認証鍵を、端末の外へ出せない鍵で封じる。
 *
 * 開くたびに指紋などを求めると読むのが面倒になるので、利用者の認証は条件にしない。
 * 端末の画面ロックとアプリ専用領域に加えて、ファイルだけを持ち出しても読めないようにする。
 */
@TauriPlugin
class SecretCipherPlugin(activity: Activity) : Plugin(activity) {
  @Command
  fun seal(invoke: Invoke) = respond(invoke) { plain ->
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.ENCRYPT_MODE, key())
    cipher.iv + cipher.doFinal(plain)
  }

  @Command
  fun open(invoke: Invoke) = respond(invoke) { sealed ->
    require(sealed.size > NONCE_BYTES)
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.DECRYPT_MODE, existingKey(), GCMParameterSpec(TAG_BITS, sealed, 0, NONCE_BYTES))
    cipher.doFinal(sealed, NONCE_BYTES, sealed.size - NONCE_BYTES)
  }

  private fun respond(invoke: Invoke, transform: (ByteArray) -> ByteArray) {
    try {
      val input = Base64.decode(invoke.parseArgs(SecretCipherArgs::class.java).data, Base64.NO_WRAP)
      val reply = JSObject()
      reply.put("data", Base64.encodeToString(transform(input), Base64.NO_WRAP))
      invoke.resolve(reply)
    } catch (_: Exception) {
      // 例外の文面に中身が混ざらないよう、理由は返さない。
      invoke.reject("secret cipher failed")
    }
  }

  private fun existingKey(): SecretKey {
    val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
    return store.getKey(KEY_ALIAS, null) as SecretKey
  }

  private fun key(): SecretKey {
    val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
    (store.getKey(KEY_ALIAS, null) as SecretKey?)?.let { return it }
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
    generator.init(
      KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .build(),
    )
    return generator.generateKey()
  }
}
