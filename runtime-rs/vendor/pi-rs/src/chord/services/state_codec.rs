//! Stateful operation codecs for every replicated state in one service
//! subscription. Port of `packages/chord/src/services/state-codec.ts`
//! (upstream sha256
//! `4bebb2ffc7ed0ffd62933460d5a4a5733e3d7647294476c475cb9431f121f904`).
//!
//! ONE PAIR PER INDEPENDENT STATE STREAM (delta/index.ts:1517-1523): the
//! registries below key codecs by (instance address, member) so each state
//! gets its own encoder/decoder, reset on snapshots/replacements/unavailable
//! updates and removed with their instance on close.

use std::collections::HashMap;

use crate::chord::delta::{decoder, encoder, Decoder, Encoder};
use crate::chord::types::{
    ServiceInstanceAddress, ServiceInstanceSnapshot, ServiceMemberSnapshot, ServiceProviderUpdate,
    ServiceSubscriptionSnapshot, WireServiceInstanceSnapshot, WireServiceMemberSnapshot,
    WireServiceProviderUpdate, WireServiceSubscriptionSnapshot,
};

use super::errors::ChordError;

#[derive(Debug, Default)]
struct StateCodecRegistry<C> {
    entries: HashMap<String, CodecEntry<C>>,
}

#[derive(Debug)]
struct CodecEntry<C> {
    instance: Option<ServiceInstanceAddress>,
    codec: C,
}

impl<C> StateCodecRegistry<C> {
    fn new() -> Self {
        StateCodecRegistry {
            entries: HashMap::new(),
        }
    }

    fn reset(&mut self) {
        self.entries.clear();
    }

    fn add(
        &mut self,
        instance: Option<&ServiceInstanceAddress>,
        member: &str,
        create: impl FnOnce() -> C,
    ) -> Result<&mut C, ChordError> {
        let key = state_key(instance, member);
        if self.entries.contains_key(&key) {
            return Err(ChordError::Type(format!(
                "Duplicate service state {}",
                describe_state(instance, member)
            )));
        }
        let entry = CodecEntry {
            instance: instance.cloned(),
            codec: create(),
        };
        self.entries.insert(key, entry);
        Ok(&mut self
            .entries
            .get_mut(&state_key(instance, member))
            .expect("just inserted")
            .codec)
    }

    fn get(
        &mut self,
        instance: Option<&ServiceInstanceAddress>,
        member: &str,
    ) -> Result<&mut C, ChordError> {
        let key = state_key(instance, member);
        self.entries
            .get_mut(&key)
            .map(|entry| &mut entry.codec)
            .ok_or_else(|| {
                ChordError::Type(format!(
                    "Unknown service state {}",
                    describe_state(instance, member)
                ))
            })
    }

    fn remove_instance(&mut self, instance: &ServiceInstanceAddress) {
        self.entries
            .retain(|_, entry| !same_address(entry.instance.as_ref(), instance));
    }
}

/// `createServiceStateEncoder` (`state-codec.ts:60-88`).
#[derive(Debug, Default)]
pub struct ServiceStateEncoder {
    codecs: StateCodecRegistry<Encoder>,
}

impl ServiceStateEncoder {
    pub fn new() -> Self {
        ServiceStateEncoder {
            codecs: StateCodecRegistry::new(),
        }
    }

    /// `encodeSnapshot` (`state-codec.ts:63-68`).
    pub fn encode_snapshot(
        &mut self,
        snapshot: &ServiceSubscriptionSnapshot,
    ) -> Result<WireServiceSubscriptionSnapshot, ChordError> {
        self.codecs.reset();
        let mut instances = Vec::with_capacity(snapshot.instances.len());
        for instance in &snapshot.instances {
            instances.push(encode_instance(instance, &mut self.codecs)?);
        }
        Ok(WireServiceSubscriptionSnapshot {
            service_id: snapshot.service_id.clone(),
            mode: snapshot.mode,
            instances,
        })
    }

