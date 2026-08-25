# Rust 通过 App ClassLoader 和固定 JNI 签名调用这些成员，R8 不能改名或裁剪。
-keep class com.lingkongran.pisper.EmbeddedNodeHost { *; }
-keep class com.lingkongran.pisper.EmbeddedNodeHost$Companion { *; }
