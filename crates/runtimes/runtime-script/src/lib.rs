use anyhow::{Context as AnyhowContext, anyhow, bail};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};
use tpx_runtime::{Context, ResolvedTool, Result, Runtime, Tool};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScriptRuntime {
    home_override: Option<PathBuf>,
}

impl ScriptRuntime {
    pub fn with_home<P: AsRef<Path>>(home: P) -> Self {
        Self {
            home_override: Some(home.as_ref().to_path_buf()),
        }
    }
}

impl Runtime for ScriptRuntime {
    fn name(&self) -> &str {
        "script"
    }

    fn resolve(&self, tool: &Tool, _ctx: &Context) -> Result<ResolvedTool> {
        Ok(ResolvedTool::new(self.name(), tool.clone()))
    }

    fn is_installed(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<bool> {
        resolved.ensure_runtime(self.name())?;

        match script_source(resolved.tool())? {
            ScriptSource::Inline(_) => Ok(true),
            ScriptSource::File(path) => Ok(path.is_file()),
        }
    }

    fn install(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<()> {
        resolved.ensure_runtime(self.name())?;
        let cache_root = script_cache_root(self.home_override.as_deref())?;

        fs::create_dir_all(&cache_root).with_context(|| {
            format!(
                "failed to create script runtime cache at {}",
                cache_root.display()
            )
        })?;

        Ok(())
    }

    fn execute(&self, resolved: &ResolvedTool, args: &[String], _ctx: &Context) -> Result<i32> {
        resolved.ensure_runtime(self.name())?;

        let tool = resolved.tool();
        let script_name = script_name(tool);
        let cache_entry = cache_entry(self.home_override.as_deref(), tool, args)?;

        if cache_entry.is_complete() {
            replay_output(&cache_entry.stdout_path, &cache_entry.stderr_path)?;
            return cache_entry.read_status();
        }

        let shell = script_shell(tool);
        let output = match script_source(tool)? {
            ScriptSource::Inline(contents) => Command::new(&shell)
                .arg("-c")
                .arg(contents)
                .arg(script_name)
                .args(args)
                .output(),
            ScriptSource::File(path) => Command::new(&shell).arg(path).args(args).output(),
        }
        .with_context(|| format!("failed to execute script runtime with shell {shell}"))?;
        let status = exit_code(output.status);

        fs::create_dir_all(&cache_entry.root).with_context(|| {
            format!(
                "failed to create script cache directory {}",
                cache_entry.root.display()
            )
        })?;
        fs::write(&cache_entry.stdout_path, &output.stdout).with_context(|| {
            format!(
                "failed to write script stdout cache {}",
                cache_entry.stdout_path.display()
            )
        })?;
        fs::write(&cache_entry.stderr_path, &output.stderr).with_context(|| {
            format!(
                "failed to write script stderr cache {}",
                cache_entry.stderr_path.display()
            )
        })?;
        fs::write(&cache_entry.status_path, status.to_string()).with_context(|| {
            format!(
                "failed to write script status cache {}",
                cache_entry.status_path.display()
            )
        })?;

        replay_output(&cache_entry.stdout_path, &cache_entry.stderr_path)?;

        Ok(status)
    }
}

enum ScriptSource {
    Inline(String),
    File(PathBuf),
}

struct CacheEntry {
    root: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    status_path: PathBuf,
}

impl CacheEntry {
    fn is_complete(&self) -> bool {
        self.stdout_path.is_file() && self.stderr_path.is_file() && self.status_path.is_file()
    }

    fn read_status(&self) -> Result<i32> {
        let status = fs::read_to_string(&self.status_path).with_context(|| {
            format!(
                "failed to read script status cache {}",
                self.status_path.display()
            )
        })?;

        status.trim().parse::<i32>().with_context(|| {
            format!(
                "failed to parse script exit status from {}",
                self.status_path.display()
            )
        })
    }
}

fn script_source(tool: &Tool) -> Result<ScriptSource> {
    let source =
        tool.spec.assets.first().ok_or_else(|| {
            anyhow!("script runtime requires tool.spec.assets[0] as a script source")
        })?;

    if let Some(inline) = source.strip_prefix("inline:") {
        return Ok(ScriptSource::Inline(inline.to_owned()));
    }

    Ok(ScriptSource::File(PathBuf::from(source)))
}

fn script_shell(tool: &Tool) -> String {
    tool.spec
        .version
        .clone()
        .unwrap_or_else(|| "/bin/sh".into())
}

fn script_name(tool: &Tool) -> String {
    tool.metadata
        .as_ref()
        .map(|metadata| metadata.name.clone())
        .unwrap_or_else(|| "script".into())
}

fn cache_entry(home_override: Option<&Path>, tool: &Tool, args: &[String]) -> Result<CacheEntry> {
    let key = cache_key(tool, args)?;
    let root = script_cache_root(home_override)?;

    Ok(CacheEntry {
        stdout_path: root.join(format!("{key}.stdout")),
        stderr_path: root.join(format!("{key}.stderr")),
        status_path: root.join(format!("{key}.status")),
        root,
    })
}

fn cache_key(tool: &Tool, args: &[String]) -> Result<String> {
    let mut hasher = Sha256::new();
    let shell = script_shell(tool);

    hasher.update(shell.as_bytes());
    hasher.update([0]);

    match script_source(tool)? {
        ScriptSource::Inline(contents) => {
            hasher.update(b"inline");
            hasher.update([0]);
            hasher.update(contents.as_bytes());
        }
        ScriptSource::File(path) => {
            hasher.update(b"file");
            hasher.update([0]);
            hasher.update(
                fs::read(&path)
                    .with_context(|| format!("failed to read script source {}", path.display()))?,
            );
        }
    }

    for arg in args {
        hasher.update([0]);
        hasher.update(arg.as_bytes());
    }

    Ok(hex_digest(hasher.finalize()))
}

fn script_cache_root(home_override: Option<&Path>) -> Result<PathBuf> {
    Ok(resolve_tpx_home(home_override)?
        .join("runtime")
        .join("script"))
}

fn resolve_tpx_home(home_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(home) = home_override {
        return Ok(home.to_path_buf());
    }

    if let Some(home) = env::var_os("TPX_HOME") {
        return Ok(PathBuf::from(home));
    }

    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return Ok(PathBuf::from(home).join(".tpx"));
    }

    bail!("failed to resolve TPX home directory")
}

fn replay_output(stdout_path: &Path, stderr_path: &Path) -> Result<()> {
    let stdout = fs::read(stdout_path)
        .with_context(|| format!("failed to read cached stdout {}", stdout_path.display()))?;
    let stderr = fs::read(stderr_path)
        .with_context(|| format!("failed to read cached stderr {}", stderr_path.display()))?;

    io::stdout()
        .write_all(&stdout)
        .context("failed to replay cached script stdout")?;
    io::stderr()
        .write_all(&stderr)
        .context("failed to replay cached script stderr")?;

    Ok(())
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut output = String::with_capacity(digest.len() * 2);

    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }

