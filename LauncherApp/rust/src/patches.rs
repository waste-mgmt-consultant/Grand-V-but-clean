//! Fixes for known game bugs. The launcher serves fixed copies from `patched/`; the asset files stay untouched.
use crate::{config::Settings, storage};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::UNIX_EPOCH,
};

/// GrowHeap's locals and its `if (heapCount <= 31)` check; the byte at `HEAP_LIMIT_AT` is the 31.
const HEAP_CHECK: &[u8] =
    b"\x03\x0f\x7e\x03\x7f\x01\x7b\x02\x40\x20\x00\x28\x02\xfc\x03\x22\x10\x41\x1f\x4d";
const HEAP_LIMIT_AT: usize = 18;
/// strStreamingLoader::LoadRequestedObjects after a failed allocation: `call SetExtraMemory; end;
/// i32.const 1; local.set 6; ...; local.get 6; br_if 1`. The 1 means "fits the streaming budget", and
/// the request goes on to read into chunks that were never allocated: "Read request ... is using an
/// array of destination addresses, some of which are NULL". Only a cap makes allocations fail.
const NO_ROOM_CHECK: &[u8] =
    b"\x10\xfb\x4d\x0b\x41\x01\x21\x06\x0b\x20\x29\x42\xf0\x00\x7c\x24\x00\x20\x06\x0d\x01";
const NO_ROOM_AT: usize = 5;
/// strPackfileManager::AddImageToList builds the archive flags: `local.get 0; i32.const 256;
/// i32.const 0; local.get 3; select; i32.const 8; ...`. Flag 256 is images.meta's loadCompletely:
/// the archive is read into one block of its full size. The hills' grass archives need 32–64 MB
/// blocks, which a capped heap rarely has free, so the map waits for them forever. Turning 256 into
/// `i32.const 0` (two-byte LEB128) reads them entry by entry, like the other archives.
const WHOLE_LOAD_CHECK: &[u8] =
    b"\x20\x00\x41\x80\x02\x41\x00\x20\x03\x1b\x41\x08\x41\x00\x20\x01\x1b\x72";
const WHOLE_LOAD_AT: usize = 4;
/// Part of the patched/ stamp key; bump it when a fix changes, so old fixed copies are rebuilt.
const FIXES_VERSION: u32 = 1;

/// Caps the streaming resource heaps (256 MB each). Without a cap they keep growing and the game
/// stops with "Invalid fixup". A file that finds no free block then waits for a later frame, the way
/// the game already handles an allocation it gives up on, instead of stopping the game. Archives
/// marked for whole loading are read entry by entry, so no single file needs a 32–64 MB block.
pub fn limit_heaps(wasm: &mut [u8], heaps: u8) -> Result<()> {
    if !(1..=31).contains(&heaps) {
        bail!("Memory limit must be 1–31 heaps");
    }
    let (Some(heap), Some(no_room), Some(whole_load)) = (
        find_unique(wasm, HEAP_CHECK),
        find_unique(wasm, NO_ROOM_CHECK),
        find_unique(wasm, WHOLE_LOAD_CHECK),
    ) else {
        bail!("game file differs from the known version");
    };
    // Values up to 63 stay a one-byte signed LEB128, so the file size does not change.
    wasm[heap + HEAP_LIMIT_AT] = heaps - 1;
    wasm[no_room + NO_ROOM_AT] = 0;
    wasm[whole_load + WHOLE_LOAD_AT] = 0;
    Ok(())
}

