//! The metadata of a package as OCI annotations on its manifest (ADR-0124): the standard
//! `org.opencontainers.image.*` keys, so other tools understand them, plus grauwolf's own. `pack`
//! writes them; `inspect` and the catalog read them off.

use std::collections::BTreeMap;

use grauwolf_git::Checkout;
use grauwolf_tools::PackageManifest;

/// Build date, RFC 3339.
pub const CREATED_ANNOTATION: &str = "org.opencontainers.image.created";
/// What the package is for.
pub const DESCRIPTION_ANNOTATION: &str = "org.opencontainers.image.description";
/// Where the sources live.
pub const SOURCE_ANNOTATION: &str = "org.opencontainers.image.source";
/// The commit the package was built from.
pub const REVISION_ANNOTATION: &str = "org.opencontainers.image.revision";
/// SPDX licence expression.
pub const LICENSES_ANNOTATION: &str = "org.opencontainers.image.licenses";
/// Authors, separated by `, `.
pub const AUTHORS_ANNOTATION: &str = "org.opencontainers.image.authors";
/// Version of the package.
pub const VERSION_ANNOTATION: &str = "org.opencontainers.image.version";
/// The tool ABI of the modules.
pub const ABI_ANNOTATION: &str = "vnd.grauwolf.tools.abi";
/// The grauwolf version that packed the package.
pub const BUILT_WITH_ANNOTATION: &str = "vnd.grauwolf.tools.built-with";
/// The Rust toolchain that built the modules.
pub const RUSTC_ANNOTATION: &str = "vnd.grauwolf.tools.rustc";

/// The version of grauwolf that writes packages.
pub const GRAUWOLF_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What is known about a build besides the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildInfo {
    /// Build date as seconds since the epoch (`SOURCE_DATE_EPOCH` for reproducible builds).
    pub created: u64,
    /// `rustc --version` of the toolchain, if known.
    pub rustc: Option<String>,
    /// The git checkout the package folder lies in, if any.
    pub checkout: Option<Checkout>,
}

impl BuildInfo {
    /// The build date from `source_date_epoch` (the value of `SOURCE_DATE_EPOCH`) or else the
    /// clock; a value that is not a number is an error.
    pub fn created_from(source_date_epoch: Option<&str>) -> Result<u64, String> {
        match source_date_epoch
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(value) => value.parse().map_err(|_| {
                format!("SOURCE_DATE_EPOCH must be seconds since 1970, not `{value}`")
            }),
            None => Ok(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs())),
        }
    }
}

/// The annotations of a package: from the manifest, and `source`/`revision` from the checkout
/// when the manifest does not name a source.
pub fn package_annotations(
    manifest: &PackageManifest,
    info: &BuildInfo,
) -> BTreeMap<String, String> {
    let mut annotations = BTreeMap::new();
    let mut put = |key: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            annotations.insert(key.to_owned(), value);
        }
    };
    put(crate::TITLE_ANNOTATION, Some(manifest.name.clone()));
    put(VERSION_ANNOTATION, Some(manifest.version.clone()));
    put(DESCRIPTION_ANNOTATION, manifest.description.clone());
    put(LICENSES_ANNOTATION, manifest.license.clone());
    put(AUTHORS_ANNOTATION, Some(manifest.authors.join(", ")));
    put(CREATED_ANNOTATION, Some(rfc3339(info.created)));
    let checkout = info.checkout.as_ref();
    put(
        SOURCE_ANNOTATION,
        manifest
            .source
            .clone()
            .or_else(|| checkout.and_then(|checkout| checkout.remote.clone())),
    );
    put(
        REVISION_ANNOTATION,
        checkout.map(|checkout| checkout.commit.clone()),
    );
    put(ABI_ANNOTATION, Some(manifest.abi.to_string()));
    put(BUILT_WITH_ANNOTATION, Some(GRAUWOLF_VERSION.to_owned()));
    put(RUSTC_ANNOTATION, info.rustc.clone());
    annotations
}

/// `seconds` since the epoch as RFC 3339 in UTC, e.g. `2026-11-16T09:30:00Z`.
pub fn rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_utc_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_794_821_405), "2026-11-16T09:30:05Z");
    }

    #[test]
    fn source_date_epoch_sets_the_build_date() {
        assert_eq!(
            BuildInfo::created_from(Some("1700000000")),
            Ok(1_700_000_000)
        );
        assert!(BuildInfo::created_from(Some("yesterday")).is_err());
        assert!(BuildInfo::created_from(None).unwrap() > 1_700_000_000);
    }
}
