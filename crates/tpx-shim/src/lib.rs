use anyhow::{Context, Result, anyhow, bail};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tpx_workspace::WorkspaceContext;

pub const COMPONENT_NAME: &str = "tpx-shim";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShimManager;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShimSpec {
    pub alias: String,
    pub target: PathBuf,
}

impl ShimSpec {
    pub fn new(alias: impl Into<String>, target: impl Into<PathBuf>) -> Self {
        Self {
            alias: alias.into(),
            target: target.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LazyShimSpec {
    pub alias: String,
    pub tool: String,
}

impl LazyShimSpec {
    pub fn new(alias: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            tool: tool.into(),
        }
    }
}

impl ShimManager {
    pub const fn component_name(&self) -> &'static str {
        COMPONENT_NAME
    }

    pub fn write_workspace_shims(
        &self,
        ctx: &WorkspaceContext,
        specs: &[ShimSpec],
    ) -> Result<Vec<PathBuf>> {
        self.write_shims(&ctx.runtime_bin_dir, specs)
    }

    pub fn write_lazy_workspace_shims(
        &self,
        ctx: &WorkspaceContext,
        specs: &[LazyShimSpec],
    ) -> Result<Vec<PathBuf>> {
        let layout = ctx.write_lazy_layout()?;

        self.write_lazy_shims(&layout.bin_dir, specs)
    }

    pub fn write_shims(&self, bin_dir: &Path, specs: &[ShimSpec]) -> Result<Vec<PathBuf>> {
        recreate_directory(bin_dir)?;

        let mut sorted_specs = specs.to_vec();
        sorted_specs.sort_by(|left, right| left.alias.cmp(&right.alias));

        let mut written_paths = Vec::with_capacity(sorted_specs.len());

        for spec in sorted_specs {
            validate_alias(&spec.alias)?;

            let shim_path = bin_dir.join(&spec.alias);
            let shim_body = render_shim(&spec.target)?;

            fs::write(&shim_path, shim_body)
                .with_context(|| format!("failed to write shim {}", shim_path.display()))?;
            set_executable(&shim_path)?;
            written_paths.push(shim_path);
        }

        Ok(written_paths)
    }

    pub fn write_lazy_shims(&self, bin_dir: &Path, specs: &[LazyShimSpec]) -> Result<Vec<PathBuf>> {
        recreate_directory(bin_dir)?;

        let mut sorted_specs = specs.to_vec();
        sorted_specs.sort_by(|left, right| left.alias.cmp(&right.alias));

        let mut written_paths = Vec::with_capacity(sorted_specs.len());

        for spec in sorted_specs {
            validate_alias(&spec.alias)?;
            validate_tool_name(&spec.tool)?;

            let shim_path = bin_dir.join(&spec.alias);
            let shim_body = render_lazy_shim(&spec.tool);

            fs::write(&shim_path, shim_body)
                .with_context(|| format!("failed to write shim {}", shim_path.display()))?;
            set_executable(&shim_path)?;
            written_paths.push(shim_path);
        }

        Ok(written_paths)
    }
}

fn recreate_directory(path: &Path) -> Result<()> {
    if path.exists() {
        if path.is_dir() {
            fs::remove_dir_all(path)
                .with_context(|| format!("failed to remove shim directory {}", path.display()))?;
        } else {
            fs::remove_file(path)
                .with_context(|| format!("failed to remove shim file {}", path.display()))?;
        }
    }

    fs::create_dir_all(path)
        .with_context(|| format!("failed to create shim directory {}", path.display()))
}

fn validate_alias(alias: &str) -> Result<()> {
    if alias.trim().is_empty() {
        bail!("shim alias cannot be empty");
    }

    if alias == "." || alias == ".." || alias.contains(['/', '\\']) {
        bail!("shim alias '{alias}' must be a simple file name");
    }

    Ok(())
}

fn validate_tool_name(tool: &str) -> Result<()> {
    if tool.trim().is_empty() {
        bail!("lazy shim tool cannot be empty");
    }

    Ok(())
}

fn render_shim(target: &Path) -> Result<String> {
    let target = target
        .to_str()
        .ok_or_else(|| anyhow!("shim target {} is not valid UTF-8", target.display()))?;

    Ok(format!("#!/bin/sh\nexec {} \"$@\"\n", single_quote(target)))
}

fn render_lazy_shim(tool: &str) -> String {
    format!("#!/bin/sh\nexec tpx run {} \"$@\"\n", single_quote(tool))
}

fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to read shim metadata for {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to mark shim {} executable", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        process::{self, Command},
        time::{SystemTime, UNIX_EPOCH},
    };
    use tpx_workspace::load_workspace;

    #[test]
    fn exposes_component_name() {
        assert_eq!(ShimManager.component_name(), "tpx-shim");
    }