// Cash hotkey: game.html turns a key press into a flag byte in the engine's keyboard block, and the
// CStatsMgr::Update entry consumes the flag through the GetStatId / IsKeyValid / IncrementStat path a
// money pickup uses. Functions are found by name, values by byte pattern.
const CASH_AMOUNT: f32 = 999_999.0; // exact below 2^24
/// Flag byte in the 256-byte keyboard block; VK 0xE8 is unassigned.
const CASH_KEY: i64 = 0xE8;
const STATS_UPDATE: &[u8] = b"CStatsMgr::Update()";
const GET_STAT_ID: &[u8] = b"StatId_char::GetStatId() const";
const IS_KEY_VALID: &[u8] = b"CStatsDataMgr::IsKeyValid(StatId const&) const";
const INCREMENT_STAT: &[u8] = b"CStatsMgr::IncrementStat(StatId const&, float, unsigned int)";
const STAT_ID_CTOR: &[u8] = b"StatId_char::StatId_char(";
const INPUT_TEST_KEY: &[u8] = b"wasm_input_test_key";
/// Stack frame setup at the original CStatsMgr::Update entry.
const UPDATE_PROLOGUE: &[u8] = b"\x23\x00\x42\x40\x7c\x22\x05\x24\x00";
/// game.html's keydown handler; the hotkey goes between these two parts, before the engine sees the key.
const PAGE_ANCHOR: [&str; 2] = [
    "e.preventDefault();\n\t\t",
    "if(!pressedPhysical.has(e.code))",
];
const PAGE_HOTKEY: &str = "if (e.code === 'Minus') { if (!e.repeat) { Atomics.store(keys, 0xE8, 1); trace('cash hotkey'); } return; }\n\t\t";

