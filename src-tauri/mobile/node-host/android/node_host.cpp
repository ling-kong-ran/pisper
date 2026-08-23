// JNI 只负责把已校验参数交给 Node；线程所有权由 Kotlin 宿主持有，确保
// node::Start 及其事件循环永不占用 Android WebView 主线程。
#include <jni.h>
#include <node.h>

#include <string>
#include <vector>

extern "C" JNIEXPORT jint JNICALL
Java_com_lingkongran_pisper_EmbeddedNodeHost_nativeRun(
    JNIEnv* env,
    jclass,
    jobjectArray arguments) {
  const jsize count = env->GetArrayLength(arguments);
  std::vector<std::string> owned;
  owned.reserve(static_cast<size_t>(count));
  for (jsize index = 0; index < count; ++index) {
    auto value = static_cast<jstring>(env->GetObjectArrayElement(arguments, index));
    if (value == nullptr) return 64;
    const char* utf8 = env->GetStringUTFChars(value, nullptr);
    if (utf8 == nullptr) {
      env->DeleteLocalRef(value);
      return 65;
    }
    owned.emplace_back(utf8);
    env->ReleaseStringUTFChars(value, utf8);
    env->DeleteLocalRef(value);
  }

  std::vector<char*> argv;
  argv.reserve(owned.size());
  for (auto& value : owned) argv.push_back(value.data());
  return node::Start(static_cast<int>(argv.size()), argv.data());
}
