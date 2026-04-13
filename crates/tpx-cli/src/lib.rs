use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum, error::ErrorKind};
use runtime_local::LocalRuntime;
use runtime_oci::OciRuntime;
use runtime_script::ScriptRuntime;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};
use tar::{Builder, Header, HeaderMode};
use tpx_core::{Context as CoreContext, Engine};
use tpx_parser::{
    API_VERSION, Metadata, ParsedDocument, ProviderManifest, Tool, ToolSpec, WorkspaceManifest,
    WorkspaceProviderRef, parse_document_file,
};
use tpx_runtime::{BUILTIN_RUNTIMES, RuntimeRegistry};
use tpx_shim::{LazyShimSpec, ShimManager, ShimSpec};
use tpx_workspace::{
    HomeConfig, ProviderRuntimeState, ResolveOptions, WORKSPACE_LAZY_DIR, WORKSPACE_MANIFEST_FILE,
    WORKSPACE_RUNTIME_DIR, WorkspaceContext, WorkspaceRuntimeState, WorkspaceStatus,
    load_home_config, load_workspace, register_workspace, registered_workspaces, resolve_tpx_home,
    resolve_workspace, save_home_config, set_active_workspace, unregister_workspace,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InventoryScope {
    Workspace,
    Default,
}

#[derive(Debug, Parser)]
#[command(
    name = "tpx",
    version,
    about = "OCI-native provider runtime and packager",
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[arg(short = 'w', long = "workspace", global = true)]
    pub workspace: Option<String>,
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
    Install {
        reference: String,
        #[arg(value_parser = ["as"])]
        as_keyword: Option<String>,
        alias: Option<String>,
    },
    Exec {
        command: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Shell,
    Status {
        #[arg(short, long)]
        short: bool,
        #[arg(short, long)]
        verbose: bool,
    },
    Pack {
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long = "artifact-root")]
        artifact_root: Option<PathBuf>,
        #[arg(long)]
        tag: Option<String>,
    },
    Release {
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long = "dist")]
        artifact_root: Option<PathBuf>,
        #[arg(long)]
        push: Option<String>,
        #[arg(long)]
        tag: Option<String>,
    },
    Version,
    #[command(visible_aliases = ["ws", "workspaces"])]
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    #[command(visible_aliases = ["providers", "p"])]
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Use {
        workspace: String,
        #[arg(last = true)]
        command: Vec<String>,
    },
    Add {
        provider: String,
        #[arg(value_parser = ["as"])]
        as_keyword: Option<String>,
        alias: Option<String>,
        #[arg(long = "plain-http")]
        plain_http: bool,
    },
    Remove {
        target: String,
    },
    Update {
        targets: Vec<String>,
    },
    #[command(visible_alias = "ls")]
    List {
        #[command(subcommand)]
        command: Option<ListCommand>,
        #[arg(value_enum)]
        scope: Option<InventoryScope>,
    },
    Run {
        tool: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    Create {
        target: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },
    List {
        #[arg(long)]
        active: bool,
        #[arg(long)]
        ready: bool,
        #[arg(long)]
        missing: bool,
        #[arg(long)]
        short: bool,
    },
    Current,
    Use {
        workspace: String,
        #[arg(last = true)]
        command: Vec<String>,
    },
    Delete {
        workspace: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    Add {
        provider: String,
        #[arg(value_parser = ["as"])]
        as_keyword: Option<String>,
        alias: Option<String>,
        #[arg(long = "plain-http")]
        plain_http: bool,
    },
    List {
        #[arg(value_enum)]
        scope: Option<InventoryScope>,
    },
    Remove {
        target: String,
    },
    Update {
        targets: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ListCommand {
    Providers {
        #[arg(value_enum)]
        scope: Option<InventoryScope>,
    },
    Workspaces,
}

struct PreparedWorkspace {
    context: WorkspaceContext,
    state: WorkspaceRuntimeState,
}

struct ProviderStatusRow {
    alias: String,
    source: String,
    runtime: String,
    installed: bool,
    error: Option<String>,
}

struct ReleaseResult {
    layout_path: PathBuf,
    pushed_path: Option<PathBuf>,
}

const OCI_LAYOUT_FILE: &str = "oci-layout";
const OCI_INDEX_FILE: &str = "index.json";
const OCI_PROVIDER_MANIFEST_FILE: &str = "tpx.yaml";
const OCI_IMAGE_LAYOUT_VERSION: &str = "1.0.0";
const OCI_BLOB_ALGORITHM: &str = "sha256";
const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_IMAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";
const OCI_IMAGE_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

#[derive(serde::Serialize)]
struct OciImageLayoutDocument {
    #[serde(rename = "imageLayoutVersion")]
    image_layout_version: &'static str,
}

#[derive(serde::Serialize)]
struct OciIndexDocument<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType")]
    media_type: &'static str,
    manifests: &'a [OciDescriptor],
}

#[derive(Clone, serde::Serialize)]
struct OciDescriptor {
    #[serde(rename = "mediaType")]
    media_type: &'static str,
    digest: String,
    size: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    annotations: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<OciPlatformDescriptor>,
}

#[derive(Clone, serde::Serialize)]
struct OciPlatformDescriptor {
    architecture: String,
    os: String,
}

#[derive(serde::Serialize)]
struct OciManifestDocument<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType")]
    media_type: &'static str,
    config: &'a OciDescriptor,
    layers: &'a [OciDescriptor],
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    annotations: BTreeMap<String, String>,
}

#[derive(serde::Serialize)]
struct OciImageConfig<'a> {
    architecture: &'a str,
    os: &'a str,
    rootfs: OciRootFs,
}

#[derive(serde::Serialize)]
struct OciRootFs {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "diff_ids")]
    diff_ids: Vec<String>,
}

pub fn run_cli_from<I, T, W, E>(args: I, cwd: &Path, stdout: &mut W, stderr: &mut E) -> Result<i32>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    W: Write,
    E: Write,
{
    let raw_args = normalize_passthrough_args(args);
    let cli = match Cli::try_parse_from(raw_args) {
        Ok(cli) => cli,
        Err(error) => return handle_clap_error(error, stdout, stderr),
    };

    match cli.command {
        Command::Init { name } => {
            let context = create_or_materialize_workspace(cwd, cwd, name, cli.tpx_home.as_deref())?;
            writeln!(
                stdout,
                "initialized workspace '{}' at {}",
                context.workspace_name(),
                context.manifest_path.display()
            )?;
            Ok(0)
        }
        Command::Install {
            reference,
            as_keyword,
            alias,
        } => {
            let alias = parse_alias_keyword(as_keyword, alias, "install")?
                .unwrap_or_else(|| derive_alias(&reference));
            install_default_provider(cwd, cli.tpx_home.as_deref(), &reference, &alias)?;
            writeln!(stdout, "installed provider '{}' as '{}'", reference, alias)?;
            Ok(0)
        }
        Command::Exec { command, args } => execute_workspace_command(
            cwd,
            cli.workspace.as_deref(),
            cli.tpx_home.as_deref(),
            &command,
            &args,
        ),
        Command::Shell => {
            launch_workspace_shell(cwd, cli.workspace.as_deref(), cli.tpx_home.as_deref())
        }
        Command::Status { short, verbose } => {
            print_workspace_status(
                cwd,
                cli.workspace.as_deref(),
                cli.tpx_home.as_deref(),
                short,
                verbose,
                stdout,
            )?;
            Ok(0)
        }
        Command::Pack {
            manifest,
            output,
            artifact_root,
            tag,
        } => {
            let layout_path = pack_provider(
                cwd,
                manifest.as_deref(),
                output.as_deref(),
                artifact_root.as_deref(),
                tag.as_deref(),
            )?;
            writeln!(
                stdout,
                "packed provider OCI layout at {}",
                layout_path.display()
            )?;
            Ok(0)
        }
        Command::Release {
            manifest,
            output,
            artifact_root,
            push,
            tag,
        } => {
            let release = release_provider(
                cwd,
                manifest.as_deref(),
                output.as_deref(),
                artifact_root.as_deref(),
                push.as_deref(),
                tag.as_deref(),
            )?;
            writeln!(
                stdout,
                "released provider OCI layout {}",
                release.layout_path.display()
            )?;
            if let Some(pushed_path) = release.pushed_path {
                writeln!(stdout, "pushed OCI layout to {}", pushed_path.display())?;
            }
            Ok(0)
        }
        Command::Version => {
            writeln!(stdout, "{}", env!("CARGO_PKG_VERSION"))?;
            Ok(0)
        }
        Command::Workspace { command } => handle_workspace_command(
            command,
            cwd,
            cli.workspace.as_deref(),
            cli.tpx_home.as_deref(),
            stdout,
        ),
        Command::Provider { command } => handle_provider_command(
            command,
            cwd,
            cli.workspace.as_deref(),
            cli.tpx_home.as_deref(),
            stdout,
        ),
        Command::Use { workspace, command } => {
            workspace_use(cwd, cli.tpx_home.as_deref(), &workspace, &command, stdout)
        }
        Command::Add {
            provider,
            as_keyword,
            alias,
            plain_http,
        } => {
            let alias = parse_alias_keyword(as_keyword, alias, "add")?
                .unwrap_or_else(|| derive_alias(&provider));
            add_provider_to_workspace(
                cwd,
                cli.workspace.as_deref(),
                cli.tpx_home.as_deref(),
                &provider,
                &alias,
                plain_http,
            )?;
            writeln!(stdout, "added provider '{}' as '{}'", provider, alias)?;
            Ok(0)
        }
        Command::Remove { target } => {
            let removed = remove_provider_from_workspace(
                cwd,
                cli.workspace.as_deref(),
                cli.tpx_home.as_deref(),
                &target,
            )?;
            writeln!(stdout, "removed provider '{}'", removed)?;
            Ok(0)
        }
        Command::Update { targets } => {
            let updated = update_workspace_providers(
                cwd,
                cli.workspace.as_deref(),
                cli.tpx_home.as_deref(),
                &targets,
            )?;
            for alias in updated {
                writeln!(stdout, "updated provider '{}'", alias)?;
            }
            Ok(0)
        }
        Command::List { command, scope } => handle_list_command(
            command,
            scope,
            cwd,
            cli.workspace.as_deref(),
            cli.tpx_home.as_deref(),
            stdout,
        ),
        Command::Run { tool, args } => run_compat(
            cwd,
            cli.workspace.as_deref(),
            cli.tpx_home.as_deref(),
            &tool,
            &args,
            stderr,
        ),
    }
}

