//! Provider 密钥保管（M2）：apiKey 不再明文落盘，统一经 `KeyCustody`
//! 封装为 base64(nonce‖密文) 后写入 providers.json。
//!
//! 三个后端的信任级别不同，刻意分级：
//! - Android：密钥由 AndroidKeyStore 持有，加解密经 JNI 走系统 Keystore，
//!   Rust 进程拿不到密钥本体，卸载应用即销毁。
//! - iOS：随机主密钥存 Keychain（AfterFirstUnlockThisDeviceOnly，不随
//!   iCloud/备份迁移），加解密在本进程内用 AES-256-GCM 完成。
//! - 桌面（开发与测试回退）：主密钥存应用数据目录 master.key。
//!   桌面端不运行本机 Runtime 的真实对话，此路径只为测试服务。
use std::path::Path;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

/// 统一的密钥保管入口：seal 加密、open 解密，负载均为 base64 文本。
#[derive(Clone)]
pub struct KeyCustody {
    backend: Backend,
}

#[derive(Clone)]
enum Backend {
    #[cfg(target_os = "android")]
    AndroidKeystore,
    #[cfg(any(target_os = "ios", not(any(target_os = "android", target_os = "ios"))))]
    SoftKey([u8; 32]),
}

impl KeyCustody {
    pub fn load_or_create(dir: &Path) -> Result<Self, String> {
        #[cfg(target_os = "android")]
        {
            // 密钥由系统 Keystore 持有，与本地目录无关。
            let _ = dir;
            android::ensure_key()?;
            return Ok(Self {
                backend: Backend::AndroidKeystore,
            });
        }
        #[cfg(target_os = "ios")]
        {
            let key = ios::load_or_create_key()?;
            return Ok(Self {
                backend: Backend::SoftKey(key),
            });
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let key = filekey::load_or_create_key(dir)?;
            Ok(Self {
                backend: Backend::SoftKey(key),
            })
        }
    }

    /// 诊断用后端标识（设置页/日志可见，不含任何密钥材料）。
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            #[cfg(target_os = "android")]
            Backend::AndroidKeystore => "android-keystore",
            #[cfg(target_os = "ios")]
            Backend::SoftKey(_) => "ios-keychain",
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Backend::SoftKey(_) => "file",
        }
    }

    /// 空字符串不加密：空密钥以空串存储，读取时同样短路。
    pub fn seal(&self, plaintext: &str) -> Result<String, String> {
        if plaintext.is_empty() {
            return Ok(String::new());
        }
        let blob = match &self.backend {
            #[cfg(target_os = "android")]
            Backend::AndroidKeystore => android::seal(plaintext.as_bytes())?,
            #[cfg(any(target_os = "ios", not(any(target_os = "android", target_os = "ios"))))]
            Backend::SoftKey(key) => soft::seal(key, plaintext.as_bytes())?,
        };
        Ok(BASE64.encode(blob))
    }

    pub fn open(&self, sealed: &str) -> Result<String, String> {
        if sealed.is_empty() {
            return Ok(String::new());
        }
        let blob = BASE64
            .decode(sealed.trim())
            .map_err(|_| "密钥密文格式无效。".to_string())?;
        let bytes = match &self.backend {
            #[cfg(target_os = "android")]
            Backend::AndroidKeystore => android::open(&blob)?,
            #[cfg(any(target_os = "ios", not(any(target_os = "android", target_os = "ios"))))]
            Backend::SoftKey(key) => soft::open(key, &blob)?,
        };
        String::from_utf8(bytes).map_err(|_| "密钥解密结果不是有效文本。".to_string())
    }
}

/// 进程内软加密：AES-256-GCM，随机 96-bit nonce ‖ 密文+tag。
/// 供 iOS（Keychain 持钥）与桌面（文件持钥）两个后端复用。
#[cfg(not(target_os = "android"))]
mod soft {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use rand::RngCore;

    const NONCE_BYTES: usize = 12;

    pub fn random_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        key
    }

    pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "初始化加密器失败。")?;
        let mut nonce = [0u8; NONCE_BYTES];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| "加密失败。")?;
        let mut blob = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Ok(blob)
    }

    pub fn open(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, String> {
        if blob.len() < NONCE_BYTES + 16 {
            return Err("密钥密文不完整。".into());
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_BYTES);
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "初始化加密器失败。")?;
        cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| "密钥解密失败（Keystore 可能已重置）。".to_string())
    }
}