/// Story mode cash hotkey: each press of the key right of 0 adds $999,999 to the current character.
pub fn add_cash_hotkey(wasm: &[u8]) -> Result<Vec<u8>> {
    cash_hotkey(wasm).context("game file differs from the known version")
}
fn cash_hotkey(wasm: &[u8]) -> Result<Vec<u8>> {
    ensure!(wasm.starts_with(b"\0asm\x01\0\0\0"), "not a wasm file");
    let (mut imports, mut code, mut names) = (None, None, None);
    let mut at = 8;
    while at < wasm.len() {
        let section = at;
        let id = byte(wasm, &mut at)?;
        let size = leb(wasm, &mut at)?;
        let end = at
            .checked_add(size)
            .filter(|&end| end <= wasm.len())
            .context("section size")?;
        match id {
            0 => {
                let mut name = at;
                let length = leb(wasm, &mut name)?;
                if take(wasm, &mut name, length)? == b"name" {
                    names = Some(name..end);
                }
            }
            2 => imports = Some(at..end),
            10 => code = Some((section, at..end)),
            _ => {}
        }
        at = end;
    }
    let first = imported_functions(wasm, imports.context("no import section")?)?;
    let names = function_names(wasm, names.context("no name section")?)?;
    let index = |name: &[u8]| {
        let mut found = names.iter().filter(|(_, n)| *n == name);
        match (found.next(), found.next()) {
            (Some((index, _)), None) => Ok(*index),
            _ => bail!("{}", String::from_utf8_lossy(name)),
        }
    };
    let (section_start, code) = code.context("no code section")?;
    let mut at = code.start;
    let count = leb(wasm, &mut at)?;
    let mut bodies = Vec::new();
    while at < code.end {
        let size = leb(wasm, &mut at)?;
        bodies.push(take(wasm, &mut at, size)?);
    }
    ensure!(bodies.len() == count, "code section");
    let body = |name: &[u8]| {
        let i = index(name)?
            .checked_sub(first)
            .context("imported function")?;
        Ok::<_, anyhow::Error>((i, *bodies.get(i).context("function index")?))
    };

    // wasm_input_test_key: i64.const 255; i64.and; i64.const <keyboard block>; i64.add
    let (_, test_key) = body(INPUT_TEST_KEY)?;
    let mut at = find_unique(test_key, b"\x42\xff\x01\x83\x42").context("keyboard block")? + 5;
    let keyboard = sleb(test_key, &mut at)?;
    ensure!(test_key.get(at) == Some(&0x7c), "keyboard block add");

    // __wasm_call_ctors builds the cash StatId_char from the five {SP0,SP1,SP2,MP0,MP1}_TOTAL_CASH hashes.
    let ctor: Vec<_> = names
        .iter()
        .filter(|(_, n)| n.starts_with(STAT_ID_CTOR))
        .collect();
    let [(ctor, _)] = ctor[..] else {
        bail!("StatId_char constructor");
    };
    let (mut stores, mut tail) = (Vec::new(), Vec::new());
    for (slot, offset) in [
        ("SP0", 28),
        ("SP1", 24),
        ("SP2", 20),
        ("MP0", 16),
        ("MP1", 12),
    ] {
        let hash = joaat(&format!("{slot}_TOTAL_CASH")) as i32;
        stores.extend(
            [
                &[0x20, 0x00, 0x41][..],
                &encode_sleb(hash.into()),
                &[0x36, 0x02, offset],
            ]
            .concat(),
        );
        tail.extend(
            [
                &[0x20, 0x00, 0x42][..],
                &encode_sleb(offset.into()),
                &[0x7c],
            ]
            .concat(),
        );
    }
    tail.extend([&[0x10][..], &encode_leb(*ctor)].concat());
    let section = &wasm[code.clone()];
    let mut at = find_unique(section, &stores).context("cash stat")? + stores.len();
    ensure!(byte(section, &mut at)? == 0x42, "cash stat");
    let cash_stat = sleb(section, &mut at)?;
    ensure!(
        section.get(at..at + tail.len()) == Some(&tail[..]),
        "cash stat constructor"
    );

    let i64_const = |value: i64| [&[0x42][..], &encode_sleb(value)].concat();
    let call =
        |name: &[u8]| Ok::<_, anyhow::Error>([&[0x10][..], &encode_leb(index(name)?)].concat());
    let flag = i64_const(keyboard.checked_add(CASH_KEY).context("keyboard block")?);
    #[rustfmt::skip]
    let hotkey = [
        // if (flag) { flag = 0; sp -= 16;
        &flag[..], b"\x2d\x00\x00\x04\x40",
        &flag, b"\x41\x00\x3a\x00\x00",
        b"\x23\x00\x42\x10\x7d\x24\x00",
        // id (at sp + 8) = the current character's cash stat; if (IsKeyValid(id)) IncrementStat(id, amount, 3);
        b"\x23\x00\x42\x08\x7c", &i64_const(cash_stat), &call(GET_STAT_ID)?,
        b"\x23\x00\x42\x08\x7c", &call(IS_KEY_VALID)?, b"\x04\x40",
        b"\x23\x00\x42\x08\x7c\x43", &CASH_AMOUNT.to_le_bytes(), b"\x41\x03", &call(INCREMENT_STAT)?,
        b"\x0b",
        // sp += 16; }
        b"\x23\x00\x42\x10\x7c\x24\x00",
        b"\x0b",
    ]
    .concat();
    let (update_at, update) = body(STATS_UPDATE)?;
    ensure!(
        find_unique(update, UPDATE_PROLOGUE).is_some(),
        "CStatsMgr::Update prologue"
    );
    let mut at = 0;
    for _ in 0..leb(update, &mut at)? {
        leb(update, &mut at)?;
        byte(update, &mut at)?;
    }
    // Already there, e.g. served from an override made by the old cash_hotkey.py.
    if update[at..].starts_with(&hotkey) {
        return Ok(wasm.to_vec());
    }
    let update = [&update[..at], &hotkey, &update[at..]].concat();
    let mut payload = encode_leb(count);
    for (i, body) in bodies.iter().enumerate() {
        let body = if i == update_at { &update[..] } else { body };
        payload.extend(encode_leb(body.len()));
        payload.extend_from_slice(body);
    }
    Ok([
        &wasm[..section_start],
        &[10],
        &encode_leb(payload.len()),
        &payload,
        &wasm[code.end..],
    ]
    .concat())
}

/// Adds the cash hotkey's key handler to game.html.
pub fn add_cash_key(html: &str) -> Result<String> {
    if html.contains(PAGE_HOTKEY) {
        return Ok(html.into());
    }
    let anchor = PAGE_ANCHOR.concat();
    let (Some(at), 1) = (html.find(&anchor), html.matches(&anchor).count()) else {
        bail!("game page differs from the known version");
    };
    let at = at + PAGE_ANCHOR[0].len();
    Ok([&html[..at], PAGE_HOTKEY, &html[at..]].concat())
}

