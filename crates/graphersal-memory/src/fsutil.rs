//! Small file helpers: atomic writes and a content fingerprint.

use std::path::Path;

/// A 64-bit FNV-1a fingerprint of `bytes` as 16 hex digits. It detects changes (a hand edit
/// in the managed block, a configuration file rewritten since `install`); it is not a
/// cryptographic hash and protects nothing.
pub fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Writes `bytes` to `path` through a temporary file in the same directory and a rename, so a
/// reader never sees half a file. Keeps the permissions of an existing file on Unix.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.graphersal-memory.tmp"));
    std::fs::write(&tmp, bytes).map_err(|err| format!("cannot write {}: {err}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot replace {}: {err}", path.display())
    })
}

/// Reads `path`: `Ok(None)` when it does not exist.
pub fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("cannot read {}: {err}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_differ_and_are_stable() {
        assert_eq!(fingerprint(b""), "cbf29ce484222325");
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
        assert_eq!(fingerprint(b"abc").len(), 16);
    }

    #[test]
    fn atomic_writes_replace_and_leave_no_temporary_file() {
        let dir = crate::mcp_tests::TempDir::new("atomic");
        let file = dir.path().join("x.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(read_optional(&dir.path().join("none")).unwrap(), None);
    }
}
