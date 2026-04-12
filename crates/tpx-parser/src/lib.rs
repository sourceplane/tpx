use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::PathBuf,
};

pub const COMPONENT_NAME: &str = "tpx-parser";
pub const API_VERSION: &str = "tpx.io/v1";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InternalModel {
    pub tools: HashMap<String, Tool>,
    pub assets: HashMap<String, Asset>,
    pub envs: HashMap<String, Environment>,
    pub providers: HashMap<String, InlineProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Metadata {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct InlineProvider {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: InlineProviderSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct InlineProviderSpec {
    #[serde(default)]
    pub tools: Vec<InlineProviderTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct InlineProviderTool {
    pub name: String,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default, alias = "env", alias = "environment", alias = "environments")]
    pub envs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Tool {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: ToolSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ToolSpec {
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default, alias = "env", alias = "environment", alias = "environments")]
    pub envs: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Bundle {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: BundleSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct BundleSpec {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default, alias = "env", alias = "environment", alias = "environments")]
    pub envs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Asset {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: AssetSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct AssetSpec {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub checksum: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Environment {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: EnvironmentSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct EnvironmentSpec {
    #[serde(default)]
    pub values: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Secret {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: SecretSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct SecretSpec {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct LegacyWorkspace {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: LegacyWorkspaceSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct LegacyWorkspaceSpec {
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, alias = "env", alias = "environment", alias = "environments")]
    pub envs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WorkspaceManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub workspace: String,
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub providers: BTreeMap<String, WorkspaceProviderRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "WorkspaceProviderRefValue")]
pub struct WorkspaceProviderRef {
    pub source: String,
    #[serde(rename = "plainHTTP")]
    pub plain_http: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub metadata: ProviderMetadata,
    pub spec: ProviderManifestSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderMetadata {
    pub namespace: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRuntime {
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderManifestSpec {
    pub runtime: ProviderRuntime,
    pub entrypoint: String,
    pub platforms: Vec<ProviderPlatform>,
    #[serde(default)]
    pub capabilities: BTreeMap<String, Capability>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub layers: Option<ProviderLayers>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderPlatform {
    pub os: String,
    pub arch: String,
    pub binary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Capability {
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ProviderLayers {
    #[serde(default)]
    pub assets: Option<ProviderAssetsLayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderAssetsLayer {
    pub root: String,
    #[serde(default)]
    pub includes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WorkspaceLock {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub workspace: String,
    #[serde(default)]
    pub providers: Vec<LockedProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LockedProvider {
    pub alias: String,
    pub provider: String,
    pub source: String,
    pub version: String,
    pub resolved: String,
    pub store: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedDocument {
    WorkspaceManifest(WorkspaceManifest),
    ProviderManifest(ProviderManifest),
    WorkspaceLock(WorkspaceLock),
    InlineProvider(InlineProvider),
    Tool(Tool),
    Bundle(Bundle),
    Asset(Asset),
    Environment(Environment),
    Secret(Secret),
    LegacyWorkspace(LegacyWorkspace),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum WorkspaceProviderRefValue {
    Shorthand(String),
    Expanded(WorkspaceProviderRefExpanded),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct WorkspaceProviderRefExpanded {
    source: String,
    #[serde(default, rename = "plainHTTP")]
    plain_http: bool,
}

impl From<WorkspaceProviderRefValue> for WorkspaceProviderRef {
    fn from(value: WorkspaceProviderRefValue) -> Self {
        match value {
            WorkspaceProviderRefValue::Shorthand(source) => Self {
                source,
                plain_http: false,
            },
            WorkspaceProviderRefValue::Expanded(expanded) => Self {
                source: expanded.source,
                plain_http: expanded.plain_http,
            },
        }
    }
}

enum Document {
    WorkspaceManifest(WorkspaceManifest),
    ProviderManifest(ProviderManifest),
    WorkspaceLock(WorkspaceLock),
    InlineProvider(InlineProvider),
    Tool(Tool),
    Bundle(Bundle),
    Asset(Asset),
    Environment(Environment),
    Secret(Secret),
    LegacyWorkspace(LegacyWorkspace),
}

impl From<Document> for ParsedDocument {
    fn from(document: Document) -> Self {
        match document {
            Document::WorkspaceManifest(workspace) => Self::WorkspaceManifest(workspace),
            Document::ProviderManifest(provider) => Self::ProviderManifest(provider),
            Document::WorkspaceLock(lock) => Self::WorkspaceLock(lock),
            Document::InlineProvider(provider) => Self::InlineProvider(provider),
            Document::Tool(tool) => Self::Tool(tool),
            Document::Bundle(bundle) => Self::Bundle(bundle),
            Document::Asset(asset) => Self::Asset(asset),
            Document::Environment(environment) => Self::Environment(environment),
            Document::Secret(secret) => Self::Secret(secret),
            Document::LegacyWorkspace(workspace) => Self::LegacyWorkspace(workspace),
        }
    }
}

impl Tool {
    fn from_provider_tool(provider_name: &str, provider_tool: InlineProviderTool) -> Result<Self> {
        let tool_name = normalized_name(&provider_tool.name, "tool")?.to_owned();

        Ok(Self {
            metadata: Some(Metadata { name: tool_name }),
            spec: ToolSpec {
                runtime: provider_tool.runtime,
                version: provider_tool.version,
                dependencies: provider_tool.dependencies,
                assets: provider_tool.assets,
                envs: provider_tool.envs,
                provider: Some(provider_name.to_owned()),
            },
        })
    }
}

pub fn parse_file(path: &str) -> Result<InternalModel> {
    let source = read_source(path)?;

    parse_legacy_source(&source, fallback_provider_name(path))
}

pub fn parse_documents_file(path: &str) -> Result<Vec<ParsedDocument>> {
    let source = read_source(path)?;

    parse_documents_source(&source)
}

pub fn parse_document_file(path: &str) -> Result<ParsedDocument> {
    let mut documents = parse_documents_file(path)?;

    match documents.len() {
        1 => Ok(documents.remove(0)),
        0 => bail!("expected exactly one YAML document, found none"),
        count => bail!("expected exactly one YAML document, found {count}"),
    }
}

fn read_source(path: &str) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read parser input at {path}"))
}

fn parse_legacy_source(source: &str, fallback_provider_name: String) -> Result<InternalModel> {
    if source.trim().is_empty() {
        return Ok(InternalModel::default());
    }

    let documents = collect_documents(source)?;

    normalize_documents(documents, &fallback_provider_name)
}

fn parse_documents_source(source: &str) -> Result<Vec<ParsedDocument>> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }

    collect_documents(source)
        .map(|documents| documents.into_iter().map(ParsedDocument::from).collect())
}

fn collect_documents(source: &str) -> Result<Vec<Document>> {
    serde_yaml::Deserializer::from_str(source)
        .into_iter()
        .enumerate()
        .filter_map(
            |(index, deserializer)| match serde_yaml::Value::deserialize(deserializer) {
                Ok(value) if value.is_null() => None,
                Ok(value) => Some(parse_document(value, index + 1)),
                Err(error) => Some(
                    Err(error)
                        .with_context(|| format!("failed to parse YAML document {}", index + 1)),
                ),
            },
        )
        .collect::<Result<Vec<_>>>()
}

fn parse_document(value: serde_yaml::Value, document_index: usize) -> Result<Document> {
    let kind = value
        .get("kind")
        .and_then(serde_yaml::Value::as_str)
        .ok_or_else(|| anyhow!("document {document_index} is missing kind"))?;
    let has_api_version = value.get("apiVersion").is_some();

    match (kind, has_api_version) {
        ("Provider", true) => serde_yaml::from_value(value)
            .map_err(anyhow::Error::from)
            .and_then(validate_provider_manifest)
            .map(Document::ProviderManifest)
            .with_context(|| {
                format!("failed to deserialize canonical Provider in document {document_index}")
            }),
        ("Provider", false) => serde_yaml::from_value(value)
            .map(Document::InlineProvider)
            .with_context(|| {
                format!("failed to deserialize inline Provider in document {document_index}")
            }),
        ("Workspace", true) => serde_yaml::from_value(value)
            .map_err(anyhow::Error::from)
            .and_then(validate_workspace_manifest)
            .map(Document::WorkspaceManifest)
            .with_context(|| {
                format!("failed to deserialize canonical Workspace in document {document_index}")
            }),
        ("Workspace", false) => serde_yaml::from_value(value)
            .map(Document::LegacyWorkspace)
            .with_context(|| {
                format!("failed to deserialize legacy Workspace in document {document_index}")
            }),
        ("WorkspaceLock", _) => serde_yaml::from_value(value)
            .map_err(anyhow::Error::from)
            .and_then(validate_workspace_lock)
            .map(Document::WorkspaceLock)
            .with_context(|| {
                format!("failed to deserialize WorkspaceLock in document {document_index}")
            }),
        ("Tool", _) => serde_yaml::from_value(value)
            .map(Document::Tool)
            .with_context(|| format!("failed to deserialize Tool in document {document_index}")),
        ("Bundle", _) => serde_yaml::from_value(value)
            .map(Document::Bundle)
            .with_context(|| format!("failed to deserialize Bundle in document {document_index}")),
        ("Asset", _) => serde_yaml::from_value(value)
            .map(Document::Asset)
            .with_context(|| format!("failed to deserialize Asset in document {document_index}")),
        ("Environment", _) => serde_yaml::from_value(value)
            .map(Document::Environment)
            .with_context(|| {
                format!("failed to deserialize Environment in document {document_index}")
            }),
        ("Secret", _) => serde_yaml::from_value(value)
            .map(Document::Secret)
            .with_context(|| format!("failed to deserialize Secret in document {document_index}")),
        (other, _) => bail!("unsupported kind '{other}' in document {document_index}"),
    }
}

fn normalize_documents(
    documents: Vec<Document>,
    fallback_provider_name: &str,
) -> Result<InternalModel> {
    let mut model = InternalModel::default();

    for document in documents {
        match document {
            Document::InlineProvider(mut provider) => {
                let provider_name =
                    normalize_provider_name(provider.metadata.take(), fallback_provider_name)?;
                provider.metadata = Some(Metadata {
                    name: provider_name.clone(),
                });

                for provider_tool in provider.spec.tools.iter().cloned() {
                    let tool = Tool::from_provider_tool(&provider_name, provider_tool)?;
                    let tool_name = required_metadata_name(tool.metadata.as_ref(), "tool")?;

                    insert_unique(&mut model.tools, tool_name.to_owned(), tool, "tool")?;
                }

                insert_unique(&mut model.providers, provider_name, provider, "provider")?;
            }
            Document::Tool(mut tool) => {
                let tool_name = normalize_required_metadata_name(tool.metadata.take(), "tool")?;
                tool.metadata = Some(Metadata {
                    name: tool_name.clone(),
                });

                insert_unique(&mut model.tools, tool_name, tool, "tool")?;
            }
            Document::Asset(mut asset) => {
                let asset_name = normalize_required_metadata_name(asset.metadata.take(), "asset")?;
                asset.metadata = Some(Metadata {
                    name: asset_name.clone(),
                });

                insert_unique(&mut model.assets, asset_name, asset, "asset")?;
            }
            Document::Environment(mut environment) => {
                let env_name =
                    normalize_required_metadata_name(environment.metadata.take(), "environment")?;
                environment.metadata = Some(Metadata {
                    name: env_name.clone(),
                });

                insert_unique(&mut model.envs, env_name, environment, "environment")?;
            }
            Document::ProviderManifest(_)
            | Document::WorkspaceManifest(_)
            | Document::WorkspaceLock(_)
            | Document::Bundle(_)
            | Document::Secret(_)
            | Document::LegacyWorkspace(_) => {}
        }
    }

    Ok(model)
}

fn validate_workspace_manifest(manifest: WorkspaceManifest) -> Result<WorkspaceManifest> {
    validate_api_version(&manifest.api_version, "workspace manifest")?;
    normalized_name(&manifest.workspace, "workspace")?;

    for (alias, provider_ref) in &manifest.providers {
        normalized_name(alias, "provider alias")?;
        normalized_name(&provider_ref.source, "provider source")?;
    }

    Ok(manifest)
}

fn validate_provider_manifest(manifest: ProviderManifest) -> Result<ProviderManifest> {
    validate_api_version(&manifest.api_version, "provider manifest")?;
    normalized_name(&manifest.metadata.namespace, "provider namespace")?;
    normalized_name(&manifest.metadata.name, "provider name")?;
    normalized_name(&manifest.metadata.version, "provider version")?;
    normalized_name(&manifest.spec.entrypoint, "provider entrypoint")?;

    if manifest.spec.platforms.is_empty() {
        bail!("provider manifest must declare at least one platform");
    }

    for platform in &manifest.spec.platforms {
        normalized_name(&platform.os, "platform os")?;
        normalized_name(&platform.arch, "platform arch")?;
        normalized_name(&platform.binary, "platform binary")?;
    }

    for capability_name in manifest.spec.capabilities.keys() {
        normalized_name(capability_name, "capability name")?;
    }

    for (env_key, env_value) in &manifest.spec.env {
        normalized_name(env_key, "provider env key")?;
        if env_key.starts_with("TPX_") {
            bail!("provider env key '{env_key}' uses reserved TPX_ prefix");
        }
        normalized_name(env_value, "provider env value")?;
    }

    for path_entry in &manifest.spec.path {
        normalized_name(path_entry, "provider path entry")?;
    }

    if let Some(layers) = &manifest.spec.layers {
        if let Some(assets) = &layers.assets {
            normalized_name(&assets.root, "assets root")?;

            for include in &assets.includes {
                normalized_name(include, "asset include pattern")?;
            }
        }
    }

    Ok(manifest)
}

fn validate_workspace_lock(lock: WorkspaceLock) -> Result<WorkspaceLock> {
    validate_api_version(&lock.api_version, "workspace lock")?;
    normalized_name(&lock.workspace, "workspace lock name")?;

    for provider in &lock.providers {
        normalized_name(&provider.alias, "locked provider alias")?;
        normalized_name(&provider.provider, "locked provider name")?;
        normalized_name(&provider.source, "locked provider source")?;
        normalized_name(&provider.version, "locked provider version")?;
        normalized_name(&provider.resolved, "locked provider resolved ref")?;
        normalized_name(&provider.store, "locked provider store id")?;
    }

    Ok(lock)
}

fn validate_api_version(api_version: &str, kind: &str) -> Result<()> {
    if api_version.trim() != API_VERSION {
        bail!("{kind} apiVersion must be {API_VERSION}");
    }

    Ok(())
}

fn fallback_provider_name(path: &str) -> String {
    PathBuf::from(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("provider")
        .to_owned()
}

fn normalize_provider_name(metadata: Option<Metadata>, fallback: &str) -> Result<String> {
    match metadata {
        Some(metadata) => Ok(normalized_name(&metadata.name, "provider")?.to_owned()),
        None => Ok(normalized_name(fallback, "provider")?.to_owned()),
    }
}

fn normalize_required_metadata_name(metadata: Option<Metadata>, kind: &str) -> Result<String> {
    match metadata {
        Some(metadata) => Ok(normalized_name(&metadata.name, kind)?.to_owned()),
        None => bail!("missing {kind} metadata.name"),
    }
}

fn required_metadata_name<'a>(metadata: Option<&'a Metadata>, kind: &str) -> Result<&'a str> {
    match metadata {
        Some(metadata) => normalized_name(&metadata.name, kind),
        None => bail!("missing {kind} metadata.name"),
    }
}

fn normalized_name<'a>(value: &'a str, kind: &str) -> Result<&'a str> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        bail!("{kind} name cannot be empty");
    }

    Ok(trimmed)
}

fn insert_unique<T>(
    target: &mut HashMap<String, T>,
    name: String,
    value: T,
    kind: &str,
) -> Result<()> {
    if target.insert(name.clone(), value).is_some() {
        bail!("duplicate {kind} '{name}'");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env, fs,
        path::{Path, PathBuf},
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn parses_inline_provider_yaml() {
        let path = write_fixture(
            "inline-provider",
            r#"kind: Provider
spec:
  tools:
    - name: kubectl
      runtime: local
"#,
        );

        let model = parse_file(path.to_str().expect("fixture path should be valid UTF-8"))
            .expect("inline provider YAML should parse");
        let provider_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("fixture path should contain a file stem");

        assert_eq!(model.providers.len(), 1);
        assert!(model.providers.contains_key(provider_name));

        let tool = model
            .tools
            .get("kubectl")
            .expect("tool should be normalized");
        assert_eq!(tool.spec.runtime.as_deref(), Some("local"));
        assert_eq!(tool.spec.provider.as_deref(), Some(provider_name));

        remove_fixture(&path);
    }

    #[test]
    fn parses_multi_document_yaml() {
        let path = write_fixture(
            "multi-kind",
            r#"---
kind: Provider
metadata:
  name: builtins
spec:
  tools:
    - name: helm
      runtime: script
---
kind: Tool
metadata:
  name: kubectl
spec:
  runtime: oci
  dependencies:
    - helm
---
kind: Asset
metadata:
  name: kubectl-bundle
spec:
  source: oci://ghcr.io/sourceplane/kubectl
---
kind: Environment
metadata:
  name: cluster-env
spec:
  values:
    KUBECONFIG: /tmp/kubeconfig
---
kind: Bundle
metadata:
  name: base
spec:
  tools:
    - kubectl
    - helm
---
kind: Secret
metadata:
  name: registry
spec:
  provider: vault
  key: registry/token
---
kind: Workspace
metadata:
  name: dev
spec:
  providers:
    - builtins
  tools:
    - kubectl
"#,
        );

        let model = parse_file(path.to_str().expect("fixture path should be valid UTF-8"))
            .expect("multi-document YAML should parse");

        assert_eq!(model.providers.len(), 1);
        assert_eq!(model.tools.len(), 2);
        assert_eq!(model.assets.len(), 1);
        assert_eq!(model.envs.len(), 1);
        assert!(model.tools.contains_key("helm"));
        assert!(model.tools.contains_key("kubectl"));
        assert!(model.assets.contains_key("kubectl-bundle"));
        assert!(model.envs.contains_key("cluster-env"));

        remove_fixture(&path);
    }

    #[test]
    fn fails_on_duplicate_tool_names() {
        let path = write_fixture(
            "duplicate-tool",
            r#"---
kind: Tool
metadata:
  name: kubectl
spec:
  runtime: local
---
kind: Tool
metadata:
  name: kubectl
spec:
  runtime: oci
"#,
        );

        let error = parse_file(path.to_str().expect("fixture path should be valid UTF-8"))
            .expect_err("duplicate tool names should fail");

        assert!(error.to_string().contains("duplicate tool 'kubectl'"));

        remove_fixture(&path);
    }

    #[test]
    fn parses_workspace_manifest_with_provider_refs() {
        let path = write_fixture(
            "workspace-manifest",
            r#"apiVersion: tpx.io/v1
kind: Workspace
workspace: dev
metadata:
  name: Developer Workspace
providers:
  node: core/node
  kubectl:
    source: ghcr.io/acme/kubectl:v1.31.0
    plainHTTP: true
"#,
        );

        let document =
            parse_document_file(path.to_str().expect("fixture path should be valid UTF-8"))
                .expect("workspace manifest should parse");

        match document {
            ParsedDocument::WorkspaceManifest(workspace) => {
                assert_eq!(workspace.api_version, API_VERSION);
                assert_eq!(workspace.workspace, "dev");
                assert_eq!(
                    workspace
                        .metadata
                        .as_ref()
                        .map(|metadata| metadata.name.as_str()),
                    Some("Developer Workspace")
                );
                assert_eq!(workspace.providers.len(), 2);
                assert_eq!(
                    workspace
                        .providers
                        .get("node")
                        .map(|provider| provider.source.as_str()),
                    Some("core/node")
                );
                assert_eq!(
                    workspace
                        .providers
                        .get("node")
                        .map(|provider| provider.plain_http),
                    Some(false)
                );
                assert_eq!(
                    workspace
                        .providers
                        .get("kubectl")
                        .map(|provider| provider.source.as_str()),
                    Some("ghcr.io/acme/kubectl:v1.31.0")
                );
                assert_eq!(
                    workspace
                        .providers
                        .get("kubectl")
                        .map(|provider| provider.plain_http),
                    Some(true)
                );
            }
            other => panic!("expected WorkspaceManifest, got {other:?}"),
        }

        remove_fixture(&path);
    }

    #[test]
    fn parses_provider_manifest_with_platforms_and_assets() {
        let path = write_fixture(
            "provider-manifest",
            r#"apiVersion: tpx.io/v1
kind: Provider
metadata:
  namespace: acme
  name: node
  version: v20.19.0
  description: Node.js runtime provider
spec:
  runtime: binary
  entrypoint: node
  platforms:
    - os: darwin
      arch: arm64
      binary: bin/darwin/arm64/node
    - os: linux
      arch: amd64
      binary: bin/linux/amd64/node
  capabilities:
    build:
      description: Compile the application
  env:
    NODE_EXTRA_CA_CERTS: ${provider_assets}/certs/root-ca.pem
  path:
    - tools/bin
  layers:
    assets:
      root: assets
      includes:
        - certs/*.pem
"#,
        );

        let document =
            parse_document_file(path.to_str().expect("fixture path should be valid UTF-8"))
                .expect("provider manifest should parse");

        match document {
            ParsedDocument::ProviderManifest(provider) => {
                assert_eq!(provider.api_version, API_VERSION);
                assert_eq!(provider.metadata.namespace, "acme");
                assert_eq!(provider.metadata.name, "node");
                assert_eq!(provider.metadata.version, "v20.19.0");
                assert_eq!(provider.spec.runtime, ProviderRuntime::Binary);
                assert_eq!(provider.spec.entrypoint, "node");
                assert_eq!(provider.spec.platforms.len(), 2);
                assert_eq!(provider.spec.platforms[0].os, "darwin");
                assert_eq!(provider.spec.platforms[1].arch, "amd64");
                assert_eq!(
                    provider
                        .spec
                        .capabilities
                        .get("build")
                        .and_then(|capability| capability.description.as_deref()),
                    Some("Compile the application")
                );
                assert_eq!(
                    provider
                        .spec
                        .env
                        .get("NODE_EXTRA_CA_CERTS")
                        .map(String::as_str),
                    Some("${provider_assets}/certs/root-ca.pem")
                );
                assert_eq!(provider.spec.path, vec!["tools/bin"]);
                assert_eq!(
                    provider
                        .spec
                        .layers
                        .as_ref()
                        .and_then(|layers| layers.assets.as_ref())
                        .map(|assets| assets.root.as_str()),
                    Some("assets")
                );
            }
            other => panic!("expected ProviderManifest, got {other:?}"),
        }

        remove_fixture(&path);
    }

    #[test]
    fn parses_workspace_lock_document() {
        let path = write_fixture(
            "workspace-lock",
            r#"apiVersion: tpx.io/v1
kind: WorkspaceLock
workspace: dev
providers:
  - alias: node
    provider: core/node
    source: core/node
    version: v20.19.0
    resolved: ghcr.io/acme/node-provider@sha256:1234
    store: abc123
"#,
        );

        let document =
            parse_document_file(path.to_str().expect("fixture path should be valid UTF-8"))
                .expect("workspace lock should parse");

        match document {
            ParsedDocument::WorkspaceLock(lock) => {
                assert_eq!(lock.api_version, API_VERSION);
                assert_eq!(lock.workspace, "dev");
                assert_eq!(lock.providers.len(), 1);
                assert_eq!(lock.providers[0].alias, "node");
                assert_eq!(
                    lock.providers[0].resolved,
                    "ghcr.io/acme/node-provider@sha256:1234"
                );
            }
            other => panic!("expected WorkspaceLock, got {other:?}"),
        }

        remove_fixture(&path);
    }

    #[test]
    fn fails_on_invalid_api_version() {
        let path = write_fixture(
            "invalid-api-version",
            r#"apiVersion: tinx.io/v1
kind: Workspace
workspace: dev
providers:
  node: core/node
"#,
        );

        let error = parse_document_file(path.to_str().expect("fixture path should be valid UTF-8"))
            .expect_err("invalid apiVersion should fail");
        let error_message = format!("{error:#}");

        assert!(error_message.contains("apiVersion must be tpx.io/v1"));

        remove_fixture(&path);
    }

    fn write_fixture(stem: &str, contents: &str) -> PathBuf {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "tpx-parser-{stem}-{}-{unique_suffix}.yaml",
            process::id()
        ));

        fs::write(&path, contents).expect("fixture should be written");

        path
    }

    fn remove_fixture(path: &Path) {
        fs::remove_file(path).expect("fixture should be removed");
    }
}