fn normalize_passthrough_args<I, T>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();

    if args.len() > 1 && args[1] == OsStr::new("--") {
        args.remove(1);
        args.insert(1, OsString::from("exec"));
    }

    args
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

fn handle_workspace_command<W: Write>(
    command: WorkspaceCommand,
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    stdout: &mut W,
) -> Result<i32> {
    match command {
        WorkspaceCommand::Create { target, name } => {
            let target = target.as_deref().unwrap_or(cwd);
            let context = create_or_materialize_workspace(cwd, target, name, tpx_home)?;
            writeln!(
                stdout,
                "workspace '{}' ready at {}",
                context.workspace_name(),
                context.root.display()
            )?;
            Ok(0)
        }
        WorkspaceCommand::List {
            active,
            ready,
            missing,
            short,
        } => {
            print_registered_workspaces(tpx_home, active, ready, missing, short, stdout)?;
            Ok(0)
        }
        WorkspaceCommand::Current => {
            let context = resolve_workspace_context(cwd, workspace_selection, tpx_home)?;
            writeln!(
                stdout,
                "{}\t{}",
                context.workspace_name(),
                context.root.display()
            )?;
            Ok(0)
        }
        WorkspaceCommand::Use { workspace, command } => {
            workspace_use(cwd, tpx_home, &workspace, &command, stdout)
        }
        WorkspaceCommand::Delete { workspace } => {
            let deleted_root = delete_workspace(cwd, tpx_home, &workspace)?;
            writeln!(
                stdout,
                "deleted workspace state at {}",
                deleted_root.display()
            )?;
            Ok(0)
        }
    }
}

fn handle_provider_command<W: Write>(
    command: ProviderCommand,
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    stdout: &mut W,
) -> Result<i32> {
    match command {
        ProviderCommand::Add {
            provider,
            as_keyword,
            alias,
            plain_http,
        } => {
            let alias = parse_alias_keyword(as_keyword, alias, "provider add")?
                .unwrap_or_else(|| derive_alias(&provider));
            add_provider_to_workspace(
                cwd,
                workspace_selection,
                tpx_home,
                &provider,
                &alias,
                plain_http,
            )?;
            writeln!(stdout, "added provider '{}' as '{}'", provider, alias)?;
            Ok(0)
        }
        ProviderCommand::List { scope } => {
            print_provider_inventory(
                cwd,
                workspace_selection,
                tpx_home,
                scope.unwrap_or(InventoryScope::Workspace),
                stdout,
            )?;
            Ok(0)
        }
        ProviderCommand::Remove { target } => {
            let removed =
                remove_provider_from_workspace(cwd, workspace_selection, tpx_home, &target)?;
            writeln!(stdout, "removed provider '{}'", removed)?;
            Ok(0)
        }
        ProviderCommand::Update { targets } => {
            let updated = update_workspace_providers(cwd, workspace_selection, tpx_home, &targets)?;
            for alias in updated {
                writeln!(stdout, "updated provider '{}'", alias)?;
            }
            Ok(0)
        }
    }
}

fn handle_list_command<W: Write>(
    command: Option<ListCommand>,
    scope: Option<InventoryScope>,
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    stdout: &mut W,
) -> Result<i32> {
    match command {
        Some(ListCommand::Providers { scope }) => {
            print_provider_inventory(
                cwd,
                workspace_selection,
                tpx_home,
                scope.unwrap_or(InventoryScope::Workspace),
                stdout,
            )?;
        }
        Some(ListCommand::Workspaces) => {
            print_registered_workspaces(tpx_home, false, false, false, true, stdout)?;
        }
        None => {
            print_provider_inventory(
                cwd,
                workspace_selection,
                tpx_home,
                scope.unwrap_or(InventoryScope::Workspace),
                stdout,
            )?;
        }
    }

    Ok(0)
}

fn parse_alias_keyword(
    as_keyword: Option<String>,
    alias: Option<String>,
    command_name: &str,
) -> Result<Option<String>> {
    match (as_keyword, alias) {
        (None, None) => Ok(None),
        (Some(_), Some(alias)) => Ok(Some(alias)),
        (Some(_), None) => bail!("{command_name} requires an alias after 'as'"),
        (None, Some(alias)) => Ok(Some(alias)),
    }
}

fn create_or_materialize_workspace(
    cwd: &Path,
    target: &Path,
    name: Option<String>,
    tpx_home: Option<&Path>,
) -> Result<WorkspaceContext> {
    let (root, manifest_path) = workspace_target_paths(cwd, target)?;

    fs::create_dir_all(&root)
        .with_context(|| format!("failed to create workspace directory {}", root.display()))?;

    if manifest_path.is_file() {
        let existing = load_workspace(&root, tpx_home)?;

        if let Some(expected_name) = name.as_deref() {
            if existing.workspace_name() != expected_name {
                bail!(
                    "workspace at {} already exists as '{}'",
                    manifest_path.display(),
                    existing.workspace_name()
                );
            }
        }

        prepare_workspace_shell_from_context(&existing)?;
        register_and_activate_workspace(&existing, tpx_home)?;

        return Ok(existing);
    }

    let workspace_name = name.unwrap_or_else(|| default_workspace_name(&root));
    let manifest = WorkspaceManifest {
        api_version: API_VERSION.to_owned(),
        workspace: workspace_name.clone(),
        metadata: Some(Metadata {
            name: workspace_name,
        }),
        providers: BTreeMap::new(),
    };

    write_workspace_manifest(&manifest_path, &manifest)?;

    let context = load_workspace(&root, tpx_home)?;
    prepare_workspace_shell_from_context(&context)?;
    register_and_activate_workspace(&context, tpx_home)?;

    Ok(context)
}

fn workspace_target_paths(cwd: &Path, target: &Path) -> Result<(PathBuf, PathBuf)> {
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        cwd.join(target)
    };

    if target.file_name().and_then(|name| name.to_str()) == Some(WORKSPACE_MANIFEST_FILE) {
        let root = target
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("cannot derive workspace root from {}", target.display()))?;
        return Ok((root, target));
    }

    Ok((target.clone(), target.join(WORKSPACE_MANIFEST_FILE)))
}