/// 桌面回退后端：主密钥存应用数据目录 master.key。
/// 桌面端不承担真实移动对话，这里只为开发与测试提供确定性路径。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod filekey {
    use std::{fs, path::Path};

    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

    use super::soft;

    pub fn load_or_create_key(dir: &Path) -> Result<[u8; 32], String> {
        let path = dir.join("master.key");
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(bytes) = BASE64.decode(text.trim()) {
                if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
                    return Ok(key);
                }
            }
        }
        let key = soft::random_key();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        write_owner_only(&path, &BASE64.encode(key))?;
        Ok(key)
    }

    #[cfg(unix)]
    fn write_owner_only(path: &Path, content: &str) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, content.as_bytes()))
            .map_err(|error| error.to_string())
    }

    #[cfg(not(unix))]
    fn write_owner_only(path: &Path, content: &str) -> Result<(), String> {
        // Windows 的应用数据目录本身受用户 ACL 保护，无 POSIX 权限位。
        fs::write(path, content).map_err(|error| error.to_string())
    }
}

/// Android 后端：密钥永不进入 Rust 进程，全部经 JNI 调系统 Keystore。
/// AndroidKeyStore 不需要 Context，只需要 JVM——tao 已初始化 Android 上下文。
#[cfg(target_os = "android")]
mod android {
    use jni::objects::{JByteArray, JObject, JValue};
    use jni::JNIEnv;

    const ALIAS: &str = "pisper-local-runtime-master";
    const PURPOSE_ENCRYPT: i32 = 1;
    const PURPOSE_DECRYPT: i32 = 2;
    const ENCRYPT_MODE: i32 = 1;
    const DECRYPT_MODE: i32 = 2;
    const GCM_TAG_BITS: i32 = 128;

    /// 获取 JNI 环境：从 tao 保存的 AndroidContext 取 JavaVM，
    /// 以 daemon 方式附加当前 tokio 线程，避免阻塞 JVM 退出。
    fn with_env<T>(f: impl FnOnce(&mut JNIEnv) -> Result<T, String>) -> Result<T, String> {
        let context = tauri::tao::platform::android::prelude::main_android_context()
            .ok_or_else(|| "Android 上下文尚未就绪。".to_string())?;
        let vm = unsafe { jni::JavaVM::from_raw(context.java_vm.cast()) }
            .map_err(|error| format!("获取 JavaVM 失败：{error}"))?;
        let mut env = vm
            .attach_current_thread_as_daemon()
            .map_err(|error| format!("附加 JNI 线程失败：{error}"))?;
        let result = f(&mut env);
        if result.is_err() {
            // 失败的 JNI 调用会留下挂起的 Java 异常，必须清除后才能继续使用环境。
            let _ = env.exception_clear();
        }
        result
    }