fn imported_functions(wasm: &[u8], imports: Range<usize>) -> Result<usize> {
    let mut at = imports.start;
    let mut functions = 0;
    for _ in 0..leb(wasm, &mut at)? {
        for _ in 0..2 {
            let length = leb(wasm, &mut at)?; // module, field
            take(wasm, &mut at, length)?;
        }
        match byte(wasm, &mut at)? {
            0 => {
                leb(wasm, &mut at)?;
                functions += 1;
            }
            kind @ (1 | 2) => {
                if kind == 1 {
                    byte(wasm, &mut at)?; // table element type
                }
                let flags = leb(wasm, &mut at)?;
                leb(wasm, &mut at)?;
                if flags & 1 != 0 {
                    leb(wasm, &mut at)?;
                }
            }
            3 => drop(take(wasm, &mut at, 2)?), // global type, mutability
            _ => {
                byte(wasm, &mut at)?; // tag attribute, type
                leb(wasm, &mut at)?;
            }
        }
    }
    ensure!(at == imports.end, "import section");
    Ok(functions)
}
fn function_names(wasm: &[u8], section: Range<usize>) -> Result<Vec<(usize, &[u8])>> {
    let (mut at, mut names) = (section.start, Vec::new());
    while at < section.end {
        let kind = byte(wasm, &mut at)?;
        let size = leb(wasm, &mut at)?;
        let end = at.checked_add(size).context("name section")?;
        if kind == 1 {
            for _ in 0..leb(wasm, &mut at)? {
                let index = leb(wasm, &mut at)?;
                let length = leb(wasm, &mut at)?;
                names.push((index, take(wasm, &mut at, length)?));
            }
        }
        at = end;
    }
    Ok(names)
}
fn find_unique(data: &[u8], pattern: &[u8]) -> Option<usize> {
    let mut found = data
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, w)| *w == pattern);
    match (found.next(), found.next()) {
        (Some((at, _)), None) => Some(at),
        _ => None,
    }
}
fn take<'a>(data: &'a [u8], at: &mut usize, length: usize) -> Result<&'a [u8]> {
    let bytes = at
        .checked_add(length)
        .and_then(|end| data.get(*at..end))
        .context("ends early")?;
    *at += length;
    Ok(bytes)
}
fn byte(data: &[u8], at: &mut usize) -> Result<u8> {
    Ok(take(data, at, 1)?[0])
}
fn leb(data: &[u8], at: &mut usize) -> Result<usize> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let b = byte(data, at)?;
        value |= u64::from(b & 0x7f) << shift;
        if b < 0x80 {
            return Ok(usize::try_from(value)?);
        }
    }
    bail!("number too long")
}
fn sleb(data: &[u8], at: &mut usize) -> Result<i64> {
    let mut value = 0i64;
    for shift in (0..64).step_by(7) {
        let b = byte(data, at)?;
        value |= i64::from(b & 0x7f) << shift;
        if b < 0x80 {
            if shift < 57 && b & 0x40 != 0 {
                value |= -1 << (shift + 7);
            }
            return Ok(value);
        }
    }
    bail!("number too long")
}
fn encode_leb(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}
fn encode_sleb(mut value: i64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (value & 0x7f) as u8;
        value >>= 7;
        if (value == 0 && b & 0x40 == 0) || (value == -1 && b & 0x40 != 0) {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}
/// Jenkins one-at-a-time hash of the lowercased name, as the game hashes stat names.
fn joaat(text: &str) -> u32 {
    let mut h = 0u32;
    for c in text.to_ascii_lowercase().bytes() {
        h = h.wrapping_add(c.into());
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// Last result per game file: (key, summary, served from `patched/`). Kept for the launcher window and
/// so a file that cannot be fixed is not rescanned.
type Results = BTreeMap<String, (String, String, bool)>;
fn results() -> &'static Mutex<Results> {
    static RESULTS: OnceLock<Mutex<Results>> = OnceLock::new();
    RESULTS.get_or_init(Default::default)
}
fn remember(results: &mut Results, relative: &str, key: String, summary: String, applied: bool) {
    if results
        .get(relative)
        .is_none_or(|(_, old, _)| *old != summary)
    {
        crate::diagnostics::record("Game fixes", summary.clone());
    }
    results.insert(relative.into(), (key, summary, applied));
}
pub fn status() -> Vec<String> {
    let results = results().lock().unwrap_or_else(|e| e.into_inner());
    results
        .values()
        .map(|(_, summary, _)| summary.clone())
        .collect()
}
pub fn forget() {
    results().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Returns game.html with the cash hotkey's key handler, or unchanged when it cannot be added.
pub fn cash_page(html: String) -> String {
    let (summary, html, applied) = match add_cash_key(&html) {
        Ok(fixed) => ("game.html: cash hotkey applied".into(), fixed, true),
        Err(e) => (
            format!("game.html: cash hotkey not applied ({e:#})"),
            html,
            false,
        ),
    };
    let mut results = results().lock().unwrap_or_else(|e| e.into_inner());
    remember(&mut results, "game.html", String::new(), summary, applied);
    html
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
    if !settings.memory_limit && !settings.cash_hotkey {
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
        "{}\n{}\n{}\nfixes={FIXES_VERSION} memory_limit={} memory_limit_heaps={} cash_hotkey={}\n",
        source.display(),
        meta.len(),
        meta.modified()?.duration_since(UNIX_EPOCH)?.as_nanos(),
        settings.memory_limit,
        settings.memory_limit_heaps,
        settings.cash_hotkey,
    );
    std::fs::create_dir_all(&patched)?;
    let target = storage::safe_target(&patched, relative)?;
    let stamp = stamp_path(&target);
    // The stamp holds the key and the summary of the build that made the copy.
    let stamped = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.strip_prefix(&key).map(str::to_owned));
    let (summary, applied) = if let Some(summary) = stamped.filter(|_| target.is_file()) {
        (summary, true)
    } else if let Some((_, summary, false)) = results.get(relative).filter(|(k, ..)| *k == key) {
        (summary.clone(), false)
    } else {
        build(&source, &target, &stamp, &key, name, settings)
    };
    remember(&mut results, relative, key, summary, applied);
    Ok(if applied { target } else { source })
}
fn stamp_path(target: &Path) -> PathBuf {
    let mut path = target.as_os_str().to_owned();
    path.push(".stamp");
    path.into()
}
/// Applies each enabled fix on its own, so one that no longer matches the game does not block the others.
fn build(
    source: &Path,
    target: &Path,
    stamp: &Path,
    key: &str,
    name: &str,
    settings: &Settings,
) -> (String, bool) {
    let result = (|| {
        let mut wasm = std::fs::read(source)?;
        let (mut outcomes, mut applied) = (Vec::new(), false);
        let mut outcome = |fix: &str, result: Result<()>| {
            applied |= result.is_ok();
            outcomes.push(match result {
                Ok(()) => format!("{fix} applied"),
                Err(e) => format!("{fix} not applied ({e:#})"),
            });
        };
        if settings.memory_limit {
            outcome(
                "memory limit",
                limit_heaps(&mut wasm, settings.memory_limit_heaps),
            );
        }
        if settings.cash_hotkey {
            outcome(
                "cash hotkey",
                add_cash_hotkey(&wasm).map(|fixed| wasm = fixed),
            );
        }
        let summary = format!("{name}: {}", outcomes.join(", "));
        if applied {
            storage::atomic_write(target, &wasm, false)?;
            storage::atomic_write(stamp, format!("{key}{summary}").as_bytes(), false)?;
        }
        Ok::<_, anyhow::Error>((summary, applied))
    })();
    let (summary, applied) =
        result.unwrap_or_else(|e| (format!("{name}: game fixes not applied ({e:#})"), false));
    if !applied {
        let _ = std::fs::remove_file(stamp);
        let _ = std::fs::remove_file(target);
    }
    (summary, applied)
}
