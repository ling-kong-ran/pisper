//! release `services/decision-*.mjs` 的原生移植:决策模型领域。
//! 状态/配置(默认值、白名单校验、apiKey 只写、审批绑定首读推断)与
//! 真实远端协议(typesafe-decisions:POST {model,state,questions},Bearer 认证,
//! 答案逐项验证),错误使用稳定机器可读码 + {error, code} 响应体。
//!
//! 与 release 的诚实接缝:SDK 的重试/退避(3 次、上限 8s)未复刻,单次请求
//! 挂 120s 总时限;客户端断开不取消在途请求。

use axum::{http::StatusCode, Json};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

use crate::ApiError;

const CONFIG_VERSION: u32 = 1;
const MAX_STATE_CHARS: usize = 200_000;
const MAX_QUESTIONS: usize = 32;
const MAX_INSTRUCTIONS_CHARS: usize = 2_000;
const MAX_OPTION_LABEL_CHARS: usize = 200;
const MAX_CHOICE_OPTIONS: usize = 255;
const MIN_SCORE_LEVELS: usize = 2;
const MAX_SCORE_LEVELS: usize = 10;
const MAX_REMOTE_ESTIMATED_TOKENS: u64 = 28_000;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// release shared/decision-provider-catalog.mjs。
/// (defaultBaseUrl, path, defaultModelId, endpointSuffixes)
fn provider_preset(provider: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match provider {
        "typesafe" => Some(("https://api.typesafe.ai", "/v1/systemone", "jev-1.13.0")),
        "openrouter" => {
            Some(("https://openrouter.ai/api", "/alpha/decisions", "typesafe/jev-1.13"))
        }
        "custom" => Some(("", "/alpha/decisions", "typesafe/jev-1.13")),
        _ => None,
    }
}

/// 三个供应商共用同一协议路径特例(release endpointSuffixes)。
const ENDPOINT_SUFFIXES: [&str; 2] = ["/systemone", "/decisions"];

/// release registry models:审批策略仅登记这三组型号。
fn approval_policy_id(provider: &str, model_id: &str) -> Option<&'static str> {
    match (provider, model_id) {
        ("typesafe", "jev-1.13.0")
        | ("openrouter", "typesafe/jev-1.13")
        | ("custom", "typesafe/jev-1.13") => Some("legacy-jev-v1"),
        _ => None,
    }
}

fn remote_failure(code: &'static str) -> ApiError {
    let (message, status) = match code {
        "unsupported_provider" => ("不支持所选决策服务商。", 400),
        "unsupported_capability" => ("所选决策模型不支持此判断能力。", 400),
        "config_missing" => ("尚未配置决策服务 密钥或地址。", 400),
        "auth" => ("决策服务 认证失败，请检查 API 密钥与账户权限。", 401),
        "rate_limited" => ("决策服务 请求过于频繁，请稍后重试。", 429),
        "overloaded" => ("决策服务 暂时过载，请稍后重试。", 502),
        "state_too_large" => ("输入超出所选决策模型的上下文限制。", 413),
        "invalid" => ("决策服务 拒绝了请求，请检查问题定义。", 400),
        "network" => ("无法连接 决策服务。", 502),
        "timeout" => ("决策服务 请求超时。", 502),
        "aborted" => ("请求已取消。", 400),
        "bad_response" => ("决策服务 返回了无法解析的响应。", 502),
        _ => ("无法连接 决策服务。", 502),
    };
    ApiError::new(
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
        code,
        message,
    )
}

fn decision_error(message: impl Into<String>) -> ApiError {
    ApiError::bad_request(message)
}

/// JS `Number(value)` 宽松转换:NaN 之外的字符串/布尔/null 都给数值。
fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Null => Some(0.0),
        Value::Bool(true) => Some(1.0),
        Value::Bool(false) => Some(0.0),
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Some(0.0);
            }
            let (radix, digits) = if let Some(rest) = trimmed.strip_prefix("0x") {
                (16, rest)
            } else if let Some(rest) = trimmed.strip_prefix("0o") {
                (8, rest)
            } else if let Some(rest) = trimmed.strip_prefix("0b") {
                (2, rest)
            } else {
                (10, trimmed)
            };
            if radix != 10 {
                return i128::from_str_radix(digits, radix)
                    .ok()
                    .map(|value| value as f64);
            }
            match trimmed {
                "Infinity" | "+Infinity" => Some(f64::INFINITY),
                "-Infinity" => Some(f64::NEG_INFINITY),
                _ => trimmed.parse::<f64>().ok(),
            }
        }
        // JS 走 ToString:单元素数组透传,空数组 0,其余 NaN。
        Value::Array(items) => match items.as_slice() {
            [] => Some(0.0),
            [one] => js_number(one),
            _ => None,
        },
        Value::Object(_) => None,
    }
}

fn js_safe_integer(value: f64) -> bool {
    value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0
}

// ---------------------------------------------------------------- storage

#[derive(Clone, Debug)]
struct StoredConfig {
    remote: Value,   // 归一化 {provider, baseUrl, modelId, apiKey}
    delegate: Value, // 归一化 {enabled, allowThreshold, verifyActions}
    /// None = 文件未写 approvalBinding 键(每次读取按当前远端推断);
    /// Some(value) = 键存在(value 可能是 Null,对应 release 的畸形绑定)。
    approval_binding_explicit: Option<Value>,
}

