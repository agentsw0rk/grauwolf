//! Setup of the tool-package commands (ADR-0107, ADR-0118): resolvers for the registries, git
//! repositories and skills of the environment, the tool policy, keys, packing a package folder and
//! pushing it. Shared by the host (agent runs) and the lean `grauwolf-tools` program; the host
//! re-exports it as `packages`.

use std::path::{Path, PathBuf};

use std::sync::Arc;

use grauwolf_config::{ProcessEnv, RegistryConfig};
use grauwolf_spec::ToolSpec;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::MANIFEST_FILE;

use crate::BuildInfo;
use crate::cosign::{PublicKey, SigningKey};
use crate::{
    AllowedSigners, LOCKFILE_NAME, ModuleBuilder, PackageResolver, PackedPackage, Policy,
    RegistryAuth, RegistryClient, pack_sources,
};

/// The lockfile of the spec `file`: `grauwolf.lock` in its directory.
pub fn lock_path(file: &Path) -> PathBuf {
    file.parent().unwrap_or(Path::new(".")).join(LOCKFILE_NAME)
}

/// The name of the policy file next to a spec.
pub const POLICY_FILE: &str = "grauwolf.toml";

/// The tool policy for the spec `file` (`None`: no spec, e.g. `tools pull`): the file named by
/// `GRAUWOLF_TOOL_POLICY`, else `grauwolf.toml` next to the spec (in the working directory
/// without a spec). Without either there is no policy: everything is allowed and nothing is
/// signed. A named file that is missing or invalid is an error.
pub fn load_policy(file: Option<&Path>, config: &RegistryConfig) -> Result<Option<Policy>, String> {
    if let Some(path) = &config.tool_policy {
        return Policy::load(path).map(Some);
    }
    let dir = match file {
        Some(file) => file.parent().unwrap_or(Path::new(".")),
        None => Path::new("."),
    };
    let path = dir.join(POLICY_FILE);
    if path.is_file() {
        Policy::load(&path).map(Some)
    } else {
        Ok(None)
    }
}

/// Resolver for the registries of the environment (`GRAUWOLF_REGISTRY_AUTH`), caching below
/// `tool_cache`; with `locked` it never changes the lockfile.
pub fn resolver(tool_cache: &Path, locked: bool) -> Result<PackageResolver, String> {
    resolver_with(tool_cache, locked, &RegistryConfig::from_env(&ProcessEnv))
}

/// Like [`resolver`] with the registry settings in `config`, without a tool policy.
pub fn resolver_with(
    tool_cache: &Path,
    locked: bool,
    config: &RegistryConfig,
) -> Result<PackageResolver, String> {
    let auth = registry_auth(config)?;
    Ok(PackageResolver::new(tool_cache, RegistryClient::new(auth)).with_locked(locked))
}

/// Like [`resolver_with`], with the tool policy of the spec `file` (see [`load_policy`]).
pub fn resolver_for(
    file: Option<&Path>,
    tool_cache: &Path,
    locked: bool,
    config: &RegistryConfig,
) -> Result<PackageResolver, String> {
    let resolver = resolver_with(tool_cache, locked, config)?;
    Ok(match load_policy(file, config)? {
        Some(policy) => resolver.with_policy(policy),
        None => resolver,
    })
}

/// Resolver for the tool packages of a spec tree (ADR-0115): policy of the root spec `file`,
/// `GIT_TOKEN` from `config`, git packages compiled with `builder`.
pub fn package_resolver(
    file: &Path,
    tool_cache: &Path,
    locked: bool,
    config: &RegistryConfig,
    builder: &ToolBuilder,
) -> Result<PackageResolver, String> {
    Ok(resolver_for(Some(file), tool_cache, locked, config)?
        .with_git(config.git_token.clone(), Arc::new(Builds(builder.clone()))))
}

/// Resolver for git packages outside a spec (`tools pull|inspect`): like [`resolver_for`] with the
/// tool policy of the working directory, plus `GIT_TOKEN` from `config` and `builder` for the
/// modules.
pub fn git_resolver(
    tool_cache: &Path,
    config: &RegistryConfig,
    builder: &ToolBuilder,
) -> Result<PackageResolver, String> {
    Ok(resolver_for(None, tool_cache, false, config)?
        .with_git(config.git_token.clone(), Arc::new(Builds(builder.clone()))))
}

/// Resolver for the git skill sources of the spec `file` (ADR-0103): like [`resolver_for`] with
/// `GIT_TOKEN` from `config`; nothing is ever built.
pub fn skills_resolver(
    file: &Path,
    tool_cache: &Path,
    locked: bool,
    config: &RegistryConfig,
) -> Result<PackageResolver, String> {
    Ok(resolver_for(Some(file), tool_cache, locked, config)?
        .with_git(config.git_token.clone(), Arc::new(NoBuilds)))
}

/// Skills are text, nothing is compiled for them.
struct NoBuilds;

