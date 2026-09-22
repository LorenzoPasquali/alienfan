use std::fs;
use std::path::{Path, PathBuf};

use alienfan_core::SysfsRoot;
use tempfile::TempDir;

pub fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/fake-sysfs")
}

/// A writable copy of the fake sysfs tree. Keep the `TempDir` alive.
pub fn fake_sysfs() -> (TempDir, SysfsRoot) {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture_path(), dir.path());
    let root = SysfsRoot::new(dir.path());
    (dir, root)
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

pub fn read(root: &SysfsRoot, rel: &str) -> String {
    fs::read_to_string(root.path().join(rel))
        .unwrap()
        .trim()
        .to_owned()
}

pub fn write(root: &SysfsRoot, rel: &str, value: &str) {
    fs::write(root.path().join(rel), format!("{value}\n")).unwrap();
}

/// Every file of the tree with its content, to prove nothing else changed.
pub fn snapshot(root: &SysfsRoot) -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push((path.clone(), fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root.path(), &mut out);
    out.sort();
    out
}