    /// `encodeUpdate` (`state-codec.ts:69-86`).
    pub fn encode_update(
        &mut self,
        update: &ServiceProviderUpdate,
    ) -> Result<WireServiceProviderUpdate, ChordError> {
        Ok(match update {
            ServiceProviderUpdate::State {
                instance,
                member,
                sequence,
                ops,
            } => WireServiceProviderUpdate::State {
                instance: instance.clone(),
                member: member.clone(),
                sequence: *sequence,
                ops: self.codecs.get(instance.as_ref(), member)?.encode(ops),
            },
            ServiceProviderUpdate::Reset { snapshot } => {
                self.codecs.reset();
                let mut instances = Vec::with_capacity(snapshot.instances.len());
                for instance in &snapshot.instances {
                    instances.push(encode_instance(instance, &mut self.codecs)?);
                }
                WireServiceProviderUpdate::Reset {
                    snapshot: WireServiceSubscriptionSnapshot {
                        service_id: snapshot.service_id.clone(),
                        mode: snapshot.mode,
                        instances,
                    },
                }
            }
            ServiceProviderUpdate::Replaced { snapshot } => {
                self.codecs.reset();
                WireServiceProviderUpdate::Replaced {
                    snapshot: encode_instance(snapshot, &mut self.codecs)?,
                }
            }
            ServiceProviderUpdate::Spawned { instance } => WireServiceProviderUpdate::Spawned {
                instance: encode_instance(instance, &mut self.codecs)?,
            },
            ServiceProviderUpdate::Unavailable => {
                self.codecs.reset();
                WireServiceProviderUpdate::Unavailable
            }
            ServiceProviderUpdate::Closed { instance } => {
                self.codecs.remove_instance(instance);
                WireServiceProviderUpdate::Closed {
                    instance: instance.clone(),
                }
            }
        })
    }
}

/// `createServiceStateDecoder` (`state-codec.ts:90-118`).
#[derive(Debug, Default)]
pub struct ServiceStateDecoder {
    codecs: StateCodecRegistry<Decoder>,
}

impl ServiceStateDecoder {
    pub fn new() -> Self {
        ServiceStateDecoder {
            codecs: StateCodecRegistry::new(),
        }
    }

    /// `decodeSnapshot` (`state-codec.ts:92-97`).
    pub fn decode_snapshot(
        &mut self,
        snapshot: &WireServiceSubscriptionSnapshot,
    ) -> Result<ServiceSubscriptionSnapshot, ChordError> {
        self.codecs.reset();
        let mut instances = Vec::with_capacity(snapshot.instances.len());
        for instance in &snapshot.instances {
            instances.push(decode_instance(instance, &mut self.codecs)?);
        }
        Ok(ServiceSubscriptionSnapshot {
            service_id: snapshot.service_id.clone(),
            mode: snapshot.mode,
            instances,
        })
    }

    /// `decodeUpdate` (`state-codec.ts:98-116`).
    pub fn decode_update(
        &mut self,
        update: &WireServiceProviderUpdate,
    ) -> Result<ServiceProviderUpdate, ChordError> {
        Ok(match update {
            WireServiceProviderUpdate::State {
                instance,
                member,
                sequence,
                ops,
            } => ServiceProviderUpdate::State {
                instance: instance.clone(),
                member: member.clone(),
                sequence: *sequence,
                ops: self.codecs.get(instance.as_ref(), member)?.decode(ops)?,
            },
            WireServiceProviderUpdate::Reset { snapshot } => {
                self.codecs.reset();
                let mut instances = Vec::with_capacity(snapshot.instances.len());
                for instance in &snapshot.instances {
                    instances.push(decode_instance(instance, &mut self.codecs)?);
                }
                ServiceProviderUpdate::Reset {
                    snapshot: ServiceSubscriptionSnapshot {
                        service_id: snapshot.service_id.clone(),
                        mode: snapshot.mode,
                        instances,
                    },
                }
            }
            WireServiceProviderUpdate::Replaced { snapshot } => {
                self.codecs.reset();
                ServiceProviderUpdate::Replaced {
                    snapshot: decode_instance(snapshot, &mut self.codecs)?,
                }
            }
            WireServiceProviderUpdate::Spawned { instance } => ServiceProviderUpdate::Spawned {
                instance: decode_instance(instance, &mut self.codecs)?,
            },
            WireServiceProviderUpdate::Unavailable => {
                self.codecs.reset();
                ServiceProviderUpdate::Unavailable
            }
            WireServiceProviderUpdate::Closed { instance } => {
                self.codecs.remove_instance(instance);
                ServiceProviderUpdate::Closed {
                    instance: instance.clone(),
                }
            }
        })
    }
}