impl Default for StoredConfig {
    // derive(Default) 会把 Value 字段置为 Null;release 语义里缺失 = 空对象。
    fn default() -> Self {
        Self {
            remote: json!({}),
            delegate: json!({}),
            approval_binding_explicit: None,
        }
    }
}

fn config_path(data_dir: &str) -> std::path::PathBuf {
    Path::new(data_dir).join("decisions").join("config.json")
}

/// release init + effectiveConfig:version≠1 的文件整体视为不存在;
/// approvalBinding 键存在时按 readApprovalBinding 读(畸形 → Null),
/// 键不存在时回落推断(见 effective_binding)。
fn load_config(state: &crate::AppState) -> StoredConfig {
    let mut stored = StoredConfig::default();
    let Ok(bytes) = std::fs::read(config_path(&state.data_dir)) else {
        return stored;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return stored;
    };
    if value.get("version").and_then(Value::as_u64) != Some(CONFIG_VERSION as u64) {
        return stored;
    }
    if value.get("remote").map(Value::is_object).unwrap_or(false) {
        stored.remote = value["remote"].clone();
    }
    if value.get("delegate").map(Value::is_object).unwrap_or(false) {
        stored.delegate = value["delegate"].clone();
    }
    if value.as_object().unwrap().contains_key("approvalBinding") {
        stored.approval_binding_explicit = Some(read_approval_binding(&value["approvalBinding"]));
    }
    stored
}

fn save_config(state: &crate::AppState, stored: &StoredConfig) -> Result<(), ApiError> {
    let directory = Path::new(&state.data_dir).join("decisions");
    std::fs::create_dir_all(&directory).map_err(|e| ApiError::internal(e.to_string()))?;
    let value = json!({
        "version": CONFIG_VERSION,
        "remote": stored.remote,
        "delegate": stored.delegate,
        "approvalBinding": stored.effective_binding(&stored.remote),
    });
    std::fs::write(
        config_path(&state.data_dir),
        serde_json::to_vec_pretty(&value).map_err(|e| ApiError::internal(e.to_string()))?,
    )
    .map_err(|e| ApiError::internal(e.to_string()))
}

/// release readApprovalBinding:四字段全为字符串才算绑定。
fn read_approval_binding(raw: &Value) -> Value {
    if raw.is_object()
        && ["provider", "modelId", "endpoint", "policyId"]
            .iter()
            .all(|key| raw[key].is_string())
    {
        raw.clone()
    } else {
        Value::Null
    }
}

impl StoredConfig {
    /// release effectiveConfig 的绑定推断:v1 旧文件首次读取时绑定原
    /// 供应商与型号;后续保存带出绑定,切换模型不会重新推断。
    fn effective_binding(&self, remote: &Value) -> Value {
        match &self.approval_binding_explicit {
            Some(value) => value.clone(),
            None => model_approval_binding(remote),
        }
    }
}

/// release normalizeRemoteConfig:类型不对的字段静默回默认值。
fn normalize_remote_config(raw: &Value) -> Result<Value, ApiError> {
    let provider = match raw["provider"].as_str() {
        Some(text) => text.to_string(),
        None => "typesafe".to_string(),
    };
    let preset = provider_preset(&provider).ok_or_else(|| remote_failure("unsupported_provider"))?;
    let base_url = raw["baseUrl"].as_str().map(str::trim).unwrap_or("").to_string();
    let model_id = raw["modelId"].as_str().map(str::trim).unwrap_or("").to_string();
    let model_id = if model_id.is_empty() {
        preset.2.to_string()
    } else {
        model_id
    };
    let api_key = raw["apiKey"].as_str().map(str::trim).unwrap_or("").to_string();
    Ok(json!({
        "provider": provider,
        "baseUrl": base_url,
        "modelId": model_id,
        "apiKey": api_key,
    }))
}

fn effective_remote(stored: &StoredConfig) -> Value {
    normalize_remote_config(&stored.remote).unwrap_or_else(|_| {
        json!({
            "provider": "typesafe",
            "baseUrl": "",
            "modelId": provider_preset("typesafe").unwrap().2,
            "apiKey": "",
        })
    })
}

fn effective_delegate(stored: &StoredConfig) -> Value {
    normalize_delegate_config(&stored.delegate, false).unwrap_or_else(|_| {
        json!({"enabled": false, "allowThreshold": 0.9, "verifyActions": false})
    })
}

/// release normalizeDelegateConfig:strict(用户输入)对显式非法阈值报错,
/// 非 strict(读旧配置)回退默认。
fn normalize_delegate_config(raw: &Value, strict: bool) -> Result<Value, ApiError> {
    let empty = serde_json::Map::new();
    let object = raw.as_object().unwrap_or(&empty);
    let enabled = object.get("enabled").and_then(Value::as_bool) == Some(true);
    let verify_actions = object.get("verifyActions").and_then(Value::as_bool) == Some(true);
    let allow = js_number(object.get("allowThreshold").unwrap_or(&Value::Null));
    let allow_valid = allow
        .map(|value| value.is_finite() && (0.5..=1.0).contains(&value))
        .unwrap_or(false);
    let has_explicit = object.contains_key("allowThreshold");
    if strict && has_explicit && !allow_valid {
        return Err(decision_error("批准阈值必须在 0.5–1 之间。"));
    }
    let allow_threshold = if allow_valid { allow.unwrap() } else { 0.9 };
    Ok(json!({
        "enabled": enabled,
        "allowThreshold": allow_threshold,
        "verifyActions": verify_actions,
    }))
}

