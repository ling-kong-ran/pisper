# iOS 本地语音验收

本次实现覆盖桌面、Android 和 iOS。Windows 可以运行共享逻辑测试、资源一致性检查及本机支持的构建，不能代替 Swift 编译、XCTest、iOS 链接或真机验证。iOS 的这些验证由用户在 Mac 上执行。

## 统一资源

唯一来源为 `shared/speech-model-catalog.json`，三个平台不维护各自的模型清单。

- ASR：`x-asr-480ms-int8`，480 ms X-ASR，中英识别。
- TTS：`vits-melo-tts-zh_en`，FP32，中英单女声，公开音色 `melo-zh-en-female`、sid 0。
- Melo 官方归档为 167,006,755 字节（159.27 MiB），SHA-256 `e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514`。
- `model.onnx` 为 170,429,550 字节，SHA-256 `bf30582eb1b012250a35b1a4a80e7dfbcf8485e7bb9de0d95efbbeef0e4ad86d`。
- 全部已安装 TTS 文件合计 191,224,149 字节，符合当前约 200 MB 的模型体积目标；这不是全应用大小，也不是运行内存预算。
- 归档内的 133 字节 `model.int8.onnx` 是 LFS 指针，不是可用模型，不安装。
- VITS 参数统一为 4 线程、`noiseScale=0.667`、`noiseScaleW=0.8`、`lengthScale=1`。公开 sid 0 由 SDK 映射为模型内部 speaker_id 1，不修改权重或 speaker metadata。
- sherpa 1.13.7 的 `dictDir/dict_dir` 是保留字段；实际 Melo 分词路径使用 lexicon 的 `PhraseMatcher`，不能用字段赋值声称外部 Jieba 字典参与了推理。

发行包只带原生运行库、catalog、离线 notices 和小型 BPE 词表。权重由用户主动下载，完成归档及各文件校验后安装。没有 Kokoro 回退或音色选择器。

## 启播与引擎生命周期

桌面、Android、iOS 共用 `src/features/chat/speech-stream-text.ts`。直接接收已验证归属的 SSE 文本，不等待打字机动画；“你好，”等短语可在逗号处提交，首段也可在可信正文空格闭词时提交（如 `Hello `）。长句按 ICU 词边界释放，250 ms 缓冲期限只释放已经稳定的文本。未完成的词、链接和 Markdown 仍须保留，250 ms 不代表实际首音时间。

三端进入对话模式、确认模型已安装后即预热 ASR + TTS，普通语音输入则在开麦前预热 ASR。同配置复用已加载的原生引擎，不能只把空 worker 启动当作预热成功。ASR 和 TTS 分别使用独立串行队列，加载与推理可跨模型并行，同模型的请求仍按入队顺序执行，播放顺序不变。

语音模式独立于录音轮次持有模型；静音、思考、播报和正常轮次切换不解除持有。仍有任一活跃语音会话时不启动空闲淘汰，最后退出后才开始 30 秒窗口。桌面以 `/api/speech/session` SSE 连接持有，断开即释放；移动端用 `mobile_prepare_speech_session` / `mobile_release_speech_session`，并在后台、内存压力、宿主销毁时清理。取消旧请求只作废自身，不能解除新会话持有或停止另一个模型；释放句柄必须进入其所属队列。配置变化、引擎错误或取消正在执行的桌面 native 推理仍可能要求重新加载。

