use anyhow::{Context as AnyhowContext, anyhow, bail};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
};
use tar::Archive;
use tpx_parser::{ParsedDocument, parse_document_file};
use tpx_runtime::{Context, ResolvedTool, Result, Runtime, Tool};
use tpx_store::ContentStore;

const OCI_LAYOUT_FILE: &str = "oci-layout";
const OCI_INDEX_FILE: &str = "index.json";
const OCI_PROVIDER_MANIFEST_FILE: &str = "tpx.yaml";
const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

#[derive(Debug, Deserialize)]
struct OciIndexDocument {
    manifests: Vec<OciDescriptor>,
}

#[derive(Debug, Clone, Deserialize)]
struct OciDescriptor {
    #[serde(default, rename = "mediaType")]
    media_type: Option<String>,
    digest: String,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
    #[serde(default)]
    platform: Option<OciPlatform>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct OciPlatform {
    architecture: String,
    os: String,
}

#[derive(Debug, Deserialize)]
struct OciManifestDocument {
    layers: Vec<OciDescriptor>,
}

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
        let layers = download_layers(source)?;

        if layers.is_empty() {
            bail!(
                "OCI source '{}' did not contain any extractable layers",
                source
            );
        }

        let home = resolve_tpx_home(self.home_override.as_deref())?;
        let store = ContentStore::with_home(&home);
        let digests = layers
            .iter()
            .map(|bytes| {
                store
                    .put(bytes)
                    .with_context(|| format!("failed to persist OCI layer from {source}"))
            })
            .collect::<Result<Vec<_>>>()?;
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