fn register_and_activate_workspace(
    context: &WorkspaceContext,
    tpx_home: Option<&Path>,
) -> Result<()> {
    let mut config = load_home_config(tpx_home)?;

    register_workspace(&mut config, context.workspace_name(), &context.root)?;
    set_active_workspace(&mut config, &context.root)?;
    save_home_config(tpx_home, &config)?;

    Ok(())
}

fn resolve_workspace_context(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
) -> Result<WorkspaceContext> {
    let mut options = ResolveOptions::new(cwd);
    options.workspace = workspace_selection.map(str::to_owned);
    options.tpx_home = tpx_home.map(Path::to_path_buf);

    resolve_workspace(&options)
}

fn prepare_workspace_shell(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
) -> Result<PreparedWorkspace> {
    let context = resolve_workspace_context(cwd, workspace_selection, tpx_home)?;
    let state = prepare_workspace_shell_from_context(&context)?;

    Ok(PreparedWorkspace { context, state })
}

fn prepare_workspace_shell_from_context(
    context: &WorkspaceContext,
) -> Result<WorkspaceRuntimeState> {
    let shim_manager = ShimManager;
    let lazy_specs = context
        .manifest
        .providers
        .keys()
        .map(|alias| LazyShimSpec::new(alias.clone(), alias.clone()))
        .collect::<Vec<_>>();
    let lazy_layout = context.lazy_layout();

    shim_manager.write_lazy_workspace_shims(context, &lazy_specs)?;

    let provider_states = context
        .manifest
        .providers
        .iter()
        .map(|(alias, provider_ref)| {
            ProviderRuntimeState::new(
                alias.clone(),
                provider_ref.source.clone(),
                provider_runtime_home(context, alias),
                lazy_layout.bin_dir.join(alias),
            )
        })
        .collect::<Vec<_>>();
    let state = context.write_runtime_state(&provider_states)?;
    let direct_specs = context
        .manifest
        .providers
        .keys()
        .map(|alias| ShimSpec::new(alias.clone(), lazy_layout.bin_dir.join(alias)))
        .collect::<Vec<_>>();

    shim_manager.write_workspace_shims(context, &direct_specs)?;

    Ok(state)
}

fn provider_runtime_home(context: &WorkspaceContext, alias: &str) -> PathBuf {
    context.home.join("providers").join(alias)
}

fn install_default_provider(
    cwd: &Path,
    tpx_home: Option<&Path>,
    reference: &str,
    alias: &str,
) -> Result<()> {
    let mut config = load_home_config(tpx_home)?;

    if let Some(existing) = config.aliases.get(alias) {
        if existing != reference {
            bail!(
                "default provider alias '{}' already points at '{}'",
                alias,
                existing
            );
        }
    }

    config
        .aliases
        .insert(alias.to_owned(), reference.to_owned());
    save_home_config(tpx_home, &config)?;

    let home = resolve_tpx_home(tpx_home)?;
    let tool = tool_from_source(reference, alias, Some(cwd))?;
    install_tool(&tool, &home)
}

fn add_provider_to_workspace(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    provider: &str,
    alias: &str,
    plain_http: bool,
) -> Result<()> {
    let context = resolve_workspace_context(cwd, workspace_selection, tpx_home)?;
    let mut manifest = context.manifest.clone();

    if manifest.providers.contains_key(alias) {
        bail!("provider alias '{}' already exists", alias);
    }

    manifest.providers.insert(
        alias.to_owned(),
        WorkspaceProviderRef {
            source: provider.to_owned(),
            plain_http,
        },
    );
    write_workspace_manifest(&context.manifest_path, &manifest)?;

    let refreshed = load_workspace(&context.root, tpx_home)?;
    prepare_workspace_shell_from_context(&refreshed)?;

    Ok(())
}

fn remove_provider_from_workspace(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    target: &str,
) -> Result<String> {
    let context = resolve_workspace_context(cwd, workspace_selection, tpx_home)?;
    let mut manifest = context.manifest.clone();
    let removed_alias = find_provider_alias(&manifest, target)?;

    manifest.providers.remove(&removed_alias);
    write_workspace_manifest(&context.manifest_path, &manifest)?;

    let refreshed = load_workspace(&context.root, tpx_home)?;
    prepare_workspace_shell_from_context(&refreshed)?;

    Ok(removed_alias)
}

fn update_workspace_providers(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    targets: &[String],
) -> Result<Vec<String>> {
    let context = resolve_workspace_context(cwd, workspace_selection, tpx_home)?;
    let aliases = matching_provider_aliases(&context.manifest, targets)?;

    prepare_workspace_shell_from_context(&context)?;

    for alias in &aliases {
        let tool = workspace_tool_from_alias(&context, alias)?;
        install_tool(&tool, &context.home)?;
    }

    Ok(aliases)
}

fn print_provider_inventory<W: Write>(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    scope: InventoryScope,
    stdout: &mut W,
) -> Result<()> {
    match scope {
        InventoryScope::Workspace => {
            let context = resolve_workspace_context(cwd, workspace_selection, tpx_home)?;

            for (alias, provider_ref) in &context.manifest.providers {
                writeln!(stdout, "{}\t{}", alias, provider_ref.source)?;
            }
        }
        InventoryScope::Default => {
            let config = load_home_config(tpx_home)?;

            for (alias, reference) in config.aliases {
                writeln!(stdout, "{}\t{}", alias, reference)?;
            }
        }
    }

    Ok(())
}

fn print_registered_workspaces<W: Write>(
    tpx_home: Option<&Path>,
    active_only: bool,
    ready_only: bool,
    missing_only: bool,
    short: bool,
    stdout: &mut W,
) -> Result<()> {
    let config = load_home_config(tpx_home)?;
    let mut workspaces = registered_workspaces(&config);

    if active_only {
        workspaces.retain(|entry| entry.active);
    }
    if ready_only {
        workspaces.retain(|entry| entry.status == WorkspaceStatus::Ready);
    }
    if missing_only {
        workspaces.retain(|entry| entry.status == WorkspaceStatus::Missing);
    }

    if short {
        for workspace in workspaces {
            writeln!(stdout, "{}", workspace.name)?;
        }

        return Ok(());
    }

    for workspace in workspaces {
        let marker = if workspace.active { "*" } else { " " };
        let status = match workspace.status {
            WorkspaceStatus::Ready => "ready",
            WorkspaceStatus::Missing => "missing",
        };
        writeln!(
            stdout,
            "{} {}\t{}",
            marker,
            status,
            workspace.root.display()
        )?;
    }

    Ok(())
}

fn workspace_use<W: Write>(
    cwd: &Path,
    tpx_home: Option<&Path>,
    workspace: &str,
    command: &[String],
    stdout: &mut W,
) -> Result<i32> {
    let context = resolve_workspace_context(cwd, Some(workspace), tpx_home)?;
    let mut config = load_home_config(tpx_home)?;

    register_workspace(&mut config, context.workspace_name(), &context.root)?;
    set_active_workspace(&mut config, &context.root)?;
    save_home_config(tpx_home, &config)?;

    if command.is_empty() {
        writeln!(
            stdout,
            "using workspace '{}' at {}",
            context.workspace_name(),
            context.root.display()
        )?;
        return Ok(0);
    }

    let executable = command[0].clone();
    let args = command[1..].to_vec();

    execute_workspace_command(cwd, Some(workspace), tpx_home, &executable, &args)
}

fn delete_workspace(cwd: &Path, tpx_home: Option<&Path>, workspace: &str) -> Result<PathBuf> {
    let mut config = load_home_config(tpx_home)?;

    let (workspace_name, root) = resolve_workspace_delete_target(cwd, &config, workspace)?;
    let runtime_dir = root.join(WORKSPACE_RUNTIME_DIR);
    let lazy_dir = root.join(WORKSPACE_LAZY_DIR);

    if runtime_dir.exists() {
        fs::remove_dir_all(&runtime_dir).with_context(|| {
            format!(
                "failed to remove runtime directory {}",
                runtime_dir.display()
            )
        })?;
    }
    if lazy_dir.exists() {
        fs::remove_dir_all(&lazy_dir)
            .with_context(|| format!("failed to remove lazy directory {}", lazy_dir.display()))?;
    }

    if let Some(name) = workspace_name.as_deref() {
        unregister_workspace(&mut config, name);
        save_home_config(tpx_home, &config)?;
    }

    Ok(root)
}

