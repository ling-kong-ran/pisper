//! Port of `src/documents.ts`: document definitions, tokens, address
//! resolution, and materialization.
//!
//! Divergence (structural, disclosed): upstream document definitions are
//! plain objects with optional function members and TypeScript infers their
//! token types; the port carries the handlers as `Arc`-boxed closures on
//! [`DocDefinition`], and `defineDoc` / `defineDocFamily` become
//! [`define_doc`] / [`define_doc_family`] constructors. Thrown `TypeError`s /
//! `Error`s become `Err(PlainError/StorageError)` with identical message
//! text.

use std::sync::Arc;

use serde_json::Value;

use crate::chord::delta::Op;
use crate::chord::json::copy_json;

use super::errors::PlainError;
use super::ids::DocumentId;
use super::types::{
    CheckpointInfo, DocumentAddress, DocumentCreate, DocumentFork, DocumentHistory, DocumentRecord,
    DocumentScope, JsonObject, StoredDocument,
};

/// Document scope discriminant of a definition (`documents.ts` via
/// `DocumentSemantics.scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionScope {
    Session,
    Conversation,
    Task,
}

/// `initial(seed?)`: first value of a new incarnation; families receive their
/// member seed.
pub type InitialFn = Arc<dyn Fn(Option<&Value>) -> JsonObject + Send + Sync>;
/// `migrate(value, fromVersion)`: convert a stored older version.
pub type MigrateFn = Arc<dyn Fn(&JsonObject, i64) -> JsonObject + Send + Sync>;
/// `checkpointWhen(value, ops, info)`: store this ordinary change as a
/// complete base instead of a delta.
pub type CheckpointWhenFn = Arc<dyn Fn(&JsonObject, &[Op], CheckpointInfo) -> bool + Send + Sync>;

/// Erased definition shape used by the Session after overload resolution
/// (`documents.ts` `AnyDocDefinition`).
#[derive(Clone)]
pub struct DocDefinition {
    /// Stable persisted kind; part of the public protocol.
    pub kind: String,
    /// Positive integer version of the stored value shape.
    pub version: i64,
    pub scope: DefinitionScope,
    /// Conversation documents only: history retention policy.
    pub history: Option<DocumentHistory>,
    /// Conversation documents only: fork initialization policy.
    pub fork: Option<DocumentFork>,
    /// Keyed document family flag (`documents.ts` `DocFamilyDefinition.family`).
    pub family: bool,
    /// `initial(seed?)`: first value of a new incarnation; families receive
    /// their member seed.
    pub initial: InitialFn,
    /// `migrate(value, fromVersion)`: convert a stored older version.
    pub migrate: Option<MigrateFn>,
    /// `checkpointWhen(value, ops, info)`: store this ordinary change as a
    /// complete base instead of a delta.
    pub checkpoint_when: Option<CheckpointWhenFn>,
}

impl std::fmt::Debug for DocDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocDefinition")
            .field("kind", &self.kind)
            .field("version", &self.version)
            .field("scope", &self.scope)
            .field("family", &self.family)
            .finish()
    }
}

/// `validateDefinition` (`documents.ts:82-88`).
fn validate_definition(definition: &DocDefinition) -> Result<(), PlainError> {
    if definition.version < 1 || definition.version > 9_007_199_254_740_991 {
        return Err(PlainError::new(format!(
            "Document {} version must be a positive integer",
            definition.kind
        )));
    }
    Ok(())
}

/// `defineDoc(definition)` (`documents.ts:48-51`): define a singleton
/// document token.
pub fn define_doc(definition: DocDefinition) -> Result<DocToken, PlainError> {
    validate_definition(&definition)?;
    Ok(DocToken { definition })
}

/// `defineDocFamily(definition)` (`documents.ts:68-71`): define a keyed
/// document family token.
pub fn define_doc_family(definition: DocDefinition) -> Result<DocToken, PlainError> {
    validate_definition(&definition)?;
    Ok(DocToken {
        definition: DocDefinition {
            family: true,
            ..definition
        },
    })
}

/// Typed document token passed explicitly to typed access (`types.ts`
/// `DocToken` / `AnyDocToken`).
#[derive(Clone)]
pub struct DocToken {
    pub definition: DocDefinition,
}

/// Logical address plus its string identity for maps (`documents.ts`
/// `ResolvedAddress`).
pub struct ResolvedAddress {
    pub address: DocumentAddress,
    pub id: String,
    /// Index after the owner and family key in the upstream argument list
    /// (the context position).
    pub next_argument: usize,
}

/// `resolveAddress(definition, args)` (`documents.ts:97-117`): the port takes
/// the owner and family key explicitly instead of an erased argument list.
pub fn resolve_address(
    definition: &DocDefinition,
    owner: Option<i64>,
    key: Option<&str>,
) -> Result<ResolvedAddress, PlainError> {
    let scope = match definition.scope {
        DefinitionScope::Session => DocumentScope::Session,
        DefinitionScope::Conversation => DocumentScope::Conversation {
            conversation_id: owner_id(owner, definition, "conversation")?,
        },
        DefinitionScope::Task => DocumentScope::Task {
            task_id: owner_id(owner, definition, "task")?,
        },
    };
    let mut next_argument = match definition.scope {
        DefinitionScope::Session => 0,
        _ => 1,
    };
    if definition.family {
        next_argument += 1;
    }
    let key = key.map(str::to_string);
    let address = DocumentAddress {
        kind: definition.kind.clone(),
        scope,
        key,
    };
    let id = address_id(&address);
    Ok(ResolvedAddress {
        address,
        id,
        next_argument,
    })
}

