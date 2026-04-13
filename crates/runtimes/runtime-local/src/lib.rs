use anyhow::{Context as AnyhowContext, bail};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};
use tpx_runtime::{Context, ResolvedTool, Result, Runtime, Tool};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LocalRuntime;

impl Runtime for LocalRuntime {
    fn name(&self) -> &str {
        "local"
    }

    fn resolve(&self, tool: &Tool, _ctx: &Context) -> Result<ResolvedTool> {
        Ok(ResolvedTool::new(self.name(), tool.clone()))
    }

    fn is_installed(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<bool> {
        resolved.ensure_runtime(self.name())?;

        Ok(resolve_executable_path(resolved.tool())?.is_file())
    }

    fn install(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<()> {
        resolved.ensure_runtime(self.name())?;

        let executable = resolve_executable_path(resolved.tool())?;

        if executable.is_file() {
            return Ok(());
        }

        bail!(
            "local runtime executable '{}' is not available",
            executable.display()
        )
    }

    fn execute(&self, resolved: &ResolvedTool, args: &[String], _ctx: &Context) -> Result<i32> {
        resolved.ensure_runtime(self.name())?;

        let executable = resolve_executable_path(resolved.tool())?;
        let status = Command::new(&executable)
            .args(args)
            .status()
            .with_context(|| {
                format!(
                    "failed to execute local runtime binary {}",
                    executable.display()
                )
            })?;

        Ok(exit_code(status))
    }
}

fn resolve_executable_path(tool: &Tool) -> Result<PathBuf> {
    let hint = executable_hint(tool)?;

    if hint.is_absolute() || hint.components().count() > 1 {
        return Ok(hint);
    }

    if let Some(path) = find_on_path(&hint) {
        return Ok(path);
    }

    Ok(hint)
}

fn executable_hint(tool: &Tool) -> Result<PathBuf> {
    if let Some(asset) = tool.spec.assets.first() {
        return Ok(PathBuf::from(asset));
    }

    if let Some(metadata) = tool.metadata.as_ref() {
        return Ok(PathBuf::from(&metadata.name));
    }

    bail!("local runtime requires tool.spec.assets[0] or tool.metadata.name")
}

fn find_on_path(executable: &Path) -> Option<PathBuf> {
    let executable = executable.as_os_str();

    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|entry| entry.join(executable))
            .find(|candidate| candidate.is_file())
    })
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
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn reports_runtime_name() {
        assert_eq!(LocalRuntime.name(), "local");
    }

    #[test]
    fn resolves_tools_for_local_runtime() {
        let temp = temp_dir("runtime-local-resolve");
        let executable = temp.path().join("tool.sh");
        let tool = local_tool(&executable);

        write_executable(&executable, "#!/bin/sh\nexit 0\n");

        let resolved = LocalRuntime
            .resolve(&tool, &Context::default())
            .expect("tool should resolve");

        assert_eq!(resolved.runtime(), "local");
        assert!(
            LocalRuntime
                .is_installed(&resolved, &Context::default())
                .expect("local runtime should report installation")
        );
    }

    #[test]
    fn executes_local_binary() {
        let temp = temp_dir("runtime-local-execute");
        let executable = temp.path().join("exit-seven.sh");
        let tool = local_tool(&executable);
        let resolved = LocalRuntime
            .resolve(&tool, &Context::default())
            .expect("tool should resolve");

        write_executable(&executable, "#!/bin/sh\nexit 7\n");

        LocalRuntime
            .install(&resolved, &Context::default())
            .expect("local install validation should succeed");
        assert_eq!(
            LocalRuntime
                .execute(&resolved, &[], &Context::default())
                .expect("local executable should run"),
            7
        );
    }

    fn local_tool(executable: &Path) -> Tool {
        let mut tool = Tool::default();
        tool.spec.runtime = Some("local".into());
        tool.spec.assets.push(
            executable
                .to_str()
                .expect("path should be valid UTF-8")
                .into(),
        );

        tool
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("executable should be written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(path)
                .expect("executable metadata should be readable")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("permissions should be updated");
        }
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