fn resolve_workspace_delete_target(
    cwd: &Path,
    config: &HomeConfig,
    workspace: &str,
) -> Result<(Option<String>, PathBuf)> {
    if let Some(root) = config.workspaces.get(workspace) {
        return Ok((Some(workspace.to_owned()), root.clone()));
    }

    let candidate = PathBuf::from(workspace);
    let root = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };

    if !root.exists() {
        bail!(
            "workspace '{}' is neither registered nor present on disk",
            workspace
        );
    }

    let manifest_path =
        if root.file_name().and_then(|name| name.to_str()) == Some(WORKSPACE_MANIFEST_FILE) {
            root.clone()
        } else {
            root.join(WORKSPACE_MANIFEST_FILE)
        };

    if !manifest_path.is_file() {
        bail!(
            "workspace manifest not found at {}",
            manifest_path.display()
        );
    }

    let manifest = load_workspace_manifest(&manifest_path)?;
    let root = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow!(
                "cannot derive workspace root from {}",
                manifest_path.display()
            )
        })?;

    Ok((Some(manifest.workspace), root))
}

fn execute_workspace_command(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    executable: &str,
    args: &[String],
) -> Result<i32> {
    let prepared = prepare_workspace_shell(cwd, workspace_selection, tpx_home)?;
    let execution_dir = prepared.context.execution_dir(cwd)?;
    let mut command = ProcessCommand::new(executable);

    command.args(args);
    command.current_dir(&execution_dir);
    command.envs(
        prepared
            .state
            .env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    command.env("PATH", build_workspace_path(&prepared.state.path_entries)?);

    let status = command.status().with_context(|| {
        format!(
            "failed to execute '{}' inside workspace {}",
            executable,
            prepared.context.root.display()
        )
    })?;

    Ok(exit_code(status))
}

fn launch_workspace_shell(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
) -> Result<i32> {
    let prepared = prepare_workspace_shell(cwd, workspace_selection, tpx_home)?;
    let execution_dir = prepared.context.execution_dir(cwd)?;
    let shell = env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));
    let mut command = ProcessCommand::new(&shell);

    command.current_dir(&execution_dir);
    command.envs(
        prepared
            .state
            .env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    command.env("PATH", build_workspace_path(&prepared.state.path_entries)?);

    let status = command.status().with_context(|| {
        format!(
            "failed to launch shell '{}' for workspace {}",
            PathBuf::from(&shell).display(),
            prepared.context.root.display()
        )
    })?;

    Ok(exit_code(status))
}

fn build_workspace_path(path_entries: &[PathBuf]) -> Result<OsString> {
    let mut combined = path_entries.to_vec();

    if let Some(self_dir) = current_executable_dir() {
        combined.push(self_dir);
    }

    if let Some(existing) = env::var_os("PATH") {
        combined.extend(env::split_paths(&existing));
    }

    env::join_paths(combined).context("failed to construct workspace PATH")
}

fn current_executable_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn print_workspace_status<W: Write>(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    short: bool,
    verbose: bool,
    stdout: &mut W,
) -> Result<()> {
    let prepared = prepare_workspace_shell(cwd, workspace_selection, tpx_home)?;
    let default_aliases = load_home_config(tpx_home)?.aliases;
    let provider_rows = provider_status_rows(&prepared.context)?;

    if short {
        let ready_count = provider_rows
            .iter()
            .filter(|row| row.installed && row.error.is_none())
            .count();
        writeln!(
            stdout,
            "workspace={} providers={} ready={} root={}",
            prepared.context.workspace_name(),
            provider_rows.len(),
            ready_count,
            prepared.context.root.display()
        )?;
        return Ok(());
    }

    writeln!(stdout, "workspace: {}", prepared.context.workspace_name())?;
    writeln!(stdout, "root: {}", prepared.context.root.display())?;
    writeln!(stdout, "home: {}", prepared.context.home.display())?;
    writeln!(
        stdout,
        "manifest: {}",
        prepared.context.manifest_path.display()
    )?;
    writeln!(
        stdout,
        "runtime dir: {}",
        prepared.context.runtime_dir.display()
    )?;
    writeln!(
        stdout,
        "lazy dir: {}",
        prepared.context.root.join(WORKSPACE_LAZY_DIR).display()
    )?;
    writeln!(stdout, "providers: {}", provider_rows.len())?;

    for row in provider_rows {
        match row.error {
            Some(error) => writeln!(
                stdout,
                "{}\t{}\t{}\terror={}",
                row.alias, row.runtime, row.source, error
            )?,
            None => writeln!(
                stdout,
                "{}\t{}\tinstalled={}\t{}",
                row.alias, row.runtime, row.installed, row.source
            )?,
        }
    }

    if verbose {
        writeln!(stdout, "default aliases: {}", default_aliases.len())?;
        for (alias, reference) in default_aliases {
            writeln!(stdout, "default\t{}\t{}", alias, reference)?;
        }
        writeln!(stdout, "env file: {}", prepared.state.env_file.display())?;
        writeln!(stdout, "path file: {}", prepared.state.path_file.display())?;
        writeln!(
            stdout,
            "available runtimes: {}",
            BUILTIN_RUNTIMES.join(", ")
        )?;
    }

    Ok(())
}

fn provider_status_rows(context: &WorkspaceContext) -> Result<Vec<ProviderStatusRow>> {
    let mut rows = Vec::new();
    let registry = runtime_registry(&context.home);
    let execution_context = CoreContext::new(internal_model_from_workspace(&context.manifest));

    for (alias, provider_ref) in &context.manifest.providers {
        let tool = workspace_tool_from_alias(context, alias)?;
        let runtime =
            tool.spec.runtime.clone().unwrap_or_else(|| {
                infer_runtime(&provider_ref.source, Some(&context.root)).to_owned()
            });

        let status = registry
            .get(&runtime)
            .ok_or_else(|| anyhow!("runtime '{}' is not registered", runtime))
            .and_then(|runtime_impl| {
                let resolved = runtime_impl.resolve(&tool, &execution_context)?;
                runtime_impl.is_installed(&resolved, &execution_context)
            });

        match status {
            Ok(installed) => rows.push(ProviderStatusRow {
                alias: alias.clone(),
                source: provider_ref.source.clone(),
                runtime,
                installed,
                error: None,
            }),
            Err(error) => rows.push(ProviderStatusRow {
                alias: alias.clone(),
                source: provider_ref.source.clone(),
                runtime,
                installed: false,
                error: Some(error.to_string()),
            }),
        }
    }

    Ok(rows)
}

fn run_compat<E: Write>(
    cwd: &Path,
    workspace_selection: Option<&str>,
    tpx_home: Option<&Path>,
    tool: &str,
    args: &[String],
    stderr: &mut E,
) -> Result<i32> {
    writeln!(
        stderr,
        "warning: 'tpx run' is deprecated; use 'tpx exec {}' or 'tpx -- {}'",
        tool, tool
    )?;

    let context = match resolve_workspace_context(cwd, workspace_selection, tpx_home) {
        Ok(context) => Some(context),
        Err(error) if workspace_selection.is_some() => return Err(error),
        Err(_) => None,
    };

    if let Some(context) = context.as_ref() {
        if let Ok(alias) = find_provider_alias(&context.manifest, tool) {
            let execution_context =
                CoreContext::new(internal_model_from_workspace(&context.manifest));
            Engine::new()
                .build_execution_plan(&alias, &execution_context)
                .with_context(|| format!("failed to build execution plan for '{alias}'"))?;
            let runtime_tool = workspace_tool_from_alias(context, &alias)?;

            return execute_tool(&runtime_tool, args, &context.home);
        }
    }

    if !looks_like_provider_source(tool, Some(cwd)) {
        bail!("tool '{}' was not found in the selected workspace", tool);
    }

    let home = resolve_tpx_home(tpx_home)?;
    let direct_tool = tool_from_source(tool, &derive_alias(tool), Some(cwd))?;

    execute_tool(&direct_tool, args, &home)
}

