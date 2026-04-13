use anyhow::{Context as AnyhowContext, anyhow, bail};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
};
use tar::Archive;
use tpx_runtime::{Context, ResolvedTool, Result, Runtime, Tool};
use tpx_store::ContentStore;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OciRuntime {
    home_override: Option<PathBuf>,
}

impl OciRuntime {
    pub fn with_home<P: AsRef<Path>>(home: P) -> Self {
        Self {
            home_override: Some(home.as_ref().to_path_buf()),
        }
    }
}

impl Runtime for OciRuntime {
    fn name(&self) -> &str {
        "oci"
    }

    fn resolve(&self, tool: &Tool, _ctx: &Context) -> Result<ResolvedTool> {
        Ok(ResolvedTool::new(self.name(), tool.clone()))
    }

    fn is_installed(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<bool> {
        resolved.ensure_runtime(self.name())?;

        let tool = resolved.tool();
        let source_root = source_root(self.home_override.as_deref(), oci_source(tool)?)?;
        let extract_root = source_root.join("root");

        if !source_root.join(".installed").is_file() {
            return Ok(false);
        }

        Ok(resolve_entrypoint_path(tool, &extract_root)
            .map(|path| path.is_file())
            .unwrap_or(false))
    }

    fn install(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<()> {
        resolved.ensure_runtime(self.name())?;

        if self.is_installed(resolved, _ctx)? {
            return Ok(());
        }

        let tool = resolved.tool();
        let source = oci_source(tool)?;
        let bytes = download_layer(source)?;
        let home = resolve_tpx_home(self.home_override.as_deref())?;
        let store = ContentStore::with_home(&home);
        let digest = store
            .put(&bytes)
            .with_context(|| format!("failed to persist OCI layer from {source}"))?;
        let source_root = source_root(Some(&home), source)?;
        let extract_root = source_root.join("root");

        fs::create_dir_all(&source_root).with_context(|| {
            format!(
                "failed to create OCI runtime directory {}",
                source_root.display()
            )
        })?;

        if extract_root.exists() {
            fs::remove_dir_all(&extract_root).with_context(|| {
                format!(
                    "failed to remove OCI extract directory {}",
                    extract_root.display()
                )
            })?;
        }
        fs::create_dir_all(&extract_root).with_context(|| {
            format!(
                "failed to create OCI extract directory {}",
                extract_root.display()
            )
        })?;

        Archive::new(Cursor::new(bytes))
            .unpack(&extract_root)
            .with_context(|| {
                format!(
                    "failed to extract OCI layer into {}",
                    extract_root.display()
                )
            })?;
        fs::write(
            source_root.join(".installed"),
            format!("source={source}\ndigest={digest}\n"),
        )
        .with_context(|| {
            format!(
                "failed to write OCI install marker in {}",
                source_root.display()
            )
        })?;

        Ok(())
    }

    fn execute(&self, resolved: &ResolvedTool, args: &[String], ctx: &Context) -> Result<i32> {
        resolved.ensure_runtime(self.name())?;

        if !self.is_installed(resolved, ctx)? {
            self.install(resolved, ctx)?;
        }

        let tool = resolved.tool();
        let extract_root =
            source_root(self.home_override.as_deref(), oci_source(tool)?)?.join("root");
        let entrypoint = resolve_entrypoint_path(tool, &extract_root)?;
        let status = Command::new(&entrypoint)
            .args(args)
            .status()
            .with_context(|| {
                format!(
                    "failed to execute OCI runtime binary {}",
                    entrypoint.display()
                )
            })?;

        Ok(exit_code(status))
    }
}

fn oci_source(tool: &Tool) -> Result<&str> {
    tool.spec
        .assets
        .first()
        .map(String::as_str)
        .ok_or_else(|| anyhow!("oci runtime requires tool.spec.assets[0] as a tar layer source"))
}

fn source_root(home_override: Option<&Path>, source: &str) -> Result<PathBuf> {
    Ok(resolve_tpx_home(home_override)?
        .join("runtime")
        .join("oci")
        .join(sha256_hex(source.as_bytes())))
}

fn resolve_entrypoint_path(tool: &Tool, extract_root: &Path) -> Result<PathBuf> {
    let hint = entrypoint_hint(tool)?;
    let hint_path = Path::new(&hint);

    if hint_path.components().count() > 1 {
        let path = extract_root.join(hint_path);

        if path.is_file() {
            return Ok(path);
        }

        bail!(
            "OCI runtime entrypoint '{}' was not extracted",
            path.display()
        )
    }

    search_by_file_name(extract_root, &hint)?.ok_or_else(|| {
        anyhow!(
            "OCI runtime entrypoint '{}' was not found under {}",
            hint,
            extract_root.display()
        )
    })
}

fn entrypoint_hint(tool: &Tool) -> Result<String> {
    if let Some(metadata) = tool.metadata.as_ref() {
        if !metadata.name.trim().is_empty() {
            return Ok(metadata.name.clone());
        }
    }

    if let Some(version) = tool.spec.version.as_ref() {
        if !version.trim().is_empty() {
            return Ok(version.clone());
        }
    }

    bail!("oci runtime requires tool.metadata.name or tool.spec.version as an entrypoint hint")
}

fn search_by_file_name(root: &Path, name: &str) -> Result<Option<PathBuf>> {
    let mut stack = vec![root.to_path_buf()];
    let mut matches = Vec::new();

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
            } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
                matches.push(path);
            }
        }
    }

    matches.sort();

    Ok(matches.into_iter().next())
}

