use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, error::ErrorKind};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tpx_core::{Context as CoreContext, Engine};
use tpx_parser::{
    API_VERSION, InternalModel, Metadata, ParsedDocument, Tool, ToolSpec, WorkspaceManifest,
    WorkspaceProviderRef, parse_document_file, parse_documents_file, parse_file,
};
use tpx_runtime::BUILTIN_RUNTIMES;
use tpx_workspace::{
    WORKSPACE_MANIFEST_FILE, discover_workspace_root, load_workspace, resolve_tpx_home,
};

#[derive(Debug, Parser)]
#[command(
    name = "tpx",
    version,
    about = "TPX CLI",
    arg_required_else_help = true
)]
pub struct Cli {
    #[arg(short = 'w', long = "workspace", global = true)]
    pub workspace: Option<PathBuf>,
    #[arg(long = "tpx-home", global = true)]
    pub tpx_home: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Init {
        #[arg(long)]
        name: Option<String>,
    },
    Run {
        tool: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    Add {
        source: String,
        #[arg(long = "alias", short = 'a')]
        alias: Option<String>,
        #[arg(long = "plain-http")]
        plain_http: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    List,
}

#[derive(Debug, Subcommand)]
pub enum RuntimeCommand {
    List,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    Clean,
}

pub fn run_cli_from<I, T, W, E>(args: I, cwd: &Path, stdout: &mut W, stderr: &mut E) -> Result<i32>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    W: Write,
    E: Write,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => return handle_clap_error(error, stdout, stderr),
    };

    match cli.command {
        Command::Init { name } => {
            let manifest_path = init_workspace(cwd, name, cli.tpx_home.as_deref())?;
            writeln!(
                stdout,
                "initialized workspace at {}",
                manifest_path.display()
            )?;
            Ok(0)
        }
        Command::Run { tool, args } => run_tool(cwd, cli.workspace.as_deref(), &tool, &args),
        Command::Provider { command } => match command {
            ProviderCommand::Add {
                source,
                alias,
                plain_http,
            } => {
                let added_alias = provider_add(
                    cwd,
                    cli.workspace.as_deref(),
                    cli.tpx_home.as_deref(),
                    &source,
                    alias,
                    plain_http,
                )?;
                writeln!(stdout, "added provider '{}' as '{}'", source, added_alias)?;
                Ok(0)
            }
        },
        Command::Tools { command } => match command {
            ToolsCommand::List => {
                for tool in list_tools(cwd, cli.workspace.as_deref())? {
                    writeln!(stdout, "{tool}")?;
                }
                Ok(0)
            }
        },
        Command::Runtime { command } => match command {
            RuntimeCommand::List => {
                for runtime in BUILTIN_RUNTIMES {
                    writeln!(stdout, "{runtime}")?;
                }
                Ok(0)
            }
        },
        Command::Cache { command } => match command {
            CacheCommand::Clean => {
                let removed = clean_cache(cli.tpx_home.as_deref())?;
                writeln!(stdout, "cache clean complete ({removed} paths removed)")?;
                Ok(0)
            }
        },
    }
}

fn handle_clap_error<W: Write, E: Write>(
    error: clap::Error,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<i32> {
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            write!(stdout, "{error}")?;
            Ok(0)
        }
        _ => {
            write!(stderr, "{error}")?;
            Ok(2)
        }
    }
}

fn init_workspace(cwd: &Path, name: Option<String>, tpx_home: Option<&Path>) -> Result<PathBuf> {
    let manifest_path = cwd.join(WORKSPACE_MANIFEST_FILE);

    if manifest_path.exists() {
        bail!(
            "workspace manifest already exists at {}",
            manifest_path.display()
        );
    }

    let workspace_name = name.unwrap_or_else(|| default_workspace_name(cwd));
    let manifest = WorkspaceManifest {
        api_version: API_VERSION.to_owned(),
        workspace: workspace_name.clone(),
        metadata: Some(Metadata {
            name: workspace_name,
        }),
        providers: BTreeMap::new(),
    };

    write_workspace_manifest(&manifest_path, &manifest)?;
    load_workspace(cwd, tpx_home)?.write_lazy_layout()?;

    Ok(manifest_path)
}

