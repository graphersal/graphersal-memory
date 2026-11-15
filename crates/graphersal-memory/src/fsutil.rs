//! Small file helpers: atomic writes and a content fingerprint.

use std::path::{Path, PathBuf};

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

/// The file a write to `path` changes: `path` itself, or the final target of a symbolic link
/// (a chain followed; a relative link resolved against its own directory; a dangling one gives
/// its target, which the write creates). A configuration file kept in a dotfiles repository and
/// linked into place stays a link: the write goes into the repository's file.
pub fn write_target(path: &Path) -> Result<PathBuf, String> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = std::fs::read_link(&current)
                    .map_err(|err| format!("cannot read the link {}: {err}", current.display()))?;
                current = match current.parent() {
                    Some(dir) if link.is_relative() => dir.join(link),
                    _ => link,
                };
            }
            _ => return Ok(current),
        }
    }
    Err(format!(
        "{} is a chain of symbolic links that does not end (a loop?); not changed",
        path.display()
    ))
}

/// A temporary file name beside `name` that no other process or call uses at the same time
/// (process id, a counter and the time).
fn temp_name(name: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        ".{name}.{}-{n}-{nanos}.graphersal-memory.tmp",
        std::process::id()
    )
}

/// Why `path` cannot be changed by a write, checked before anything is written: an existing
/// file (the link's target, [`write_target`]) that is read-only. `None` when it can be.
pub fn not_writable(path: &Path) -> Option<String> {
    let target = write_target(path).ok()?;
    let meta = std::fs::metadata(&target).ok()?;
    if meta.permissions().readonly() {
        return Some(format!(
            "{} is read-only, so graphersal-memory does not change it{}. Make it writable \
             (chmod u+w) and run the command again, or leave this agent out with --harness",
            path.display(),
            if target != path {
                format!(" (a link to {})", target.display())
            } else {
                String::new()
            }
        ));
    }
    None
}

/// Writes `bytes` to `path` through a temporary file in the same directory and a rename, so a
/// reader never sees half a file. Keeps the permissions of an existing file on Unix. A symbolic
/// link is written through: its final target is replaced and the link stays ([`write_target`]).
/// A read-only file is refused ([`not_writable`]).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic_checked(path, bytes, &mut |_| true)
}

/// [`write_atomic`] that calls `still` with the target right before the rename: when it
/// answers false (the file changed since the caller read it), nothing is replaced and the
/// error says so (`CHANGED_MEANWHILE` starts it).
pub fn write_atomic_checked(
    path: &Path,
    bytes: &[u8],
    still: &mut dyn FnMut(&Path) -> bool,
) -> Result<(), String> {
    if let Some(reason) = not_writable(path) {
        return Err(reason);
    }
    let target = write_target(path)?;
    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
    let name = target
        .file_name()
        .ok_or_else(|| format!("{} has no file name", target.display()))?
        .to_string_lossy();
    let tmp = dir.join(temp_name(&name));
    std::fs::write(&tmp, bytes).map_err(|err| format!("cannot write {}: {err}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if !still(&target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{CHANGED_MEANWHILE}: {}", path.display()));
    }
    std::fs::rename(&tmp, &target).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot replace {}: {err}", path.display())
    })
}

/// The start of the error of [`write_atomic_checked`] when the file changed meanwhile.
pub const CHANGED_MEANWHILE: &str = "changed by another program while graphersal-memory wrote it";

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
        let dir = crate::runtime::mcp_tests::TempDir::new("atomic");
        let file = dir.path().join("x.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(read_optional(&dir.path().join("none")).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_write_through_a_link_keeps_the_link_and_changes_its_target() {
        let dir = crate::runtime::mcp_tests::TempDir::new("atomic-link");
        let repo = dir.path().join("dotfiles");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("s.json"), "{}").unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        // A relative link to a relative link.
        std::os::unix::fs::symlink("../dotfiles/s.json", home.join("one.json")).unwrap();
        std::os::unix::fs::symlink("one.json", home.join("settings.json")).unwrap();
        write_atomic(&home.join("settings.json"), b"{\"a\": 1}").unwrap();
        for link in ["settings.json", "one.json"] {
            let meta = std::fs::symlink_metadata(home.join(link)).unwrap();
            assert!(meta.file_type().is_symlink(), "{link} stays a link");
        }
        assert_eq!(std::fs::read(repo.join("s.json")).unwrap(), b"{\"a\": 1}");
        assert_eq!(
            std::fs::read_dir(&home).unwrap().count(),
            2,
            "no temporary file left"
        );
        // A loop is refused.
        std::os::unix::fs::symlink("b", home.join("a")).unwrap();
        std::os::unix::fs::symlink("a", home.join("b")).unwrap();
        assert!(
            write_atomic(&home.join("a"), b"x")
                .unwrap_err()
                .contains("loop")
        );
    }

    #[test]
    fn a_read_only_file_is_refused_and_unchanged() {
        let dir = crate::runtime::mcp_tests::TempDir::new("atomic-ro");
        let file = dir.path().join("config.toml");
        std::fs::write(&file, "x = 1\n").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms.clone()).unwrap();
        let err = write_atomic(&file, b"y = 2\n").unwrap_err();
        assert!(
            err.contains("read-only") && err.contains("config.toml"),
            "{err}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"x = 1\n");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&file, perms).unwrap();
    }

    #[test]
    fn a_change_meanwhile_stops_the_replace() {
        let dir = crate::runtime::mcp_tests::TempDir::new("atomic-race");
        let file = dir.path().join("x.json");
        std::fs::write(&file, "one").unwrap();
        let err = write_atomic_checked(&file, b"two", &mut |_| false).unwrap_err();
        assert!(err.starts_with(CHANGED_MEANWHILE), "{err}");
        assert_eq!(std::fs::read(&file).unwrap(), b"one");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