impl ModuleBuilder for NoBuilds {
    fn prepare(&self) -> Result<(), String> {
        Ok(())
    }

    fn build(&self, tool: &ToolSpec) -> Result<Vec<u8>, String> {
        Err(format!(
            "cannot build tool '{}' from a skills source",
            tool.name
        ))
    }
}

/// Reads an allowed-signers file (the format of git's `gpg.ssh.allowedSignersFile`).
pub fn load_allowed_signers(path: &Path) -> Result<AllowedSigners, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("allowed signers {}: {error}", path.display()))?;
    AllowedSigners::parse(&text)
        .map_err(|error| format!("allowed signers {}: {error}", path.display()))
}

/// The isolated tool build (ADR-0030) as the module builder of tool packages built from sources.
struct Builds(ToolBuilder);

impl ModuleBuilder for Builds {
    fn prepare(&self) -> Result<(), String> {
        self.0.prepare().map_err(|error| error.to_string())
    }

    fn build(&self, tool: &ToolSpec) -> Result<Vec<u8>, String> {
        self.0
            .build(tool)
            .map(|artifact| artifact.bytes)
            .map_err(|error| error.to_string())
    }
}

/// The credentials of `config`: `GRAUWOLF_REGISTRY_AUTH` first, then the Docker configuration with
/// the credential helpers found in `PATH`.
pub fn registry_auth(config: &RegistryConfig) -> Result<RegistryAuth, String> {
    let auth = match &config.auth {
        Some(text) => RegistryAuth::from_json(text)?,
        None => RegistryAuth::none(),
    };
    Ok(match &config.docker_config {
        Some(path) => auth.with_docker_config(path, config.helper_dirs.clone()),
        None => auth,
    })
}

/// Writes a new signing key pair: `<name>.key` (PKCS#8 PEM, **unencrypted**, readable by the
/// owner only) and `<name>.pub` (SPKI PEM, what the policy lists under `oci_keys`). Existing
/// files are never overwritten. Returns the two paths and the fingerprint of the public key.
pub fn generate_key(name: &Path) -> Result<(PathBuf, PathBuf, String), String> {
    let with_extension = |extension: &str| {
        let mut file = name.as_os_str().to_owned();
        file.push(".");
        file.push(extension);
        PathBuf::from(file)
    };
    let (private, public) = (with_extension("key"), with_extension("pub"));
    for path in [&private, &public] {
        if path.exists() {
            return Err(format!(
                "{}: exists already, not overwritten",
                path.display()
            ));
        }
    }
    for parent in [private.parent(), public.parent()].into_iter().flatten() {
        create_private_dir(parent)?;
    }
    let key = SigningKey::generate();
    write_new(&private, key.to_pem().as_bytes(), true)?;
    write_new(&public, key.public_key().to_pem().as_bytes(), false)?;
    Ok((private, public, key.public_key().fingerprint()))
}

/// Creates a missing folder (and its missing parents); new folders are private (0700).
fn create_private_dir(dir: &Path) -> Result<(), String> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))
}

