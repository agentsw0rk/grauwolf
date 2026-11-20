//! Shared libgit2 adapter (ADR-0107 stage 3, ADR-0108): process-wide libgit2 settings and the own
//! smart-HTTP transport with token authentication, and fetching one commit of a repository into
//! memory. Used by the git tools and the tool packages.

mod checkout;
mod fetch;
mod project;
mod sshsig;
#[cfg(feature = "test-git-server")]
pub mod testing;
mod transport;

pub use checkout::{BuildSource, Checkout, build_source, checkout_of};
pub use fetch::{FetchRequest, Fetched, GitError, MAX_FETCHED_BYTES, Wanted, fetch_files};
pub use project::{Credentials, ProjectClone, commit_folder, ensure_project_clone, push};

pub use sshsig::{
    AllowedSigners, GIT_NAMESPACE, SignatureError, Signed, SignedObjects, Verified,
    commit_signature, tag_signature, verify_objects,
};
pub use transport::{Settings, origin_of, register, with_settings};

use std::sync::Once;

use git2::ConfigLevel;

/// Process-wide libgit2 settings, made once: the user and system configuration are not read (a
/// `url.*.insteadOf` there could redirect the token) and the directory owner is not checked (the
/// directory belongs to the agent). No socket timeouts: libgit2's own network code never runs
/// (HTTP goes through the transport, which has its timeouts; ssh and `git://` are not built).
pub fn init_libgit2() {
    static ONCE: Once = Once::new();
    // SAFETY: these options only change global settings of libgit2; no other thread uses them
    // concurrently in a way that depends on their value.
    unsafe {
        ONCE.call_once(|| {
            for level in [
                ConfigLevel::ProgramData,
                ConfigLevel::System,
                ConfigLevel::XDG,
                ConfigLevel::Global,
            ] {
                let _ = git2::opts::set_search_path(level, "");
            }
            let _ = git2::opts::set_verify_owner_validation(false);
        });
    }
}
