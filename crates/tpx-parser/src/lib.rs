use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::PathBuf};

pub const COMPONENT_NAME: &str = "tpx-parser";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InternalModel {
    pub tools: HashMap<String, Tool>,
    pub assets: HashMap<String, Asset>,
    pub envs: HashMap<String, Environment>,
    pub providers: HashMap<String, Provider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Metadata {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Provider {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: ProviderSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ProviderSpec {
    #[serde(default)]
    pub tools: Vec<ProviderTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ProviderTool {
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
pub struct Workspace {
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub spec: WorkspaceSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct WorkspaceSpec {
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, alias = "env", alias = "environment", alias = "environments")]
    pub envs: Vec<String>,
}

enum Document {
    Provider(Provider),
    Tool(Tool),
    Bundle,
    Asset(Asset),
    Environment(Environment),
    Secret,
    Workspace,
}

impl Tool {
    fn from_provider_tool(provider_name: &str, provider_tool: ProviderTool) -> Result<Self> {
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
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read parser input at {path}"))?;

    parse_source(&source, fallback_provider_name(path))
}

fn parse_source(source: &str, fallback_provider_name: String) -> Result<InternalModel> {
    if source.trim().is_empty() {
        return Ok(InternalModel::default());
    }

    let documents = serde_yaml::Deserializer::from_str(source)
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
        .collect::<Result<Vec<_>>>()?;

    normalize_documents(documents, &fallback_provider_name)
}

fn parse_document(value: serde_yaml::Value, document_index: usize) -> Result<Document> {
    let kind = value
        .get("kind")
        .and_then(serde_yaml::Value::as_str)
        .ok_or_else(|| anyhow!("document {document_index} is missing kind"))?;

    match kind {
        "Provider" => serde_yaml::from_value(value)
            .map(Document::Provider)
            .with_context(|| {
                format!("failed to deserialize Provider in document {document_index}")
            }),
        "Tool" => serde_yaml::from_value(value)
            .map(Document::Tool)
            .with_context(|| format!("failed to deserialize Tool in document {document_index}")),
        "Bundle" => serde_yaml::from_value::<Bundle>(value)
            .map(|_| Document::Bundle)
            .with_context(|| format!("failed to deserialize Bundle in document {document_index}")),
        "Asset" => serde_yaml::from_value(value)
            .map(Document::Asset)
            .with_context(|| format!("failed to deserialize Asset in document {document_index}")),
        "Environment" => serde_yaml::from_value(value)
            .map(Document::Environment)
            .with_context(|| {
                format!("failed to deserialize Environment in document {document_index}")
            }),
        "Secret" => serde_yaml::from_value::<Secret>(value)
            .map(|_| Document::Secret)
            .with_context(|| format!("failed to deserialize Secret in document {document_index}")),
        "Workspace" => serde_yaml::from_value::<Workspace>(value)
            .map(|_| Document::Workspace)
            .with_context(|| {
                format!("failed to deserialize Workspace in document {document_index}")
            }),
        other => bail!("unsupported kind '{other}' in document {document_index}"),
    }
}

fn normalize_documents(
    documents: Vec<Document>,
    fallback_provider_name: &str,
) -> Result<InternalModel> {
    let mut model = InternalModel::default();

    for document in documents {
        match document {
            Document::Provider(mut provider) => {
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
            Document::Bundle | Document::Secret | Document::Workspace => {}
        }
    }

    Ok(model)
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
