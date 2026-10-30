//! Replacing the binary in place (09 §1, M4-2 P31): the previous binary is kept beside the
//! current one as `<name>.prev` (`ferrite.prev`), and every change of the current path is a
//! single `rename` within its own folder, so the path never goes missing and never holds a
//! partly written file.
//!
//! `self-update` uses [`install_new`]; `rollback` uses [`swap_with_prev`].

use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};

/// Where the binary at `current` keeps its predecessor: `<name>.prev` in the same folder.
pub fn prev_path(current: &Path) -> PathBuf {
    let mut name = current
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("ferrite"));
    name.push(".prev");
    current.with_file_name(name)
}

/// Installs the binary at `new` as `current`, keeping the binary at `current` as
/// [`prev_path`] (an older `.prev` is replaced). `new` may live anywhere: it is copied
/// into `current`'s folder first. The installed binary gets `current`'s permission bits.
///
/// Order: copy new -> temp (current's mode) and current -> temp; rename the second to
/// `.prev`, then the first to current. A failure before the last rename leaves `current`
/// as it was (and at worst a `.prev` that is a copy of it); temp files are removed on
/// failure.
pub fn install_new(current: &Path, new: &Path) -> Result<()> {
    let meta = fs::metadata(current)
        .with_context(|| format!("cannot read the current binary {}", current.display()))?;
    if !meta.is_file() {
        bail!("{} is not a file", current.display());
    }
    let prev = prev_path(current);
    let staged = copy_to_temp(new, current, Some(meta.permissions()))?;
    let kept = copy_to_temp(current, current, None)?;
    kept.rename_to(&prev)?;
    staged.rename_to(current)?;
    sync_dir(current);
    Ok(())
}

/// Exchanges `current` and its [`prev_path`]: afterwards `current` is the former `.prev`
/// and `.prev` the former `current`. Both are copied to temp files in the folder first and
/// then renamed into place, so at every moment both paths hold a complete binary.
pub fn swap_with_prev(current: &Path) -> Result<()> {
    let prev = prev_path(current);
    if !fs::metadata(&prev).map(|m| m.is_file()).unwrap_or(false) {
        bail!("there is no previous binary at {}", prev.display());
    }
    let old_current = copy_to_temp(current, current, None)?;
    let old_prev = copy_to_temp(&prev, current, None)?;
    old_prev.rename_to(current)?;
    old_current.rename_to(&prev)?;
    sync_dir(current);
    Ok(())
}

/// A temp file beside a target, removed on drop unless renamed into place.
struct Temp {
    path: PathBuf,
    done: bool,
}

impl Temp {
    fn rename_to(mut self, target: &Path) -> Result<()> {
        fs::rename(&self.path, target).with_context(|| {
            format!(
                "cannot rename {} to {}",
                self.path.display(),
                target.display()
            )
        })?;
        self.done = true;
        Ok(())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        if !self.done {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Copies `src` to a fresh hidden temp file in `beside`'s folder, flushed to disk. The
/// copy keeps `src`'s permission bits, or gets `mode` when given.
fn copy_to_temp(src: &Path, beside: &Path, mode: Option<fs::Permissions>) -> Result<Temp> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = beside
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let base = beside
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ferrite".to_owned());
    let path = dir.join(format!(
        ".{base}.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // `fs::copy` creates (or truncates) the target and copies the permission bits; a
    // leftover file of the same name can only be ours from an earlier crash.
    let temp = Temp { path, done: false };
    fs::copy(src, &temp.path)
        .with_context(|| format!("cannot copy {} into {}", src.display(), dir.display()))?;
    if let Some(mode) = mode {
        fs::set_permissions(&temp.path, mode)
            .with_context(|| format!("cannot set the mode of {}", temp.path.display()))?;
    }
    File::open(&temp.path)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("cannot flush {}", temp.path.display()))?;
    Ok(temp)
}

/// Flushes the folder entry changes (best effort: not every platform can open a folder).
fn sync_dir(current: &Path) {
    if let Some(dir) = current.parent().filter(|p| !p.as_os_str().is_empty())
        && let Ok(f) = File::open(dir)
    {
        let _ = f.sync_all();
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn write(path: &Path, body: &str, mode: u32) {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn prev_path_is_beside() {
        assert_eq!(
            prev_path(Path::new("/usr/local/bin/ferrite")),
            PathBuf::from("/usr/local/bin/ferrite.prev")
        );
    }

    #[test]
    fn install_new_keeps_prev_and_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let current = bin.join("ferrite");
        write(&current, "old", 0o750);
        let new = tmp.path().join("new-ferrite");
        write(&new, "new", 0o600);
        install_new(&current, &new).unwrap();
        assert_eq!(fs::read_to_string(&current).unwrap(), "new");
        assert_eq!(fs::read_to_string(prev_path(&current)).unwrap(), "old");
        assert_eq!(mode(&current), 0o750);
        assert_eq!(mode(&prev_path(&current)), 0o750);
        assert_eq!(entries(&bin), vec!["ferrite", "ferrite.prev"]);
        // The source is left alone.
        assert_eq!(fs::read_to_string(&new).unwrap(), "new");
    }

    #[test]
    fn install_new_with_a_missing_source_changes_nothing_current() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("ferrite");
        write(&current, "old", 0o755);
        let err = install_new(&current, &tmp.path().join("absent")).unwrap_err();
        assert!(format!("{err:#}").contains("cannot copy"), "{err:#}");
        assert_eq!(fs::read_to_string(&current).unwrap(), "old");
        // Nothing was kept or left behind.
        assert_eq!(entries(tmp.path()), vec!["ferrite"]);
    }

    #[test]
    fn swap_with_prev_exchanges_both() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("ferrite");
        write(&current, "two", 0o755);
        write(&prev_path(&current), "one", 0o700);
        swap_with_prev(&current).unwrap();
        assert_eq!(fs::read_to_string(&current).unwrap(), "one");
        assert_eq!(fs::read_to_string(prev_path(&current)).unwrap(), "two");
        assert_eq!(mode(&current), 0o700);
        assert_eq!(mode(&prev_path(&current)), 0o755);
        assert_eq!(entries(tmp.path()), vec!["ferrite", "ferrite.prev"]);
        swap_with_prev(&current).unwrap();
        assert_eq!(fs::read_to_string(&current).unwrap(), "two");
    }

    #[test]
    fn swap_without_prev_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("ferrite");
        write(&current, "two", 0o755);
        let err = swap_with_prev(&current).unwrap_err();
        assert!(format!("{err:#}").contains("no previous binary"), "{err:#}");
        assert_eq!(entries(tmp.path()), vec!["ferrite"]);
    }
}
