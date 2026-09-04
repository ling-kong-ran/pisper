package com.lingkongran.pisper

import android.content.Context
import android.content.ContextWrapper
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.annotation.Keep

class MainActivity : TauriActivity() {
  private var rendererRecoveryScheduled = false

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    // WebView 必须随输入法可视区缩放，否则底部会话输入框会落在键盘后方。
    window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
    super.onCreate(savedInstanceState)
  }

  @Keep
  fun setTrustedProxyPort(port: Int) {
    require(port in 1..65535) { "Invalid Pisper proxy port" }
    trustedProxyPort = port
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    val recoveryUrl = intent.getStringExtra(RENDERER_RECOVERY_URL) ?: return
    intent.removeExtra(RENDERER_RECOVERY_URL)

    // Wry 会在这个回调返回后加载初始 URL，因此把恢复导航排到下一轮主线程消息。
    webView.post { webView.loadUrl(recoveryUrl) }
  }

  private fun recoverRenderer(webView: WebView, lastKnownUrl: String, didCrash: Boolean): Boolean {
    if (rendererRecoveryScheduled) return true
    rendererRecoveryScheduled = true

    val currentUrl = runCatching { webView.url }.getOrNull()
    recoverableUrl(currentUrl, lastKnownUrl)?.let { intent.putExtra(RENDERER_RECOVERY_URL, it) }
    Log.e(LOG_TAG, "WebView renderer 已退出，准备重建（didCrash=$didCrash）")

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
