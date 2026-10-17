//! Fixes for known game bugs. The launcher serves fixed copies from `patched/`; the asset files stay untouched.
use crate::{config::Settings, storage};
use anyhow::{Result, bail};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::UNIX_EPOCH,
};

/// GrowHeap's locals and its `if (heapCount <= 31)` check; the byte at `HEAP_LIMIT_AT` is the 31.
const HEAP_CHECK: &[u8] =
    b"\x03\x0f\x7e\x03\x7f\x01\x7b\x02\x40\x20\x00\x28\x02\xfc\x03\x22\x10\x41\x1f\x4d";
const HEAP_LIMIT_AT: usize = 18;

/// Caps the streaming resource heaps (256 MB each). Without a cap they keep growing and the game
/// stops with "Invalid fixup".
pub fn limit_heaps(wasm: &mut [u8], heaps: u8) -> Result<()> {
    if !(1..=31).contains(&heaps) {
        bail!("Memory limit must be 1–31 heaps");
    }
    let found: Vec<usize> = wasm
        .windows(HEAP_CHECK.len())
        .enumerate()
        .filter(|(_, w)| *w == HEAP_CHECK)
        .map(|(at, _)| at)
        .take(2)
        .collect();
    let [at] = found[..] else {
        bail!("game file differs from the known version");
    };
    // Values up to 63 stay a one-byte signed LEB128, so the file size does not change.
    wasm[at + HEAP_LIMIT_AT] = heaps - 1;
    Ok(())
}

/// Last result per game file: (key, error). Kept for the launcher window and so a file that cannot be fixed is not rescanned.
type Results = BTreeMap<String, (String, Option<String>)>;
fn results() -> &'static Mutex<Results> {
    static RESULTS: OnceLock<Mutex<Results>> = OnceLock::new();
    RESULTS.get_or_init(Default::default)
}
fn describe(relative: &str, error: &Option<String>) -> String {
    let name = relative.rsplit('/').next().unwrap_or_default();
    match error {
        None => format!("Memory limit applied to {name}"),
        Some(e) => format!("Memory limit not applied to {name}: {e}"),
    }
}
pub fn status() -> Vec<String> {
    let results = results().lock().unwrap_or_else(|e| e.into_inner());
    results
        .iter()
        .map(|(relative, (_, error))| describe(relative, error))
        .collect()
}
pub fn forget() {
    results().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Returns the file to serve for `relative`: a fixed copy when a fix applies, else `source`.
/// `source` is the asset or its override; the copy is rebuilt when it or the fix settings change.
pub fn serve_path(
    folder: &Path,
    relative: &str,
    source: PathBuf,
    settings: &Settings,
) -> Result<PathBuf> {
    let name = relative.rsplit('/').next().unwrap_or_default();
    if !(name.starts_with("game") && name.ends_with(".wasm")) {
        return Ok(source);
    }
    // ponytail: one lock for all files; builds are rare and take under a second.
    let mut results = results().lock().unwrap_or_else(|e| e.into_inner());
    let patched = folder.join("patched");
    if !settings.memory_limit {
        results.remove(relative);
        if patched.is_dir() {
            let target = storage::safe_target(&patched, relative)?;
            let _ = std::fs::remove_file(stamp_path(&target));
            let _ = std::fs::remove_file(target);
        }
        return Ok(source);
    }
    let meta = source.metadata()?;
    let key = format!(
        "{}\n{}\n{}\nmemory_limit_heaps={}\n",
        source.display(),
        meta.len(),
        meta.modified()?.duration_since(UNIX_EPOCH)?.as_nanos(),
        settings.memory_limit_heaps
    );
    std::fs::create_dir_all(&patched)?;
    let target = storage::safe_target(&patched, relative)?;
    let stamp = stamp_path(&target);
    let error = if target.is_file() && std::fs::read_to_string(&stamp).is_ok_and(|s| s == key) {
        None
    } else if let Some((_, Some(error))) = results.get(relative).filter(|(k, _)| *k == key) {
        Some(error.clone())
    } else {
        build(&source, &target, &stamp, &key, settings.memory_limit_heaps)
            .err()
            .map(|e| format!("{e:#}"))
    };
    if results.get(relative).is_none_or(|(_, old)| *old != error) {
        crate::diagnostics::record("Game fixes", describe(relative, &error));
    }
    let applied = error.is_none();
    results.insert(relative.into(), (key, error));
    Ok(if applied { target } else { source })
}
fn stamp_path(target: &Path) -> PathBuf {
    let mut path = target.as_os_str().to_owned();
    path.push(".stamp");
    path.into()
}
fn build(source: &Path, target: &Path, stamp: &Path, key: &str, heaps: u8) -> Result<()> {
    let result = (|| {
        let mut wasm = std::fs::read(source)?;
        limit_heaps(&mut wasm, heaps)?;
        storage::atomic_write(target, &wasm, false)?;
        storage::atomic_write(stamp, key.as_bytes(), false)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(stamp);
        let _ = std::fs::remove_file(target);
    }
    result
}