fn encode_instance(
    instance: &ServiceInstanceSnapshot,
    codecs: &mut StateCodecRegistry<Encoder>,
) -> Result<WireServiceInstanceSnapshot, ChordError> {
    let mut members = Vec::with_capacity(instance.members.len());
    for member in &instance.members {
        members.push(match member {
            ServiceMemberSnapshot::Method { name } => {
                WireServiceMemberSnapshot::Method { name: name.clone() }
            }
            ServiceMemberSnapshot::State {
                name,
                sequence,
                ops,
            } => {
                let codec = codecs.add(instance.instance.as_ref(), name, encoder)?;
                let wire_ops = codec.encode(ops);
                WireServiceMemberSnapshot::State {
                    name: name.clone(),
                    sequence: *sequence,
                    ops: wire_ops,
                }
            }
        });
    }
    Ok(WireServiceInstanceSnapshot {
        instance: instance.instance.clone(),
        members,
    })
}

fn decode_instance(
    instance: &WireServiceInstanceSnapshot,
    codecs: &mut StateCodecRegistry<Decoder>,
) -> Result<ServiceInstanceSnapshot, ChordError> {
    let mut members = Vec::with_capacity(instance.members.len());
    for member in &instance.members {
        members.push(match member {
            WireServiceMemberSnapshot::Method { name } => {
                ServiceMemberSnapshot::Method { name: name.clone() }
            }
            WireServiceMemberSnapshot::State {
                name,
                sequence,
                ops,
            } => {
                let wire_ops = ops.clone();
                let decoded = codecs
                    .add(instance.instance.as_ref(), name, decoder)?
                    .decode(&wire_ops)?;
                ServiceMemberSnapshot::State {
                    name: name.clone(),
                    sequence: *sequence,
                    ops: decoded,
                }
            }
        });
    }
    Ok(ServiceInstanceSnapshot {
        instance: instance.instance.clone(),
        members,
    })
}

/// `stateKey` (`state-codec.ts:148-150`): JSON of
/// `[instance?.key ?? null, instance?.generation ?? null, member]`.
fn state_key(instance: Option<&ServiceInstanceAddress>, member: &str) -> String {
    let key = instance.map_or(serde_json::Value::Null, |address| {
        serde_json::Value::String(address.key.clone())
    });
    let generation = instance.map_or(serde_json::Value::Null, |address| {
        serde_json::Value::Number(serde_json::Number::from(address.generation))
    });
    serde_json::to_string(&serde_json::json!([key, generation, member]))
        .expect("state key serialization cannot fail")
}

fn same_address(left: Option<&ServiceInstanceAddress>, right: &ServiceInstanceAddress) -> bool {
    left.is_some_and(|left| left.key == right.key && left.generation == right.generation)
}

fn describe_state(instance: Option<&ServiceInstanceAddress>, member: &str) -> String {
    match instance {
        None => member.to_owned(),
        Some(address) => format!("{}@{}.{}", address.key, address.generation, member),
    }
}
