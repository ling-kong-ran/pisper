//! 与 release 的 local-embedding.mjs 保持相同的离线特征与评分。
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

pub fn normalize_key(value: &str, limit: usize) -> String {
    let clean: String = value.replace('\0', "").trim().chars().take(limit).collect();
    let normalized = clean.nfkc().collect::<String>().to_lowercase();
    normalized
        .split(|c: char| !c.is_alphanumeric())
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

pub fn topic_identity(topic: &str, title: &str) -> String {
    let topic = normalize_key(topic, 180);
    let title = normalize_key(title, 140);
    if topic.is_empty() {
        return format!("title.{title}");
    }
    let parts: Vec<_> = topic.split('.').collect();
    if parts.len() < 2
        || [
            "architecture",
            "config",
            "configuration",
            "general",
            "memory",
            "project",
            "settings",
            "user",
        ]
        .contains(parts.last().unwrap_or(&""))
    {
        format!("{topic}.{title}")
    } else {
        topic
    }
}

fn tokenize(value: &str) -> Vec<String> {
    let normalized = value.nfkc().collect::<String>().to_lowercase();
    let mut features = Vec::new();
    for token in normalized.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if token.is_empty() {
            continue;
        }
        let characters: Vec<_> = token.chars().collect();
        if characters.iter().all(char::is_ascii) {
            features.push(token.to_owned());
            features.extend(characters.windows(3).map(|w| w.iter().collect()));
        } else {
            features.extend(characters.iter().map(char::to_string));
            features.extend(characters.windows(2).map(|w| w.iter().collect()));
            features.extend(characters.windows(3).map(|w| w.iter().collect()));
        }
    }
    features
}

pub fn local_embedding(value: &str) -> Vec<f32> {
    let mut vector = vec![0_f32; 384];
    for token in tokenize(value) {
        let mut hash = 2166136261_u32;
        for character in token.chars() {
            hash = (hash ^ character as u32).wrapping_mul(16777619);
        }
        vector[hash as usize % 384] += if hash & 0x80000000 == 0 { 1.0 } else { -1.0 };
    }
    let magnitude: f64 = vector.iter().map(|n| f64::from(*n) * f64::from(*n)).sum();
    if magnitude > 0.0 {
        for number in &mut vector {
            *number = (f64::from(*number) / magnitude.sqrt()) as f32;
        }
    }
    vector
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum()
}

pub fn keyword_overlap(query: &str, text: &str) -> f64 {
    let query: HashSet<_> = tokenize(query).into_iter().collect();
    if query.is_empty() {
        return 0.0;
    }
    let text: HashSet<_> = tokenize(text).into_iter().collect();
    query.intersection(&text).count() as f64 / query.len() as f64
}

pub fn should_retrieve(query: &str) -> bool {
    let query = query.trim().to_lowercase();
    [
        "之前",
        "以前",
        "上次",
        "还记得",
        "记忆",
        "偏好",
        "习惯",
        "约定",
        "决定过",
        "继续上次",
        "按我的",
        "我的默认",
        "remember",
        "memory",
        "previous",
        "earlier",
        "last time",
        "my preference",
        "we decided",
        "continue where",
    ]
    .iter()
    .any(|word| query.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_features_are_normalized_and_distinguish_unrelated_text() {
        let a = local_embedding("使用 Rust 编译原生服务");
        assert_eq!(a.len(), 384);
        // 固定合成短句由 release local-embedding.mjs 实际计算，约束跨语言哈希和 Float32 舍入。
        let indices: Vec<_> = a
            .iter()
            .enumerate()
            .filter(|(_, value)| **value != 0.0)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            indices,
            vec![
                23, 35, 46, 87, 104, 106, 124, 146, 153, 169, 174, 206, 216, 251, 296, 297, 306,
                341, 345, 359, 364
            ]
        );
        assert_eq!(a[23], 0.2182178944349289_f32);
        assert_eq!(a[35], -0.2182178944349289_f32);
        assert_eq!(keyword_overlap("Rust 编译", "使用 Rust 编译原生服务"), 1.0);
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
        assert!(
            cosine_similarity(&a, &local_embedding("Rust 原生服务编译"))
                > cosine_similarity(&a, &local_embedding("香蕉水果"))
        );
        assert_eq!(
            topic_identity("project.architecture", "Use Rust"),
            "project.architecture.use.rust"
        );
        assert_eq!(
            topic_identity("runtime.language", "Use Rust"),
            "runtime.language"
        );
    }
}