/// release modelApprovalBinding:登记型号 + 协议能力 → 绑定或 Null。
fn model_approval_binding(remote: &Value) -> Value {
    let provider = remote["provider"].as_str().unwrap_or("");
    let model_id = remote["modelId"].as_str().unwrap_or("");
    let Some(policy_id) = approval_policy_id(provider, model_id) else {
        return Value::Null;
    };
    // typesafe-decisions 适配器能力:boolean/choice/score + booleanProbability。
    let Ok(endpoint) = resolve_endpoint(remote) else {
        return Value::Null;
    };
    json!({
        "provider": provider,
        "modelId": model_id,
        "endpoint": endpoint,
        "policyId": policy_id,
    })
}

fn same_binding(a: &Value, b: &Value) -> bool {
    a.is_object() && b.is_object() && a == b
}

fn approval_status(stored: &StoredConfig, remote: &Value) -> &'static str {
    let expected = model_approval_binding(remote);
    if expected.is_null() {
        return "model_unverified";
    }
    if same_binding(&expected, &stored.effective_binding(remote)) {
        "ready"
    } else {
        "threshold_required"
    }
}

/// release publicConfig:密钥只回传 hasKey,绝不明文。
fn public_config(stored: &StoredConfig) -> Value {
    let remote = effective_remote(stored);
    let preset = provider_preset(remote["provider"].as_str().unwrap_or("typesafe"));
    let base_url = remote["baseUrl"].as_str().unwrap_or("");
    let base_url = if base_url.is_empty() {
        preset.map(|p| p.0).unwrap_or("")
    } else {
        base_url
    };
    let mut public_remote = remote.clone();
    public_remote["baseUrl"] = json!(base_url);
    public_remote["hasKey"] = json!(!remote["apiKey"].as_str().unwrap_or("").is_empty());
    public_remote
        .as_object_mut()
        .map(|object| object.remove("apiKey"));
    json!({
        "remote": public_remote,
        "delegate": effective_delegate(stored),
        "approval": {"status": approval_status(stored, &remote)},
    })
}

/// release resolveDecisionEndpoint:https、无凭据/查询/锚点,路径后缀识别。
/// path 后缀区分大小写;endpointSuffixes 不区分。
fn resolve_endpoint(remote: &Value) -> Result<String, ApiError> {
    let provider = remote["provider"].as_str().unwrap_or("typesafe");
    let Some((default_base, path, _)) = provider_preset(provider) else {
        return Err(remote_failure("unsupported_provider"));
    };
    // release `config.baseUrl || preset.defaultBaseUrl`:空串回落默认。
    let base = remote["baseUrl"].as_str().unwrap_or(default_base);
    let base = if base.is_empty() { default_base } else { base };
    let Ok(url) = reqwest::Url::parse(base) else {
        return Err(remote_failure("config_missing"));
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(remote_failure("config_missing"));
    }
    let clean = base.trim_end_matches('/').to_string();
    let suffix_match = clean.ends_with(path)
        || ENDPOINT_SUFFIXES
            .iter()
            .any(|suffix| clean.to_lowercase().ends_with(&suffix.to_lowercase()));
    if suffix_match {
        Ok(clean)
    } else {
        Ok(format!("{clean}{path}"))
    }
}

// ------------------------------------------------------------ decide input

