package com.lingkongran.pisper

import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

class EmbeddedNodeHost private constructor() {
  companion object {
    private val started = AtomicBoolean(false)

    @JvmStatic
    fun start(arguments: Array<String>): String? {
      if (!started.compareAndSet(false, true)) return null
      try {
        System.loadLibrary("pisper_node_host")
      } catch (error: Throwable) {
        started.set(false)
        return error.message ?: error.javaClass.simpleName
      }
      thread(start = true, isDaemon = true, name = "pisper-embedded-node") {
        try {
          val code = nativeRun(arguments)
          Log.i("PisperNode", "Embedded Node stopped with code $code")
        } catch (error: Throwable) {
          Log.e("PisperNode", "Embedded Node stopped unexpectedly", error)
        } finally {
          // Node 退出后允许下一次前台恢复重新建立宿主线程，避免 Rust 侧永久认为它仍在运行。
          started.set(false)
        }
      }
      return null
    }

    @JvmStatic
    fun isStarted(): Boolean = started.get()

    @JvmStatic
    private external fun nativeRun(arguments: Array<String>): Int
  }
}