fn run_tool(cwd: &Path, workspace: Option<&Path>, tool: &str, args: &[String]) -> Result<i32> {
    let manifest_path = resolve_manifest_path(cwd, workspace)?;
    let model = load_execution_model(&manifest_path)?;
    let context = CoreContext::new(model);

    Engine::new().run(tool, args, &context)
}

fn provider_add(
    cwd: &Path,
    workspace: Option<&Path>,
    tpx_home: Option<&Path>,
    source: &str,
    alias: Option<String>,
    plain_http: bool,
) -> Result<String> {
    let manifest_path = resolve_manifest_path(cwd, workspace)?;
    let mut manifest = load_workspace_manifest(&manifest_path)?;
    let alias = alias.unwrap_or_else(|| derive_alias(source));

    if manifest.providers.contains_key(&alias) {
        bail!("provider alias '{}' already exists", alias);
    }

    manifest.providers.insert(
        alias.clone(),
        WorkspaceProviderRef {
            source: source.to_owned(),
            plain_http,
        },
    );

    write_workspace_manifest(&manifest_path, &manifest)?;
    let root = manifest_path.parent().ok_or_else(|| {
        anyhow!(
            "cannot derive workspace root from {}",
            manifest_path.display()
        )
    })?;
    load_workspace(root, tpx_home)?.write_lazy_layout()?;

    Ok(alias)
}

fn list_tools(cwd: &Path, workspace: Option<&Path>) -> Result<Vec<String>> {
    let manifest_path = resolve_manifest_path(cwd, workspace)?;

    match load_workspace_descriptor(&manifest_path)? {
        WorkspaceDescriptor::Canonical(manifest) => {
            Ok(manifest.providers.keys().cloned().collect())
        }
        WorkspaceDescriptor::Legacy(model) => {
            let mut tools = model.tools.keys().cloned().collect::<Vec<_>>();
            tools.sort();
            Ok(tools)
        }
    }
}

fn clean_cache(tpx_home: Option<&Path>) -> Result<usize> {
    let home = resolve_tpx_home(tpx_home)?;
    let mut removed = 0usize;

    for subdir in ["providers", "store", "runtime"] {
        let path = home.join(subdir);

        if path.is_dir() {
            fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove cache directory {}", path.display()))?;
            removed += 1;
        } else if path.is_file() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove cache file {}", path.display()))?;
            removed += 1;
        }
    }

    Ok(removed)
}

fn resolve_manifest_path(cwd: &Path, workspace: Option<&Path>) -> Result<PathBuf> {
    if let Some(workspace) = workspace {
        let root = normalize_workspace_override(cwd, workspace)?;
        let manifest_path = root.join(WORKSPACE_MANIFEST_FILE);

        if manifest_path.is_file() {
            return Ok(manifest_path);
        }

        bail!(
            "workspace manifest not found at {}",
            manifest_path.display()
        );
    }

    let root = discover_workspace_root(cwd)?.ok_or_else(|| {
        anyhow!(
            "failed to resolve workspace: no {} found from {}",
            WORKSPACE_MANIFEST_FILE,
            cwd.display()
        )
    })?;

    Ok(root.join(WORKSPACE_MANIFEST_FILE))
}

fn normalize_workspace_override(cwd: &Path, workspace: &Path) -> Result<PathBuf> {
    let candidate = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        cwd.join(workspace)
    };
    let root =
        if candidate.file_name().and_then(|name| name.to_str()) == Some(WORKSPACE_MANIFEST_FILE) {
            candidate.parent().map(Path::to_path_buf).ok_or_else(|| {
                anyhow!("cannot derive workspace root from {}", candidate.display())
            })?
        } else {
            candidate
        };

    fs::canonicalize(&root)
        .with_context(|| format!("failed to canonicalize workspace path {}", root.display()))
}