fn execute_tool(tool: &Tool, args: &[String], home: &Path) -> Result<i32> {
    let registry = runtime_registry(home);
    let runtime_name = tool
        .spec
        .runtime
        .as_deref()
        .ok_or_else(|| anyhow!("tool '{}' is missing a runtime", tool_name(tool)))?;
    let runtime = registry
        .get(runtime_name)
        .ok_or_else(|| anyhow!("runtime '{}' is not registered", runtime_name))?;
    let execution_context = execution_context_for_tool(tool.clone());
    let resolved = runtime.resolve(tool, &execution_context)?;

    if !runtime.is_installed(&resolved, &execution_context)? {
        runtime.install(&resolved, &execution_context)?;
    }

    runtime.execute(&resolved, args, &execution_context)
}

fn install_tool(tool: &Tool, home: &Path) -> Result<()> {
    let registry = runtime_registry(home);
    let runtime_name = tool
        .spec
        .runtime
        .as_deref()
        .ok_or_else(|| anyhow!("tool '{}' is missing a runtime", tool_name(tool)))?;
    let runtime = registry
        .get(runtime_name)
        .ok_or_else(|| anyhow!("runtime '{}' is not registered", runtime_name))?;
    let execution_context = execution_context_for_tool(tool.clone());
    let resolved = runtime.resolve(tool, &execution_context)?;

    runtime.install(&resolved, &execution_context)
}

fn execution_context_for_tool(tool: Tool) -> CoreContext {
    let mut model = tpx_parser::InternalModel::default();
    let name = tool_name(&tool).to_owned();

    model.tools.insert(name, tool);

    CoreContext::new(model)
}

fn runtime_registry(home: &Path) -> RuntimeRegistry {
    let mut registry = RuntimeRegistry::new();

    registry.register(LocalRuntime);
    registry.register(ScriptRuntime::with_home(home));
    registry.register(OciRuntime::with_home(home));

    registry
}

fn workspace_tool_from_alias(context: &WorkspaceContext, alias: &str) -> Result<Tool> {
    let provider_ref = context
        .manifest
        .providers
        .get(alias)
        .ok_or_else(|| anyhow!("provider alias '{}' was not found", alias))?;

    tool_from_source(&provider_ref.source, alias, Some(&context.root))
}

fn tool_from_source(source: &str, display_name: &str, base_dir: Option<&Path>) -> Result<Tool> {
    let runtime = infer_runtime(source, base_dir).to_owned();
    let asset = resolve_runtime_asset(source, &runtime, base_dir)?;
    let mut tool = Tool {
        metadata: Some(Metadata {
            name: display_name.to_owned(),
        }),
        spec: ToolSpec {
            runtime: Some(runtime),
            provider: Some(source.to_owned()),
            ..ToolSpec::default()
        },
    };

    tool.spec.assets.push(asset);

    Ok(tool)
}

fn resolve_runtime_asset(source: &str, runtime: &str, base_dir: Option<&Path>) -> Result<String> {
    if source.starts_with("inline:") {
        return Ok(source.to_owned());
    }

    if runtime == "oci"
        && (source.starts_with("http://")
            || source.starts_with("https://")
            || source.starts_with("file://"))
    {
        return Ok(source.to_owned());
    }

    if let Some(path) = source_path(source, base_dir) {
        if runtime == "local" || runtime == "script" || path.is_file() {
            return Ok(path_to_string(&path)?);
        }

        if runtime == "oci" {
            return Ok(path_to_string(&path)?);
        }
    }

    Ok(source.to_owned())
}

fn infer_runtime(source: &str, base_dir: Option<&Path>) -> &'static str {
    if source.starts_with("inline:") || source.ends_with(".sh") {
        return "script";
    }

    if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("file://")
        || source.ends_with(".tar")
        || source.ends_with(".tar.gz")
        || source.ends_with(".tgz")
    {
        return "oci";
    }

    if let Some(path) = source_path(source, base_dir) {
        if path.is_dir() && is_oci_layout_root(&path) {
            return "oci";
        }

        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("sh"))
            .unwrap_or(false)
        {
            return "script";
        }

        return "local";
    }

    if source.starts_with("./") || source.starts_with('/') {
        return "local";
    }

    "oci"
}

fn is_oci_layout_root(path: &Path) -> bool {
    path.join(OCI_LAYOUT_FILE).is_file() && path.join(OCI_INDEX_FILE).is_file()
}

fn source_path(source: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    if source.starts_with("file://")
        || source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("inline:")
    {
        return None;
    }

    let path = PathBuf::from(source);

    if path.is_absolute() {
        return Some(path);
    }

    base_dir.map(|base| base.join(path))
}

fn looks_like_provider_source(target: &str, base_dir: Option<&Path>) -> bool {
    infer_runtime(target, base_dir) != "oci"
        || target.contains('/')
        || target.contains(':')
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("file://")
}

