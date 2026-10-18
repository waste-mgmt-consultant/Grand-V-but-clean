//! Read-only check that every compressed part of the game archives still decompresses.
//! A torrent creates files at full size before downloading them, so the size check at start
//! cannot see a download that never reached the disk; the game then crashes with
//! "zstd error Data corruption detected" when it streams that area.
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{io, path::Path, time::UNIX_EPOCH};

const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
// Every compressed entry in the packaged archives starts 16 bytes after a 512-byte boundary
// (131,088 of 131,088 in the 2026-10 package). Checking only those offsets avoids false matches.
const SLOT: usize = 512;
const PREFIX: usize = 16;

#[derive(Debug, Default)]
pub struct Report {
    pub archives: usize,
    /// Archives with no compressed parts (mostly audio); nothing in them can be checked.
    pub unchecked: usize,
    pub frames: usize,
    pub damaged: Vec<String>,
}

pub const REPAIR_STEPS: [&str; 3] = [
    "In your torrent client, force a recheck of GrandV-assets (qBittorrent: right-click, Force recheck) and let it download to 100%.",
    "Run this check again.",
    "Reload the game page. The browser drops its own copy of the game data when a game file changes and downloads the repaired data. If the game still crashes in the same area, open More options, saves and troubleshooting, then Clear game cache; saves and settings are kept.",
];
pub const LIMITS: &str = "Covers the compressed parts of the game archives, where a damaged download crashes the game. Archives without them (mostly audio) and files outside archives are not covered; a torrent recheck covers every file.";

impl Report {
    pub fn summary(&self) -> String {
        if self.damaged.is_empty() {
            format!(
                "No damage found: {} compressed parts in {} archives open correctly ({} archives have none).",
                self.frames,
                self.archives - self.unchecked,
                self.unchecked
            )
        } else {
            format!(
                "{} of {} game archives are damaged.",
                self.damaged.len(),
                self.archives
            )
        }
    }
}

pub fn check(folder: &Path, progress: impl Fn(usize, usize)) -> Result<Report> {
    if !folder.join("grandv-assets.json").is_file() {
        bail!(
            "This folder has no grandv-assets.json. The check needs the GrandV-assets release package."
        );
    }
    let archives: Vec<String> = crate::config::package_files(folder)?
        .into_iter()
        .map(|(relative, _)| relative)
        .filter(|relative| relative.to_ascii_lowercase().ends_with(".rpf"))
        .collect();
    let mut report = Report {
        archives: archives.len(),
        ..Report::default()
    };
    for (index, relative) in archives.iter().enumerate() {
        progress(index, archives.len());
        match std::fs::read(folder.join(relative)).and_then(|data| frames(&data)) {
            Ok(0) => report.unchecked += 1,
            Ok(count) => report.frames += count,
            Err(error) => report.damaged.push(format!("{relative} ({error})")),
        }
    }
    progress(archives.len(), archives.len());
    Ok(report)
}

/// manifest.json with a version made from the listed game files' sizes and change times. The game keeps
/// its own copy of the game data in the browser under that version and cannot see a repaired file of
/// the same size; a new version makes it drop the copy. Only the version value changes: the game reads
/// the rest of the text with its own parser.
pub fn manifest_with_version(manifest: &str, data: &Path) -> Result<String> {
    let parsed: serde_json::Value = serde_json::from_str(manifest)?;
    let version = parsed["version"].as_str().context("no version")?;
    let files = parsed["files"].as_array().context("no file list")?;
    let mut hash = Sha256::new();
    hash.update(manifest);
    for path in files.iter().filter_map(|entry| entry[0].as_str()) {
        let stamp = std::fs::metadata(data.join(path)).ok().map(|meta| {
            let changed = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
            (meta.len(), changed.map(|d| d.as_nanos()))
        });
        hash.update(format!("{path}\n{stamp:?}\n"));
    }
    // The game keeps only the version's digits, as a double; up to 15 digits stay exact.
    let number = u64::from_be_bytes(hash.finalize()[..8].try_into()?) % 1_000_000_000_000_000;
    let old = format!("\"version\":{}", serde_json::to_string(version)?);
    ensure!(manifest.matches(&old).count() == 1, "version not found");
    Ok(manifest.replacen(
        &old,
        &format!("\"version\":\"grandv-files-{number:015}\""),
        1,
    ))
}

/// Decompresses every frame; returns how many there were, or an error at the first broken one.
fn frames(data: &[u8]) -> io::Result<usize> {
    let mut count = 0;
    let mut offset = PREFIX;
    while offset + MAGIC.len() <= data.len() {
        if data[offset..offset + MAGIC.len()] != MAGIC {
            offset += SLOT;
            continue;
        }
        let broken = |reason: &dyn std::fmt::Display| {
            io::Error::other(format!("compressed part at byte {offset}: {reason}"))
        };
        let end = frame_end(data, offset).ok_or_else(|| broken(&"data ends early"))?;
        let mut decoder =
            zstd::stream::read::Decoder::with_buffer(&data[offset..end])?.single_frame();
        io::copy(&mut decoder, &mut io::sink()).map_err(|error| broken(&error))?;
        count += 1;
        offset = (end - PREFIX).div_ceil(SLOT) * SLOT + PREFIX;
    }
    Ok(count)
}

/// End of the zstd frame at `start`, from its block headers; None when they run past the data.
fn frame_end(data: &[u8], start: usize) -> Option<usize> {
    let descriptor = *data.get(start + 4)?;
    let single_segment = descriptor & 0x20 != 0;
    let mut at = start
        + 5
        + usize::from(!single_segment)
        + [0, 1, 2, 4][usize::from(descriptor & 3)]
        + [usize::from(single_segment), 2, 4, 8][usize::from(descriptor >> 6)];
    loop {
        let header = data.get(at..at + 3)?;
        let header = u32::from(header[0]) | u32::from(header[1]) << 8 | u32::from(header[2]) << 16;
        let kind = (header >> 1) & 3;
        if kind == 3 {
            return None;
        }
        at += 3 + if kind == 1 { 1 } else { header as usize >> 3 };
        if header & 1 == 1 {
            let end = at + if descriptor & 4 != 0 { 4 } else { 0 };
            return (end <= data.len()).then_some(end);
        }
    }
}