enum WorkspaceDescriptor {
    Canonical(WorkspaceManifest),
    Legacy(InternalModel),
}

fn load_workspace_descriptor(manifest_path: &Path) -> Result<WorkspaceDescriptor> {
    let source_path = path_as_str(manifest_path)?;
    let documents = parse_documents_file(source_path)?;

    if documents.len() == 1 {
        if let Some(ParsedDocument::WorkspaceManifest(manifest)) = documents.into_iter().next() {
            return Ok(WorkspaceDescriptor::Canonical(manifest));
        }
    }

    Ok(WorkspaceDescriptor::Legacy(parse_file(source_path)?))
}

fn load_execution_model(manifest_path: &Path) -> Result<InternalModel> {
    match load_workspace_descriptor(manifest_path)? {
        WorkspaceDescriptor::Canonical(manifest) => Ok(internal_model_from_workspace(&manifest)),
        WorkspaceDescriptor::Legacy(model) => Ok(model),
    }
}

fn load_workspace_manifest(manifest_path: &Path) -> Result<WorkspaceManifest> {
    match parse_document_file(path_as_str(manifest_path)?)? {
        ParsedDocument::WorkspaceManifest(manifest) => Ok(manifest),
        other => bail!(
            "expected canonical Workspace manifest in {}, found {other:?}",
            manifest_path.display()
        ),
    }
}

fn write_workspace_manifest(manifest_path: &Path, manifest: &WorkspaceManifest) -> Result<()> {
    #[derive(serde::Serialize)]
    struct WorkspaceManifestDocument<'a> {
        #[serde(rename = "apiVersion")]
        api_version: &'a str,
        kind: &'static str,
        workspace: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<&'a Metadata>,
        providers: &'a BTreeMap<String, WorkspaceProviderRef>,
    }

    let document = WorkspaceManifestDocument {
        api_version: &manifest.api_version,
        kind: "Workspace",
        workspace: &manifest.workspace,
        metadata: manifest.metadata.as_ref(),
        providers: &manifest.providers,
    };

    fs::write(
        manifest_path,
        serde_yaml::to_string(&document).context("failed to serialize workspace manifest")?,
    )
    .with_context(|| {
        format!(
            "failed to write workspace manifest at {}",
            manifest_path.display()
        )
    })
}

fn internal_model_from_workspace(manifest: &WorkspaceManifest) -> InternalModel {
    let mut model = InternalModel::default();

    for (alias, provider_ref) in &manifest.providers {
        let mut tool = Tool {
            metadata: Some(Metadata {
                name: alias.clone(),
            }),
            spec: ToolSpec {
                runtime: Some(infer_runtime(&provider_ref.source).to_owned()),
                provider: Some(provider_ref.source.clone()),
                ..ToolSpec::default()
            },
        };
        tool.spec.assets.push(provider_ref.source.clone());
        model.tools.insert(alias.clone(), tool);
    }

    model
}

fn infer_runtime(source: &str) -> &str {
    if source.starts_with("inline:") || source.ends_with(".sh") {
        "script"
    } else if source.starts_with("./") || source.starts_with('/') {
        "local"
    } else {
        "oci"
    }
}

fn derive_alias(source: &str) -> String {
    let tail = source
        .rsplit('/')
        .next()
        .unwrap_or(source)
        .split('@')
        .next()
        .unwrap_or(source)
        .split(':')
        .next()
        .unwrap_or(source)
        .trim();

    if tail.is_empty() {
        "provider".into()
    } else {
        tail.into()
    }
}

fn default_workspace_name(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace")
        .to_owned()
}

