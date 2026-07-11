//! Data-driven tool/provider/model legality and reasoning capabilities.
//!
//! The effective catalog is built once per command from shipped data, then the
//! global/user layer, then the project layer. Each layer independently chooses
//! `extend` (upsert) or `replace` (discard lower entries/scopes first).

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

const SHIPPED_CATALOG: &str = include_str!("../data/model-catalog.toml");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ModelIdentity {
    tool: String,
    provider: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ScopeIdentity {
    tool: String,
    provider: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasoningEffort {
    Default,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "default" | "none" => Some(Self::Default),
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "extra-high" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CatalogProvenance {
    Shipped { key: String },
    Global { path: PathBuf, key: String },
    Project { path: PathBuf, key: String },
    Inline { source: String, key: String },
}

impl CatalogProvenance {
    pub fn source_label(&self) -> String {
        match self {
            Self::Shipped { key } => format!("shipped model catalog ({key})"),
            Self::Global { path, key } => format!("global config {} ({key})", path.display()),
            Self::Project { path, key } => format!("project config {} ({key})", path.display()),
            Self::Inline { source, key } => format!("{source} ({key})"),
        }
    }

    fn with_key(&self, key: String) -> Self {
        match self {
            Self::Shipped { .. } => Self::Shipped { key },
            Self::Global { path, .. } => Self::Global {
                path: path.clone(),
                key,
            },
            Self::Project { path, .. } => Self::Project {
                path: path.clone(),
                key,
            },
            Self::Inline { source, .. } => Self::Inline {
                source: source.clone(),
                key,
            },
        }
    }
}

#[derive(Debug, Clone)]
struct CatalogCapability {
    enabled: bool,
    reasoning_efforts: BTreeSet<ReasoningEffort>,
    allow_custom_reasoning: bool,
    provenance: CatalogProvenance,
}

#[derive(Debug, Clone, Default)]
struct ConfiguredSpecProvenance {
    model_sources: BTreeSet<CatalogProvenance>,
    reasoning_sources: BTreeSet<CatalogProvenance>,
}

impl ConfiguredSpecProvenance {
    fn primary_source(&self) -> CatalogProvenance {
        self.model_sources
            .iter()
            .chain(self.reasoning_sources.iter())
            .next()
            .cloned()
            .expect("configured spec provenance must contain a source")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogWarningKind {
    UnverifiedModel,
    UnverifiedReasoningEffort,
}

#[derive(Debug, Clone)]
pub struct CatalogWarning {
    kind: CatalogWarningKind,
    tool: Box<str>,
    provider: Box<str>,
    model: Box<str>,
    reasoning: Box<str>,
    model_sources: Box<[CatalogProvenance]>,
    reasoning_sources: Box<[CatalogProvenance]>,
}

impl CatalogWarning {
    pub fn kind(&self) -> CatalogWarningKind {
        self.kind
    }
}

impl fmt::Display for CatalogWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let subject = match self.kind {
            CatalogWarningKind::UnverifiedModel => "model identity",
            CatalogWarningKind::UnverifiedReasoningEffort => "reasoning effort",
        };
        let model_sources = format_provenance_list(&self.model_sources);
        let reasoning_sources = if self.reasoning_sources.is_empty() {
            String::new()
        } else {
            format!(
                "; effective reasoning selected by {}",
                format_provenance_list(&self.reasoning_sources)
            )
        };
        write!(
            formatter,
            "configured {subject} ({}, {}, {}, {}) selected by {model_sources}{reasoning_sources} is not verified by shipped/provider metadata; execution will continue and the backend may reject it",
            self.tool, self.provider, self.model, self.reasoning,
        )
    }
}

fn format_provenance_list(provenances: &[CatalogProvenance]) -> String {
    provenances
        .iter()
        .map(CatalogProvenance::source_label)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone)]
pub struct CatalogAdmission {
    pub provenance: CatalogProvenance,
    warning: Option<CatalogWarning>,
}

impl CatalogAdmission {
    pub fn source_label(&self) -> String {
        self.provenance.source_label()
    }

    pub fn warning(&self) -> Option<&CatalogWarning> {
        self.warning.as_ref()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfiguredSpecError {
    #[error("configured model spec field '{field}' must not be empty at {provenance}")]
    EmptyField {
        field: &'static str,
        provenance: String,
    },
    #[error(
        "configured model spec field '{field}' must not contain leading/trailing whitespace at {provenance}"
    )]
    SurroundingWhitespace {
        field: &'static str,
        provenance: String,
    },
    #[error("configured reasoning value '{value}' is malformed at {provenance}")]
    InvalidReasoning { value: String, provenance: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogErrorKind {
    MalformedIdentity,
    UnknownTool,
    UnknownProvider,
    UnknownModel,
    DisabledModel,
    UnsupportedReasoningEffort,
    UnsupportedCustomReasoning,
}

#[derive(Debug, Clone)]
pub struct CatalogLegalityError {
    kind: CatalogErrorKind,
    tool: Box<str>,
    provider: Box<str>,
    model: Box<str>,
    reasoning: Box<str>,
    known: Box<[String]>,
    source: Box<str>,
    malformed_field: Option<Box<str>>,
}

impl CatalogLegalityError {
    pub fn kind(&self) -> CatalogErrorKind {
        self.kind
    }

    pub fn known(&self) -> &[String] {
        &self.known
    }

    pub fn source_label(&self) -> &str {
        &self.source
    }
}

impl fmt::Display for CatalogLegalityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CatalogErrorKind::MalformedIdentity => write!(
                formatter,
                "malformed model identity field '{}' contains leading/trailing whitespace or is empty under effective model catalog from {}",
                self.malformed_field.as_deref().unwrap_or("unknown"),
                self.source
            ),
            CatalogErrorKind::UnknownTool => write!(
                formatter,
                "unknown tool '{}' under closed effective model catalog from {}; known tools: {:?}",
                self.tool, self.source, self.known
            ),
            CatalogErrorKind::UnknownProvider => write!(
                formatter,
                "unknown provider '{}' for tool '{}' under closed effective model catalog from {}; known providers: {:?}",
                self.provider, self.tool, self.source, self.known
            ),
            CatalogErrorKind::UnknownModel => write!(
                formatter,
                "unknown model '{}' for exact identity ({}, {}, {}) under closed effective model catalog from {}; known models: {:?}",
                self.model, self.tool, self.provider, self.model, self.source, self.known
            ),
            CatalogErrorKind::DisabledModel => write!(
                formatter,
                "model identity ({}, {}, {}) is disabled by catalog tombstone from {}",
                self.tool, self.provider, self.model, self.source
            ),
            CatalogErrorKind::UnsupportedReasoningEffort => write!(
                formatter,
                "reasoning effort '{}' is unsupported for ({}, {}, {}) by catalog entry from {}; allowed efforts: {:?}",
                self.reasoning, self.tool, self.provider, self.model, self.source, self.known
            ),
            CatalogErrorKind::UnsupportedCustomReasoning => write!(
                formatter,
                "custom reasoning budget '{}' is unsupported for ({}, {}, {}) by catalog entry from {}",
                self.reasoning, self.tool, self.provider, self.model, self.source
            ),
        }
    }
}

impl std::error::Error for CatalogLegalityError {}

#[derive(Debug, thiserror::Error)]
pub enum CatalogLoadError {
    #[error("failed to read model catalog layer {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse model catalog layer {layer}: {message}")]
    Parse { layer: String, message: String },
    #[error("invalid model catalog layer {layer}: {message}")]
    Invalid { layer: String, message: String },
}

#[derive(Debug, Clone)]
pub struct EffectiveModelCatalog {
    entries: BTreeMap<ModelIdentity, CatalogCapability>,
    open_scopes: BTreeMap<ScopeIdentity, CatalogCapability>,
    configured_specs: BTreeMap<ModelIdentity, BTreeMap<String, ConfiguredSpecProvenance>>,
    closed: bool,
    policy_provenance: CatalogProvenance,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogResolutionError {
    #[error(
        "cannot resolve provider for model '{model}' and tool '{tool}' from effective model catalog {catalog_source}: no provider is declared"
    )]
    MissingProvider {
        tool: String,
        model: String,
        catalog_source: String,
    },
    #[error(
        "cannot resolve provider for model '{model}' and tool '{tool}' from effective model catalog {catalog_source}: matching providers are {providers:?}; use provider/model"
    )]
    AmbiguousProvider {
        tool: String,
        model: String,
        providers: Vec<String>,
        catalog_source: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShippedModelAlias {
    pub canonical: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ShippedPolicyDocument {
    #[serde(default)]
    defaults: BTreeMap<String, String>,
    #[serde(default)]
    compatibility: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    aliases: Vec<ShippedModelAlias>,
    #[serde(default)]
    retry: BTreeMap<String, BTreeMap<String, String>>,
}

fn shipped_policy() -> Result<ShippedPolicyDocument, CatalogLoadError> {
    toml::from_str(SHIPPED_CATALOG).map_err(|error| CatalogLoadError::Parse {
        layer: "shipped:model-catalog.toml".to_string(),
        message: error.to_string(),
    })
}

pub fn shipped_model_aliases() -> Result<Vec<ShippedModelAlias>, CatalogLoadError> {
    Ok(shipped_policy()?.aliases)
}

pub fn shipped_model_default(key: &str) -> Result<Option<String>, CatalogLoadError> {
    Ok(shipped_policy()?.defaults.remove(key))
}

pub fn shipped_compatibility_models(key: &str) -> Result<Vec<String>, CatalogLoadError> {
    Ok(shipped_policy()?
        .compatibility
        .remove(key)
        .unwrap_or_default())
}

pub fn shipped_retry_model(tool: &str, key: &str) -> Result<Option<String>, CatalogLoadError> {
    Ok(shipped_policy()?
        .retry
        .remove(tool)
        .and_then(|mut values| values.remove(key)))
}

impl EffectiveModelCatalog {
    pub fn shipped() -> Result<Self, CatalogLoadError> {
        let provenance = CatalogProvenance::Shipped {
            key: "model_catalog".to_string(),
        };
        let layer = parse_layer(SHIPPED_CATALOG, &provenance)?.ok_or_else(|| {
            CatalogLoadError::Invalid {
                layer: provenance.source_label(),
                message: "missing [model_catalog] table".to_string(),
            }
        })?;
        let mut catalog = Self {
            entries: BTreeMap::new(),
            open_scopes: BTreeMap::new(),
            configured_specs: BTreeMap::new(),
            closed: true,
            policy_provenance: provenance.clone(),
        };
        catalog.apply_layer(layer, &provenance)?;
        Ok(catalog)
    }

    pub fn from_toml_str(contents: &str, source: &str) -> Result<Self, CatalogLoadError> {
        let provenance = CatalogProvenance::Inline {
            source: source.to_string(),
            key: "model_catalog".to_string(),
        };
        let layer =
            parse_layer(contents, &provenance)?.ok_or_else(|| CatalogLoadError::Invalid {
                layer: source.to_string(),
                message: "missing [model_catalog] table".to_string(),
            })?;
        let mut catalog = Self {
            entries: BTreeMap::new(),
            open_scopes: BTreeMap::new(),
            configured_specs: BTreeMap::new(),
            closed: true,
            policy_provenance: provenance.clone(),
        };
        catalog.apply_layer(layer, &provenance)?;
        Ok(catalog)
    }

    pub fn load_with_paths(
        global_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Result<Self, CatalogLoadError> {
        let mut catalog = Self::shipped()?;
        if let Some(path) = global_path
            && path.exists()
        {
            catalog.apply_file(path, true)?;
        }
        if let Some(path) = project_path
            && path.exists()
        {
            catalog.apply_file(path, false)?;
        }
        Ok(catalog)
    }

    /// Build one catalog generation from already captured source bytes.
    pub fn load_from_captured_sources(
        global: Option<(&Path, &str)>,
        project: Option<(&Path, &str)>,
    ) -> Result<Self, CatalogLoadError> {
        let mut catalog = Self::shipped()?;
        if let Some((path, contents)) = global {
            catalog.apply_contents(path, contents, true)?;
        }
        if let Some((path, contents)) = project {
            catalog.apply_contents(path, contents, false)?;
        }
        Ok(catalog)
    }

    fn apply_file(&mut self, path: &Path, global: bool) -> Result<(), CatalogLoadError> {
        let contents = std::fs::read_to_string(path).map_err(|source| CatalogLoadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        self.apply_contents(path, &contents, global)
    }

    fn apply_contents(
        &mut self,
        path: &Path,
        contents: &str,
        global: bool,
    ) -> Result<(), CatalogLoadError> {
        let provenance = if global {
            CatalogProvenance::Global {
                path: path.to_path_buf(),
                key: "model_catalog".to_string(),
            }
        } else {
            CatalogProvenance::Project {
                path: path.to_path_buf(),
                key: "model_catalog".to_string(),
            }
        };
        if let Some(layer) = parse_layer(contents, &provenance)? {
            self.apply_layer(layer, &provenance)?;
        }
        Ok(())
    }

    fn apply_layer(
        &mut self,
        layer: RawCatalogLayer,
        provenance: &CatalogProvenance,
    ) -> Result<(), CatalogLoadError> {
        if layer.mode == CatalogMergeMode::Replace {
            self.entries.clear();
            self.open_scopes.clear();
        }
        if let Some(closed) = layer.closed {
            self.closed = closed;
            self.policy_provenance = provenance.with_key("model_catalog.closed".to_string());
        } else if layer.mode == CatalogMergeMode::Replace {
            self.policy_provenance = provenance.with_key("model_catalog.mode".to_string());
        }

        let mut identities = BTreeSet::new();
        for (index, entry) in layer.entries.into_iter().enumerate() {
            let key = format!("model_catalog.entries[{index}]");
            validate_field(&entry.tool, "tool", &key, provenance)?;
            validate_field(&entry.provider, "provider", &key, provenance)?;
            validate_field(&entry.model, "model", &key, provenance)?;
            let identity = ModelIdentity {
                tool: entry.tool,
                provider: entry.provider,
                model: entry.model,
            };
            if !identities.insert(identity.clone()) {
                return invalid(
                    provenance,
                    format!(
                        "duplicate exact identity ({}, {}, {}) in this layer",
                        identity.tool, identity.provider, identity.model
                    ),
                );
            }
            let capability = parse_capability(
                entry.enabled,
                entry.reasoning_efforts,
                entry.allow_custom_reasoning,
                provenance.with_key(key),
                provenance,
            )?;
            self.entries.insert(identity, capability);
        }

        let mut scopes = BTreeSet::new();
        for (index, scope) in layer.open_scopes.into_iter().enumerate() {
            let key = format!("model_catalog.open_scopes[{index}]");
            validate_field(&scope.tool, "tool", &key, provenance)?;
            validate_field(&scope.provider, "provider", &key, provenance)?;
            if scope.tool == "*" {
                return invalid(provenance, format!("{key}.tool must not be '*'"));
            }
            let identity = ScopeIdentity {
                tool: scope.tool,
                provider: scope.provider,
            };
            if !scopes.insert(identity.clone()) {
                return invalid(
                    provenance,
                    format!(
                        "duplicate open scope ({}, {}) in this layer",
                        identity.tool, identity.provider
                    ),
                );
            }
            let capability = parse_capability(
                true,
                scope.reasoning_efforts,
                scope.allow_custom_reasoning,
                provenance.with_key(key),
                provenance,
            )?;
            self.open_scopes.insert(identity, capability);
        }
        Ok(())
    }
}

include!("model_catalog_admission.rs");

#[derive(Debug, Deserialize)]
struct RawDocument {
    model_catalog: Option<RawCatalogLayer>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalogLayer {
    #[serde(default)]
    mode: CatalogMergeMode,
    closed: Option<bool>,
    #[serde(default)]
    entries: Vec<RawCatalogEntry>,
    #[serde(default)]
    open_scopes: Vec<RawOpenScope>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CatalogMergeMode {
    #[default]
    Extend,
    Replace,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalogEntry {
    tool: String,
    provider: String,
    model: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    reasoning_efforts: Vec<String>,
    #[serde(default)]
    allow_custom_reasoning: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOpenScope {
    tool: String,
    provider: String,
    #[serde(default)]
    reasoning_efforts: Vec<String>,
    #[serde(default)]
    allow_custom_reasoning: bool,
}

fn default_enabled() -> bool {
    true
}

fn parse_layer(
    contents: &str,
    provenance: &CatalogProvenance,
) -> Result<Option<RawCatalogLayer>, CatalogLoadError> {
    toml::from_str::<RawDocument>(contents)
        .map(|document| document.model_catalog)
        .map_err(|error| CatalogLoadError::Parse {
            layer: provenance.source_label(),
            message: error.to_string(),
        })
}

fn parse_capability(
    enabled: bool,
    efforts: Vec<String>,
    allow_custom_reasoning: bool,
    provenance: CatalogProvenance,
    layer_provenance: &CatalogProvenance,
) -> Result<CatalogCapability, CatalogLoadError> {
    let mut reasoning_efforts = BTreeSet::new();
    for effort in efforts {
        let parsed = ReasoningEffort::parse(&effort).ok_or_else(|| CatalogLoadError::Invalid {
            layer: layer_provenance.source_label(),
            message: format!(
                "unknown reasoning effort '{effort}' at {}",
                provenance.source_label()
            ),
        })?;
        if !reasoning_efforts.insert(parsed) {
            return invalid(
                layer_provenance,
                format!(
                    "duplicate reasoning effort '{}' at {}",
                    parsed.as_str(),
                    provenance.source_label()
                ),
            );
        }
    }
    if enabled && reasoning_efforts.is_empty() && !allow_custom_reasoning {
        return invalid(
            layer_provenance,
            format!(
                "{} must declare reasoning_efforts and/or allow_custom_reasoning",
                provenance.source_label()
            ),
        );
    }
    Ok(CatalogCapability {
        enabled,
        reasoning_efforts,
        allow_custom_reasoning,
        provenance,
    })
}

fn validate_field(
    value: &str,
    field: &str,
    key: &str,
    provenance: &CatalogProvenance,
) -> Result<(), CatalogLoadError> {
    if value.trim().is_empty() {
        return invalid(provenance, format!("{key}.{field} must not be empty"));
    }
    if value != value.trim() {
        return invalid(
            provenance,
            format!("{key}.{field} must not contain leading/trailing whitespace"),
        );
    }
    Ok(())
}

fn invalid<T>(provenance: &CatalogProvenance, message: String) -> Result<T, CatalogLoadError> {
    Err(CatalogLoadError::Invalid {
        layer: provenance.source_label(),
        message,
    })
}
