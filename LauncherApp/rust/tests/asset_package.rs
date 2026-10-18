use gta_browser_launcher::{config, integrity};
use std::{fs, io::Write, path::PathBuf};

#[test]
fn branded_folder_and_incomplete_download_errors() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("grandv-assets-test-{}", std::process::id()));
    let assets = root.join("GrandV-assets");
    fs::create_dir_all(assets.join("mirror/playgta5.com")).unwrap();
    fs::write(assets.join("mirror/playgta5.com/launcher.html"), b"ok").unwrap();
    assert_eq!(
        config::resolve_folder(&root).unwrap(),
        assets.canonicalize().unwrap()
    );
    let marker = assets.join("grandv-assets.json");
    fs::write(&marker, r#"{"format":"grandv-assets-v1","files":[["mirror/playgta5.com/launcher.html",2],["missing.rpf",4]]}"#).unwrap();
    assert!(
        config::validate_asset_package(&assets)
            .unwrap_err()
            .to_string()
            .contains("missing.rpf")
    );
    fs::write(assets.join("missing.rpf"), b"bad").unwrap();
    assert!(config::validate_asset_package(&assets).is_err());
    fs::write(assets.join("missing.rpf"), b"good").unwrap();
    assert!(config::validate_asset_package(&assets).is_ok());
    fs::write(
        &marker,
        r#"{"format":"grandv-assets-v1","files":[["../outside",1]]}"#,
    )
    .unwrap();
    assert!(config::validate_asset_package(&assets).is_err());
    fs::remove_file(&marker).unwrap();
    assert!(config::validate_asset_package(&assets).is_ok());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn integrity_check_reports_cut_and_corrupt_archives() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path();
    assert!(integrity::check(folder, |_, _| {}).is_err());
    let data: Vec<u8> = (0..60_000_u32)
        .map(|i| ((i % 251) ^ (i / 97)) as u8)
        .collect();
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.write_all(&data).unwrap();
    let frame = encoder.finish().unwrap();
    // Two entries, each 16 bytes after a 512-byte boundary, like the packaged archives.
    let mut good = vec![7_u8; 16];
    good.extend(&frame);
    let second = good.len().div_ceil(512) * 512 + 16;
    good.resize(second, 0);
    good.extend(&frame);
    good.resize(good.len().div_ceil(512) * 512, 0);
    let mut cut = good.clone();
    cut[second + frame.len() / 2..].fill(0);
    let mut flipped = good.clone();
    flipped[second + frame.len() / 2] ^= 0xff;
    let files = [
        ("good.rpf", good.clone()),
        ("audio.rpf", vec![1_u8; 4096]),
        ("cut.rpf", cut),
        ("flipped.rpf", flipped),
        ("short.rpf", good[..second + 100].to_vec()),
        ("notes.txt", b"not an archive".to_vec()),
    ];
    let mut list = Vec::new();
    for (name, bytes) in &files {
        fs::write(folder.join(name), bytes).unwrap();
        list.push(serde_json::json!([name, bytes.len()]));
    }
    fs::write(
        folder.join("grandv-assets.json"),
        serde_json::json!({"format": "grandv-assets-v1", "files": list}).to_string(),
    )
    .unwrap();
    let last = std::cell::Cell::new((0, 0));
    let report = integrity::check(folder, |done, total| last.set((done, total))).unwrap();
    assert_eq!(last.get(), (5, 5));
    assert_eq!(
        (report.archives, report.unchecked, report.frames),
        (5, 1, 2)
    );
    assert_eq!(report.damaged.len(), 3, "{:?}", report.damaged);
    for (entry, name) in report
        .damaged
        .iter()
        .zip(["cut.rpf", "flipped.rpf", "short.rpf"])
    {
        assert!(entry.starts_with(name), "{entry}");
        assert!(entry.contains(&format!("at byte {second}")), "{entry}");
    }
}