/// `ownerId<I>(value, definition)` (`documents.ts:119-126`).
fn owner_id(
    value: Option<i64>,
    definition: &DocDefinition,
    scope: &str,
) -> Result<i64, PlainError> {
    match value {
        Some(value) if (0..=9_007_199_254_740_991).contains(&value) => Ok(value),
        _ => Err(PlainError::new(format!(
            "Document {} requires a {} ID",
            definition.kind, scope
        ))),
    }
}

/// `addressId(address)` (`documents.ts:129-139`): stable string identity of
/// one logical address.
pub fn address_id(address: &DocumentAddress) -> String {
    let owner = match &address.scope {
        DocumentScope::Session => Value::Null,
        DocumentScope::Conversation { conversation_id } => Value::from(*conversation_id),
        DocumentScope::Task { task_id } => Value::from(*task_id),
    };
    let array = vec![
        Value::from(address.kind.clone()),
        Value::from(match &address.scope {
            DocumentScope::Session => "session",
            DocumentScope::Conversation { .. } => "conversation",
            DocumentScope::Task { .. } => "task",
        }),
        owner,
        match &address.key {
            Some(key) => Value::from(key.clone()),
            None => Value::Null,
        },
    ];
    Value::Array(array).to_string()
}

/// `documentCreate(definition, address, id)` (`documents.ts:142-161`): build
/// the storage create record for a new incarnation at an address.
pub fn document_create(
    definition: &DocDefinition,
    address: &DocumentAddress,
    id: DocumentId,
) -> DocumentCreate {
    DocumentCreate {
        id,
        kind: address.kind.clone(),
        key: address.key.clone(),
        scope: address.scope,
        history: definition.history,
        fork: definition.fork,
    }
}

/// `checkRecordScope` (`documents.ts:164-176`): reject typed access whose
/// token disagrees with the persisted scope, history, or fork semantics.
pub fn check_record_scope(
    definition: &DocDefinition,
    record: &DocumentCreate,
) -> Result<(), PlainError> {
    let scope_matches = match (&record.scope, definition.scope) {
        (DocumentScope::Session, DefinitionScope::Session) => true,
        (DocumentScope::Task { .. }, DefinitionScope::Task) => true,
        (DocumentScope::Conversation { .. }, DefinitionScope::Conversation) => {
            record.history == definition.history && record.fork == definition.fork
        }
        _ => false,
    };
    if !scope_matches {
        return Err(PlainError::new(format!(
            "Document {} ({}) does not match the supplied definition semantics",
            record.id, record.kind
        )));
    }
    Ok(())
}

/// `checkRecordVersion` (`documents.ts:179-191`): reject typed access to a
/// stored version the supplied definition cannot use.
pub fn check_record_version(
    definition: &DocDefinition,
    record: &DocumentCreate,
    version: i64,
) -> Result<(), PlainError> {
    if version > definition.version {
        return Err(PlainError::new(format!(
            "Document {} ({}) has newer version {version} than {}",
            record.id, record.kind, definition.version
        )));
    }
    if version < definition.version && definition.migrate.is_none() {
        return Err(PlainError::new(format!(
            "Document {} ({}) requires migration from version {version}",
            record.id, record.kind
        )));
    }
    Ok(())
}

/// `materializeDocument(definition, stored)` (`documents.ts:194-196`):
/// validate and materialize a detached stored value for typed access.
pub fn materialize_document(
    definition: &DocDefinition,
    stored: &StoredDocument,
) -> Result<JsonObject, PlainError> {
    materialize_document_value(
        definition,
        &document_create_of_record(&stored.record),
        stored.version,
        &stored.value,
    )
}

/// `materializeDocumentValue(definition, record, version, value)`
/// (`documents.ts:199-208`): validate and materialize one detached value
/// before its first persisted incarnation.
pub fn materialize_document_value(
    definition: &DocDefinition,
    record: &DocumentCreate,
    version: i64,
    value: &JsonObject,
) -> Result<JsonObject, PlainError> {
    check_record_scope(definition, record)?;
    check_record_version(definition, record, version)?;
    if version == definition.version {
        return Ok(value.clone());
    }
    let migrate = definition
        .migrate
        .as_ref()
        .expect("migration presence checked by check_record_version");
    Ok(copy_json(&Value::Object(migrate(value, version)), None)
        .as_object()
        .cloned()
        .unwrap_or_default())
}

/// `DocumentCreate` view of a committed record (the checks share one shape).
pub fn document_create_of_record(record: &DocumentRecord) -> DocumentCreate {
    DocumentCreate {
        id: record.id,
        kind: record.kind.clone(),
        key: record.key.clone(),
        scope: record.scope,
        history: record.history,
        fork: record.fork,
    }
}

/// Convenience wrapper accepting a committed record directly
/// (`checkRecordScope` over `DocumentRecord`).
pub fn check_record_scope_of_record(
    definition: &DocDefinition,
    record: &DocumentRecord,
) -> Result<(), PlainError> {
    check_record_scope(definition, &document_create_of_record(record))
}