fn write_new(path: &Path, bytes: &[u8], private: bool) -> Result<(), String> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    options
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Reads a private signing key written by [`generate_key`] (unencrypted PKCS#8 PEM).
pub fn load_signing_key(path: &Path) -> Result<SigningKey, String> {
    let pem =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    SigningKey::from_pem(&pem).map_err(|error| format!("{}: {error}", path.display()))
}

/// Reads a public key (SPKI PEM, e.g. `cosign.pub`).
pub fn load_public_key(path: &Path) -> Result<PublicKey, String> {
    let pem =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    PublicKey::from_pem(&pem).map_err(|error| format!("{}: {error}", path.display()))
}

/// The package to push: an OCI image layout directory (as `tools pack` writes it) is read and
/// verified, any other directory is a package folder and is packed first.
pub fn load_packed(path: &Path, builder: &ToolBuilder) -> Result<PackedPackage, String> {
    if path.join("oci-layout").is_file() {
        PackedPackage::read_layout(path).map_err(|error| format!("{}: {error}", path.display()))
    } else {
        pack_folder(path, builder)
    }
}

/// Pushes `package` to `reference` (`registry/repository[:tag]`) with the credentials of the
/// environment; returns the reference with the manifest digest, `<reference>@sha256:…`.
pub async fn push_package(reference: &str, package: &PackedPackage) -> Result<String, String> {
    push_package_with(reference, package, &RegistryConfig::from_env(&ProcessEnv)).await
}

/// Like [`push_package`] with the registry settings in `config`.
pub async fn push_package_with(
    reference: &str,
    package: &PackedPackage,
    config: &RegistryConfig,
) -> Result<String, String> {
    push_package_signed_with(reference, package, config, None).await
}

/// Like [`push_package_with`]; with a `key` the pushed package is then signed (cosign-compatible,
/// next to the package under the tag `sha256-<hex>.sig`; signatures already there are kept).
pub async fn push_package_signed_with(
    reference: &str,
    package: &PackedPackage,
    config: &RegistryConfig,
    key: Option<&SigningKey>,
) -> Result<String, String> {
    let parsed = grauwolf_spec::OciReference::parse(reference)
        .map_err(|reason| format!("{reference}: {reason}"))?;
    let client = RegistryClient::new(registry_auth(config)?);
    let digest = client
        .push(&parsed, package)
        .await
        .map_err(|error| format!("{reference}: {error}"))?;
    if let Some(key) = key {
        client
            .push_signature(&parsed, &digest, key)
            .await
            .map_err(|error| format!("{reference}: signing failed: {error}"))?;
    }
    Ok(match parsed.digest {
        Some(_) => reference.to_owned(),
        None => format!("{reference}@{digest}"),
    })
}

/// Packs the package folder `dir` (`grauwolf-tools.json` and `src/<tool>.rs` with the code body of
/// every tool): builds each tool with `builder` and returns the manifest, modules and source
/// layer, annotated with the package metadata (ADR-0124: the manifest's fields, the build date –
/// `SOURCE_DATE_EPOCH` if set, else now –, `source`/`revision` from the git checkout the folder
/// lies in, the grauwolf and rustc versions). The same folder and build date give the same
/// package.
pub fn pack_folder(dir: &Path, builder: &ToolBuilder) -> Result<PackedPackage, String> {
    let created = BuildInfo::created_from(std::env::var("SOURCE_DATE_EPOCH").ok().as_deref())?;
    pack_folder_built_at(dir, builder, created)
}

/// Like [`pack_folder`] with the build date `created` (seconds since 1970) instead of
/// `SOURCE_DATE_EPOCH` or the clock.
pub fn pack_folder_built_at(
    dir: &Path,
    builder: &ToolBuilder,
    created: u64,
) -> Result<PackedPackage, String> {
    let file = |relative: &str| dir.join(relative);
    let mut read = |relative: &str| {
        std::fs::read(file(relative))
            .map_err(|error| format!("{}: {error}", file(relative).display()))
    };
    let manifest = read(MANIFEST_FILE)?;
    let packed = pack_sources(
        manifest,
        &mut read,
        &|relative| file(relative).display().to_string(),
        &Builds(builder.clone()),
    )?;
    let info = BuildInfo {
        created,
        rustc: builder.rustc_version(),
        checkout: grauwolf_git::checkout_of(dir),
    };
    let package = packed.package().map_err(|error| error.to_string())?;
    Ok(packed.with_annotations(crate::package_annotations(&package.manifest, &info)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grauwolf_config::MapEnv;

    fn temp(name: &str) -> grauwolf_test::TestDir {
        grauwolf_test::TestDir::new(&format!("keys-{name}"))
    }

    #[test]
    fn keygen_writes_a_private_and_a_public_key_that_load_again() {
        let dir = temp("keygen");
        let (private, public, fingerprint) = generate_key(&dir.join("acme")).unwrap();
        assert_eq!(private, dir.join("acme.key"));
        assert_eq!(public, dir.join("acme.pub"));
        let key = load_signing_key(&private).unwrap();
        assert_eq!(load_public_key(&public).unwrap(), key.public_key());
        assert_eq!(fingerprint, key.public_key().fingerprint());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&private).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn keygen_creates_a_missing_folder_that_is_private() {
        let dir = temp("keygen-folder");
        let (private, public, _) = generate_key(&dir.join("keys/acme")).unwrap();
        assert!(private.is_file() && public.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("keys"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn keygen_never_overwrites_existing_keys() {
        let dir = temp("overwrite");
        generate_key(&dir.join("acme")).unwrap();
        let before = std::fs::read(dir.join("acme.key")).unwrap();
        let error = generate_key(&dir.join("acme")).unwrap_err();
        assert!(error.contains("exists already"), "{error}");
        assert_eq!(std::fs::read(dir.join("acme.key")).unwrap(), before);
    }

    #[test]
    fn a_file_that_is_no_key_is_an_error_naming_the_file() {
        let dir = temp("nokey");
        std::fs::write(dir.join("x.key"), "nope").unwrap();
        assert!(
            load_signing_key(&dir.join("x.key"))
                .unwrap_err()
                .contains("x.key")
        );
        assert!(
            load_public_key(&dir.join("missing.pub"))
                .unwrap_err()
                .contains("missing.pub")
        );
    }

    #[test]
    fn an_invalid_registry_auth_is_an_error_that_names_the_variable() {
        let config = RegistryConfig::from_env(&MapEnv::new([("GRAUWOLF_REGISTRY_AUTH", "[1]")]));
        let error = registry_auth(&config).err().unwrap();
        assert!(error.contains("GRAUWOLF_REGISTRY_AUTH"), "{error}");
    }

    #[test]
    fn without_variables_the_credentials_are_empty() {
        assert!(registry_auth(&RegistryConfig::from_env(&MapEnv::default())).is_ok());
    }
}