    #[test]
    fn recreates_directory_and_writes_sorted_shims() {
        let temp = temp_dir("tpx-shim-sort");
        let bin_dir = temp.path().join(".workspace/bin");
        let old_file = bin_dir.join("stale");
        let manager = ShimManager;

        fs::create_dir_all(&bin_dir).expect("bin directory should be created");
        fs::write(&old_file, "old").expect("stale file should be written");

        let written = manager
            .write_shims(
                &bin_dir,
                &[
                    ShimSpec::new("zeta", "/tmp/zeta"),
                    ShimSpec::new("alpha", "/tmp/alpha"),
                ],
            )
            .expect("shims should be written");

        assert_eq!(written, vec![bin_dir.join("alpha"), bin_dir.join("zeta")]);
        assert!(!old_file.exists());
        assert_eq!(
            fs::read_to_string(bin_dir.join("alpha")).expect("alpha shim should be readable"),
            "#!/bin/sh\nexec '/tmp/alpha' \"$@\"\n"
        );
        assert_eq!(
            fs::read_to_string(bin_dir.join("zeta")).expect("zeta shim should be readable"),
            "#!/bin/sh\nexec '/tmp/zeta' \"$@\"\n"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(bin_dir.join("alpha"))
                .expect("alpha shim metadata should be readable")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o755);
        }
    }

    #[test]
    fn writes_shims_into_workspace_context_bin_dir() {
        let temp = temp_dir("tpx-shim-workspace");
        let workspace_root = create_workspace(temp.path(), "demo");
        let ctx = load_workspace(&workspace_root, Some(temp.path()))
            .expect("workspace context should load");
        let manager = ShimManager;
        let target = temp.path().join("bin/node");

        fs::create_dir_all(target.parent().expect("target parent should exist"))
            .expect("target directory should be created");

        let written = manager
            .write_workspace_shims(&ctx, &[ShimSpec::new("node", &target)])
            .expect("workspace shims should be written");

        assert_eq!(written, vec![ctx.runtime_bin_dir.join("node")]);
        assert!(ctx.runtime_bin_dir.join("node").exists());
        assert_eq!(
            fs::read_to_string(ctx.runtime_bin_dir.join("node"))
                .expect("node shim should be readable"),
            format!(
                "#!/bin/sh\nexec {} \"$@\"\n",
                single_quote(target.to_str().expect("target path should be valid UTF-8"))
            )
        );
    }

    #[test]
    fn writes_lazy_shims_into_dot_tpx_bin_dir() {
        let temp = temp_dir("tpx-shim-lazy-workspace");
        let workspace_root = create_workspace(temp.path(), "demo");
        let ctx = load_workspace(&workspace_root, Some(temp.path()))
            .expect("workspace context should load");
        let manager = ShimManager;

        let written = manager
            .write_lazy_workspace_shims(&ctx, &[LazyShimSpec::new("node", "node")])
            .expect("lazy workspace shims should be written");
        let lazy_bin_dir = ctx.root.join(".tpx/bin");
        let lazy_env_path = ctx.root.join(".tpx/env");

        assert_eq!(written, vec![lazy_bin_dir.join("node")]);
        assert!(lazy_bin_dir.join("node").exists());
        assert!(lazy_env_path.exists());
        assert_eq!(
            fs::read_to_string(lazy_bin_dir.join("node"))
                .expect("lazy node shim should be readable"),
            "#!/bin/sh\nexec tpx run 'node' \"$@\"\n"
        );
    }

    #[test]
    fn lazy_shim_dispatches_to_tpx_run() {
        let temp = temp_dir("tpx-shim-lazy-dispatch");
        let workspace_root = create_workspace(temp.path(), "demo");
        let ctx = load_workspace(&workspace_root, Some(temp.path()))
            .expect("workspace context should load");
        let manager = ShimManager;
        let fake_bin_dir = temp.path().join("fake-bin");
        let capture_path = temp.path().join("capture.txt");

        fs::create_dir_all(&fake_bin_dir).expect("fake bin directory should be created");
        write_executable(
            &fake_bin_dir.join("tpx"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n",
                single_quote(
                    capture_path
                        .to_str()
                        .expect("capture path should be valid UTF-8")
                )
            ),
        );

        let written = manager
            .write_lazy_workspace_shims(&ctx, &[LazyShimSpec::new("node", "node")])
            .expect("lazy workspace shims should be written");
        let shim_path = written
            .first()
            .cloned()
            .expect("lazy shim should be created");
        let path_env = match env::var_os("PATH") {
            Some(existing) => format!(
                "{}:{}",
                fake_bin_dir.display(),
                PathBuf::from(existing).display()
            ),
            None => fake_bin_dir.display().to_string(),
        };
        let status = Command::new(&shim_path)
            .arg("build")
            .arg("--json")
            .env("PATH", path_env)
            .status()
            .expect("lazy shim should execute successfully");

        assert_eq!(status.code(), Some(0));
        assert_eq!(
            fs::read_to_string(capture_path).expect("captured invocation should be readable"),
            "run\nnode\nbuild\n--json\n"
        );
    }

    #[test]
    fn rejects_invalid_aliases() {
        let temp = temp_dir("tpx-shim-invalid");
        let manager = ShimManager;
        let error = manager
            .write_shims(
                &temp.path().join("bin"),
                &[ShimSpec::new("nested/alias", "/tmp/tool")],
            )
            .expect_err("invalid alias should fail");

        assert_eq!(
            error.to_string(),
            "shim alias 'nested/alias' must be a simple file name"
        );
    }

    fn create_workspace(base: &Path, name: &str) -> PathBuf {
        let root = base.join(name);

        fs::create_dir_all(&root).expect("workspace root should be created");
        fs::write(
            root.join("tpx.yaml"),
            format!(
                "apiVersion: tpx.io/v1\nkind: Workspace\nworkspace: {name}\nproviders:\n  node:\n    source: core/node\n"
            ),
        )
        .expect("workspace manifest should be written");

        fs::canonicalize(root).expect("workspace root should canonicalize")
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("executable should be written");
        set_executable(path).expect("executable bit should be set");
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
