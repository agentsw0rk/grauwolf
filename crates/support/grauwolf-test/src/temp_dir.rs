//! A temporary directory per test that removes itself.

use std::ffi::OsStr;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty directory in the system temp dir, removed recursively when dropped.
///
/// The name is unique per test: `grauwolf-<label>-<process id>-<counter>`, so parallel tests
/// and parallel test processes never share a directory. Keep the guard alive for the whole
/// test (also for background tasks that use the path); removal errors are ignored.
#[derive(Debug)]
pub struct TestDir(PathBuf);

impl TestDir {
    /// Creates the directory; `label` only makes leftovers recognisable.
    pub fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("grauwolf-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the test directory");
        Self(path)
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path below the directory.
    pub fn join(&self, child: impl AsRef<Path>) -> PathBuf {
        self.0.join(child)
    }
}

impl Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<OsStr> for TestDir {
    fn as_ref(&self) -> &OsStr {
        self.0.as_os_str()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_removed_with_its_content_when_dropped() {
        let dir = TestDir::new("drop");
        let file = dir.join("a/b.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "x").unwrap();
        let path = dir.path().to_owned();
        assert!(file.exists());

        drop(dir);

        assert!(!path.exists());
    }

    #[test]
    fn two_instances_with_the_same_label_differ() {
        let a = TestDir::new("same");
        let b = TestDir::new("same");

        assert_ne!(a.path(), b.path());
        assert!(a.exists() && b.exists());
    }
}