#[derive(Debug, Clone)]
struct DecisionQuestion {
    id: String,
    /// boolean / choice / score(内部名称)。
    kind: &'static str,
    instructions: String,
    options: Vec<String>,
    /// boolean 问题可选 criteria {true?, false?}(修剪后的非空字符串)。
    criteria: Option<Value>,
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// release normalizeModelInput(公共协议用 noul 命名,内部转 boolean)。
fn normalize_decide_input(body: &Value) -> Result<(String, Vec<DecisionQuestion>), ApiError> {
    if !body.is_object() {
        return Err(decision_error("请求必须是 JSON 对象。"));
    }
    let state = match &body["state"] {
        Value::String(text) => text.clone(),
        Value::Null => return Err(decision_error("state 不能为空。")),
        other => serde_json::to_string(other).map_err(|_| decision_error("state 无法序列化为 JSON。"))?,
    };
    if state.trim().is_empty() {
        return Err(decision_error("state 不能为空。"));
    }
    if utf16_len(&state) > MAX_STATE_CHARS {
        return Err(remote_failure("state_too_large"));
    }
    let Some(questions) = body["questions"].as_array() else {
        return Err(decision_error("questions 必须是非空数组。"));
    };
    if questions.is_empty() {
        return Err(decision_error("questions 必须是非空数组。"));
    }
    if questions.len() > MAX_QUESTIONS {
        return Err(decision_error(format!("一次最多 {MAX_QUESTIONS} 个问题。")));
    }
    let id_pattern = |id: &str| -> bool {
        let bytes = id.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 64
            && bytes[0].is_ascii_alphabetic()
            && bytes[1..]
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
    };
    let mut normalized: Vec<DecisionQuestion> = Vec::new();
    let mut used_ids = std::collections::HashSet::new();
    for (index, raw) in questions.iter().enumerate() {
        if !raw.is_object() {
            return Err(decision_error(format!("第 {} 个问题不是对象。", index + 1)));
        }
        let id = raw["id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("q{index}"));
        if !id_pattern(&id) {
            return Err(decision_error(format!(
                "问题 id「{id}」只能包含字母、数字、下划线和连字符，且以字母开头。"
            )));
        }
        if !used_ids.insert(id.clone()) {
            return Err(decision_error(format!("问题 id「{id}」重复。")));
        }
        let instructions = raw["instructions"].as_str().unwrap_or("").trim().to_string();
        if instructions.is_empty() {
            return Err(decision_error(format!("问题「{id}」缺少 instructions。")));
        }
        if utf16_len(&instructions) > MAX_INSTRUCTIONS_CHARS {
            return Err(decision_error(format!("问题「{id}」的 instructions 过长。")));
        }
        let kind = raw["type"].as_str().unwrap_or("");
        match kind {
            "noul" => {
                // release:criteria 非对象整体忽略;true/false 键只留非空字符串。
                let mut criteria = serde_json::Map::new();
                if let Some(map) = raw["criteria"].as_object() {
                    for key in ["true", "false"] {
                        if let Some(text) = map.get(key).and_then(Value::as_str) {
                            let text = text.trim();
                            if !text.is_empty() {
                                criteria.insert(key.to_string(), json!(text));
                            }
                        }
                    }
                }
                let criteria = if criteria.is_empty() {
                    None
                } else {
                    Some(Value::Object(criteria))
                };
                normalized.push(DecisionQuestion {
                    id,
                    kind: "boolean",
                    instructions,
                    options: Vec::new(),
                    criteria,
                });
            }
            // boolean 是内部名称,v1 只接受原有 noul 名称,避免悄悄扩展公共协议。
            "boolean" => return Err(remote_failure("invalid")),
            "choice" | "score" => {
                let Some(options) = raw["options"].as_array() else {
                    return Err(decision_error(format!("问题「{id}」缺少 options 数组。")));
                };
                let mut labels = Vec::new();
                for option in options {
                    let Some(text) = option.as_str() else {
                        return Err(decision_error(format!("问题「{id}」的选项必须是字符串。")));
                    };
                    let label = text.trim().to_string();
                    if label.is_empty() {
                        return Err(decision_error(format!("问题「{id}」存在空选项。")));
                    }
                    if labels.contains(&label) {
                        return Err(decision_error(format!("问题「{id}」存在重复选项。")));
                    }
                    if utf16_len(&label) > MAX_OPTION_LABEL_CHARS {
                        return Err(decision_error(format!("问题「{id}」的选项标签过长。")));
                    }
                    labels.push(label);
                }
                if kind == "choice" && !(2..=MAX_CHOICE_OPTIONS).contains(&labels.len()) {
                    return Err(decision_error(format!(
                        "choice 问题「{id}」需要 2–{MAX_CHOICE_OPTIONS} 个选项。"
                    )));
                }
                if kind == "score" && !(MIN_SCORE_LEVELS..=MAX_SCORE_LEVELS).contains(&labels.len())
                {
                    return Err(decision_error(format!(
                        "score 问题「{id}」需要 {MIN_SCORE_LEVELS}–{MAX_SCORE_LEVELS} 个有序档位。"
                    )));
                }
                normalized.push(DecisionQuestion {
                    id,
                    kind: if kind == "choice" { "choice" } else { "score" },
                    instructions,
                    options: labels,
                    criteria: None,
                });
            }
            _ => {
                return Err(decision_error(format!(
                    "问题「{id}」的 type 必须是 boolean / choice / score。"
                )))
            }
        }
    }
    Ok((state, normalized))
}

/// release estimateTokens:中文按 1.05 token/字,其余按 0.5 token/字符。
fn estimate_tokens(text: &str) -> u64 {
    let mut cjk = 0u64;
    let mut other = 0u64;
    for character in text.chars() {
        let code = character as u32;
        if (0x2e80..=0x9fff).contains(&code)
            || (0xf900..=0xfaff).contains(&code)
            || (0xff00..=0xffef).contains(&code)
            || (0x3000..=0x303f).contains(&code)
        {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    (cjk as f64 * 1.05 + other as f64 * 0.5).ceil() as u64
}

/// release assertWithinRemoteLimit:state + 最长问题定义的超限估算。
fn assert_within_remote_limit(state: &str, questions: &[DecisionQuestion]) -> Result<(), ApiError> {
    let mut longest = 0u64;
    for question in questions {
        let options_text = question.options.join("");
        let criteria_text = question
            .criteria
            .as_ref()
            .and_then(|value| value.as_object())
            .map(|map| map.values().filter_map(Value::as_str).collect::<String>())
            .unwrap_or_default();
        longest = longest.max(estimate_tokens(&format!(
            "{}{options_text}{criteria_text}",
            question.instructions
        )));
    }
    if estimate_tokens(state) + longest > MAX_REMOTE_ESTIMATED_TOKENS {
        return Err(remote_failure("state_too_large"));
    }
    Ok(())
}

/// release toRemoteQuestions:内部 boolean → 协议 noul 命名;
/// choice 的 criteria 是 map<option, null>(null 表示该项无需说明);
/// score 的 criteria 是档位元组。
fn to_remote_questions(questions: &[DecisionQuestion]) -> Value {
    let mut remote = serde_json::Map::new();
    for question in questions {
        let entry = match question.kind {
            "boolean" => {
                let mut entry = json!({"type": "noul", "instructions": question.instructions});
                if let Some(criteria) = &question.criteria {
                    entry["criteria"] = criteria.clone();
                }
                entry
            }
            "choice" => json!({
                "type": "choice",
                "instructions": question.instructions,
                "criteria": question
                    .options
                    .iter()
                    .map(|option| (option.clone(), Value::Null))
                    .collect::<std::collections::BTreeMap<String, Value>>(),
            }),
            _ => json!({
                "type": "score",
                "instructions": question.instructions,
                "criteria": question.options,
            }),
        };
        remote.insert(question.id.clone(), entry);
    }
    Value::Object(remote)
}

fn probability(value: &Value) -> Result<f64, ApiError> {
    match value.as_f64() {
        Some(number) if number.is_finite() && (0.0..=1.0).contains(&number) => Ok(number),
        _ => Err(remote_failure("bad_response")),
    }
}

/// release normalizeRemoteResponse 的 probabilities:键必须命中标签表
/// (score 用档位下标字符串),值必须是 [0,1] 概率;缺失 → 空映射。
fn normalized_probabilities(
    value: &Value,
    labels: &[String],
) -> Result<std::collections::BTreeMap<String, Value>, ApiError> {
    let mut result = std::collections::BTreeMap::new();
    if let Some(map) = value.as_object() {
        for (key, item) in map {
            if !labels.iter().any(|label| label == key) {
                return Err(remote_failure("bad_response"));
            }
            result.insert(key.clone(), json!(probability(item)?));
        }
    }
    Ok(result)
}

/// release normalizeRemoteResponse:答案必须逐项匹配本次问题,异常概率
/// 不得修正;usage/model 归一化为公共形状。
fn normalize_remote_response(
    payload: &Value,
    questions: &[DecisionQuestion],
) -> Result<(Value, Option<String>, Value), ApiError> {
    let invalid = || remote_failure("bad_response");
    let Some(answers) = payload["answers"].as_object() else {
        return Err(invalid());
    };
    if answers.len() != questions.len()
        || !questions
            .iter()
            .all(|question| answers.contains_key(&question.id))
    {
        return Err(invalid());
    }
    let mut public_answers = serde_json::Map::new();
    for question in questions {
        let answer = &answers[&question.id];
        if !answer.is_object() {
            return Err(invalid());
        }
        let kinds = ["noul", "choice", "score"]
            .iter()
            .filter(|kind| answer.get(**kind).is_some())
            .count();
        if kinds != 1 {
            return Err(invalid());
        }
        let expected_type = match question.kind {
            "boolean" => "noul",
            other => other,
        };
        if answer.get("type").map(Value::is_string).unwrap_or(false)
            && answer["type"].as_str() != Some(expected_type)
        {
            return Err(invalid());
        }
        match question.kind {
            "boolean" => {
                let noul = probability(&answer["noul"])?;
                public_answers.insert(question.id.clone(), json!({"type": "noul", "noul": noul}));
            }
            "choice" => {
                let choice = answer["choice"].as_str().unwrap_or("");
                if choice.is_empty() || !question.options.iter().any(|option| option == choice) {
                    return Err(invalid());
                }
                let confidence = if answer["confidence"].is_null() {
                    Value::Null
                } else {
                    json!(probability(&answer["confidence"])?)
                };
                let probabilities = normalized_probabilities(&answer["probabilities"], &question.options)?;
                public_answers.insert(
                    question.id.clone(),
                    json!({
                        "type": "choice",
                        "choice": choice,
                        "confidence": confidence,
                        "probabilities": probabilities,
                    }),
                );
            }
            _ => {
                // score 是 0 基档位下标,上限是 档位数-1。
                let score = match answer["score"].as_f64() {
                    Some(value) if value.is_finite() && (0.0..=(question.options.len() as f64 - 1.0)).contains(&value) => {
                        value
                    }
                    _ => return Err(invalid()),
                };
                let confidence = if answer["confidence"].is_null() {
                    Value::Null
                } else {
                    json!(probability(&answer["confidence"])?)
                };
                if !answer["legend"].is_null() && !answer["legend"].is_object() {
                    return Err(invalid());
                }
                let legend = if answer["legend"].is_object() {
                    answer["legend"].clone()
                } else {
                    Value::Null
                };
                let index_labels: Vec<String> = (0..question.options.len())
                    .map(|index| index.to_string())
                    .collect();
                let probabilities = normalized_probabilities(&answer["probabilities"], &index_labels)?;
                public_answers.insert(
                    question.id.clone(),
                    json!({
                        "type": "score",
                        "score": score,
                        "legend": legend,
                        "confidence": confidence,
                        "probabilities": probabilities,
                    }),
                );
            }
        }
    }
    let model = payload["model"].as_str().map(str::to_string);
    let usage = payload.get("usage").filter(|usage| usage.is_object());
    let raw_tokens = usage
        .map(|usage| {
            usage
                .get("input_tokens")
                .filter(|value| !value.is_null())
                .or_else(|| usage.get("inputTokens").filter(|value| !value.is_null()))
                .cloned()
                .unwrap_or(json!(0))
        })
        .unwrap_or(json!(0));
    let input_tokens = js_number(&raw_tokens)
        .filter(|value| js_safe_integer(*value) && *value >= 0.0)
        .ok_or_else(invalid)? as u64;
    let cost = usage
        .map(|usage| usage.get("cost").cloned().unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    let cost_usd = match &cost {
        Value::Null => Value::Null,
        value => {
            let number = js_number(value).ok_or_else(invalid)?;
            if !number.is_finite() || number < 0.0 {
                return Err(invalid());
            }
            json!(number)
        }
    };
    Ok((
        Value::Object(public_answers),
        model,
        json!({"inputTokens": input_tokens, "costUsd": cost_usd}),
    ))
}

/// 真实远端调用:POST {model, state, questions},Bearer 认证,1MiB 上限。
async fn request_remote(
    remote: &Value,
    state: &str,
    questions: &Value,
) -> Result<Value, ApiError> {
    let endpoint = resolve_endpoint(remote)?;
    let api_key = remote["apiKey"].as_str().unwrap_or("").trim().to_string();
    let model_id = remote["modelId"].as_str().unwrap_or("").to_string();
    if api_key.is_empty() || model_id.trim().is_empty() {
        return Err(remote_failure("config_missing"));
    }
    let client = crate::desktop_ops::http_client();
    let response = client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&json!({"model": model_id, "state": state, "questions": questions}))
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                remote_failure("timeout")
            } else {
                remote_failure("network")
            }
        })?;
    let status = response.status().as_u16();
    if ![200, 201].contains(&status) {
        // release 错误映射:不转发上游正文,稳定错误码离开本边界。
        return Err(match status {
            401..=403 => remote_failure("auth"),
            413 => remote_failure("state_too_large"),
            400 | 422 => remote_failure("invalid"),
            429 => remote_failure("rate_limited"),
            status if status >= 500 => remote_failure("overloaded"),
            _ => remote_failure("bad_response"),
        });
    }
    let declared = response
        .content_length()
        .map(|length| length as usize)
        .unwrap_or(0);
    if declared > MAX_RESPONSE_BYTES {
        return Err(remote_failure("bad_response"));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| remote_failure("bad_response"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(remote_failure("bad_response"));
    }
    serde_json::from_slice(&bytes).map_err(|_| remote_failure("bad_response"))
}

async fn request_model(
    remote: &Value,
    state_text: &str,
    questions: &[DecisionQuestion],
    questions_payload: &Value,
) -> Result<(Value, Option<String>, Value), ApiError> {
    assert_within_remote_limit(state_text, questions)?;
    let payload = request_remote(remote, state_text, questions_payload).await?;
    normalize_remote_response(&payload, questions)
}

// -------------------------------------------------------------- HTTP 界面

/// release GET /api/decisions/status:{config: publicConfig()}。
pub async fn status(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
) -> Json<Value> {
    let stored = load_config(&state);
    Json(json!({"config": public_config(&stored)}))
}

fn fields_whitelist(value: &Value, allowed: &[&str], message: &str) -> Result<(), ApiError> {
    if !value.is_object() {
        return Err(decision_error(message));
    }
    for key in value.as_object().unwrap().keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(decision_error(message));
        }
    }
    Ok(())
}

