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
    let mut staged = copy_to_temp(new, current, Some(meta.permissions()))?;
    let mut kept = copy_to_temp(current, current, None)?;
    kept.rename_to(&prev)?;
    staged.rename_to(current)?;
    sync_dir(current);
    Ok(())
}

/// Exchanges `current` and its [`prev_path`]: afterwards `current` is the former `.prev`
/// and `.prev` the former `current`. Both are copied to temp files in the folder first and
/// then renamed into place, so at every moment both paths hold a complete binary.
///
/// No binary is lost on failure: when the second rename (into `.prev`) fails, the former
/// current binary is renamed back to `current` (the state before the call); when that
/// fails too, its temp copy is kept and the error names it.
pub fn swap_with_prev(current: &Path) -> Result<()> {
    swap_with(current, |from, to| fs::rename(from, to))
}

/// [`swap_with_prev`] with the rename used for moving the temp files into place (a fault
/// seam for the tests).
fn swap_with(current: &Path, rename: impl Fn(&Path, &Path) -> std::io::Result<()>) -> Result<()> {
    let prev = prev_path(current);
    if !fs::metadata(&prev).map(|m| m.is_file()).unwrap_or(false) {
        bail!("there is no previous binary at {}", prev.display());
    }
    let mut old_current = copy_to_temp(current, current, None)?;
    let mut old_prev = copy_to_temp(&prev, current, None)?;
    old_prev.rename_with(current, &rename)?;
    if let Err(e) = old_current.rename_with(&prev, &rename) {
        // `current` holds the former `.prev` and `.prev` is unchanged: put the former
        // current binary back.
        if let Err(restore) = old_current.rename_with(current, &rename) {
            let kept = old_current.keep();
            bail!(
                "{e:#}; putting the binary back failed too ({restore:#}): {} now holds the \
                 previous binary, and the binary that was current is kept at {}",
                current.display(),
                kept.display()
            );
        }
        sync_dir(current);
        bail!("{e:#}; nothing was changed");
    }
    sync_dir(current);
    Ok(())
}

/// A temp file beside a target, removed on drop unless renamed into place.
struct Temp {
    path: PathBuf,
    done: bool,
}

impl Temp {
    fn rename_to(&mut self, target: &Path) -> Result<()> {
        self.rename_with(target, |from, to| fs::rename(from, to))
    }

    fn rename_with(
        &mut self,
        target: &Path,
        rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<()> {
        rename(&self.path, target).with_context(|| {
            format!(
                "cannot rename {} to {}",
                self.path.display(),
                target.display()
            )
        })?;
        self.done = true;
        Ok(())
    }

    /// Keeps the file (no removal on drop) and returns its path.
    fn keep(mut self) -> PathBuf {
        self.done = true;
        std::mem::take(&mut self.path)
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

/// Leftovers older than this are swept by [`sweep_stale`].
const STALE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// The prefix of `self-update`'s work folder beside the binary.
pub const WORK_PREFIX: &str = ".ferrite-update-";

/// Removes what a killed `self-update` or `rollback` left beside `current` (SIGKILL or a
/// power loss skips the cleanup): `self-update`'s work folders (`.ferrite-update-*`, up to
/// the size of a release) and the temp copies of the binary (`.<name>.tmp-*`), when older
/// than a day, so a run in progress is never touched. Best effort: what cannot be listed
/// or removed is logged and left.
pub fn sweep_stale(current: &Path) {
    let dir = current
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let base = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ferrite".to_owned());
    let temp_prefix = format!(".{base}.tmp-");
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let work = name.starts_with(WORK_PREFIX) && meta.is_dir();
        let temp = name.starts_with(&temp_prefix) && meta.is_file();
        let old = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if !(old && (work || temp)) {
            continue;
        }
        let path = entry.path();
        let removed = if work {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match removed {
            Ok(()) => tracing::info!(path = %path.display(), "removed a stale update leftover"),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot remove a stale update leftover")
            }
        }
    }
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

    /// Work folders and temp copies older than a day go; younger ones, other names, a
    /// file with the work folder's prefix and the binaries stay.
    #[test]
    fn sweep_stale_removes_old_leftovers_only() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("ferrite");
        write(&current, "bin", 0o755);
        write(&tmp.path().join("ferrite.prev"), "prev", 0o755);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 3600);
        let age = |p: &Path| File::open(p).unwrap().set_modified(old).unwrap();
        let stale_work = tmp.path().join(".ferrite-update-abc");
        fs::create_dir(&stale_work).unwrap();
        write(&stale_work.join("ferrite"), "half", 0o755);
        age(&stale_work);
        let young_work = tmp.path().join(".ferrite-update-def");
        fs::create_dir(&young_work).unwrap();
        for (name, stale) in [
            (".ferrite.tmp-1-0", true),
            (".ferrite.tmp-2-0", false),
            (".ferrite-update-file", true),
            (".other.tmp-1-0", true),
        ] {
            let p = tmp.path().join(name);
            write(&p, "x", 0o644);
            if stale {
                age(&p);
            }
        }
        sweep_stale(&current);
        assert_eq!(
            entries(tmp.path()),
            vec![
                ".ferrite-update-def",
                ".ferrite-update-file",
                ".ferrite.tmp-2-0",
                ".other.tmp-1-0",
                "ferrite",
                "ferrite.prev"
            ]
        );
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

    /// A rename that fails from call `fail_from` on (1-based).
    fn failing_rename(fail_from: usize) -> impl Fn(&Path, &Path) -> std::io::Result<()> {
        let calls = std::cell::Cell::new(0);
        move |from, to| {
            calls.set(calls.get() + 1);
            if calls.get() >= fail_from {
                Err(std::io::Error::other("injected rename failure"))
            } else {
                fs::rename(from, to)
            }
        }
    }

    #[test]
    fn swap_failing_second_rename_restores_current() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("ferrite");
        write(&current, "two", 0o755);
        write(&prev_path(&current), "one", 0o755);
        // Call 1 (prev -> current) works, call 2 (-> .prev) fails, call 3 (restore) works.
        let calls = std::cell::Cell::new(0);
        let err = swap_with(&current, |from, to| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                Err(std::io::Error::other("injected rename failure"))
            } else {
                fs::rename(from, to)
            }
        })
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("nothing was changed"),
            "{err:#}"
        );
        assert_eq!(fs::read_to_string(&current).unwrap(), "two");
        assert_eq!(fs::read_to_string(prev_path(&current)).unwrap(), "one");
        assert_eq!(entries(tmp.path()), vec!["ferrite", "ferrite.prev"]);
    }

    #[test]
    fn swap_failing_restore_keeps_the_former_current() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("ferrite");
        write(&current, "two", 0o755);
        write(&prev_path(&current), "one", 0o755);
        let err = swap_with(&current, failing_rename(2)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("is kept at"), "{msg}");
        assert_eq!(fs::read_to_string(&current).unwrap(), "one");
        assert_eq!(fs::read_to_string(prev_path(&current)).unwrap(), "one");
        // The former current binary survives in the temp file the error names.
        let kept: Vec<String> = entries(tmp.path())
            .into_iter()
            .filter(|n| n.starts_with(".ferrite.tmp-"))
            .collect();
        assert_eq!(kept.len(), 1, "{:?}", entries(tmp.path()));
        assert!(msg.contains(&kept[0]), "{msg}");
        assert_eq!(
            fs::read_to_string(tmp.path().join(&kept[0])).unwrap(),
            "two"
        );
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