fn find_provider_alias(manifest: &WorkspaceManifest, target: &str) -> Result<String> {
    if manifest.providers.contains_key(target) {
        return Ok(target.to_owned());
    }

    manifest
        .providers
        .iter()
        .find_map(|(alias, provider_ref)| {
            if provider_ref.source == target {
                Some(alias.clone())
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("provider '{}' was not found", target))
}

fn matching_provider_aliases(
    manifest: &WorkspaceManifest,
    targets: &[String],
) -> Result<Vec<String>> {
    if targets.is_empty() {
        return Ok(manifest.providers.keys().cloned().collect());
    }

    let mut aliases = Vec::with_capacity(targets.len());

    for target in targets {
        let alias = find_provider_alias(manifest, target)?;

        if !aliases.contains(&alias) {
            aliases.push(alias);
        }
    }

    Ok(aliases)
}

fn internal_model_from_workspace(manifest: &WorkspaceManifest) -> tpx_parser::InternalModel {
    let mut model = tpx_parser::InternalModel::default();

    for (alias, provider_ref) in &manifest.providers {
        let mut tool = Tool {
            metadata: Some(Metadata {
                name: alias.clone(),
            }),
            spec: ToolSpec {
                runtime: Some(infer_runtime(&provider_ref.source, None).to_owned()),
                provider: Some(provider_ref.source.clone()),
                ..ToolSpec::default()
            },
        };
        tool.spec.assets.push(provider_ref.source.clone());
        model.tools.insert(alias.clone(), tool);
    }

    model
}

fn pack_provider(
    cwd: &Path,
    manifest: Option<&Path>,
    output: Option<&Path>,
    artifact_root: Option<&Path>,
    tag: Option<&str>,
) -> Result<PathBuf> {
    let manifest_path = resolve_provider_manifest_path(cwd, manifest)?;
    let provider_manifest = load_provider_manifest(&manifest_path)?;
    let output_path = provider_layout_path(&manifest_path, output)?;
    let artifact_root = resolve_artifact_root(&manifest_path, artifact_root)?;
    let tag = provider_layout_tag(&provider_manifest, tag);

    if output_path == artifact_root {
        bail!(
            "output OCI layout directory {} must not match artifact root {}",
            output_path.display(),
            artifact_root.display()
        );
    }

    write_oci_layout(
        &output_path,
        &manifest_path,
        &provider_manifest,
        &artifact_root,
        &tag,
    )?;

    Ok(output_path)
}

fn release_provider(
    cwd: &Path,
    manifest: Option<&Path>,
    output: Option<&Path>,
    artifact_root: Option<&Path>,
    push: Option<&str>,
    tag: Option<&str>,
) -> Result<ReleaseResult> {
    let layout_path = pack_provider(cwd, manifest, output, artifact_root, tag)?;
    let pushed_path = match push {
        Some(push_target) => Some(push_layout(&layout_path, push_target)?),
        None => None,
    };

    Ok(ReleaseResult {
        layout_path,
        pushed_path,
    })
}

fn push_layout(layout_path: &Path, push_target: &str) -> Result<PathBuf> {
    let target = if let Some(path) = push_target.strip_prefix("file://") {
        PathBuf::from(path)
    } else if push_target.contains("://") {
        bail!("remote push target '{}' is not supported yet", push_target);
    } else {
        PathBuf::from(push_target)
    };

    let destination =
        if target.is_dir() || push_target.ends_with('/') {
            target.join(layout_path.file_name().ok_or_else(|| {
                anyhow!("cannot derive layout name from {}", layout_path.display())
            })?)
        } else {
            target
        };

    if destination.exists() {
        if destination.is_dir() {
            fs::remove_dir_all(&destination).with_context(|| {
                format!(
                    "failed to replace OCI layout destination {}",
                    destination.display()
                )
            })?;
        } else {
            bail!(
                "release destination {} already exists and is not a directory",
                destination.display()
            );
        }
    }

    copy_directory_recursive(layout_path, &destination)?;

    Ok(destination)
}

fn resolve_provider_manifest_path(cwd: &Path, manifest: Option<&Path>) -> Result<PathBuf> {
    let manifest = manifest.unwrap_or_else(|| Path::new(WORKSPACE_MANIFEST_FILE));
    let manifest_path = if manifest.is_absolute() {
        manifest.to_path_buf()
    } else {
        cwd.join(manifest)
    };

    if !manifest_path.is_file() {
        bail!("provider manifest not found at {}", manifest_path.display());
    }

    Ok(manifest_path)
}

fn provider_layout_path(manifest_path: &Path, output: Option<&Path>) -> Result<PathBuf> {
    if let Some(output) = output {
        if output.is_absolute() {
            return Ok(output.to_path_buf());
        }

        let manifest_dir = manifest_path.parent().ok_or_else(|| {
            anyhow!(
                "cannot derive manifest directory from {}",
                manifest_path.display()
            )
        })?;

        return Ok(manifest_dir.join(output));
    }

    let manifest_dir = manifest_path.parent().ok_or_else(|| {
        anyhow!(
            "cannot derive manifest directory from {}",
            manifest_path.display()
        )
    })?;

    Ok(manifest_dir.join("oci"))
}

fn resolve_artifact_root(manifest_path: &Path, artifact_root: Option<&Path>) -> Result<PathBuf> {
    let manifest_dir = manifest_path.parent().ok_or_else(|| {
        anyhow!(
            "cannot derive manifest directory from {}",
            manifest_path.display()
        )
    })?;
    let artifact_root = match artifact_root {
        Some(root) if root.is_absolute() => root.to_path_buf(),
        Some(root) => manifest_dir.join(root),
        None => manifest_dir.to_path_buf(),
    };

    if !artifact_root.exists() {
        bail!("artifact root {} does not exist", artifact_root.display());
    }

    Ok(artifact_root)
}

fn provider_layout_tag(provider_manifest: &ProviderManifest, tag: Option<&str>) -> String {
    tag.map(str::to_owned).unwrap_or_else(|| {
        format!(
            "{}/{}:{}",
            provider_manifest.metadata.namespace,
            provider_manifest.metadata.name,
            provider_manifest.metadata.version
        )
    })
}

fn write_oci_layout(
    layout_root: &Path,
    manifest_path: &Path,
    provider_manifest: &ProviderManifest,
    artifact_root: &Path,
    tag: &str,
) -> Result<()> {
    reset_layout_root(layout_root)?;
    fs::create_dir_all(layout_root.join("blobs").join(OCI_BLOB_ALGORITHM)).with_context(|| {
        format!(
            "failed to create OCI blob directory under {}",
            layout_root.display()
        )
    })?;

    let mut platforms = provider_manifest.spec.platforms.clone();
    platforms.sort_by(|left, right| {
        (&left.os, &left.arch, &left.binary).cmp(&(&right.os, &right.arch, &right.binary))
    });

    let mut image_manifest_descriptors = Vec::with_capacity(platforms.len());

    for platform in &platforms {
        let layer_bytes =
            build_provider_layer(manifest_path, provider_manifest, artifact_root, platform)?;
        let layer_digest = write_layout_blob(layout_root, &layer_bytes)?;
        let config_bytes = serde_json::to_vec(&OciImageConfig {
            architecture: &platform.arch,
            os: &platform.os,
            rootfs: OciRootFs {
                kind: "layers",
                diff_ids: vec![format!("sha256:{}", sha256_hex(&layer_bytes))],
            },
        })
        .context("failed to serialize OCI config JSON")?;
        let config_digest = write_layout_blob(layout_root, &config_bytes)?;
        let config_descriptor = OciDescriptor {
            media_type: OCI_IMAGE_CONFIG_MEDIA_TYPE,
            digest: format!("sha256:{}", config_digest),
            size: config_bytes.len() as u64,
            annotations: BTreeMap::new(),
            platform: None,
        };
        let layer_descriptor = OciDescriptor {
            media_type: OCI_IMAGE_LAYER_MEDIA_TYPE,
            digest: format!("sha256:{}", layer_digest),
            size: layer_bytes.len() as u64,
            annotations: BTreeMap::new(),
            platform: None,
        };
        let image_manifest_bytes = serde_json::to_vec(&OciManifestDocument {
            schema_version: 2,
            media_type: OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            config: &config_descriptor,
            layers: std::slice::from_ref(&layer_descriptor),
            annotations: provider_manifest_annotations(provider_manifest),
        })
        .context("failed to serialize OCI image manifest")?;
        let image_manifest_digest = write_layout_blob(layout_root, &image_manifest_bytes)?;

        image_manifest_descriptors.push(OciDescriptor {
            media_type: OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            digest: format!("sha256:{}", image_manifest_digest),
            size: image_manifest_bytes.len() as u64,
            annotations: BTreeMap::new(),
            platform: Some(OciPlatformDescriptor {
                architecture: platform.arch.clone(),
                os: platform.os.clone(),
            }),
        });
    }

    let image_index_bytes = serde_json::to_vec(&OciIndexDocument {
        schema_version: 2,
        media_type: OCI_IMAGE_INDEX_MEDIA_TYPE,
        manifests: &image_manifest_descriptors,
    })
    .context("failed to serialize OCI image index")?;
    let image_index_digest = write_layout_blob(layout_root, &image_index_bytes)?;
    let mut root_annotations = BTreeMap::new();

    root_annotations.insert("org.opencontainers.image.ref.name".into(), tag.to_owned());

    let root_descriptor = OciDescriptor {
        media_type: OCI_IMAGE_INDEX_MEDIA_TYPE,
        digest: format!("sha256:{}", image_index_digest),
        size: image_index_bytes.len() as u64,
        annotations: root_annotations,
        platform: None,
    };

    fs::write(
        layout_root.join(OCI_LAYOUT_FILE),
        serde_json::to_vec(&OciImageLayoutDocument {
            image_layout_version: OCI_IMAGE_LAYOUT_VERSION,
        })
        .context("failed to serialize OCI layout marker")?,
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            layout_root.join(OCI_LAYOUT_FILE).display()
        )
    })?;
    fs::write(
        layout_root.join(OCI_INDEX_FILE),
        serde_json::to_vec(&OciIndexDocument {
            schema_version: 2,
            media_type: OCI_IMAGE_INDEX_MEDIA_TYPE,
            manifests: std::slice::from_ref(&root_descriptor),
        })
        .context("failed to serialize OCI root index")?,
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            layout_root.join(OCI_INDEX_FILE).display()
        )
    })?;

    Ok(())
}

fn reset_layout_root(layout_root: &Path) -> Result<()> {
    if layout_root.exists() {
        if layout_root.is_dir() {
            fs::remove_dir_all(layout_root).with_context(|| {
                format!(
                    "failed to clear OCI layout directory {}",
                    layout_root.display()
                )
            })?;
        } else {
            bail!(
                "OCI layout output path {} already exists and is not a directory",
                layout_root.display()
            );
        }
    }

    fs::create_dir_all(layout_root).with_context(|| {
        format!(
            "failed to create OCI layout directory {}",
            layout_root.display()
        )
    })
}

fn build_provider_layer(
    manifest_path: &Path,
    provider_manifest: &ProviderManifest,
    artifact_root: &Path,
    platform: &tpx_parser::ProviderPlatform,
) -> Result<Vec<u8>> {
    let mut builder = Builder::new(Vec::new());

    builder.mode(HeaderMode::Deterministic);

    append_file_to_tar(
        &mut builder,
        Path::new(OCI_PROVIDER_MANIFEST_FILE),
        manifest_path,
        0o644,
    )?;

    let binary_source = resolve_provider_artifact_path(artifact_root, Path::new(&platform.binary));

    if !binary_source.is_file() {
        bail!(
            "provider platform binary '{}' was not found at {}",
            platform.binary,
            binary_source.display()
        );
    }

    append_file_to_tar(
        &mut builder,
        Path::new(&platform.binary),
        &binary_source,
        file_mode(&binary_source, 0o755)?,
    )?;

    if let Some(layers) = provider_manifest.spec.layers.as_ref() {
        if let Some(assets) = layers.assets.as_ref() {
            let assets_root =
                resolve_provider_artifact_path(artifact_root, Path::new(&assets.root));

            if !assets_root.exists() {
                bail!(
                    "provider assets root '{}' was not found at {}",
                    assets.root,
                    assets_root.display()
                );
            }

            append_directory_to_tar(&mut builder, Path::new(&assets.root), &assets_root)?;
        }
    }

    builder
        .into_inner()
        .context("failed to finalize OCI provider layer tar")
}

