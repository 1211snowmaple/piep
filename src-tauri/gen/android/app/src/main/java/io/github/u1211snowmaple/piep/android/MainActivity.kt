package io.github.u1211snowmaple.piep.android

import android.os.Bundle
import android.webkit.WebView
import android.webkit.JavascriptInterface
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  override val handleBackNavigation: Boolean = false
  @Volatile private var imeBottomInsetPx = 0
  @Volatile private var systemTopInsetPx = 0
  @Volatile private var systemBottomInsetPx = 0

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
}