        for bytes in layers {
            Archive::new(Cursor::new(bytes))
                .unpack(&extract_root)
                .with_context(|| {
                    format!(
                        "failed to extract OCI layer into {}",
                        extract_root.display()
                    )
                })?;
        }
        fs::write(
            source_root.join(".installed"),
            format!("source={source}\ndigests={}\n", digests.join(",")),
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
    tool.spec.assets.first().map(String::as_str).ok_or_else(|| {
        anyhow!("oci runtime requires tool.spec.assets[0] as a tar layer or OCI layout source")
    })
}

fn source_root(home_override: Option<&Path>, source: &str) -> Result<PathBuf> {
    Ok(resolve_tpx_home(home_override)?
        .join("runtime")
        .join("oci")
        .join(sha256_hex(source.as_bytes())))
}

fn resolve_entrypoint_path(tool: &Tool, extract_root: &Path) -> Result<PathBuf> {
    let hint = entrypoint_hint(tool, extract_root)?;
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

fn entrypoint_hint(tool: &Tool, extract_root: &Path) -> Result<String> {
    if let Some(entrypoint) = provider_manifest_entrypoint(extract_root)? {
        return Ok(entrypoint);
    }

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

fn provider_manifest_entrypoint(extract_root: &Path) -> Result<Option<String>> {
    let manifest_path = extract_root.join(OCI_PROVIDER_MANIFEST_FILE);

    if !manifest_path.is_file() {
        return Ok(None);
    }

    match parse_document_file(path_as_str(&manifest_path)?)? {
        ParsedDocument::ProviderManifest(manifest) => Ok(Some(manifest.spec.entrypoint)),
        _ => Ok(None),
    }
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

fn download_layers(source: &str) -> Result<Vec<Vec<u8>>> {
    if let Some(path) = source.strip_prefix("file://") {
        return read_oci_source_path(Path::new(path), source);
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        let response = Client::new()
            .get(source)
            .send()
            .and_then(|response| response.error_for_status())
            .with_context(|| format!("failed to download OCI tar layer from {source}"))?;

        return response
            .bytes()
            .map(|bytes| vec![bytes.to_vec()])
            .with_context(|| format!("failed to read OCI tar layer body from {source}"));
    }

    read_oci_source_path(Path::new(source), source)
}

fn read_oci_source_path(path: &Path, source: &str) -> Result<Vec<Vec<u8>>> {
    if path.is_dir() {
        return read_layout_layers(path, source);
    }

    fs::read(path)
        .map(|bytes| vec![bytes])
        .with_context(|| format!("failed to read OCI tar layer from {source}"))
}

fn read_layout_layers(layout_root: &Path, source: &str) -> Result<Vec<Vec<u8>>> {
    if !layout_root.join(OCI_LAYOUT_FILE).is_file() {
        bail!(
            "OCI layout marker '{}' was not found under {}",
            OCI_LAYOUT_FILE,
            layout_root.display()
        );
    }

    let root_index = read_json::<OciIndexDocument>(&layout_root.join(OCI_INDEX_FILE), source)?;
    let root_descriptor = select_descriptor(&root_index.manifests)?;
    let manifest_descriptor = resolve_manifest_descriptor(layout_root, &root_descriptor)?;
    let manifest = read_blob_json::<OciManifestDocument>(layout_root, &manifest_descriptor.digest)?;

    if manifest.layers.is_empty() {
        bail!(
            "OCI manifest in {} did not contain any layers",
            layout_root.display()
        );
    }

    manifest
        .layers
        .iter()
        .map(|layer| read_blob(layout_root, &layer.digest))
        .collect()
}

fn resolve_manifest_descriptor(
    layout_root: &Path,
    descriptor: &OciDescriptor,
) -> Result<OciDescriptor> {
    match descriptor.media_type.as_deref() {
        Some(OCI_IMAGE_INDEX_MEDIA_TYPE) => {
            let nested = read_blob_json::<OciIndexDocument>(layout_root, &descriptor.digest)?;
            select_descriptor(&nested.manifests)
        }
        Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE) | None => Ok(descriptor.clone()),
        Some(other) => bail!(
            "unsupported OCI descriptor media type '{}' in {}",
            other,
            layout_root.display()
        ),
    }
}

fn select_descriptor(descriptors: &[OciDescriptor]) -> Result<OciDescriptor> {
    if descriptors.is_empty() {
        bail!("OCI descriptor list was empty");
    }

    if let Some(platform) = current_oci_platform() {
        if let Some(descriptor) = descriptors.iter().find(|descriptor| {
            descriptor
                .platform
                .as_ref()
                .map(|candidate| candidate == &platform)
                .unwrap_or(false)
        }) {
            return Ok(descriptor.clone());
        }
    }

    descriptors
        .iter()
        .find(|descriptor| {
            descriptor
                .annotations
                .contains_key("org.opencontainers.image.ref.name")
        })
        .cloned()
        .or_else(|| descriptors.first().cloned())
        .ok_or_else(|| anyhow!("OCI descriptor list was empty"))
}

fn current_oci_platform() -> Option<OciPlatform> {
    Some(OciPlatform {
        architecture: normalize_arch(env::consts::ARCH).to_owned(),
        os: normalize_os(env::consts::OS).to_owned(),
    })
}

fn normalize_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn normalize_os(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        other => other,
    }
}

fn read_json<T: DeserializeOwned>(path: &Path, source: &str) -> Result<T> {
    serde_json::from_slice(
        &fs::read(path)
            .with_context(|| format!("failed to read OCI document {}", path.display()))?,
    )
    .with_context(|| {
        format!(
            "failed to parse OCI document {} from {}",
            path.display(),
            source
        )
    })
}

fn read_blob_json<T: DeserializeOwned>(layout_root: &Path, digest: &str) -> Result<T> {
    serde_json::from_slice(&read_blob(layout_root, digest)?).with_context(|| {
        format!(
            "failed to parse OCI blob '{}' from {}",
            digest,
            layout_root.display()
        )
    })
}

fn read_blob(layout_root: &Path, digest: &str) -> Result<Vec<u8>> {
    let blob_path = blob_path(layout_root, digest)?;

    fs::read(&blob_path).with_context(|| format!("failed to read OCI blob {}", blob_path.display()))
}

fn blob_path(layout_root: &Path, digest: &str) -> Result<PathBuf> {
    let (algorithm, hex) = digest
        .split_once(':')
        .ok_or_else(|| anyhow!("OCI digest '{}' is missing an algorithm prefix", digest))?;

    if algorithm != "sha256" {
        bail!("unsupported OCI digest algorithm '{}'", algorithm);
    }

    Ok(layout_root.join("blobs").join(algorithm).join(hex))
}

fn path_as_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path {} is not valid UTF-8", path.display()))
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
    use serde_json::json;
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

    #[test]
    fn installs_and_executes_from_local_oci_layout() {
        let temp = temp_dir("runtime-oci-layout");
        let home = temp.path().join("home");
        let layout_root = temp.path().join("oci");
        let marker = temp.path().join("layout-executed.txt");
        let runtime = OciRuntime::with_home(&home);
        let tool = oci_layout_tool(&layout_root, "workspace-alias");
        let resolved = runtime
            .resolve(&tool, &Context::default())
            .expect("tool should resolve");

        write_oci_layout(
            &layout_root,
            "provider-binary",
            &format!("#!/bin/sh\nprintf layout > '{}'\n", marker.display()),
        );

        runtime
            .install(&resolved, &Context::default())
            .expect("OCI layout should install");
        assert_eq!(
            runtime
                .execute(&resolved, &[], &Context::default())
                .expect("OCI layout tool should execute"),
            0
        );
        assert_eq!(
            fs::read_to_string(marker).expect("marker file should be readable"),
            "layout"
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

    fn oci_layout_tool(layout_root: &Path, alias: &str) -> Tool {
        let mut tool = Tool::default();

        tool.metadata = Some(tpx_parser::Metadata { name: alias.into() });
        tool.spec.runtime = Some("oci".into());
        tool.spec.assets.push(
            layout_root
                .to_str()
                .expect("path should be valid UTF-8")
                .into(),
        );

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

    fn write_oci_layout(layout_root: &Path, entry_name: &str, contents: &str) {
        let provider_manifest = format!(
            "apiVersion: tpx.io/v1\nkind: Provider\nmetadata:\n  namespace: acme\n  name: test\n  version: v1.0.0\nspec:\n  runtime: binary\n  entrypoint: {}\n  platforms:\n    - os: darwin\n      arch: arm64\n      binary: {}\n",
            entry_name, entry_name
        );
        let provider_tar = layout_root.join("provider.tar");
        let provider_bytes;

        fs::create_dir_all(layout_root).expect("layout root should be created");
        write_tar_layer_with_extra(
            &provider_tar,
            &[
                (entry_name, contents),
                (OCI_PROVIDER_MANIFEST_FILE, &provider_manifest),
            ],
        );
        provider_bytes = fs::read(&provider_tar).expect("provider tar should be readable");
        fs::remove_file(&provider_tar).expect("provider tar should be removed");

        let layer_digest = write_layout_blob(layout_root, &provider_bytes);
        let config_bytes = serde_json::to_vec(&json!({
            "architecture": "arm64",
            "os": "darwin",
            "rootfs": {
                "type": "layers",
                "diff_ids": [format!("sha256:{}", sha256_hex(&provider_bytes))]
            }
        }))
        .expect("config JSON should serialize");
        let config_digest = write_layout_blob(layout_root, &config_bytes);
        let manifest_bytes = serde_json::to_vec(&json!({
            "schemaVersion": 2,
            "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": format!("sha256:{}", config_digest),
                "size": config_bytes.len()
            },
            "layers": [{
                "mediaType": "application/vnd.oci.image.layer.v1.tar",
                "digest": format!("sha256:{}", layer_digest),
                "size": provider_bytes.len()
            }]
        }))
        .expect("manifest JSON should serialize");
        let manifest_digest = write_layout_blob(layout_root, &manifest_bytes);

        fs::write(
            layout_root.join(OCI_LAYOUT_FILE),
            serde_json::to_vec(&json!({"imageLayoutVersion": "1.0.0"}))
                .expect("layout JSON should serialize"),
        )
        .expect("oci-layout should be written");
        fs::write(
            layout_root.join(OCI_INDEX_FILE),
            serde_json::to_vec(&json!({
                "schemaVersion": 2,
                "mediaType": OCI_IMAGE_INDEX_MEDIA_TYPE,
                "manifests": [{
                    "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
                    "digest": format!("sha256:{}", manifest_digest),
                    "size": manifest_bytes.len(),
                    "platform": {
                        "os": "darwin",
                        "architecture": "arm64"
                    },
                    "annotations": {
                        "org.opencontainers.image.ref.name": "test:latest"
                    }
                }]
            }))
            .expect("index JSON should serialize"),
        )
        .expect("index.json should be written");
    }

    fn write_layout_blob(layout_root: &Path, bytes: &[u8]) -> String {
        let digest = sha256_hex(bytes);
        let blob_path = layout_root.join("blobs/sha256").join(&digest);

        fs::create_dir_all(blob_path.parent().expect("blob parent should exist"))
            .expect("blob directory should be created");
        fs::write(&blob_path, bytes).expect("blob should be written");

        digest
    }

    fn write_tar_layer_with_extra(path: &Path, entries: &[(&str, &str)]) {
        let file = File::create(path).expect("tar file should be created");
        let mut builder = Builder::new(file);

        for (name, contents) in entries {
            let mut header = Header::new_gnu();

            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, *name, contents.as_bytes())
                .expect("tar entry should be appended");
        }

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
