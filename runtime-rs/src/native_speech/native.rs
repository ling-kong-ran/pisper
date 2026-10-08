//! Real sherpa-onnx 1.13.7 C ABI, hosted only by the isolated Rust worker process.
use super::{
    asr_abi as asr,
    catalog::{self, Model},
    error::{engine, Result},
    terms, tts_abi as tts,
};
use libloading::Library;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    ffi::{c_char, c_void, CStr, CString},
    path::{Path, PathBuf},
};
struct Api {
    library: Library,
}
impl Api {
    fn open(directory: &Path) -> Result<Self> {
        let filename = if cfg!(windows) {
            "sherpa-onnx-c-api.dll"
        } else if cfg!(target_os = "macos") {
            "libsherpa-onnx-c-api.dylib"
        } else {
            "libsherpa-onnx-c-api.so"
        };
        let path = directory.join(filename);
        let actual = std::fs::canonicalize(&path).map_err(|_| engine("worker"))?;
        if super::storage::linked(&std::fs::symlink_metadata(&path).map_err(|_| engine("worker"))?)
        {
            return Err(engine("worker"));
        }
        // SAFETY: trusted, explicitly staged native library; dependency search stays local/system on Windows.
        #[cfg(windows)]
        let library = unsafe {
            libloading::os::windows::Library::load_with_flags(actual, 0x00000100 | 0x00001000)
                .map(Library::from)
                .map_err(|_| engine("worker"))?
        };
        #[cfg(not(windows))]
        let library = unsafe { Library::new(actual).map_err(|_| engine("worker"))? };
        let api = Self { library };
        unsafe {
            let version: unsafe extern "C" fn() -> *const c_char =
                api.function(b"SherpaOnnxGetVersionStr\0")?;
            let pointer = version();
            if pointer.is_null() || CStr::from_ptr(pointer).to_bytes() != b"1.13.7" {
                return Err(engine("config"));
            }
        }
        Ok(api)
    }
    unsafe fn function<T: Copy>(&self, name: &[u8]) -> Result<T> {
        self.library
            .get::<T>(name)
            .map(|symbol| *symbol)
            .map_err(|_| engine("worker"))
    }
}
struct Stream {
    pointer: *const asr::OnlineStream,
    terms: Vec<String>,
    samples: usize,
}
pub struct NativeInference {
    api: Api,
    kind: String,
    model: Model,
    model_dir: PathBuf,
    resource_dir: PathBuf,
    hotwords_dir: PathBuf,
    recognizer: *const asr::OnlineRecognizer,
    recognizer_terms: String,
    streams: HashMap<String, Stream>,
    tts: *const tts::SherpaOnnxOfflineTts,
}
impl NativeInference {
    pub fn new(input: &Value) -> Result<Self> {
        let model: Model =
            serde_json::from_value(input["model"].clone()).map_err(|_| engine("config"))?;
        let path = |key: &str| {
            input[key]
                .as_str()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or_else(|| engine("config"))
        };
        let kind = input["kind"]
            .as_str()
            .filter(|v| ["asr", "tts"].contains(v))
            .ok_or_else(|| engine("config"))?
            .to_owned();
        let api = Api::open(&path("nativeLibraryDir")?)?;
        let mut inference = Self {
            api,
            kind,
            model,
            model_dir: path("modelDir")?,
            resource_dir: path("resourceDir")?,
            hotwords_dir: path("hotwordsDir")?,
            recognizer: std::ptr::null(),
            recognizer_terms: String::new(),
            streams: HashMap::new(),
            tts: std::ptr::null(),
        };
        super::downloads::tree(&inference.model_dir, &inference.model, true)
            .map_err(|_| engine("config"))?;
        for file in &inference.model.files {
            let handle =
                super::storage::checked_file(&inference.model_dir.join(&file.path), false, false)
                    .map_err(|_| engine("config"))?;
            if handle.metadata().map_err(|_| engine("config"))?.len() != file.bytes {
                return Err(engine("config"));
            }
        }
        if inference.kind == "tts" {
            inference.load_tts()?;
        }
        Ok(inference)
    }
    pub fn version(directory: &Path) -> Result<Value> {
        let api = Api::open(directory)?;
        unsafe {
            let version: unsafe extern "C" fn() -> *const c_char =
                api.function(b"SherpaOnnxGetVersionStr\0")?;
            let ort: unsafe extern "C" fn() -> *const c_char =
                api.function(b"SherpaOnnxGetOnnxruntimeVersionStr\0")?;
            Ok(
                json!({"sherpa":CStr::from_ptr(version()).to_string_lossy(),"onnxruntime":CStr::from_ptr(ort()).to_string_lossy()}),
            )
        }
    }
    fn model_path(&self, key: &str) -> Result<CString> {
        let relative = self.model.config[key]
            .as_str()
            .ok_or_else(|| engine("config"))?;
        if !catalog::safe_relative(relative) {
            return Err(engine("config"));
        }
        cstring(&self.model_dir.join(relative).to_string_lossy())
    }
    fn load_asr(&mut self, requested: &[String]) -> Result<()> {
        let hotwords = terms::hotwords(requested);
        if !self.recognizer.is_null() && self.recognizer_terms == hotwords {
            return Ok(());
        }
        if !self.streams.is_empty() {
            return Err(engine("busy"));
        }
        unsafe {
            self.unload_asr();
        }
        let encoder = self.model_path("encoder")?;
        let decoder = self.model_path("decoder")?;
        let joiner = self.model_path("joiner")?;
        let tokens = self.model_path("tokens")?;
        let cpu = cstring("cpu")?;
        let bpe_path = self.resource_dir.join("speech-resources/xasr-bpe.vocab");
        let supports = !hotwords.is_empty() && bpe_path.is_file();
        let bpe = cstring(&bpe_path.to_string_lossy())?;
        let unit = cstring(if supports { "bpe" } else { "" })?;
        let active = self.hotwords_dir.join("active-terms.txt");
        let hotwords_path = cstring(&active.to_string_lossy())?;
        let method = cstring(if supports {
            "modified_beam_search"
        } else {
            "greedy_search"
        })?;
        if supports {
            super::storage::directory(&self.hotwords_dir, true).map_err(|_| engine("config"))?;
            let mut file =
                super::storage::checked_file(&active, true, true).map_err(|_| engine("config"))?;
            use std::io::Write;
            file.set_len(0).map_err(|_| engine("config"))?;
            file.write_all(format!("{hotwords}\n").as_bytes())
                .map_err(|_| engine("config"))?;
        }
        // SAFETY: official v1.13.7 repr(C) structs, zero initialized as the C API requires. CString storage outlives construction.
        unsafe {
            let mut config: asr::OnlineRecognizerConfig = std::mem::zeroed();
            config.feat_config.sample_rate = 16000;
            config.feat_config.feature_dim = 80;
            config.model_config.transducer.encoder = encoder.as_ptr();
            config.model_config.transducer.decoder = decoder.as_ptr();
            config.model_config.transducer.joiner = joiner.as_ptr();
            config.model_config.tokens = tokens.as_ptr();
            config.model_config.num_threads = 1;
            config.model_config.provider = cpu.as_ptr();
            config.decoding_method = method.as_ptr();
            if supports {
                config.model_config.modeling_unit = unit.as_ptr();
                config.model_config.bpe_vocab = bpe.as_ptr();
                config.max_active_paths = 2;
                config.hotwords_file = hotwords_path.as_ptr();
                config.hotwords_score = 1.5;
            }
            let create: unsafe extern "C" fn(
                *const asr::OnlineRecognizerConfig,
            ) -> *const asr::OnlineRecognizer =
                self.api.function(b"SherpaOnnxCreateOnlineRecognizer\0")?;
            let pointer = create(&config);
            if pointer.is_null() {
                return Err(engine("inference"));
            }
            self.recognizer = pointer;
            self.recognizer_terms = hotwords;
        }
        Ok(())
    }
    fn load_tts(&mut self) -> Result<()> {
        if self.model.engine != "vits" {
            return Err(engine("config"));
        }
        let model = self.model_path("model")?;
        let tokens = self.model_path("tokens")?;
        let lexicon = self.model_path("lexicon")?;
        let dict = self.model_path("dictDir")?;
        let cpu = cstring("cpu")?;
        let rules = self.model.config["ruleFsts"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .filter(|p| catalog::safe_relative(p))
                            .map(|p| self.model_dir.join(p).to_string_lossy().into_owned())
                            .ok_or_else(|| engine("config"))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default()
            .join(",");
        let rules = cstring(&rules)?;
        unsafe {
            let mut config: tts::OfflineTtsConfig = std::mem::zeroed();
            let vits = &mut config.model.vits;
            vits.model = model.as_ptr();
            vits.tokens = tokens.as_ptr();
            vits.lexicon = lexicon.as_ptr();
            vits.dict_dir = dict.as_ptr();
            vits.noise_scale = self.model.config["noiseScale"].as_f64().unwrap_or(0.667) as f32;
            vits.noise_scale_w = self.model.config["noiseScaleW"].as_f64().unwrap_or(0.8) as f32;
            vits.length_scale = self.model.config["lengthScale"].as_f64().unwrap_or(1.0) as f32;
            config.model.num_threads = self.model.config["numThreads"].as_i64().unwrap_or(4) as i32;
            config.model.provider = cpu.as_ptr();
            config.max_num_sentences = 1;
            config.rule_fsts = rules.as_ptr();
            config.silence_scale = 1.0;
            let create: unsafe extern "C" fn(
                *const tts::OfflineTtsConfig,
            ) -> *const tts::SherpaOnnxOfflineTts =
                self.api.function(b"SherpaOnnxCreateOfflineTts\0")?;
            self.tts = create(&config);
            if self.tts.is_null() {
                return Err(engine("inference"));
            }
        }
        Ok(())
    }
    fn create_stream(&self) -> Result<*const asr::OnlineStream> {
        unsafe {
            let create: unsafe extern "C" fn(
                *const asr::OnlineRecognizer,
            ) -> *const asr::OnlineStream = self.api.function(b"SherpaOnnxCreateOnlineStream\0")?;
            let pointer = create(self.recognizer);
            if pointer.is_null() {
                Err(engine("inference"))
            } else {
                Ok(pointer)
            }
        }
    }
    fn destroy_stream(&self, pointer: *const asr::OnlineStream) {
        unsafe {
            if let Ok(destroy) = self
                .api
                .function::<unsafe extern "C" fn(*const asr::OnlineStream)>(
                    b"SherpaOnnxDestroyOnlineStream\0",
                )
            {
                destroy(pointer);
            }
        }
    }
    fn accept(&self, pointer: *const asr::OnlineStream, samples: &[f32]) -> Result<()> {
        validate_samples(samples)?;
        unsafe {
            let accept: unsafe extern "C" fn(*const asr::OnlineStream, i32, *const f32, i32) = self
                .api
                .function(b"SherpaOnnxOnlineStreamAcceptWaveform\0")?;
            accept(pointer, 16000, samples.as_ptr(), samples.len() as i32);
        }
        Ok(())
    }
    fn finish(&self, pointer: *const asr::OnlineStream, has_audio: bool) -> Result<()> {
        if has_audio {
            self.accept(pointer, &vec![0.0; 16000])?;
        }
        unsafe {
            let finish: unsafe extern "C" fn(*const asr::OnlineStream) = self
                .api
                .function(b"SherpaOnnxOnlineStreamInputFinished\0")?;
            finish(pointer);
        }
        Ok(())
    }
    fn decoded(&self, pointer: *const asr::OnlineStream) -> Result<String> {
        unsafe {
            let ready: unsafe extern "C" fn(
                *const asr::OnlineRecognizer,
                *const asr::OnlineStream,
            ) -> i32 = self.api.function(b"SherpaOnnxIsOnlineStreamReady\0")?;
            let decode: unsafe extern "C" fn(
                *const asr::OnlineRecognizer,
                *const asr::OnlineStream,
            ) = self.api.function(b"SherpaOnnxDecodeOnlineStream\0")?;
            while ready(self.recognizer, pointer) != 0 {
                decode(self.recognizer, pointer);
            }
            let result: unsafe extern "C" fn(
                *const asr::OnlineRecognizer,
                *const asr::OnlineStream,
            ) -> *const c_char = self
                .api
                .function(b"SherpaOnnxGetOnlineStreamResultAsJson\0")?;
            let destroy: unsafe extern "C" fn(*const c_char) = self
                .api
                .function(b"SherpaOnnxDestroyOnlineStreamResultJson\0")?;
            let text = result(self.recognizer, pointer);
            if text.is_null() {
                return Err(engine("inference"));
            }
            let value = serde_json::from_slice::<Value>(CStr::from_ptr(text).to_bytes());
            destroy(text);
            let value = value.map_err(|_| engine("inference"))?;
            Ok(value["text"]
                .as_str()
                .ok_or_else(|| engine("inference"))?
                .trim()
                .to_owned())
        }
    }
    pub fn handle(&mut self, method: &str, input: &Value) -> Result<Value> {
        if (method == "synthesize") != (self.kind == "tts") {
            return Err(engine("config"));
        }
        let requested = input["terms"]
            .as_array()
            .map(|v| {
                v.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| engine("invalid"))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        match method {
            "warmup" => {
                self.load_asr(&requested)?;
                Ok(json!({"ready":true}))
            }
            "transcribe" => {
                let samples = pcm_base64(input)?;
                self.load_asr(&requested)?;
                let stream = self.create_stream()?;
                let result = (|| {
                    self.accept(stream, &samples)?;
                    self.finish(stream, true)?;
                    Ok(json!({"text":terms::format(&self.decoded(stream)?,&requested)}))
                })();
                self.destroy_stream(stream);
                result
            }
            "startSession" => {
                if self.streams.len() >= 4 {
                    return Err(engine("busy"));
                }
                self.load_asr(&requested)?;
                let pointer = self.create_stream()?;
                let id = uuid::Uuid::new_v4().to_string();
                self.streams.insert(
                    id.clone(),
                    Stream {
                        pointer,
                        terms: requested,
                        samples: 0,
                    },
                );
                Ok(json!({"id":id}))
            }
            "acceptChunk" => {
                let id = input["id"].as_str().ok_or_else(|| engine("session"))?;
                let samples = pcm_base64(input)?;
                let stream = self.streams.get_mut(id).ok_or_else(|| engine("session"))?;
                stream.samples += samples.len();
                let pointer = stream.pointer;
                if stream.samples > 16000 * 600 {
                    self.destroy_stream(pointer);
                    self.streams.remove(id);
                    return Err(engine("limit"));
                }
                self.accept(pointer, &samples)?;
                Ok(json!({"text":self.decoded(pointer)?}))
            }
            "finishSession" => {
                let id = input["id"].as_str().ok_or_else(|| engine("session"))?;
                let stream = self.streams.remove(id).ok_or_else(|| engine("session"))?;
                let result = (|| {
                    self.finish(stream.pointer, stream.samples > 0)?;
                    let text = terms::format(&self.decoded(stream.pointer)?, &stream.terms);
                    if text.is_empty() {
                        return Err(engine("inference"));
                    }
                    Ok(json!({"text":text}))
                })();
                self.destroy_stream(stream.pointer);
                result
            }
            "cancelSession" => {
                if let Some(stream) = input["id"].as_str().and_then(|id| self.streams.remove(id)) {
                    self.destroy_stream(stream.pointer);
                }
                Ok(json!({"ok":true}))
            }
            "synthesize" => {
                self.synthesize(input["text"].as_str().ok_or_else(|| engine("invalid"))?)
            }
            _ => Err(engine("invalid")),
        }
    }
    fn synthesize(&self, text: &str) -> Result<Value> {
        validate_text(
            text,
            self.model.config["maxTextCodePoints"]
                .as_u64()
                .unwrap_or(400) as usize,
        )?;
        let text = cstring(text)?;
        struct Progress {
            count: usize,
            limit: usize,
            exceeded: bool,
        }
        unsafe extern "C" fn callback(_: *const f32, n: i32, _: f32, arg: *mut c_void) -> i32 {
            let progress = &mut *(arg as *mut Progress);
            if n < 0 {
                progress.exceeded = true;
                return 0;
            }
            progress.count = progress.count.saturating_add(n as usize);
            if progress.count > progress.limit {
                progress.exceeded = true;
                0
            } else {
                1
            }
        }
        unsafe {
            let rate: unsafe extern "C" fn(*const tts::SherpaOnnxOfflineTts) -> i32 =
                self.api.function(b"SherpaOnnxOfflineTtsSampleRate\0")?;
            let speakers: unsafe extern "C" fn(*const tts::SherpaOnnxOfflineTts) -> i32 =
                self.api.function(b"SherpaOnnxOfflineTtsNumSpeakers\0")?;
            let sample_rate = rate(self.tts);
            if !(8000..=48000).contains(&sample_rate) || speakers(self.tts) < 0 {
                return Err(engine("inference"));
            }
            let mut progress = Progress {
                count: 0,
                limit: sample_rate as usize * 45,
                exceeded: false,
            };
            let mut config: tts::SherpaOnnxGenerationConfig = std::mem::zeroed();
            config.speed = 1.0;
            config.sid = 0;
            config.silence_scale = 1.0;
            let generate: unsafe extern "C" fn(
                *const tts::SherpaOnnxOfflineTts,
                *const c_char,
                *const tts::SherpaOnnxGenerationConfig,
                tts::SherpaOnnxGeneratedAudioProgressCallbackWithArg,
                *mut c_void,
            )
                -> *const tts::SherpaOnnxGeneratedAudio = self
                .api
                .function(b"SherpaOnnxOfflineTtsGenerateWithConfig\0")?;
            let destroy: unsafe extern "C" fn(*const tts::SherpaOnnxGeneratedAudio) = self
                .api
                .function(b"SherpaOnnxDestroyOfflineTtsGeneratedAudio\0")?;
            let audio = generate(
                self.tts,
                text.as_ptr(),
                &config,
                Some(callback),
                &mut progress as *mut _ as *mut _,
            );
            if audio.is_null() {
                return Err(engine("inference"));
            }
            let result = if progress.exceeded
                || (*audio).n <= 0
                || (*audio).n as usize > progress.limit
                || (*audio).samples.is_null()
                || (*audio).sample_rate != sample_rate
            {
                Err(engine("limit"))
            } else {
                encode_wav(
                    std::slice::from_raw_parts((*audio).samples, (*audio).n as usize),
                    (*audio).sample_rate,
                )
            };
            let samples_generated = (*audio).n.max(0) as usize;
            destroy(audio);
            let wav = result?;
            use base64::Engine;
            Ok(
                json!({"wav":base64::engine::general_purpose::STANDARD.encode(wav),"sampleRate":sample_rate,"durationMs":samples_generated as f64/sample_rate as f64*1000.0}),
            )
        }
    }
    unsafe fn unload_asr(&mut self) {
        if !self.recognizer.is_null() {
            if let Ok(destroy) = self
                .api
                .function::<unsafe extern "C" fn(*const asr::OnlineRecognizer)>(
                    b"SherpaOnnxDestroyOnlineRecognizer\0",
                )
            {
                destroy(self.recognizer);
            }
            self.recognizer = std::ptr::null();
        }
    }
}
impl Drop for NativeInference {
    fn drop(&mut self) {
        for (_, stream) in self.streams.drain().collect::<Vec<_>>() {
            self.destroy_stream(stream.pointer);
        }
        unsafe {
            self.unload_asr();
            if !self.tts.is_null() {
                if let Ok(destroy) = self
                    .api
                    .function::<unsafe extern "C" fn(*const tts::SherpaOnnxOfflineTts)>(
                        b"SherpaOnnxDestroyOfflineTts\0",
                    )
                {
                    destroy(self.tts);
                }
            }
        }
    }
}
fn cstring(text: &str) -> Result<CString> {
    CString::new(text).map_err(|_| engine("config"))
}
pub fn validate_samples(samples: &[f32]) -> Result<()> {
    if samples.is_empty() || samples.iter().any(|v| !v.is_finite()) {
        return Err(engine("invalid"));
    }
    if samples.len() > 16000 * 600 {
        return Err(engine("limit"));
    }
    Ok(())
}
pub fn validate_text(text: &str, max_codepoints: usize) -> Result<()> {
    if text.trim().is_empty() {
        return Err(engine("invalid"));
    }
    if text.encode_utf16().count() > 400 || text.chars().count() > max_codepoints {
        return Err(engine("limit"));
    }
    Ok(())
}
pub fn decode_pcm(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() > 32_000_000 || bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err(engine("invalid"));
    }
    let samples = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect::<Vec<_>>();
    validate_samples(&samples)?;
    Ok(samples)
}
fn pcm_base64(input: &Value) -> Result<Vec<f32>> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(input["pcm"].as_str().ok_or_else(|| engine("invalid"))?)
        .map_err(|_| engine("invalid"))?;
    decode_pcm(&bytes)
}
pub fn encode_wav(samples: &[f32], rate: i32) -> Result<Vec<u8>> {
    if !(8000..=48000).contains(&rate) || samples.is_empty() || samples.len() > rate as usize * 45 {
        return Err(engine("limit"));
    }
    if samples.iter().any(|v| !v.is_finite()) {
        return Err(engine("inference"));
    }
    let mut wav = Vec::with_capacity(44 + samples.len() * 2);
    wav.extend(b"RIFF");
    wav.extend((36 + samples.len() as u32 * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend((rate as u32).to_le_bytes());
    wav.extend((rate as u32 * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples.len() as u32 * 2).to_le_bytes());
    for sample in samples {
        let sample = sample.clamp(-1.0, 1.0);
        let value =
            (sample as f64 * if sample < 0.0 { 32768.0 } else { 32767.0 } + 0.5).floor() as i16;
        wav.extend(value.to_le_bytes());
    }
    Ok(wav)
}