    fn jstring<'a>(env: &JNIEnv<'a>, value: &str) -> Result<JObject<'a>, String> {
        env.new_string(value)
            .map(JObject::from)
            .map_err(|error| jni_error("构造字符串", error))
    }

    fn jni_error(stage: &str, error: jni::errors::Error) -> String {
        format!("Keystore {stage}失败：{error}")
    }

    /// 已加载的 AndroidKeyStore 实例。
    fn keystore<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, String> {
        let provider = jstring(env, "AndroidKeyStore")?;
        let store = env
            .call_static_method(
                "java/security/KeyStore",
                "getInstance",
                "(Ljava/lang/String;)Ljava/security/KeyStore;",
                &[JValue::Object(&provider)],
            )
            .map_err(|error| jni_error("获取实例", error))?
            .l()
            .map_err(|error| jni_error("获取实例", error))?;
        env.call_method(
            &store,
            "load",
            "(Ljava/security/KeyStore$LoadStoreParameter;)V",
            &[JValue::Object(&JObject::null())],
        )
        .map_err(|error| jni_error("加载", error))?;
        Ok(store)
    }

    fn alias_object<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, String> {
        jstring(env, ALIAS)
    }

    /// 确保主密钥存在：不存在则以 AES-256/GCM/NoPadding 生成。
    pub fn ensure_key() -> Result<(), String> {
        with_env(|env| {
            let store = keystore(env)?;
            let alias = alias_object(env)?;
            let exists = env
                .call_method(
                    &store,
                    "containsAlias",
                    "(Ljava/lang/String;)Z",
                    &[JValue::Object(&alias)],
                )
                .map_err(|error| jni_error("查询别名", error))?
                .z()
                .map_err(|error| jni_error("查询别名", error))?;
            if exists {
                return Ok(());
            }
            let builder = env
                .new_object(
                    "android/security/keystore/KeyGenParameterSpec$Builder",
                    "(Ljava/lang/String;I)V",
                    &[
                        JValue::Object(&alias),
                        JValue::Int(PURPOSE_ENCRYPT | PURPOSE_DECRYPT),
                    ],
                )
                .map_err(|error| jni_error("构造参数", error))?;
            let gcm = jstring(env, "GCM")?;
            let modes = env
                .new_object_array(1, "java/lang/String", gcm)
                .map_err(|error| jni_error("构造参数", error))?;
            let builder = env
                .call_method(
                    &builder,
                    "setBlockModes",
                    "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                    &[JValue::Object(&JObject::from(modes))],
                )
                .map_err(|error| jni_error("构造参数", error))?
                .l()
                .map_err(|error| jni_error("构造参数", error))?;
            let no_padding = jstring(env, "NoPadding")?;
            let paddings = env
                .new_object_array(1, "java/lang/String", no_padding)
                .map_err(|error| jni_error("构造参数", error))?;
            let builder = env
                .call_method(
                    &builder,
                    "setEncryptionPaddings",
                    "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                    &[JValue::Object(&JObject::from(paddings))],
                )
                .map_err(|error| jni_error("构造参数", error))?
                .l()
                .map_err(|error| jni_error("构造参数", error))?;
            let builder = env
                .call_method(
                    &builder,
                    "setKeySize",
                    "(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                    &[JValue::Int(256)],
                )
                .map_err(|error| jni_error("构造参数", error))?
                .l()
                .map_err(|error| jni_error("构造参数", error))?;
            let spec = env
                .call_method(
                    &builder,
                    "build",
                    "()Landroid/security/keystore/KeyGenParameterSpec;",
                    &[],
                )
                .map_err(|error| jni_error("构造参数", error))?
                .l()
                .map_err(|error| jni_error("构造参数", error))?;
            let aes = jstring(env, "AES")?;
            let provider_name = jstring(env, "AndroidKeyStore")?;
            let generator = env
                .call_static_method(
                    "javax/crypto/KeyGenerator",
                    "getInstance",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
                    &[JValue::Object(&aes), JValue::Object(&provider_name)],
                )
                .map_err(|error| jni_error("获取生成器", error))?
                .l()
                .map_err(|error| jni_error("获取生成器", error))?;
            env.call_method(
                &generator,
                "init",
                "(Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[JValue::Object(&spec)],
            )
            .map_err(|error| jni_error("初始化生成器", error))?;
            env.call_method(&generator, "generateKey", "()Ljavax/crypto/SecretKey;", &[])
                .map_err(|error| jni_error("生成密钥", error))?;
            Ok(())
        })
    }

    fn cipher<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, String> {
        let transformation = jstring(env, "AES/GCM/NoPadding")?;
        let cipher = env
            .call_static_method(
                "javax/crypto/Cipher",
                "getInstance",
                "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
                &[JValue::Object(&transformation)],
            )
            .map_err(|error| jni_error("获取 Cipher", error))?
            .l()
            .map_err(|error| jni_error("获取 Cipher", error))?;
        Ok(cipher)
    }

    fn key<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, String> {
        let store = keystore(env)?;
        let alias = alias_object(env)?;
        env.call_method(
            &store,
            "getKey",
            "(Ljava/lang/String;[C)Ljava/security/Key;",
            &[JValue::Object(&alias), JValue::Object(&JObject::null())],
        )
        .map_err(|error| jni_error("取密钥", error))?
        .l()
        .map_err(|error| jni_error("取密钥", error))
    }

    /// 输出格式：iv 长度(1 字节) ‖ iv ‖ 密文+tag。
    pub fn seal(plaintext: &[u8]) -> Result<Vec<u8>, String> {
        with_env(|env| {
            let cipher = cipher(env)?;
            let key = key(env)?;
            env.call_method(
                &cipher,
                "init",
                "(ILjava/security/Key;)V",
                &[JValue::Int(ENCRYPT_MODE), JValue::Object(&key)],
            )
            .map_err(|error| jni_error("加密初始化", error))?;
            let iv = env
                .call_method(&cipher, "getIV", "()[B", &[])
                .map_err(|error| jni_error("取 IV", error))?
                .l()
                .map_err(|error| jni_error("取 IV", error))?;
            let iv_bytes = env
                .convert_byte_array(JByteArray::from(iv))
                .map_err(|error| jni_error("取 IV", error))?;
            let input = env
                .byte_array_from_slice(plaintext)
                .map_err(|error| jni_error("加密输入", error))?;
            let ciphertext = env
                .call_method(
                    &cipher,
                    "doFinal",
                    "([B)[B",
                    &[JValue::Object(&JObject::from(input))],
                )
                .map_err(|error| jni_error("加密", error))?
                .l()
                .map_err(|error| jni_error("加密", error))?;
            let ciphertext = env
                .convert_byte_array(JByteArray::from(ciphertext))
                .map_err(|error| jni_error("加密", error))?;
            if iv_bytes.len() > 255 {
                return Err("Keystore 返回了异常 IV。".into());
            }
            let mut blob = Vec::with_capacity(1 + iv_bytes.len() + ciphertext.len());
            blob.push(iv_bytes.len() as u8);
            blob.extend_from_slice(&iv_bytes);
            blob.extend_from_slice(&ciphertext);
            Ok(blob)
        })
    }

    pub fn open(blob: &[u8]) -> Result<Vec<u8>, String> {
        with_env(|env| {
            if blob.len() < 2 {
                return Err("密钥密文不完整。".into());
            }
            let iv_len = blob[0] as usize;
            if blob.len() < 1 + iv_len + 16 {
                return Err("密钥密文不完整。".into());
            }
            let iv = &blob[1..1 + iv_len];
            let ciphertext = &blob[1 + iv_len..];
            let iv_array = env
                .byte_array_from_slice(iv)
                .map_err(|error| jni_error("解密输入", error))?;
            let spec = env
                .new_object(
                    "javax/crypto/spec/GCMParameterSpec",
                    "(I[B)V",
                    &[
                        JValue::Int(GCM_TAG_BITS),
                        JValue::Object(&JObject::from(iv_array)),
                    ],
                )
                .map_err(|error| jni_error("解密参数", error))?;
            let cipher = cipher(env)?;
            let key = key(env)?;
            env.call_method(
                &cipher,
                "init",
                "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[
                    JValue::Int(DECRYPT_MODE),
                    JValue::Object(&key),
                    JValue::Object(&spec),
                ],
            )
            .map_err(|error| jni_error("解密初始化", error))?;
            let input = env
                .byte_array_from_slice(ciphertext)
                .map_err(|error| jni_error("解密输入", error))?;
            let plaintext = env
                .call_method(
                    &cipher,
                    "doFinal",
                    "([B)[B",
                    &[JValue::Object(&JObject::from(input))],
                )
                .map_err(|_| "密钥解密失败（Keystore 可能已重置）".to_string())?
                .l()
                .map_err(|error| jni_error("解密", error))?;
            env.convert_byte_array(JByteArray::from(plaintext))
                .map_err(|error| jni_error("解密", error))
        })
    }
}

