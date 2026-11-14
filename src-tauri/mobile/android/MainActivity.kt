package com.lingkongran.pisper

import android.content.Context
import android.content.ContextWrapper
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.annotation.Keep
import androidx.core.graphics.Insets
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  private var rendererRecoveryScheduled = false
  private var safeAreaWebView: WebView? = null
  private var safeArea = SafeArea(0, 0, 0, 0)
  private var keyboardInset = 0

  private data class SafeArea(val top: Int, val right: Int, val bottom: Int, val left: Int)

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    // edge-to-edge 下 adjustResize 不再缩小窗口，只负责让系统派发输入法 insets；
    // 真正让出键盘高度由下面的 insets 监听完成。
    window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
    super.onCreate(savedInstanceState)
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        val webView = safeAreaWebView
        val origin = webView?.url?.let { runCatching { Uri.parse(it) }.getOrNull() }
        fun fallback() {
          isEnabled = false
          try { onBackPressedDispatcher.onBackPressed() } finally { isEnabled = true }
        }
        if (webView == null || !isTrustedProxyOrigin(origin)) { fallback(); return }
        // 不向非受信任页面注入返回协议；没有可关闭界面时保留 Tauri 默认返回行为。
        webView.evaluateJavascript("Boolean(window.__pisperRequestBack?.())") { consumed ->
          if (!isFinishing && !isDestroyed && safeAreaWebView === webView && consumed != "true") fallback()
        }
      }
    })
    // Android 15+ 的 edge-to-edge WebView 不一定把系统栏转换为 CSS env(safe-area-inset-*)。
    // 保留系统默认分发，只把真实 WindowInsets 映射为当前页面的设计变量。
    window.decorView.setOnApplyWindowInsetsListener { view, insets ->
      val next = readSafeArea(insets)
      if (next != safeArea) {
        safeArea = next
        safeAreaWebView?.let(::publishSafeArea)
      }
      view.onApplyWindowInsets(applyKeyboardInset(insets))
    }
    window.decorView.requestApplyInsets()
  }

  override fun onDestroy() {
    safeAreaWebView = null
    window.decorView.setOnApplyWindowInsetsListener(null)
    super.onDestroy()
  }

  private fun readSafeArea(insets: WindowInsets): SafeArea {
    val compatible = WindowInsetsCompat.toWindowInsetsCompat(insets, window.decorView)
    val bars = compatible.getInsets(
      WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
    )
    // 输入法弹出时内容区已让到键盘顶部，底部不能再叠加导航栏留白。
    val bottom = if (compatible.isVisible(WindowInsetsCompat.Type.ime())) 0 else bars.bottom
    return SafeArea(bars.top, bars.right, bottom, bars.left)
  }

  // edge-to-edge 窗口不会随输入法变矮：由这里把内容区底边让到键盘顶部，WebView 随之变矮，
  // 页面的 visualViewport 才会缩小，输入框才能浮在键盘上方。键盘高度只在这一处生效，
  // 交给子 View 的 insets 去掉输入法部分，避免新版 WebView 自己再让一次。
  private fun applyKeyboardInset(insets: WindowInsets): WindowInsets {
    val compatible = WindowInsetsCompat.toWindowInsetsCompat(insets, window.decorView)
    val next = if (compatible.isVisible(WindowInsetsCompat.Type.ime())) {
      compatible.getInsets(WindowInsetsCompat.Type.ime()).bottom
    } else {
      0
    }
    val content = findViewById<ViewGroup>(android.R.id.content)
    if (content != null && next != keyboardInset) {
      keyboardInset = next
      content.setPadding(content.paddingLeft, content.paddingTop, content.paddingRight, next)
    }
    return WindowInsetsCompat.Builder(compatible)
      .setInsets(WindowInsetsCompat.Type.ime(), Insets.NONE)
      .build()
      .toWindowInsets() ?: insets
  }

  private fun attachSafeArea(webView: WebView) {
    safeAreaWebView = webView
    window.decorView.rootWindowInsets?.let { safeArea = readSafeArea(it) }
    publishSafeArea(webView)
  }

  private fun publishSafeArea(webView: WebView) {
    val currentUrl = webView.url ?: return
    val uri = runCatching { Uri.parse(currentUrl) }.getOrNull()
    if (!isTrustedProxyOrigin(uri)) return
    val density = webView.resources.displayMetrics.density.takeIf { it > 0f } ?: 1f
    val top = safeArea.top / density
    val right = safeArea.right / density
    val bottom = safeArea.bottom / density
    val left = safeArea.left / density
    webView.evaluateJavascript(
      """(() => { const root = document.documentElement; if (!root) return; const style = root.style;
        style.setProperty('--pisper-safe-area-top', '${top}px');
        style.setProperty('--pisper-safe-area-right', '${right}px');
        style.setProperty('--pisper-safe-area-bottom', '${bottom}px');
        style.setProperty('--pisper-safe-area-left', '${left}px'); })();""",
      null,
    )
  }

  @Keep
  fun setTrustedProxyPort(port: Int) {
    require(port in 1..65535) { "Invalid Pisper proxy port" }
    trustedProxyPort = port
  }

  private fun recoverRenderer(webView: WebView, lastKnownUrl: String, didCrash: Boolean): Boolean {
    if (rendererRecoveryScheduled) return true
    rendererRecoveryScheduled = true

    val currentUrl = runCatching { webView.url }.getOrNull()
    recoverableUrl(currentUrl, lastKnownUrl)?.let { intent.putExtra(RENDERER_RECOVERY_URL, it) }
    Log.e(LOG_TAG, "WebView renderer 已退出，准备重建（didCrash=$didCrash）")
    if (safeAreaWebView === webView) safeAreaWebView = null

    // renderer 已失效，Android 要求先从视图树移除并销毁所有关联 WebView。
    (webView.parent as? ViewGroup)?.removeView(webView)
    webView.destroy()
    Handler(Looper.getMainLooper()).post {
      if (!isFinishing && !isDestroyed) recreate()
    }
    return true
  }

  companion object {
    private const val LOG_TAG = "Pisper/WebViewRecovery"
    private const val RENDERER_RECOVERY_URL = "pisper.rendererRecoveryUrl"

    @Volatile
    private var trustedProxyPort = 0

    @JvmStatic
    fun isTrustedProxyOrigin(origin: Uri?): Boolean {
      return origin?.scheme?.lowercase() == "http" &&
        origin.host == "127.0.0.1" &&
        origin.userInfo.isNullOrEmpty() &&
        origin.port == trustedProxyPort &&
        trustedProxyPort in 1..65535
    }

    @JvmStatic
    fun restoreRendererRoute(webView: WebView) {
      val activity = findActivity(webView.context) ?: return
      if (activity.isFinishing || activity.isDestroyed) return
      activity.attachSafeArea(webView)
      val recoveryUrl = activity.intent.getStringExtra(RENDERER_RECOVERY_URL) ?: return
      activity.intent.removeExtra(RENDERER_RECOVERY_URL)
      val uri = runCatching { Uri.parse(recoveryUrl) }.getOrNull()
      if (!isTrustedProxyOrigin(uri)) return

      // 等 Wry 初始页面完成后再恢复，避免初始导航覆盖 onWebViewCreate 排队的导航。
      webView.post {
        if (!activity.isFinishing && !activity.isDestroyed && webView.url != recoveryUrl) {
          webView.loadUrl(recoveryUrl)
        }
      }
    }

    @JvmStatic
    fun recoverFromRendererCrash(
      webView: WebView,
      lastKnownUrl: String,
      didCrash: Boolean,
    ): Boolean {
      val activity = findActivity(webView.context)
      if (activity != null) return activity.recoverRenderer(webView, lastKnownUrl, didCrash)

      // 无法解析宿主时仍必须声明已处理，否则 Chromium 会主动终止整个 App。
      Log.e(LOG_TAG, "WebView renderer 已退出，但无法解析 MainActivity")
      (webView.parent as? ViewGroup)?.removeView(webView)
      webView.destroy()
      return true
    }

    private fun findActivity(context: Context): MainActivity? {
      var current = context
      while (current is ContextWrapper) {
        if (current is MainActivity) return current
        val base = current.baseContext
        if (base === current) break
        current = base
      }
      return current as? MainActivity
    }

    private fun recoverableUrl(vararg candidates: String?): String? {
      return candidates.firstOrNull { candidate ->
        if (candidate.isNullOrBlank() || candidate == "about:blank") return@firstOrNull false
        val uri = runCatching { Uri.parse(candidate) }.getOrNull()
        // 恢复与媒体授权复用同一来源边界，renderer 重建不能改变受信任代理。
        isTrustedProxyOrigin(uri)
      }
    }
  }
}