/// release saveConfig 的纯逻辑部分:白名单 + 宽松类型归一 + provider 切换
/// 重置型号 + 阈值变更重置审批绑定。拆出便于单测。
fn apply_config_patch(stored: &StoredConfig, patch: &Value) -> Result<StoredConfig, ApiError> {
    fields_whitelist(
        patch,
        &["remote", "delegate"],
        "Invalid decisions request.",
    )?;
    let mut remote = effective_remote(stored);
    let mut delegate = effective_delegate(stored);
    let mut binding_override: Option<Value> = None;
    if let Some(patch_remote) = patch.get("remote") {
        fields_whitelist(
            patch_remote,
            &["provider", "baseUrl", "modelId", "apiKey"],
            "Invalid decisions request.",
        )?;
        // release:raw 合并后再归一化;provider 切换且未显式给 modelId 时
        // 重置为新 provider 的默认模型,避免把上一家的模型 ID 发给新接口。
        let switched_without_model = patch_remote.get("provider").is_some()
            && patch_remote.get("provider") != Some(&remote["provider"])
            && !patch_remote.as_object().unwrap().contains_key("modelId");
        let mut merged = remote.clone();
        for key in ["provider", "baseUrl", "modelId", "apiKey"] {
            if let Some(value) = patch_remote.get(key) {
                merged[key] = value.clone();
            }
        }
        if switched_without_model {
            merged["modelId"] = json!("");
        }
        remote = normalize_remote_config(&merged)?;
        // 空字符串表示「保持现有密钥」,避免前端回显时清空;null 表示清除。
        match patch_remote.get("apiKey") {
            Some(Value::String(text)) if text.is_empty() => {
                remote["apiKey"] = json!(stored.remote["apiKey"].as_str().unwrap_or(""));
            }
            _ => {}
        }
    }
    if let Some(patch_delegate) = patch.get("delegate") {
        fields_whitelist(
            patch_delegate,
            &["enabled", "allowThreshold", "verifyActions"],
            "Invalid decisions request.",
        )?;
        let mut merged = delegate.clone();
        for key in ["enabled", "allowThreshold", "verifyActions"] {
            if let Some(value) = patch_delegate.get(key) {
                merged[key] = value.clone();
            }
        }
        delegate = normalize_delegate_config(&merged, true)?;
        if patch_delegate.as_object().unwrap().contains_key("allowThreshold") {
            binding_override = Some(model_approval_binding(&remote));
        }
    }
    let mut updated = StoredConfig {
        remote,
        delegate,
        approval_binding_explicit: match binding_override {
            Some(value) => Some(value),
            None => stored.effective_binding(&stored.remote).into(),
        },
    };
    if updated.approval_binding_explicit.is_none() {
        updated.approval_binding_explicit = Some(updated.effective_binding(&updated.remote));
    }
    Ok(updated)
}