fn path_as_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path {} is not valid UTF-8", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env, process,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn init_creates_workspace_manifest_and_lazy_layout() {
        let temp = temp_dir("tpx-cli-init");
        let (code, stdout, stderr) = run_cli(&["tpx", "init"], temp.path());

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(stdout.contains("initialized workspace at"));
        assert!(temp.path().join("tpx.yaml").is_file());
        assert!(temp.path().join(".tpx/bin").is_dir());
        assert!(temp.path().join(".tpx/env").is_file());
    }

    #[test]
    fn provider_add_updates_workspace_manifest() {
        let temp = temp_dir("tpx-cli-provider-add");

        assert_eq!(run_cli(&["tpx", "init"], temp.path()).0, 0);

        let (code, stdout, stderr) = run_cli(&["tpx", "provider", "add", "core/node"], temp.path());

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(stdout.contains("added provider 'core/node' as 'node'"));

        let manifest = load_workspace_manifest(&temp.path().join("tpx.yaml"))
            .expect("workspace manifest should be readable");
        assert_eq!(manifest.providers.len(), 1);
        assert_eq!(
            manifest
                .providers
                .get("node")
                .map(|provider| provider.source.as_str()),
            Some("core/node")
        );
    }

    #[test]
    fn tools_list_outputs_workspace_aliases() {
        let temp = temp_dir("tpx-cli-tools-list");

        assert_eq!(run_cli(&["tpx", "init"], temp.path()).0, 0);
        assert_eq!(
            run_cli(
                &["tpx", "provider", "add", "ghcr.io/acme/kubectl:v1.31.0"],
                temp.path(),
            )
            .0,
            0
        );

        let (code, stdout, stderr) = run_cli(&["tpx", "tools", "list"], temp.path());

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert_eq!(stdout, "kubectl\n");
    }

    #[test]
    fn runtime_list_outputs_builtin_runtimes() {
        let temp = temp_dir("tpx-cli-runtime-list");
        let (code, stdout, stderr) = run_cli(&["tpx", "runtime", "list"], temp.path());

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert_eq!(stdout, "local\nscript\noci\n");
    }

    #[test]
    fn cache_clean_removes_cache_directories() {
        let temp = temp_dir("tpx-cli-cache-clean");
        let home = temp.path().join("home");

        fs::create_dir_all(home.join("providers")).expect("providers cache should be created");
        fs::create_dir_all(home.join("store")).expect("store cache should be created");
        fs::create_dir_all(home.join("runtime")).expect("runtime cache should be created");

        let (code, stdout, stderr) = run_cli(
            &[
                "tpx",
                "--tpx-home",
                home.to_str().expect("home path should be valid UTF-8"),
                "cache",
                "clean",
            ],
            temp.path(),
        );

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(stdout.contains("cache clean complete (3 paths removed)"));
        assert!(!home.join("providers").exists());
        assert!(!home.join("store").exists());
        assert!(!home.join("runtime").exists());
    }

    #[test]
    fn run_uses_core_for_workspace_aliases() {
        let temp = temp_dir("tpx-cli-run");

        assert_eq!(run_cli(&["tpx", "init"], temp.path()).0, 0);
        assert_eq!(
            run_cli(&["tpx", "provider", "add", "core/node"], temp.path()).0,
            0
        );

        let (code, stdout, stderr) = run_cli(&["tpx", "run", "node"], temp.path());

        assert_eq!(code, 0);
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    fn run_cli(args: &[&str], cwd: &Path) -> (i32, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_cli_from(args.iter().copied(), cwd, &mut stdout, &mut stderr)
            .expect("CLI invocation should succeed");

        (
            code,
            String::from_utf8(stdout).expect("stdout should be valid UTF-8"),
            String::from_utf8(stderr).expect("stderr should be valid UTF-8"),
        )
    }

    fn temp_dir(prefix: &str) -> TempDir {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!("{prefix}-{}-{unique_suffix}", process::id()));

        fs::create_dir_all(&path).expect("temporary directory should be created");

        TempDir { path }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if self.path.exists() {
                fs::remove_dir_all(&self.path).expect("temporary directory should be removed");
            }
        }
    }
}