/// iOS 后端：随机 32 字节主密钥存 Keychain（本机、解锁后可用、不迁移），
/// 加解密用进程内 AES-256-GCM。
#[cfg(target_os = "ios")]
mod ios {
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::data::{CFData, CFDataRef};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};

    use super::soft;

    const SERVICE: &str = "run.pisper.mobile.local-runtime";
    const ACCOUNT: &str = "provider-master-key";
    const ERR_SUCCESS: i32 = 0;
    const ERR_ITEM_NOT_FOUND: i32 = -25300;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: CFStringRef;
        static kSecClassGenericPassword: CFStringRef;
        static kSecAttrService: CFStringRef;
        static kSecAttrAccount: CFStringRef;
        static kSecValueData: CFStringRef;
        static kSecReturnData: CFStringRef;
        static kSecAttrAccessible: CFStringRef;
        static kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly: CFStringRef;
        fn SecItemAdd(query: CFDictionaryRef, result: *mut CFTypeRef) -> i32;
        fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> i32;
    }

    fn sec_string(reference: CFStringRef) -> CFString {
        // 框架常量由系统持有，用 get 规则（不转移所有权）。
        unsafe { CFString::wrap_under_get_rule(reference) }
    }

    fn base_query() -> (CFString, CFString, CFString) {
        (
            sec_string(unsafe { kSecClassGenericPassword }),
            CFString::new(SERVICE),
            CFString::new(ACCOUNT),
        )
    }

    fn read_key() -> Result<Option<[u8; 32]>, String> {
        let (class, service, account) = base_query();
        let return_data = CFBoolean::true_value();
        let query = CFDictionary::from_CFType_pairs(&[
            (
                sec_string(unsafe { kSecClass }).as_CFType(),
                class.as_CFType(),
            ),
            (
                sec_string(unsafe { kSecAttrService }).as_CFType(),
                service.as_CFType(),
            ),
            (
                sec_string(unsafe { kSecAttrAccount }).as_CFType(),
                account.as_CFType(),
            ),
            (
                sec_string(unsafe { kSecReturnData }).as_CFType(),
                return_data.as_CFType(),
            ),
        ]);
        let mut result: CFTypeRef = std::ptr::null();
        let status = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result) };
        if status == ERR_ITEM_NOT_FOUND {
            return Ok(None);
        }
        if status != ERR_SUCCESS || result.is_null() {
            return Err(format!("Keychain 读取失败（{status}）。"));
        }
        // CopyMatching 遵循 Create 规则：由我们负责释放。
        let data = unsafe { CFData::wrap_under_create_rule(result as CFDataRef) };
        let bytes = data.bytes();
        <[u8; 32]>::try_from(bytes)
            .map(Some)
            .map_err(|_| "Keychain 中的密钥长度异常。".to_string())
    }

    fn add_key(key: &[u8; 32]) -> Result<(), String> {
        let (class, service, account) = base_query();
        let value = CFData::from_buffer(key);
        let accessible = sec_string(unsafe { kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly });
        let query = CFDictionary::from_CFType_pairs(&[
            (
                sec_string(unsafe { kSecClass }).as_CFType(),
                class.as_CFType(),
            ),
            (
                sec_string(unsafe { kSecAttrService }).as_CFType(),
                service.as_CFType(),
            ),
            (
                sec_string(unsafe { kSecAttrAccount }).as_CFType(),
                account.as_CFType(),
            ),
            (
                sec_string(unsafe { kSecValueData }).as_CFType(),
                value.as_CFType(),
            ),
            (
                sec_string(unsafe { kSecAttrAccessible }).as_CFType(),
                accessible.as_CFType(),
            ),
        ]);
        let status = unsafe { SecItemAdd(query.as_concrete_TypeRef(), std::ptr::null_mut()) };
        if status == ERR_SUCCESS {
            Ok(())
        } else {
            Err(format!("Keychain 写入失败（{status}）。"))
        }
    }

    pub fn load_or_create_key() -> Result<[u8; 32], String> {
        if let Some(key) = read_key()? {
            return Ok(key);
        }
        let key = soft::random_key();
        add_key(&key)?;
        // 并发首启可能重复写入失败，回读以 Keychain 内已有值为准。
        read_key()?.ok_or_else(|| "Keychain 写入后读取失败。".to_string())
    }
}

#[cfg(test)]
mod tests {
    //! 桌面文件后端的往返与防篡改；平台后端的真机验证在设备测试中覆盖。
    use super::*;

    #[test]
    fn seal_open_roundtrip_and_tamper_detection() {
        let dir = std::env::temp_dir().join(format!("pisper-custody-test-{}", std::process::id()));
        let custody = KeyCustody::load_or_create(&dir).unwrap();
        let sealed = custody.seal("sk-secret-key-1234").unwrap();
        assert!(!sealed.contains("sk-secret-key-1234"));
        assert_eq!(custody.open(&sealed).unwrap(), "sk-secret-key-1234");

        // 重新加载（新进程语义）：同一密钥文件必须能解开旧密文。
        let reloaded = KeyCustody::load_or_create(&dir).unwrap();
        assert_eq!(reloaded.open(&sealed).unwrap(), "sk-secret-key-1234");

        // 篡改密文一字节：GCM 校验必须失败。
        let mut blob = BASE64.decode(&sealed).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        let tampered = BASE64.encode(blob);
        assert!(custody.open(&tampered).is_err());

        // 空串短路：不加密、不解密。
        assert_eq!(custody.seal("").unwrap(), "");
        assert_eq!(custody.open("").unwrap(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
