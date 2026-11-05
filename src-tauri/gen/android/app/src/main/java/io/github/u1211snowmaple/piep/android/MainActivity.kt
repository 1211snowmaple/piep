package io.github.u1211snowmaple.piep.android

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.KeyEvent
import android.view.WindowManager
import android.webkit.WebView
import android.webkit.JavascriptInterface
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import java.io.File

class MainActivity : TauriActivity() {
  override val handleBackNavigation: Boolean = false
  @Volatile private var imeBottomInsetPx = 0
  @Volatile private var systemTopInsetPx = 0
  @Volatile private var systemBottomInsetPx = 0
  @Volatile private var volumeKeysTurnPages = false
  private var readerWebView: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(object {
      @JavascriptInterface
      fun setDark(dark: Boolean) {
        runOnUiThread {
          val controller = WindowInsetsControllerCompat(window, window.decorView)
          controller.isAppearanceLightStatusBars = !dark
          controller.isAppearanceLightNavigationBars = !dark
        }
      }
      @JavascriptInterface
      fun imeInsetPx(): Int = imeBottomInsetPx
      @JavascriptInterface
      fun systemTopInsetPx(): Int = systemTopInsetPx
      @JavascriptInterface
      fun systemBottomInsetPx(): Int = systemBottomInsetPx
    }, "piepAndroidAppearance")
    // 開けるのは、検証を通ってキャッシュに置かれた APK だけ。OS が毎回確認し、署名を検査する。
    webView.addJavascriptInterface(object {
      @JavascriptInterface
      fun installDownloadedUpdate(): String {
        val apk = File(cacheDir, "updates/update.apk")
        if (!apk.isFile) return "missing"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && !packageManager.canRequestPackageInstalls()) {
          startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName")))
          return "needs_permission"
        }
        val uri = FileProvider.getUriForFile(this@MainActivity, "$packageName.fileprovider", apk)
        startActivity(
          Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        )
        return "started"
      }
    }, "piepAndroidUpdate")
    // 読書画面だけが使う。音量キーのページ送りと、自動スクロール中に画面を消さない指定。
    readerWebView = webView
    webView.addJavascriptInterface(object {
      @JavascriptInterface
      fun setVolumeKeys(enabled: Boolean) {
        volumeKeysTurnPages = enabled
      }
      @JavascriptInterface
      fun setKeepScreenOn(enabled: Boolean) {
        runOnUiThread {
          if (enabled) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
          else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
      }
    }, "piepAndroidReader")
    // WebView M138 以下では IME が visualViewport に反映されないため、
    // Android の inset を CSS へ補助的に伝える。insets は消費せず WebView にも渡す。
    ViewCompat.setOnApplyWindowInsetsListener(webView) { _, insets ->
      val next = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom
      if (next != imeBottomInsetPx) {
        imeBottomInsetPx = next
        webView.evaluateJavascript("window.piepAndroidImeInset?.($next)", null)
      }
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
      if (bars.top != systemTopInsetPx || bars.bottom != systemBottomInsetPx) {
        systemTopInsetPx = bars.top
        systemBottomInsetPx = bars.bottom
        webView.evaluateJavascript("window.piepAndroidSystemInsets?.(${bars.top}, ${bars.bottom})", null)
      }
      insets
    }
    ViewCompat.requestApplyInsets(webView)
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        webView.evaluateJavascript("window.piepAndroidBack?.() === true") { result ->
          if (result != "true") {
            isEnabled = false
            onBackPressedDispatcher.onBackPressed()
            isEnabled = true
          }
        }
      }
    })
  }

  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    val direction = when (event.keyCode) {
      KeyEvent.KEYCODE_VOLUME_DOWN -> 1
      KeyEvent.KEYCODE_VOLUME_UP -> -1
      else -> 0
    }
    if (direction == 0 || !volumeKeysTurnPages) return super.dispatchKeyEvent(event)
    // 押し続けても一度だけ送る。離したときも消費しないと音量の表示が出る。
    if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
      readerWebView?.evaluateJavascript("window.piepAndroidVolumeKey?.($direction)", null)
    }
    return true
  }
}
