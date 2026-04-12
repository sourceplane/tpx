use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};
use tpx_parser::{ParsedDocument, WorkspaceLock, WorkspaceManifest, parse_document_file};

pub const COMPONENT_NAME: &str = "tpx-workspace";
pub const DEFAULT_TPX_HOME_DIR: &str = ".tpx";
pub const HOME_CONFIG_FILE: &str = "config.yaml";
pub const WORKSPACE_MANIFEST_FILE: &str = "tpx.yaml";
pub const WORKSPACE_LOCK_FILE: &str = "tpx.lock";
pub const WORKSPACE_RUNTIME_DIR: &str = ".workspace";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Workspace;

impl Workspace {
    pub const fn component_name(&self) -> &'static str {
        COMPONENT_NAME
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct HomeConfig {
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default, rename = "activeWorkspace")]
    pub active_workspace: Option<PathBuf>,
    #[serde(default)]
    pub workspaces: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceStatus {
    Ready,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredWorkspace {
    pub name: String,
    pub root: PathBuf,
    pub status: WorkspaceStatus,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOptions {
    pub workspace: Option<String>,
    pub cwd: PathBuf,
    pub tpx_home: Option<PathBuf>,
}

impl ResolveOptions {
    pub fn new<P: Into<PathBuf>>(cwd: P) -> Self {
        Self {
            workspace: None,
            cwd: cwd.into(),
            tpx_home: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceContext {
    pub home: PathBuf,
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub lock_path: PathBuf,
    pub runtime_dir: PathBuf,
    pub runtime_env_path: PathBuf,
    pub runtime_path_path: PathBuf,
    pub runtime_bin_dir: PathBuf,
    pub manifest: WorkspaceManifest,
    pub lock: Option<WorkspaceLock>,
}

impl WorkspaceContext {
    pub fn workspace_name(&self) -> &str {
        &self.manifest.workspace
    }

    pub fn execution_dir(&self, cwd: &Path) -> Result<PathBuf> {
        execution_dir(cwd, &self.root)
    }
}

pub fn resolve_tpx_home(home_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(home) = home_override {
        return normalize_nonexistent_path(home);
    }

    if let Some(home) = env::var_os("TPX_HOME") {
        return normalize_nonexistent_path(Path::new(&home));
    }

    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return Ok(PathBuf::from(home).join(DEFAULT_TPX_HOME_DIR));
    }

    Err(anyhow!("failed to resolve TPX home directory"))
}

pub fn home_config_path(home_override: Option<&Path>) -> Result<PathBuf> {
    Ok(resolve_tpx_home(home_override)?.join(HOME_CONFIG_FILE))
}

pub fn load_home_config(home_override: Option<&Path>) -> Result<HomeConfig> {
    let home = resolve_tpx_home(home_override)?;

    load_home_config_from_home(&home)
}

pub fn save_home_config(home_override: Option<&Path>, config: &HomeConfig) -> Result<PathBuf> {
    let home = resolve_tpx_home(home_override)?;
    let config_path = home.join(HOME_CONFIG_FILE);
    let serialized =
        serde_yaml::to_string(config).context("failed to serialize TPX home config")?;

    fs::create_dir_all(&home)
        .with_context(|| format!("failed to create TPX home directory {}", home.display()))?;
    fs::write(&config_path, serialized).with_context(|| {
        format!(
            "failed to write TPX home config at {}",
            config_path.display()
        )
    })?;

    Ok(config_path)
}

pub fn register_workspace(
    config: &mut HomeConfig,
    name: impl Into<String>,
    root: &Path,
) -> Result<()> {
    let name = name.into();

    if name.trim().is_empty() {
        bail!("workspace name cannot be empty");
    }

    let root = normalize_workspace_root(root)?;
    config.workspaces.insert(name, root);

    Ok(())
}

pub fn unregister_workspace(config: &mut HomeConfig, name: &str) -> Option<PathBuf> {
    let removed = config.workspaces.remove(name);

    if let Some(root) = removed.as_ref() {
        if config.active_workspace.as_ref() == Some(root) {
            config.active_workspace = None;
        }
    }

    removed
}

pub fn set_active_workspace(config: &mut HomeConfig, root: &Path) -> Result<PathBuf> {
    let root = normalize_workspace_root(root)?;
    config.active_workspace = Some(root.clone());

    Ok(root)
}

pub fn registered_workspaces(config: &HomeConfig) -> Vec<RegisteredWorkspace> {
    config
        .workspaces
        .iter()
        .map(|(name, root)| RegisteredWorkspace {
            name: name.clone(),
            root: root.clone(),
            status: if workspace_manifest_exists(root) {
                WorkspaceStatus::Ready
            } else {
                WorkspaceStatus::Missing
            },
            active: config.active_workspace.as_ref() == Some(root),
        })
        .collect()
}

pub fn discover_workspace_root(start: &Path) -> Result<Option<PathBuf>> {
    let mut current = normalize_search_start(start)?;

    loop {
        if workspace_manifest_exists(&current) {
            return Ok(Some(current));
        }

        if !current.pop() {
            break;
        }
    }

    Ok(None)
}

pub fn load_workspace(root: &Path, home_override: Option<&Path>) -> Result<WorkspaceContext> {
    let home = resolve_tpx_home(home_override)?;
    let root = normalize_workspace_root(root)?;
    let manifest_path = root.join(WORKSPACE_MANIFEST_FILE);
    let lock_path = root.join(WORKSPACE_LOCK_FILE);
    let manifest = load_workspace_manifest(&manifest_path)?;
    let lock = if lock_path.is_file() {
        Some(load_workspace_lock(&lock_path, &manifest.workspace)?)
    } else {
        None
    };
    let runtime_dir = root.join(WORKSPACE_RUNTIME_DIR);

    Ok(WorkspaceContext {
        home,
        manifest_path,
        lock_path,
        runtime_env_path: runtime_dir.join("env"),
        runtime_path_path: runtime_dir.join("path"),
        runtime_bin_dir: runtime_dir.join("bin"),
        runtime_dir,
        manifest,
        lock,
        root,
    })
}

pub fn resolve_workspace(options: &ResolveOptions) -> Result<WorkspaceContext> {
    let home = resolve_tpx_home(options.tpx_home.as_deref())?;
    let config = load_home_config_from_home(&home)?;
    let root = resolve_workspace_root(&config, options.workspace.as_deref(), &options.cwd)?;

    load_workspace(&root, Some(&home))
}

pub fn execution_dir(cwd: &Path, workspace_root: &Path) -> Result<PathBuf> {
    let cwd = fs::canonicalize(cwd)
        .with_context(|| format!("failed to canonicalize current directory {}", cwd.display()))?;
    let workspace_root = normalize_workspace_root(workspace_root)?;

    if cwd.starts_with(&workspace_root) {
        Ok(cwd)
    } else {
        Ok(workspace_root)
    }
}

fn load_home_config_from_home(home: &Path) -> Result<HomeConfig> {
    let config_path = home.join(HOME_CONFIG_FILE);

    if !config_path.is_file() {
        return Ok(HomeConfig::default());
    }

    let source = fs::read_to_string(&config_path).with_context(|| {
        format!(
            "failed to read TPX home config at {}",
            config_path.display()
        )
    })?;

    if source.trim().is_empty() {
        return Ok(HomeConfig::default());
    }

    serde_yaml::from_str(&source).with_context(|| {
        format!(
            "failed to parse TPX home config at {}",
            config_path.display()
        )
    })
}

fn resolve_workspace_root(
    config: &HomeConfig,
    selection: Option<&str>,
    cwd: &Path,
) -> Result<PathBuf> {
    if let Some(selection) = selection {
        return resolve_explicit_workspace(selection, config, cwd);
    }

    if let Some(root) = discover_workspace_root(cwd)? {
        return Ok(root);
    }

    if let Some(active) = config.active_workspace.as_deref() {
        return normalize_workspace_root(active)
            .with_context(|| format!("active workspace at {} is unavailable", active.display()));
    }

    bail!(
        "failed to resolve workspace: no explicit target, no {} found from {}, and no active workspace configured",
        WORKSPACE_MANIFEST_FILE,
        cwd.display()
    )
}

fn resolve_explicit_workspace(selection: &str, config: &HomeConfig, cwd: &Path) -> Result<PathBuf> {
    if let Some(root) = config.workspaces.get(selection) {
        return normalize_workspace_root(root).with_context(|| {
            format!(
                "registered workspace '{selection}' at {} is unavailable",
                root.display()
            )
        });
    }

    let explicit_path = PathBuf::from(selection);
    let root = if explicit_path.is_absolute() {
        explicit_path
    } else {
        cwd.join(explicit_path)
    };

    normalize_workspace_root(&root)
        .with_context(|| format!("failed to resolve workspace target '{selection}'"))
}

fn load_workspace_manifest(path: &Path) -> Result<WorkspaceManifest> {
    match parse_document_file(path_as_str(path)?)? {
        ParsedDocument::WorkspaceManifest(manifest) => Ok(manifest),
        other => bail!(
            "expected Workspace manifest in {}, found {other:?}",
            path.display()
        ),
    }
}

fn load_workspace_lock(path: &Path, workspace_name: &str) -> Result<WorkspaceLock> {
    match parse_document_file(path_as_str(path)?)? {
        ParsedDocument::WorkspaceLock(lock) => {
            if lock.workspace != workspace_name {
                bail!(
                    "workspace lock '{}' does not match workspace manifest '{}'",
                    lock.workspace,
                    workspace_name
                );
            }

            Ok(lock)
        }
        other => bail!(
            "expected WorkspaceLock in {}, found {other:?}",
            path.display()
        ),
    }
}

fn normalize_search_start(start: &Path) -> Result<PathBuf> {
    let start = fs::canonicalize(start)
        .with_context(|| format!("failed to canonicalize search path {}", start.display()))?;

    if start.is_file() {
        start.parent().map(Path::to_path_buf).ok_or_else(|| {
            anyhow!(
                "cannot derive workspace search root from {}",
                start.display()
            )
        })
    } else {
        Ok(start)
    }
}

fn normalize_workspace_root(root: &Path) -> Result<PathBuf> {
    let directory = if root.is_file() {
        root.parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("cannot derive workspace root from {}", root.display()))?
    } else {
        root.to_path_buf()
    };
    let root = fs::canonicalize(&directory).with_context(|| {
        format!(
            "failed to canonicalize workspace root {}",
            directory.display()
        )
    })?;

    if !workspace_manifest_exists(&root) {
        bail!(
            "workspace manifest not found at {}",
            root.join(WORKSPACE_MANIFEST_FILE).display()
        );
    }

    Ok(root)
}

fn normalize_nonexistent_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(env::current_dir()
        .context("failed to resolve current directory for relative path")?
        .join(path))
}

fn workspace_manifest_exists(root: &Path) -> bool {
    root.join(WORKSPACE_MANIFEST_FILE).is_file()
}

fn path_as_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path {} is not valid UTF-8", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs, process,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn exposes_component_name() {
        assert_eq!(Workspace.component_name(), "tpx-workspace");
    }

    #[test]
    fn saves_and_loads_home_config() {
        let temp = temp_dir("tpx-workspace-config");
        let mut config = HomeConfig::default();

        config
            .aliases
            .insert("node".into(), "core/node@v20.19.0".into());
        config.active_workspace = Some(temp.path().join("workspace-a"));
        config
            .workspaces
            .insert("dev".into(), temp.path().join("workspace-a"));

        let config_path = save_home_config(Some(temp.path()), &config)
            .expect("config should be written successfully");
        let loaded = load_home_config(Some(temp.path())).expect("config should load successfully");

        assert_eq!(config_path, temp.path().join(HOME_CONFIG_FILE));
        assert_eq!(loaded, config);
    }

    #[test]
    fn registers_and_marks_missing_workspaces() {
        let temp = temp_dir("tpx-workspace-registry");
        let live_root = create_workspace(temp.path(), "live", false);
        let missing_root = temp.path().join("missing");
        let mut config = HomeConfig::default();

        register_workspace(&mut config, "live", &live_root)
            .expect("live workspace should register");
        config
            .workspaces
            .insert("missing".into(), missing_root.clone());
        set_active_workspace(&mut config, &live_root).expect("active workspace should be set");

        let registered = registered_workspaces(&config);

        assert_eq!(registered.len(), 2);
        assert_eq!(registered[0].name, "live");
        assert_eq!(registered[0].status, WorkspaceStatus::Ready);
        assert!(registered[0].active);
        assert_eq!(registered[1].name, "missing");
        assert_eq!(registered[1].root, missing_root);
        assert_eq!(registered[1].status, WorkspaceStatus::Missing);
        assert!(!registered[1].active);
    }

    #[test]
    fn discovers_workspace_root_from_nested_directory() {
        let temp = temp_dir("tpx-workspace-discovery");
        let root = create_workspace(temp.path(), "nested", false);
        let nested = root.join("src/bin");

        fs::create_dir_all(&nested).expect("nested directory should be created");

        let discovered = discover_workspace_root(&nested)
            .expect("workspace discovery should succeed")
            .expect("workspace root should be found");

        assert_eq!(discovered, root);
    }

    #[test]
    fn resolve_workspace_prefers_explicit_target() {
        let temp = temp_dir("tpx-workspace-explicit");
        let home = temp.path().join("home");
        let active_root = create_workspace(temp.path(), "active", false);
        let discovered_root = create_workspace(temp.path(), "discovered", false);
        let explicit_root = create_workspace(temp.path(), "explicit", false);
        let cwd = discovered_root.join("nested");
        let mut config = HomeConfig::default();

        fs::create_dir_all(&cwd).expect("nested cwd should be created");
        register_workspace(&mut config, "explicit", &explicit_root)
            .expect("explicit workspace should register");
        set_active_workspace(&mut config, &active_root).expect("active workspace should be set");
        save_home_config(Some(&home), &config).expect("home config should be saved");

        let context = resolve_workspace(&ResolveOptions {
            workspace: Some("explicit".into()),
            cwd,
            tpx_home: Some(home),
        })
        .expect("explicit resolution should succeed");

        assert_eq!(context.root, explicit_root);
        assert_eq!(context.workspace_name(), "explicit");
    }

    #[test]
    fn resolve_workspace_uses_upward_discovery_before_active_workspace() {
        let temp = temp_dir("tpx-workspace-priority");
        let home = temp.path().join("home");
        let active_root = create_workspace(temp.path(), "active", false);
        let discovered_root = create_workspace(temp.path(), "discovered", false);
        let cwd = discovered_root.join("examples/app");
        let mut config = HomeConfig::default();

        fs::create_dir_all(&cwd).expect("nested cwd should be created");
        set_active_workspace(&mut config, &active_root).expect("active workspace should be set");
        save_home_config(Some(&home), &config).expect("home config should be saved");

        let context = resolve_workspace(&ResolveOptions {
            workspace: None,
            cwd,
            tpx_home: Some(home),
        })
        .expect("workspace resolution should succeed");

        assert_eq!(context.root, discovered_root);
        assert_eq!(context.workspace_name(), "discovered");
    }

    #[test]
    fn resolve_workspace_falls_back_to_active_workspace() {
        let temp = temp_dir("tpx-workspace-active");
        let home = temp.path().join("home");
        let active_root = create_workspace(temp.path(), "active", false);
        let outside = temp.path().join("outside");
        let mut config = HomeConfig::default();

        fs::create_dir_all(&outside).expect("outside cwd should be created");
        set_active_workspace(&mut config, &active_root).expect("active workspace should be set");
        save_home_config(Some(&home), &config).expect("home config should be saved");

        let context = resolve_workspace(&ResolveOptions {
            workspace: None,
            cwd: outside,
            tpx_home: Some(home),
        })
        .expect("active workspace fallback should succeed");

        assert_eq!(context.root, active_root);
        assert_eq!(context.workspace_name(), "active");
    }

    #[test]
    fn load_workspace_reads_manifest_and_lock() {
        let temp = temp_dir("tpx-workspace-load");
        let root = create_workspace(temp.path(), "dev", true);
        let home = temp.path().join("home");

        let context =
            load_workspace(&root, Some(&home)).expect("workspace should load successfully");

        assert_eq!(context.home, home);
        assert_eq!(context.root, root);
        assert_eq!(context.workspace_name(), "dev");
        assert_eq!(
            context.lock.as_ref().map(|lock| lock.workspace.as_str()),
            Some("dev")
        );
        assert_eq!(
            context.runtime_dir,
            context.root.join(WORKSPACE_RUNTIME_DIR)
        );
        assert_eq!(context.runtime_env_path, context.runtime_dir.join("env"));
        assert_eq!(context.runtime_path_path, context.runtime_dir.join("path"));
        assert_eq!(context.runtime_bin_dir, context.runtime_dir.join("bin"));
    }

    #[test]
    fn execution_dir_preserves_cwd_inside_workspace_and_uses_root_outside() {
        let temp = temp_dir("tpx-workspace-execdir");
        let root = create_workspace(temp.path(), "dev", false);
        let inside = root.join("packages/demo");
        let outside = temp.path().join("outside");

        fs::create_dir_all(&inside).expect("inside cwd should be created");
        fs::create_dir_all(&outside).expect("outside cwd should be created");

        assert_eq!(
            execution_dir(&inside, &root).expect("inside cwd should resolve"),
            fs::canonicalize(&inside).expect("inside cwd should canonicalize")
        );
        assert_eq!(
            execution_dir(&outside, &root).expect("outside cwd should resolve"),
            root
        );
    }

    fn create_workspace(base: &Path, name: &str, with_lock: bool) -> PathBuf {
        let root = base.join(name);

        fs::create_dir_all(&root).expect("workspace root should be created");
        fs::write(root.join(WORKSPACE_MANIFEST_FILE), workspace_manifest(name))
            .expect("workspace manifest should be written");

        if with_lock {
            fs::write(root.join(WORKSPACE_LOCK_FILE), workspace_lock(name))
                .expect("workspace lock should be written");
        }

        fs::canonicalize(&root).expect("workspace root should canonicalize")
    }

    fn workspace_manifest(name: &str) -> String {
        format!(
            "apiVersion: tpx.io/v1\nkind: Workspace\nworkspace: {name}\nproviders:\n  node:\n    source: core/node\n"
        )
    }

    fn workspace_lock(name: &str) -> String {
        format!(
            "apiVersion: tpx.io/v1\nkind: WorkspaceLock\nworkspace: {name}\nproviders:\n  - alias: node\n    provider: core/node\n    source: core/node\n    version: v20.19.0\n    resolved: ghcr.io/sourceplane/node@sha256:deadbeef\n    store: abc123\n"
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
