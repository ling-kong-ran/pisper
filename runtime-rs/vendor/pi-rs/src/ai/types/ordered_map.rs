//! A tiny insertion-ordered string-keyed map for the classifier wire types:
//! upstream `Record<string, T>` iterates in JS insertion order, and the
//! classifier surfaces are byte-order-sensitive end to end — the System One
//! wire request embeds the questions object, the llama-cpp prompts render
//! questions/criteria in map order, and choice answers echo the
//! probabilities object. The port-wide `BTreeMap` convention (sorted keys)
//! is a *documented* substitution elsewhere; here the order is behavior, so
//! the carrier preserves the input document's order and iterates in it.
//!
//! Serialization matches a JSON map (serde serializes the entries in order;
//! deserialization records the document order). Lookup is linear — the
//! classifier maps are tiny (a handful of questions).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Clone, PartialEq)]
pub struct OrderedMap<V>(Vec<(String, V)>);

impl<V> OrderedMap<V> {
    pub fn new() -> Self {
        OrderedMap(Vec::new())
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, V)>) -> Self {
        OrderedMap(pairs.into_iter().collect())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.0
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    pub fn insert(&mut self, key: impl Into<String>, value: V) {
        let key = key.into();
        match self.0.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, existing)) => *existing = value,
            None => self.0.push((key, value)),
        }
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.0.iter().map(|(key, value)| (key, value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(key, _)| key)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().map(|(_, value)| value)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

impl<V: Default> Default for OrderedMap<V> {
    fn default() -> Self {
        OrderedMap::new()
    }
}

impl<'a, V> IntoIterator for &'a OrderedMap<V> {
    type Item = (&'a String, &'a V);
    type IntoIter = Box<dyn Iterator<Item = (&'a String, &'a V)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.0.iter().map(|(key, value)| (key, value)))
    }
}

impl<V> IntoIterator for OrderedMap<V> {
    type Item = (String, V);
    type IntoIter = std::vec::IntoIter<(String, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<V> FromIterator<(String, V)> for OrderedMap<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        let mut map = OrderedMap::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_map(self.0.iter().map(|(key, value)| (key, value)))
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for OrderedMap<V> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries: Vec<(String, V)> = Vec::<(String, V)>::deserialize(deserializer)?;
        Ok(OrderedMap(entries))
    }
}

impl<V: fmt::Debug> fmt::Debug for OrderedMap<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|(k, v)| (k, v)))
            .finish()
    }
}