fn download_layer(source: &str) -> Result<Vec<u8>> {
    if let Some(path) = source.strip_prefix("file://") {
        return fs::read(path)
            .with_context(|| format!("failed to read OCI tar layer from {source}"));
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        let response = Client::new()
            .get(source)
            .send()
            .and_then(|response| response.error_for_status())
            .with_context(|| format!("failed to download OCI tar layer from {source}"))?;

        return response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .with_context(|| format!("failed to read OCI tar layer body from {source}"));
    }

    fs::read(source).with_context(|| format!("failed to read OCI tar layer from {source}"))
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
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
        fs,
        fs::File,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tar::{Builder, Header};

    #[test]
    fn reports_runtime_name() {
        let runtime = OciRuntime::default();

        assert_eq!(runtime.name(), "oci");
    }

    #[test]
    fn resolves_tools_for_oci_runtime() {
        let temp = temp_dir("runtime-oci-resolve");
        let tar_path = temp.path().join("layer.tar");
        let tool = oci_tool(&tar_path, "kubectl");
        let runtime = OciRuntime::default();

        write_tar_layer(&tar_path, "kubectl", "#!/bin/sh\nexit 0\n");

        let resolved = runtime
            .resolve(&tool, &Context::default())
            .expect("tool should resolve");

        assert_eq!(resolved.runtime(), "oci");
        assert!(
            !runtime
                .is_installed(&resolved, &Context::default())
                .expect("oci runtime install check should succeed")
        );
    }

    #[test]
    fn installs_and_executes_extracted_tar_layer() {
        let temp = temp_dir("runtime-oci-install");
        let home = temp.path().join("home");
        let tar_path = temp.path().join("layer.tar");
        let marker = temp.path().join("executed.txt");
        let runtime = OciRuntime::with_home(&home);
        let tool = oci_tool(&tar_path, "kubectl");
        let resolved = runtime
            .resolve(&tool, &Context::default())
            .expect("tool should resolve");

        write_tar_layer(
            &tar_path,
            "kubectl",
            &format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
        );

        assert!(
            !runtime
                .is_installed(&resolved, &Context::default())
                .expect("OCI install state should be checked")
        );

        runtime
            .install(&resolved, &Context::default())
            .expect("OCI layer should install");
        assert!(
            runtime
                .is_installed(&resolved, &Context::default())
                .expect("OCI install state should be rechecked")
        );
        assert_eq!(
            runtime
                .execute(&resolved, &[], &Context::default())
                .expect("extracted OCI tool should execute"),
            0
        );
        assert_eq!(
            fs::read_to_string(marker).expect("marker file should be readable"),
            "executed"
        );
    }

    fn oci_tool(tar_path: &Path, entrypoint: &str) -> Tool {
        let mut tool = Tool::default();

        tool.spec.runtime = Some("oci".into());
        tool.spec.version = Some(entrypoint.into());
        tool.spec.assets.push(format!(
            "file://{}",
            tar_path.to_str().expect("path should be valid UTF-8")
        ));

        tool
    }

    fn write_tar_layer(path: &Path, entry_name: &str, contents: &str) {
        let file = File::create(path).expect("tar file should be created");
        let mut builder = Builder::new(file);
        let mut header = Header::new_gnu();

        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, entry_name, contents.as_bytes())
            .expect("tar entry should be appended");
        builder.finish().expect("tar archive should finish");
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