    output
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
        let runtime = ScriptRuntime::default();

        assert_eq!(runtime.name(), "script");
    }

    #[test]
    fn resolves_tools_for_script_runtime() {
        let temp = temp_dir("runtime-script-resolve");
        let script = temp.path().join("script.sh");
        let tool = script_tool(&script);
        let runtime = ScriptRuntime::default();

        write_script(&script, "#!/bin/sh\nexit 0\n");

        let resolved = runtime
            .resolve(&tool, &Context::default())
            .expect("tool should resolve");

        assert_eq!(resolved.runtime(), "script");
        assert!(
            runtime
                .is_installed(&resolved, &Context::default())
                .expect("script runtime install check should succeed")
        );
    }

    #[test]
    fn executes_script_and_uses_cached_output() {
        let temp = temp_dir("runtime-script-cache");
        let home = temp.path().join("home");
        let script = temp.path().join("count.sh");
        let counter = temp.path().join("counter.txt");
        let runtime = ScriptRuntime::with_home(&home);
        let tool = script_tool(&script);
        let resolved = runtime
            .resolve(&tool, &Context::default())
            .expect("tool should resolve");

        write_script(
            &script,
            &format!(
                "#!/bin/sh\ncount=0\nif [ -f '{}' ]; then count=$(cat '{}'); fi\ncount=$((count+1))\nprintf '%s' \"$count\" > '{}'\nprintf 'cached output\\n'\n",
                counter.display(),
                counter.display(),
                counter.display()
            ),
        );

        runtime
            .install(&resolved, &Context::default())
            .expect("script cache directory should be created");
        assert_eq!(
            runtime
                .execute(&resolved, &[], &Context::default())
                .expect("script should execute"),
            0
        );
        assert_eq!(
            runtime
                .execute(&resolved, &[], &Context::default())
                .expect("cached script should replay"),
            0
        );
        assert_eq!(
            fs::read_to_string(&counter).expect("counter file should be readable"),
            "1"
        );

        let cache_root = script_cache_root(Some(&home)).expect("cache root should resolve");
        assert!(
            fs::read_dir(cache_root)
                .expect("cache directory should be readable")
                .count()
                >= 3
        );
    }

    fn script_tool(script: &Path) -> Tool {
        let mut tool = Tool::default();
        tool.spec.runtime = Some("script".into());
        tool.spec.version = Some("/bin/sh".into());
        tool.spec
            .assets
            .push(script.to_str().expect("path should be valid UTF-8").into());

        tool
    }

    fn write_script(path: &Path, contents: &str) {
        fs::write(path, contents).expect("script should be written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(path)
                .expect("script metadata should be readable")
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
