use std::path::Path;

#[cfg(target_os = "android")]
pub(super) fn with_android_env<T>(
    action: impl FnOnce(&mut jni::JNIEnv, jni::objects::JObject) -> Result<T, String>,
) -> Result<T, String> {
    let context = tauri::tao::platform::android::prelude::main_android_context()
        .ok_or_else(|| "Android 上下文尚未就绪。".to_string())?;
    let vm = unsafe { jni::JavaVM::from_raw(context.java_vm.cast()) }
        .map_err(|error| format!("无法获取 Android JavaVM：{error}"))?;
    let mut env = vm
        .attach_current_thread_as_daemon()
        .map_err(|error| format!("无法附加 Android JNI 线程：{error}"))?;
    let object = unsafe { jni::objects::JObject::from_raw(context.context_jobject.cast()) };
    let result = action(&mut env, object);
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result
}

#[cfg(target_os = "android")]
pub(super) fn android_asset_exists(name: &str) -> bool {
    with_android_env(|env, context| {
        let assets = env
            .call_method(
                &context,
                "getAssets",
                "()Landroid/content/res/AssetManager;",
                &[],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法获取 Android Assets：{error}"))?;
        let name = env
            .new_string(name)
            .map(jni::objects::JObject::from)
            .map_err(|error| format!("无法构造 Asset 名称：{error}"))?;
        let stream = env
            .call_method(
                &assets,
                "open",
                "(Ljava/lang/String;)Ljava/io/InputStream;",
                &[jni::objects::JValue::Object(&name)],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法打开 Android Asset：{error}"))?;
        let _ = env.call_method(&stream, "close", "()V", &[]);
        Ok(())
    })
    .is_ok()
}

#[cfg(not(target_os = "android"))]
pub(super) fn android_asset_exists(_name: &str) -> bool {
    false
}

#[cfg(target_os = "android")]
pub(super) fn android_copy_asset(name: &str, target: &Path) -> Result<(), String> {
    use std::io::{BufWriter, Write};

    with_android_env(|env, context| {
        let assets = env
            .call_method(
                &context,
                "getAssets",
                "()Landroid/content/res/AssetManager;",
                &[],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法获取 Android Assets：{error}"))?;
        let name = env
            .new_string(name)
            .map(jni::objects::JObject::from)
            .map_err(|error| format!("无法构造 Asset 名称：{error}"))?;
        let stream = env
            .call_method(
                &assets,
                "open",
                "(Ljava/lang/String;)Ljava/io/InputStream;",
                &[jni::objects::JValue::Object(&name)],
            )
            .and_then(|value| value.l())
            .map_err(|error| format!("无法打开 Runtime Asset：{error}"))?;
        let array = env
            .new_byte_array(64 * 1024)
            .map_err(|error| format!("无法分配 Asset 缓冲区：{error}"))?;
        let file = std::fs::File::create(target)
            .map_err(|error| format!("无法创建 Runtime 临时文件：{error}"))?;
        let mut writer = BufWriter::new(file);
        loop {
            let read = env
                .call_method(
                    &stream,
                    "read",
                    "([B)I",
                    &[jni::objects::JValue::Object(array.as_ref())],
                )
                .and_then(|value| value.i())
                .map_err(|error| format!("无法读取 Runtime Asset：{error}"))?;
            if read < 0 {
                break;
            }
            if read == 0 {
                continue;
            }
            let bytes = env
                .convert_byte_array(&array)
                .map_err(|error| format!("无法转换 Runtime Asset：{error}"))?;
            writer
                .write_all(&bytes[..read as usize])
                .map_err(|error| format!("无法写入 Runtime Asset：{error}"))?;
        }
        writer
            .flush()
            .map_err(|error| format!("无法刷新 Runtime Asset：{error}"))?;
        env.call_method(&stream, "close", "()V", &[])
            .map_err(|error| format!("无法关闭 Runtime Asset：{error}"))?;
        Ok(())
    })
}

#[cfg(not(target_os = "android"))]
pub(super) fn android_copy_asset(_name: &str, _target: &Path) -> Result<(), String> {
    Err("当前平台没有 Android Runtime Asset。".into())
}