fn append_directory_to_tar(
    builder: &mut Builder<Vec<u8>>,
    archive_root: &Path,
    source_root: &Path,
) -> Result<()> {
    for file in collect_files(source_root)? {
        let relative = file.strip_prefix(source_root).with_context(|| {
            format!(
                "failed to compute relative asset path for {} under {}",
                file.display(),
                source_root.display()
            )
        })?;

        append_file_to_tar(
            builder,
            &archive_root.join(relative),
            &file,
            file_mode(&file, 0o644)?,
        )?;
    }

    Ok(())
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut stack = vec![root.to_path_buf()];
    let mut files = Vec::new();

    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)
            .with_context(|| format!("failed to read directory {}", directory.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("failed to enumerate directory {}", directory.display()))?;

        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }

    files.sort();

    Ok(files)
}

fn append_file_to_tar(
    builder: &mut Builder<Vec<u8>>,
    archive_path: &Path,
    source_path: &Path,
    mode: u32,
) -> Result<()> {
    let bytes = fs::read(source_path)
        .with_context(|| format!("failed to read layer source {}", source_path.display()))?;

    append_bytes_to_tar(builder, archive_path, &bytes, mode)
}

fn append_bytes_to_tar(
    builder: &mut Builder<Vec<u8>>,
    archive_path: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<()> {
    let mut header = Header::new_gnu();

    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, archive_path, bytes)
        .with_context(|| format!("failed to append {} to OCI layer", archive_path.display()))
}

fn resolve_provider_artifact_path(artifact_root: &Path, relative: &Path) -> PathBuf {
    if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        artifact_root.join(relative)
    }
}

fn file_mode(path: &Path, default_mode: u32) -> Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(path)
            .with_context(|| format!("failed to read metadata for {}", path.display()))?
            .permissions()
            .mode()
            & 0o777;

        return Ok(if mode == 0 { default_mode } else { mode });
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(default_mode)
    }
}

fn write_layout_blob(layout_root: &Path, bytes: &[u8]) -> Result<String> {
    let digest = sha256_hex(bytes);
    let blob_path = layout_root
        .join("blobs")
        .join(OCI_BLOB_ALGORITHM)
        .join(&digest);

    fs::write(&blob_path, bytes)
        .with_context(|| format!("failed to write OCI blob {}", blob_path.display()))?;

    Ok(digest)
}

fn provider_manifest_annotations(provider_manifest: &ProviderManifest) -> BTreeMap<String, String> {
    let mut annotations = BTreeMap::new();

    annotations.insert(
        "org.opencontainers.image.title".into(),
        provider_manifest.metadata.name.clone(),
    );
    annotations.insert(
        "org.opencontainers.image.version".into(),
        provider_manifest.metadata.version.clone(),
    );
    annotations.insert(
        "io.sourceplane.tpx.provider".into(),
        format!(
            "{}/{}",
            provider_manifest.metadata.namespace, provider_manifest.metadata.name
        ),
    );

    annotations
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| {
        format!(
            "failed to create OCI layout destination directory {}",
            destination.display()
        )
    })?;

    let mut entries = fs::read_dir(source)
        .with_context(|| format!("failed to read directory {}", source.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to enumerate directory {}", source.display()))?;

    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());

        if source_path.is_dir() {
            copy_directory_recursive(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy OCI layout entry {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);

    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }

    output
}

fn load_provider_manifest(manifest_path: &Path) -> Result<ProviderManifest> {
    match parse_document_file(path_as_str(manifest_path)?)? {
        ParsedDocument::ProviderManifest(manifest) => Ok(manifest),
        other => bail!(
            "expected canonical Provider manifest in {}, found {other:?}",
            manifest_path.display()
        ),
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

fn default_workspace_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace")
        .to_owned()
}

fn tool_name(tool: &Tool) -> &str {
    tool.metadata
        .as_ref()
        .map(|metadata| metadata.name.as_str())
        .unwrap_or("tool")
}

fn path_as_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path {} is not valid UTF-8", path.display()))
}

fn path_to_string(path: &Path) -> Result<String> {
    Ok(path_as_str(path)?.to_owned())
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| fallback_exit_code(&status))
}

#[cfg(unix)]
fn fallback_exit_code(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    status.signal().map(|signal| 128 + signal).unwrap_or(1)
}

