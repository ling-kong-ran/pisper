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
        val code = nativeRun(arguments)
        Log.i("PisperNode", "Embedded Node stopped with code $code")
      }
      return null
    }

    @JvmStatic
    private external fun nativeRun(arguments: Array<String>): Int
  }
}