当前模型的明确边界：[sherpa 1.13.7 VITS](https://github.com/k2-fsa/sherpa-onnx/blob/v1.13.7/sherpa-onnx/csrc/offline-tts-vits-impl.h) 在 `Process()` / `model_->Run()` 完成后才回调 PCM。现有 WAV 播放仍等待当前片段合成完成，不能把提前分句、预取或 callback 的存在称为模型内部真流式。约 200 MB 的中英真流式替代方案尚未完成验证，不以更大的模型或只支持英文的模型替换共享 catalog。

共享前端入口为 `VoiceModeOverlay` / `useVoiceSession` / `playLocalSpeech`；桌面走 `/api/speech/synthesize`、`/api/speech/cancel` 和 Web Audio，Android/iOS 走同一 Rust 桥接的 `mobile_synthesize_speech`、`mobile_play_speech`、`mobile_cancel_speech`。本次缓存修改不改变模型资源打包、麦克风授权、音频 session 或中断业务合同。

定向验证入口：

```bash
npx tsx --test runtime/tests/speech-engine.test.mjs runtime/tests/speech-vits-engine.test.mjs runtime/tests/speech-stream-text.test.mjs runtime/tests/speech-output.test.mjs runtime/tests/speech-platform-parity.test.mjs
node --test src-tauri/mobile-device-plugin/android/src/main/java/app/pisper/mobiledevice/SpeechNative.test.mjs
npm run build:android -- --release --target aarch64
npm run test:ios
```

Android JVM 测试实际编译 AAR 服务签名，并测试缓存创建次数、会话持有、最后退出计时、跨模型并行、同模型顺序、配置切换、过期回调失效及错误恢复；不能替代 R8 release 或真机。iOS 相同边界由 `SpeechAudioStateTests` 覆盖，必须在 Mac 的 XCTest 执行，Windows 源码检查不算通过。

本轮 Windows 集成验证：`npm run check`、全量 Node 1805 项测试、TUI 153 项测试、Tauri 本机测试及 clippy 均通过。Android JVM/AAR 验证与 `--release --target aarch64` 构建通过；已检查 R8 输出 mapping，新会话参数及 prepare/release 命令名称保留。产物为 `src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk`，未签名，未进行本轮设备安装或运行。iOS 尚未执行 Xcode 构建、XCTest 和真机验证。

## Windows 实测

开发模式，Windows / AMD Ryzen 7 7800X3D / Edge 152，真实 Melo FP32 与本机 X-ASR INT8。基准通过真实 HTTP SSE、生产事件分发、语音文本流、分句器、播放器及本地推理；回复文本受控，不测 LLM 生成耗时。预热使用与录音相同的项目热词，配置变化引起的重建不能算作空闲淘汰。

| 场景 | 本机测量 |
| --- | --- |
| 原串行加载 ASR + TTS | 7.77 秒，单次 |
| 双队列并行预热 ASR + TTS | 4.14 秒，单次 |
| 对话模式保持 35 秒不录音、不合成后，新建 ASR stream | 3.5 ms |
| 同一保持测试，“你好，”SSE 到首个播放节点启动 | 185 ms |
| 同一保持测试，“你好，”SSE 到首个非静音输出估算 | 307 ms |
| 随后 5 轮首个非静音输出估算 | 259–284 ms |
| 空格闭词 `Hello `，此前 5 轮热启动 | 275–313 ms |

首音使用 AudioWorklet 检测非静音采样，并通过 `AudioContext.getOutputTimestamp()` 估算输出时刻，不是麦克风回录或扬声器声学测量。测试让后文晚到 1.5 秒，用于确认首短语不等待后文；该供给间隔不能记为播放器引入的停顿。以上是桌面少量样本，不代表手机时延或性能保证。首次预热仍须等待，不能宣称消除了冷启动成本。

## Mac 准备

需要 Node.js 20+、npm、Rust、完整 Xcode 及 iOS Simulator runtime。官方 sherpa SPM 包使用 Swift tools 5.9，当前 ORT 制品要求最低 iOS 15.1；项目最低版本与之保持一致。

```bash
xcode-select -p
xcodebuild -version
xcrun simctl list runtimes
npm ci
npm run init:ios
npm run test:ios
```

`test:ios` 将生产的四个语音核心 Swift 文件、三个 XCTest 文件和同一套小资源复制到 `release/ios-speech-tests-<timestamp>/`。它只隔离依赖 App 宿主 Rust 符号的 Tauri 入口，保留正式 sherpa 和 LibArchive 依赖，不使用替代推理实现。

脚本创建独立模拟器，运行 `xcodebuild test`，将结果保留为 `SpeechTests.xcresult`，最后只关闭并删除本次创建的模拟器。不要将 Windows 的语法解析结果当作这一步通过。

## App 构建

在 Xcode 中配置自己的开发团队和签名，不将签名材料提交仓库。

```bash
npm run build:ios
```

当前命令构建带本机 Runtime 的真机调试签名 IPA，输出在 `release/pisper-ios-<App版本>-signed.ipa`。它不是 App Store 发布命令，不会发布版本。

可在临时目录解压 IPA，并验证模型权重未被打包以及三份小资源与共享源逐字节相同：

```bash
WORK=$(mktemp -d)
unzip -q release/pisper-ios-<App版本>-signed.ipa -d "$WORK"
node scripts/verify-ios-speech-bundle.mjs "$WORK/Payload/Pisper.app"
rm -rf "$WORK"
```

先确认实际 `.app` 名称，再执行上面的验证路径。CI 的 App 构建也调用此校验。

## 真机流程

1. 首次进入语音模式，分别检查麦克风授权允许、拒绝、授权弹窗出现时的失焦，以及授权结束时仍在后台的情况。未授予权限或仍在后台时不得开麦。
2. 显式下载模型，观察真实字节进度；中途取消后恢复，确认 Range 续传、校验和安装结果。重新启动后检查安装仍有效。不要将压缩包体积等同于安装空间或进程内存。
3. 试听默认女声，检查中文、英文、中英混合、日期数字的可懂度和实际完整播放。无静音/非有限样本只证明数值有效，不能替代听感评价。
4. 使用真实会话完成至少三轮“聆听、识别、自动提交、回复、播放、再次聆听”。普通麦克风和 F8 只回填草稿，不自动提交。
5. 让回复含多个句子，确认第一句在 SSE 尚未结束时已经开始播放；最终无句末标点的尾句也要播放。流结束和全部音频完成之前不得重新开麦。再覆盖 SSE 转快照恢复、临时消息 ID 替换为持久化 ID、恢复后的挂断，以及尾音期间新提示到达：同一运行的尾句不能丢失，也不能中止外来运行。
6. 在聆听、转写、思考、播报各阶段挂断、切会话、进入后台、触发来电或改变耳机路由。确认立即停止、旧结果不播放，返回前台不复活旧轮次。播放结束或失败后不应持续压低其他 App 音量。
7. 检查 Markdown、代码围栏、脚本样式 HTML、链接和 Unicode 跨增量边界不会导致读出隐藏内容、重复正文或遗失尾句。ASCII 句号可等待小数/缩写的后继字符；未显式闭合的 HTML 可能保守等待 EOF。
8. 远程聊天模式下检查模型目录、识别、合成和播放仍属于本机，只有会话请求走所选服务器。
9. 记录首次加载/首音、后续句子间隙、完整回合延迟和实际进程内存。600 MB 是参考而非硬性淘汰线，各平台 RSS、PSS、private、working set 等口径分别记录。
10. 在所支持的较旧系统 WebView 上验证取消、超时和试听。语音代码不依赖 `AbortSignal.any`、`AbortSignal.timeout` 或 `signal.throwIfAborted()`，通过显式作用域清理监听器与计时器；Windows 行为测试和 Android Chromium 110 实测仍不能替代对应 iOS 版本验收。

## 未消除的边界

X-ASR 已知对部分前导静音敏感，不能靠固定裁剪或语义替换掩盖错字。Melo 的 MIT 权重声明有独立模型卡证据，但训练/声音权利链、词典历史输入、FST 生成来源和原生二进制依赖仍有 pending。离线许可页面及 JSON 导出必须保留这些事项，本次实现不等于商用或正式分发许可全部闭环。