/// release PUT /api/decisions/config。
pub async fn update_config(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
    Json(patch): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let stored = load_config(&state);
    let updated = apply_config_patch(&stored, &patch)?;
    save_config(&state, &updated)?;
    Ok(Json(json!({"config": public_config(&updated)})))
}

/// release POST /api/decisions/test:最小判断的连通性测试。
pub async fn test_connection(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
) -> Result<Json<Value>, ApiError> {
    let stored = load_config(&state);
    let remote = effective_remote(&stored);
    let questions = vec![DecisionQuestion {
        id: "ping".to_string(),
        kind: "boolean",
        instructions: "Is this a connectivity test?".to_string(),
        options: Vec::new(),
        criteria: None,
    }];
    let payload = to_remote_questions(&questions);
    let (_, model, usage) = request_model(&remote, "The setup works.", &questions, &payload).await?;
    Ok(Json(json!({
        "backend": "remote",
        "ok": true,
        "model": model,
        "usage": usage,
    })))
}

/// release POST /api/decisions/decide:类型化问题 → 类型化答案 + 概率。
pub async fn decide(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    fields_whitelist(&body, &["state", "questions"], "Invalid decisions request.")?;
    let stored = load_config(&state);
    let remote = effective_remote(&stored);
    let (state_text, questions) = normalize_decide_input(&body)?;
    let payload = to_remote_questions(&questions);
    let (answers, model, usage) =
        request_model(&remote, &state_text, &questions, &payload).await?;
    Ok(Json(json!({
        "backend": "remote",
        "model": model,
        "usage": usage,
        "answers": answers,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn estimate_tokens_matches_release_curve() {
        assert_eq!(estimate_tokens("中文四字"), 5); // 4 * 1.05 → ceil 5
        assert_eq!(estimate_tokens("abcdefgh"), 4); // 8 * 0.5
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn normalize_rejects_public_boolean_and_bad_ids() {
        let body = json!({"state": "s", "questions": [{"type": "boolean", "instructions": "x"}]});
        assert_eq!(normalize_decide_input(&body).unwrap_err().code, "invalid");
        let body = json!({"state": "s", "questions": [{"type": "2choice", "instructions": "x", "options": ["a", "b"]}]});
        assert!(normalize_decide_input(&body).is_err());
    }

    #[test]
    fn normalize_keeps_boolean_criteria_and_rejects_dup_options() {
        let body = json!({"state": "s", "questions": [
            {"id": "a", "type": "noul", "instructions": "x", "criteria": {"true": " yes ", "false": "", "other": 1}},
            {"id": "b", "type": "choice", "instructions": "y", "options": ["a", "a"]}
        ]});
        let error = normalize_decide_input(&body).unwrap_err();
        assert_eq!(error.message, "问题「b」存在重复选项。");
        let body = json!({"state": "s", "questions": [
            {"id": "a", "type": "noul", "instructions": "x", "criteria": {"true": " yes ", "false": ""}}
        ]});
        let (state, questions) = normalize_decide_input(&body).unwrap();
        assert_eq!(state, "s");
        assert_eq!(questions[0].criteria.as_ref().unwrap(), &json!({"true": "yes"}));
    }

    #[test]
    fn remote_questions_wire_shape() {
        let (_, questions) = normalize_decide_input(&json!({"state": "s", "questions": [
            {"id": "a", "type": "noul", "instructions": "x", "criteria": {"true": "t"}},
            {"id": "b", "type": "choice", "instructions": "y", "options": ["甲", "乙"]},
            {"id": "c", "type": "score", "instructions": "z", "options": ["低", "中", "高"]}
        ]})).unwrap();
        let payload = to_remote_questions(&questions);
        assert_eq!(payload["a"], json!({"type": "noul", "instructions": "x", "criteria": {"true": "t"}}));
        assert_eq!(payload["b"]["criteria"], json!({"甲": null, "乙": null}));
        assert_eq!(payload["c"]["criteria"], json!(["低", "中", "高"]));
        // 无 criteria 的 noul 不写 criteria 键。
        let (_, questions) = normalize_decide_input(&json!({"state": "s", "questions": [
            {"id": "a", "type": "noul", "instructions": "x"}
        ]})).unwrap();
        assert_eq!(to_remote_questions(&questions)["a"], json!({"type": "noul", "instructions": "x"}));
    }

    #[test]
    fn response_normalization_score_and_usage() {
        let (_, questions) = normalize_decide_input(&json!({"state": "s", "questions": [
            {"id": "s1", "type": "score", "instructions": "rate", "options": ["低", "中", "高"]}
        ]})).unwrap();
        let payload = json!({
            "answers": {"s1": {"type": "score", "score": 2, "legend": {"x": 1}, "probabilities": {"0": 0.1, "2": 0.9}}},
            "model": "jev-1.13.0",
            "usage": {"input_tokens": 42, "cost": 0.5}
        });
        let (answers, model, usage) = normalize_remote_response(&payload, &questions).unwrap();
        assert_eq!(model.as_deref(), Some("jev-1.13.0"));
        assert_eq!(usage, json!({"inputTokens": 42, "costUsd": 0.5}));
        assert_eq!(answers["s1"]["score"], json!(2.0));
        assert_eq!(answers["s1"]["probabilities"], json!({"0": 0.1, "2": 0.9}));
        // score 超过 档位数-1 → bad_response;概率键不在下标表 → bad_response。
        let bad = json!({"answers": {"s1": {"type": "score", "score": 3}}});
        assert!(normalize_remote_response(&bad, &questions).is_err());
        let bad = json!({"answers": {"s1": {"type": "score", "score": 1, "probabilities": {"高": 0.9}}}});
        assert!(normalize_remote_response(&bad, &questions).is_err());
    }

    #[test]
    fn endpoint_resolution_matches_release() {
        let resolve = |provider: &str, base: &str| {
            resolve_endpoint(&json!({"provider": provider, "baseUrl": base})).map_err(|e| e.code)
        };
        assert_eq!(resolve("typesafe", "").unwrap(), "https://api.typesafe.ai/v1/systemone");
        assert_eq!(resolve("typesafe", "https://gw.example.com").unwrap(), "https://gw.example.com/v1/systemone");
        assert_eq!(resolve("custom", "https://relay.example.com/decisions").unwrap(), "https://relay.example.com/decisions");
        assert_eq!(resolve("openrouter", "https://openrouter.ai/api").unwrap(), "https://openrouter.ai/api/alpha/decisions");
        assert_eq!(resolve("custom", "https://x.example.com/?a=1"), Err("config_missing"));
        assert_eq!(resolve("custom", ""), Err("config_missing"));
        assert_eq!(resolve("nope", "https://x.example.com"), Err("unsupported_provider"));
    }

    #[test]
    fn config_patch_provider_switch_and_key_semantics() {
        let stored = StoredConfig {
            remote: json!({"provider": "typesafe", "baseUrl": "", "modelId": "custom-model", "apiKey": "k1"}),
            delegate: json!({"enabled": false, "allowThreshold": 0.9, "verifyActions": false}),
            approval_binding_explicit: None,
        };
        // 切换 provider 且未给 modelId → 重置为新 provider 默认;'' 保留密钥。
        let updated = apply_config_patch(&stored, &json!({"remote": {"provider": "openrouter", "apiKey": ""}})).unwrap();
        assert_eq!(updated.remote["provider"], json!("openrouter"));
        assert_eq!(updated.remote["modelId"], json!("typesafe/jev-1.13"));
        assert_eq!(updated.remote["apiKey"], json!("k1"));
        // null 清除密钥;显式 modelId 保留。
        let updated = apply_config_patch(&stored, &json!({"remote": {"apiKey": null, "modelId": "m2"}})).unwrap();
        assert_eq!(updated.remote["apiKey"], json!(""));
        assert_eq!(updated.remote["modelId"], json!("m2"));
        // 非法阈值(strict)报错;阈值变更重置绑定(按当前远端型号推断)。
        let bad = apply_config_patch(&stored, &json!({"delegate": {"allowThreshold": 0.2}}));
        assert_eq!(bad.unwrap_err().message, "批准阈值必须在 0.5–1 之间。");
        let switched = apply_config_patch(&stored, &json!({"remote": {"provider": "openrouter"}})).unwrap();
        let updated = apply_config_patch(&switched, &json!({"delegate": {"allowThreshold": 0.8}})).unwrap();
        assert_eq!(updated.delegate["allowThreshold"], json!(0.8));
        let binding = updated.approval_binding_explicit.unwrap();
        assert_eq!(binding["provider"], json!("openrouter"));
        assert_eq!(binding["policyId"], json!("legacy-jev-v1"));
    }

    #[test]
    fn approval_status_flow() {
        let mut stored = StoredConfig::default();
        let remote = effective_remote(&stored);
        // 首读推断:默认型号可解析 → ready。
        assert_eq!(approval_status(&stored, &remote), "ready");
        // 键存在但为 Null(畸形)→ 与期望不一致 → threshold_required。
        stored.approval_binding_explicit = Some(Value::Null);
        assert_eq!(approval_status(&stored, &remote), "threshold_required");
        // 未知型号 → model_unverified。
        stored.approval_binding_explicit = None;
        let remote = json!({"provider": "typesafe", "baseUrl": "", "modelId": "other", "apiKey": ""});
        assert_eq!(approval_status(&stored, &remote), "model_unverified");
    }

    #[test]
    fn public_config_default_satisfies_frontend_parser() {
        // 回归:default StoredConfig 曾因 Value::Null 恐慌(用户报告的
        // 「服务器响应格式异常」直接来源)。形状须满足 parseDecisionsStatus。
        let stored = StoredConfig::default();
        let config = public_config(&stored);
        let remote = &config["remote"];
        assert_eq!(remote["provider"], json!("typesafe"));
        assert_eq!(remote["baseUrl"], json!("https://api.typesafe.ai"));
        assert_eq!(remote["modelId"], json!("jev-1.13.0"));
        assert_eq!(remote["hasKey"], json!(false));
        assert!(remote.get("apiKey").is_none());
        assert_eq!(config["delegate"], json!({"enabled": false, "allowThreshold": 0.9, "verifyActions": false}));
        assert_eq!(config["approval"]["status"], json!("ready"));
    }

    #[test]
    fn state_limit_uses_utf16_units() {
        let question = json!({"type": "noul", "instructions": "x"});
        let body = json!({"state": "中".repeat(200_001), "questions": [question.clone()]});
        match normalize_decide_input(&body) {
            Err(error) => assert_eq!(error.code, "state_too_large"),
            Ok(_) => panic!("expected state_too_large"),
        }
        let body = json!({"state": "中".repeat(200_000), "questions": [question]});
        assert!(normalize_decide_input(&body).is_ok());
    }
}