#[cfg(not(unix))]
fn fallback_exit_code(_status: &std::process::ExitStatus) -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs, process,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn init_registers_workspace_and_writes_shell_state() {
        let temp = temp_dir("tpx-cli-init");
        let home = temp.path().join("home");

        let (code, stdout, stderr) = run_cli(&["init"], temp.path(), Some(&home));

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(stdout.contains("initialized workspace"));
        assert!(temp.path().join("tpx.yaml").is_file());
        assert!(temp.path().join(".tpx/bin").is_dir());
        assert!(temp.path().join(".workspace/env").is_file());
        assert!(temp.path().join(".workspace/path").is_file());

        let config = load_home_config(Some(&home)).expect("home config should load");
        let canonical_root =
            fs::canonicalize(temp.path()).expect("workspace root should canonicalize");
        assert_eq!(config.workspaces.len(), 1);
        assert_eq!(
            config.active_workspace.as_deref(),
            Some(canonical_root.as_path())
        );
    }

    #[test]
    fn workspace_commands_create_list_and_use_workspaces() {
        let temp = temp_dir("tpx-cli-workspaces");
        let home = temp.path().join("home");
        let alpha = temp.path().join("alpha");
        let beta = temp.path().join("beta");

        assert_eq!(
            run_cli(
                &["workspace", "create", path_as_str(&alpha).unwrap()],
                temp.path(),
                Some(&home)
            )
            .0,
            0
        );
        assert_eq!(
            run_cli(
                &["workspace", "create", path_as_str(&beta).unwrap()],
                temp.path(),
                Some(&home)
            )
            .0,
            0
        );

        let (code, stdout, _) =
            run_cli(&["workspace", "list", "--short"], temp.path(), Some(&home));
        assert_eq!(code, 0);
        assert_eq!(stdout, "alpha\nbeta\n");

        let (code, stdout, _) = run_cli(
            &["workspace", "use", path_as_str(&alpha).unwrap()],
            temp.path(),
            Some(&home),
        );
        assert_eq!(code, 0);
        assert!(stdout.contains("using workspace 'alpha'"));

        let (code, stdout, _) = run_cli(&["workspace", "current"], temp.path(), Some(&home));
        assert_eq!(code, 0);
        assert!(stdout.starts_with("alpha\t"));
    }

    #[test]
    fn provider_commands_add_list_update_and_remove() {
        let temp = temp_dir("tpx-cli-provider");
        let home = temp.path().join("home");
        let executable = temp.path().join("bin/tool.sh");

        fs::create_dir_all(executable.parent().expect("parent should exist"))
            .expect("bin directory should be created");
        write_executable(&executable, "#!/bin/sh\nexit 0\n");

        assert_eq!(run_cli(&["init"], temp.path(), Some(&home)).0, 0);
        assert_eq!(
            run_cli(
                &[
                    "provider",
                    "add",
                    path_as_str(&executable).unwrap(),
                    "as",
                    "tool"
                ],
                temp.path(),
                Some(&home)
            )
            .0,
            0
        );

        let (code, stdout, _) = run_cli(&["provider", "list"], temp.path(), Some(&home));
        assert_eq!(code, 0);
        assert_eq!(stdout, format!("tool\t{}\n", executable.display()));

        let (code, stdout, _) = run_cli(&["provider", "update", "tool"], temp.path(), Some(&home));
        assert_eq!(code, 0);
        assert!(stdout.contains("updated provider 'tool'"));

        let (code, stdout, _) = run_cli(&["remove", "tool"], temp.path(), Some(&home));
        assert_eq!(code, 0);
        assert!(stdout.contains("removed provider 'tool'"));
    }

    #[test]
    fn install_populates_default_scope() {
        let temp = temp_dir("tpx-cli-install");
        let home = temp.path().join("home");
        let executable = temp.path().join("bin/default-tool");

        fs::create_dir_all(executable.parent().expect("parent should exist"))
            .expect("bin directory should be created");
        write_executable(&executable, "#!/bin/sh\nexit 0\n");

        let (code, stdout, stderr) = run_cli(
            &[
                "install",
                path_as_str(&executable).unwrap(),
                "as",
                "default-tool",
            ],
            temp.path(),
            Some(&home),
        );

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(stdout.contains("installed provider"));

        let (code, stdout, _) = run_cli(&["provider", "list", "default"], temp.path(), Some(&home));
        assert_eq!(code, 0);
        assert_eq!(stdout, format!("default-tool\t{}\n", executable.display()));
    }

    #[test]
    fn run_executes_local_runtime() {
        let temp = temp_dir("tpx-cli-run-local");
        let home = temp.path().join("home");
        let executable = temp.path().join("bin/exit-four.sh");

        fs::create_dir_all(executable.parent().expect("parent should exist"))
            .expect("bin directory should be created");
        write_executable(&executable, "#!/bin/sh\nexit 4\n");

        assert_eq!(run_cli(&["init"], temp.path(), Some(&home)).0, 0);
        assert_eq!(
            run_cli(
                &["add", path_as_str(&executable).unwrap(), "as", "exit-four"],
                temp.path(),
                Some(&home)
            )
            .0,
            0
        );

        let (code, _, stderr) = run_cli(&["run", "exit-four"], temp.path(), Some(&home));

        assert_eq!(code, 4);
        assert!(stderr.contains("deprecated"));
    }

    #[test]
    fn exec_uses_workspace_lazy_shims() {
        let _guard = env_lock().lock().expect("env lock should succeed");
        let temp = temp_dir("tpx-cli-exec");
        let home = temp.path().join("home");
        let fake_bin = temp.path().join("fake-bin");
        let fake_tpx = fake_bin.join("tpx");
        let log = temp.path().join("dispatch.log");
        let original_path = env::var_os("PATH");

        fs::create_dir_all(&fake_bin).expect("fake bin dir should be created");
        write_executable(
            &fake_tpx,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nexit 0\n",
                log.display()
            ),
        );
        let mut path_entries = vec![fake_bin.clone()];
        if let Some(existing) = env::var_os("PATH") {
            path_entries.extend(env::split_paths(&existing));
        }
        unsafe {
            env::set_var(
                "PATH",
                env::join_paths(path_entries).expect("fake PATH should join"),
            );
        }

        assert_eq!(run_cli(&["init"], temp.path(), Some(&home)).0, 0);
        assert_eq!(
            run_cli(
                &["add", "core/example", "as", "example"],
                temp.path(),
                Some(&home)
            )
            .0,
            0
        );

        let (code, _, stderr) = run_cli(&["exec", "example"], temp.path(), Some(&home));

        if let Some(original) = original_path {
            unsafe {
                env::set_var("PATH", original);
            }
        }

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert_eq!(
            fs::read_to_string(log).expect("dispatch log should be readable"),
            "run example\n"
        );
    }

    #[test]
    fn status_reports_workspace_inventory() {
        let temp = temp_dir("tpx-cli-status");
        let home = temp.path().join("home");
        let executable = temp.path().join("bin/tool.sh");

        fs::create_dir_all(executable.parent().expect("parent should exist"))
            .expect("bin directory should be created");
        write_executable(&executable, "#!/bin/sh\nexit 0\n");

        assert_eq!(run_cli(&["init"], temp.path(), Some(&home)).0, 0);
        assert_eq!(
            run_cli(
                &["add", path_as_str(&executable).unwrap(), "as", "tool"],
                temp.path(),
                Some(&home)
            )
            .0,
            0
        );

        let (code, stdout, _) = run_cli(&["status", "--short"], temp.path(), Some(&home));
        assert_eq!(code, 0);
        assert!(stdout.contains("workspace=tpx-cli-status"));
        assert!(stdout.contains("providers=1"));
    }

    #[test]
    fn pack_and_release_create_consumable_oci_layout() {
        let temp = temp_dir("tpx-cli-pack");
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        let manifest = temp.path().join("provider.yaml");
        let artifact_root = temp.path().join("dist");
        let binary = artifact_root.join("bin/darwin/arm64/demo");
        let assets = artifact_root.join("assets");
        let layout = temp.path().join("out/oci");
        let pushed = temp.path().join("release/oci");
        let marker = temp.path().join("oci-executed.txt");

        fs::create_dir_all(binary.parent().expect("binary parent should exist"))
            .expect("binary directory should be created");
        fs::create_dir_all(&assets).expect("assets directory should be created");
        write_executable(
            &binary,
            &format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
        );
        fs::write(assets.join("README.txt"), "demo asset").expect("asset should be written");
        fs::write(
            &manifest,
            "apiVersion: tpx.io/v1\nkind: Provider\nmetadata:\n  namespace: acme\n  name: demo\n  version: v1.0.0\nspec:\n  runtime: binary\n  entrypoint: demo\n  platforms:\n    - os: darwin\n      arch: arm64\n      binary: bin/darwin/arm64/demo\n  layers:\n    assets:\n      root: assets\n",
        )
        .expect("provider manifest should be written");

        let (code, stdout, stderr) = run_cli(
            &[
                "pack",
                "--manifest",
                path_as_str(&manifest).unwrap(),
                "--artifact-root",
                path_as_str(&artifact_root).unwrap(),
                "--output",
                path_as_str(&layout).unwrap(),
            ],
            temp.path(),
            None,
        );
        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(stdout.contains(layout.to_str().expect("layout path should be UTF-8")));
        assert_oci_layout(&layout);

        assert_eq!(
            run_cli(
                &["workspace", "create", path_as_str(&workspace).unwrap()],
                temp.path(),
                Some(&home)
            )
            .0,
            0
        );
        assert_eq!(
            run_cli(
                &["add", path_as_str(&layout).unwrap(), "as", "demo"],
                &workspace,
                Some(&home)
            )
            .0,
            0
        );

        let (code, _, stderr) = run_cli(&["run", "demo"], &workspace, Some(&home));
        assert_eq!(code, 0);
        assert!(stderr.contains("deprecated"));
        assert_eq!(
            fs::read_to_string(&marker).expect("marker file should be readable"),
            "executed"
        );

        let (code, stdout, stderr) = run_cli(
            &[
                "release",
                "--manifest",
                path_as_str(&manifest).unwrap(),
                "--dist",
                path_as_str(&artifact_root).unwrap(),
                "--output",
                path_as_str(&layout).unwrap(),
                "--push",
                &format!("file://{}", pushed.display()),
            ],
            temp.path(),
            None,
        );
        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(stdout.contains(pushed.to_str().expect("push path should be UTF-8")));
        assert_oci_layout(&pushed);
    }

    fn assert_oci_layout(root: &Path) {
        assert!(root.join("oci-layout").is_file());
        assert!(root.join("index.json").is_file());
        let blob_dir = root.join("blobs/sha256");
        assert!(blob_dir.is_dir());
        assert!(
            fs::read_dir(blob_dir)
                .expect("blob directory should be readable")
                .count()
                >= 3
        );
    }

    fn run_cli(args: &[&str], cwd: &Path, home: Option<&Path>) -> (i32, String, String) {
        let mut argv = vec![OsString::from("tpx")];

        if let Some(home) = home {
            argv.push(OsString::from("--tpx-home"));
            argv.push(home.as_os_str().to_owned());
        }

        argv.extend(args.iter().map(OsString::from));

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_cli_from(argv, cwd, &mut stdout, &mut stderr)
            .expect("CLI invocation should succeed");

        (
            code,
            String::from_utf8(stdout).expect("stdout should be valid UTF-8"),
            String::from_utf8(stderr).expect("stderr should be valid UTF-8"),
        )
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("executable should be written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(path)
                .expect("metadata should be readable")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("permissions should be updated");
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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
