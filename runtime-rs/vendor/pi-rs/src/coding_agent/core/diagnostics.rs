//! Port of upstream `coding-agent/src/core/diagnostics.ts`.
//!
//! Pure type declarations (upstream has no runtime behavior): the resource
//! collision / diagnostic records surfaced by the resource loader. Upstream
//! declares plain interfaces without serialization, so the port derives only
//! value traits (no serde wire format exists upstream to match).

/// Upstream `ResourceCollision.resourceType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceCollisionType {
    Extension,
    Skill,
    Prompt,
    Theme,
}

impl ResourceCollisionType {
    /// The upstream string literal (`"extension" | "skill" | "prompt" | "theme"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Extension => "extension",
            Self::Skill => "skill",
            Self::Prompt => "prompt",
            Self::Theme => "theme",
        }
    }
}

/// Upstream `ResourceCollision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceCollision {
    /// The kind of colliding resource.
    pub resource_type: ResourceCollisionType,
    /// Skill name, command/tool/flag name, prompt name, or theme name.
    pub name: String,
    pub winner_path: String,
    pub loser_path: String,
    /// e.g. `"npm:foo"`, `"git:..."`, `"local"`.
    pub winner_source: Option<String>,
    pub loser_source: Option<String>,
}

/// Upstream `ResourceDiagnostic.type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceDiagnosticType {
    Warning,
    Error,
    Collision,
}

impl ResourceDiagnosticType {
    /// The upstream string literal (`"warning" | "error" | "collision"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Collision => "collision",
        }
    }
}

/// Upstream `ResourceDiagnostic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDiagnostic {
    pub r#type: ResourceDiagnosticType,
    pub message: String,
    pub path: Option<String>,
    pub collision: Option<ResourceCollision>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_literals_match_upstream_union_members() {
        assert_eq!(ResourceCollisionType::Extension.as_str(), "extension");
        assert_eq!(ResourceCollisionType::Skill.as_str(), "skill");
        assert_eq!(ResourceCollisionType::Prompt.as_str(), "prompt");
        assert_eq!(ResourceCollisionType::Theme.as_str(), "theme");
        assert_eq!(ResourceDiagnosticType::Warning.as_str(), "warning");
        assert_eq!(ResourceDiagnosticType::Error.as_str(), "error");
        assert_eq!(ResourceDiagnosticType::Collision.as_str(), "collision");
    }

    #[test]
    fn records_hold_the_upstream_fields() {
        let collision = ResourceCollision {
            resource_type: ResourceCollisionType::Skill,
            name: "commit".to_string(),
            winner_path: "/a/skills/commit".to_string(),
            loser_path: "/b/skills/commit".to_string(),
            winner_source: Some("npm:foo".to_string()),
            loser_source: Some("local".to_string()),
        };
        let diagnostic = ResourceDiagnostic {
            r#type: ResourceDiagnosticType::Collision,
            message: "duplicate skill".to_string(),
            path: Some("/b/skills/commit".to_string()),
            collision: Some(collision.clone()),
        };
        // Optional fields default to absent, like upstream `undefined`.
        let bare = ResourceDiagnostic {
            r#type: ResourceDiagnosticType::Warning,
            message: "m".to_string(),
            path: None,
            collision: None,
        };
        assert_eq!(diagnostic.collision, Some(collision));
        assert_eq!(bare.path, None);
        assert_eq!(bare.collision, None);
    }
}
