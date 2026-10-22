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
/// fiDeviceMemory::Create: `strncmp(name, "memory:", 7); br_if; name + 8` starts reading the buffer
/// address of a `memory:$%p,...` file name. musl's %p writes "0x" first, so Create reads address 0 and
/// every write into the file fails; the Snapmatic photo is written this way, and libjpeg's write error
/// ends the game. Open already skips the "0x"; the byte at `MEMORY_NAME_AT` (8) becomes 10 to do the
/// same. A null %p is 16 zeros without "0x", so skipping two of them still reads 0.
const MEMORY_NAME_CHECK: &[u8] = b"\x20\x01\x42\xc4\x8e\xb9\x01\x42\x07\x10\xbe\xc6\x05\x0d\x00\x20\x01\x42\x08\x7c\x21\x00\x03\x40";
const MEMORY_NAME_AT: usize = 18;
/// Part of the patched/ stamp key; bump it when a fix changes, so old fixed copies are rebuilt.
const FIXES_VERSION: u32 = 3;

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

/// Lets the game write into memory files, so a Snapmatic photo no longer ends the game.
pub fn fix_memory_files(wasm: &mut [u8]) -> Result<()> {
    let at =
        find_unique(wasm, MEMORY_NAME_CHECK).context("game file differs from the known version")?;
    wasm[at + MEMORY_NAME_AT] = 10;
    Ok(())
}

/// common:/data/control/settings.meta, served from `patched/` like the game files.
pub const CONTROL_SETTINGS: &str = "data/common/data/control/settings.meta";
/// The 2013 data has no axis for INPUT_SCALED_LOOK_LR/UD, so nothing maps them and they stay 0. With
/// keyboard and mouse, script cameras read only these (Paparazzo's film camera does not turn). Built
/// from the same halves as INPUT_LOOK_LR/UD, they get the mouse and pad mappings the look axes get.
const SCALED_LOOK_AXES: &str = "<Item><Input>INPUT_SCALED_LOOK_UD</Input><Negative>INPUT_LOOK_UP_ONLY</Negative><Positive>INPUT_LOOK_DOWN_ONLY</Positive></Item><Item><Input>INPUT_SCALED_LOOK_LR</Input><Negative>INPUT_LOOK_LEFT_ONLY</Negative><Positive>INPUT_LOOK_RIGHT_ONLY</Positive></Item>";
const AXES_END: &str = "</AxisDefinitions>";

/// Adds the scaled look axes. The io worker reads only the manifest size of this file, so they take
/// the place of trailing padding and the size stays the same.
pub fn add_scaled_look(meta: &[u8]) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(meta)?;
    ensure!(
        !text.contains("INPUT_SCALED_LOOK_LR"),
        "the file already has the scaled look axes"
    );
    ensure!(
        text.matches(AXES_END).count() == 1,
        "file differs from the known version"
    );
    let body = text.trim_end_matches([' ', '\t', '\r', '\n']);
    let padding = &text[body.len()..];
    ensure!(
        padding.len() >= SCALED_LOOK_AXES.len(),
        "no room in the file"
    );
    let fixed = body.replacen(AXES_END, &format!("{SCALED_LOOK_AXES}{AXES_END}"), 1)
        + &padding[SCALED_LOOK_AXES.len()..];
    Ok(fixed.into_bytes())
}

/// The Snapmatic photo's copy draw: one quad into the photo target, once per photo. The GPU worker
/// skips a draw while its pipeline compiles, so in a browser that never compiled this pipeline the
/// photo is black. Pipelines in the seed are compiled while the game loads. Recorded by the GPU worker
/// (IndexedDB `gta-pipelines`).
const PHOTO_PIPELINE: &str = r#"{"vs":"4e1d5d361e0ed0bc","ps":"01a05b914ee571f0","el":[[2013849738,0,6,0,0,0,0],[1009949074,0,6,0,12,0,0],[3888318712,0,28,0,24,0,0],[197415955,0,16,0,28,0,0]],"st":[36],"tp":4,"sf":"","b":[0,0,0,2,1,1,2,1,1,15,0,2,1,1,2,1,1,15,0,2,1,1,2,1,1,15,0,2,1,1,2,1,1,15,0,2,1,1,2,1,1,15,0,2,1,1,2,1,1,15,0,2,1,1,2,1,1,15,0,2,1,1,2,1,1,15],"d":[0,0,8,0,65535,1,1,1,8,1,1,1,8],"r":[3,1,1,0,0,0,1,1,0,0],"rt":["rgba8unorm"],"n":1,"ds":"","ro":0}"#;

/// Adds the photo pipeline to a pipeline seed (`shaders/pipelines*.json`).
pub fn add_photo_pipeline(json: &str) -> Result<String> {
    let mut seed: Vec<serde_json::Value> = serde_json::from_str(json).context("pipeline seed")?;
    let photo: serde_json::Value = serde_json::from_str(PHOTO_PIPELINE)?;
    if !seed.contains(&photo) {
        seed.push(photo);
    }
    Ok(serde_json::to_string(&seed)?)
}

// Snapmatic gallery photos: the engine thread sends the JPEG of each PGTA5<id> file it writes, in
// Story (/userdata) and Sandbox (/sandbox), to PUT /photos/ (server.rs). Installed after the save
// restore, so restored photos are not sent again. That thread never returns to its event loop, so
// the request is synchronous. CPhotoBuffer layout: "JPEG", u32 buffer size, u32 image size, image.
const PHOTO_EXPORT: &str = r#"onRuntimeInitialized: () => {
                const close = FS.close;
                FS.close = (stream) => {
                    const path = stream.path || '', written = (stream.flags & 3) !== 0, name = /PGTA5\d+$/.exec(path);
                    close(stream);
                    if (!written || !name) return;
                    try {
                        const d = FS.readFile(path), view = new DataView(d.buffer, d.byteOffset, d.length);
                        let jpeg = null;
                        for (let t = 0; !jpeg && t + 12 <= d.length; t++) {
                            if (view.getUint32(t) !== 0x4a504547) continue;
                            const s = t + 12, e = s + view.getUint32(t + 8, true);
                            if (e <= d.length && d[s] === 0xff && d[s + 1] === 0xd8) jpeg = d.subarray(s, e);
                        }
                        if (!jpeg) throw new Error('no JPEG');
                        const x = new XMLHttpRequest();
                        x.open('PUT', '/photos/' + name[0] + '.jpg', false);
                        x.timeout = 5000;
                        x.send(jpeg);
                        self.__log('[loader] photo: ' + (x.status === 204 ? 'saved photos/' + name[0] + '.jpg' : name[0] + ' HTTP ' + x.status));
                    } catch (e) { self.__log('[loader] photo: ' + name[0] + ' ' + e); }
                };
            },
            "#;
const PHOTO_ANCHOR: &str = "preRun: [";

/// Adds the photo export to an engine loader (`loader.js`, `loader.movie.js`).
pub fn add_photo_export(js: &str) -> Result<String> {
    if js.contains(PHOTO_EXPORT) {
        return Ok(js.into());
    }
    let (Some(at), 1) = (js.find(PHOTO_ANCHOR), js.matches(PHOTO_ANCHOR).count()) else {
        bail!("engine loader differs from the known version");
    };
    Ok([&js[..at], PHOTO_EXPORT, &js[at..]].concat())
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
    let module = Module::parse(wasm)?;
    let (names, code) = (&module.names, &module.code);
    let index = |name: &[u8]| module.index(name);
    let body = |name: &[u8]| module.body(name);

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
    Ok(module.with_bodies(wasm, &[(update_at, update)]))
}

/// rage::netLogImpl::Flush(true) waits until the log spool thread has written the pending buffer. Load Game (and
/// the return after Replay Mission) runs CNetwork::Shutdown, which stops that thread, before CAILogManager::ShutDown
/// flushes its logs, so the game froze right after "shutdown!". HandleBufferSwapFailure has the same wait. Each
/// wait loop now starts with `if (spoolThreadId == -1) break`: br_if 1 leaves Flush's loop and its if; br_if 3
/// leaves the swap wait, its retry loop and the block around them, the early return after which the caller drops
/// the log line. Same bytes as the V1 fix.
const LOG_WAITS: [(&[u8], &[u8], u8); 2] = [
    (
        b"rage::netLogImpl::Flush(bool)",
        b"\x20\x00\x42\x08\x7c\x21\x02\x03\x40",
        1,
    ),
    (
        b"rage::netLogImpl::HandleBufferSwapFailure()",
        b"\x04\x40\x03\x40",
        3,
    ),
];
/// rage::LogSpoolFunction's thread id; CNetwork::Shutdown sets it to -1 once the thread has stopped.
const SPOOL_THREAD_ID: i64 = 6_630_952;

/// Ends the log waits that froze the game on Load Game.
pub fn fix_log_waits(wasm: &[u8]) -> Result<Vec<u8>> {
    log_waits(wasm).context("game file differs from the known version")
}
fn log_waits(wasm: &[u8]) -> Result<Vec<u8>> {
    let module = Module::parse(wasm)?;
    let id = [&[0x42][..], &encode_sleb(SPOOL_THREAD_ID)].concat();
    let (_, flush) = module.body(LOG_WAITS[0].0)?;
    ensure!(flush.windows(id.len()).any(|w| w == id), "spool thread id");
    let mut replaced = Vec::new();
    for (name, wait, depth) in LOG_WAITS {
        let (i, body) = module.body(name)?;
        let at = find_unique(body, wait)
            .with_context(|| format!("{} wait loop", String::from_utf8_lossy(name)))?
            + wait.len();
        // i64.load; i64.const -1; i64.eq; br_if
        let guard = [&id[..], b"\x29\x03\x00\x42\x7f\x51\x0d", &[depth]].concat();
        if !body[at..].starts_with(&guard) {
            replaced.push((i, [&body[..at], &guard, &body[at..]].concat()));
        }
    }
    Ok(if replaced.is_empty() {
        wasm.to_vec()
    } else {
        module.with_bodies(wasm, &replaced)
    })
}

/// The parts of a wasm module the code fixes read.
struct Module<'a> {
    /// Index of the first function with a body; the imported ones come before it.
    first: usize,
    names: Vec<(usize, &'a [u8])>,
    /// The code section with its id and size, and its contents.
    section: Range<usize>,
    code: Range<usize>,
    bodies: Vec<&'a [u8]>,
}
impl<'a> Module<'a> {
    fn parse(wasm: &'a [u8]) -> Result<Self> {
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
        let (section_start, code) = code.context("no code section")?;
        let mut at = code.start;
        let count = leb(wasm, &mut at)?;
        let mut bodies = Vec::new();
        while at < code.end {
            let size = leb(wasm, &mut at)?;
            bodies.push(take(wasm, &mut at, size)?);
        }
        ensure!(bodies.len() == count, "code section");
        Ok(Self {
            first,
            names,
            section: section_start..code.end,
            code,
            bodies,
        })
    }
    fn index(&self, name: &[u8]) -> Result<usize> {
        let mut found = self.names.iter().filter(|(_, n)| *n == name);
        match (found.next(), found.next()) {
            (Some((index, _)), None) => Ok(*index),
            _ => bail!("{}", String::from_utf8_lossy(name)),
        }
    }
    /// The position in `bodies` and the body of the function called `name`.
    fn body(&self, name: &[u8]) -> Result<(usize, &'a [u8])> {
        let i = self
            .index(name)?
            .checked_sub(self.first)
            .context("imported function")?;
        Ok((i, *self.bodies.get(i).context("function index")?))
    }
    /// `wasm` with the bodies at the given positions replaced.
    fn with_bodies(&self, wasm: &[u8], replaced: &[(usize, Vec<u8>)]) -> Vec<u8> {
        let mut payload = encode_leb(self.bodies.len());
        for (i, body) in self.bodies.iter().enumerate() {
            let body = replaced
                .iter()
                .find(|(at, _)| *at == i)
                .map_or(*body, |(_, new)| &new[..]);
            payload.extend(encode_leb(body.len()));
            payload.extend_from_slice(body);
        }
        [
            &wasm[..self.section.start],
            &[10],
            &encode_leb(payload.len()),
            &payload,
            &wasm[self.section.end..],
        ]
        .concat()
    }
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

/// Returns a pipeline seed with the photo pipeline, or unchanged when it cannot be added.
pub fn pipeline_seed(relative: &str, json: String) -> String {
    let name = relative.rsplit('/').next().unwrap_or_default();
    let (summary, json, applied) = match add_photo_pipeline(&json) {
        Ok(fixed) => (format!("{name}: photo fix applied"), fixed, true),
        Err(e) => (
            format!("{name}: photo fix not applied ({e:#})"),
            json,
            false,
        ),
    };
    let mut results = results().lock().unwrap_or_else(|e| e.into_inner());
    remember(&mut results, relative, String::new(), summary, applied);
    json
}

/// Returns an engine loader with the photo export, or unchanged when it cannot be added.
pub fn photo_loader(relative: &str, js: String) -> String {
    let name = relative.rsplit('/').next().unwrap_or_default();
    let (summary, js, applied) = match add_photo_export(&js) {
        Ok(fixed) => (format!("{name}: photo export applied"), fixed, true),
        Err(e) => (
            format!("{name}: photo export not applied ({e:#})"),
            js,
            false,
        ),
    };
    let mut results = results().lock().unwrap_or_else(|e| e.into_inner());
    remember(&mut results, relative, String::new(), summary, applied);
    js
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
    if !(name.starts_with("game") && name.ends_with(".wasm")) && relative != CONTROL_SETTINGS {
        return Ok(source);
    }
    // ponytail: one lock for all files; builds are rare and take under a second.
    let mut results = results().lock().unwrap_or_else(|e| e.into_inner());
    let patched = folder.join("patched");
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
        if name.ends_with(".meta") {
            outcome(
                "camera mouse fix",
                add_scaled_look(&wasm).map(|fixed| wasm = fixed),
            );
        } else {
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
            outcome("photo fix", fix_memory_files(&mut wasm));
            outcome(
                "load game fix",
                fix_log_waits(&wasm).map(|fixed| wasm = fixed),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_look_axes_keep_the_size() {
        let meta = format!(
            "<a><AxisDefinitions><Item/></AxisDefinitions></a>{}",
            " ".repeat(400)
        );
        let fixed = String::from_utf8(add_scaled_look(meta.as_bytes()).unwrap()).unwrap();
        assert_eq!(fixed.len(), meta.len());
        assert!(fixed.contains(&format!("<Item/>{SCALED_LOOK_AXES}{AXES_END}</a> ")));
        assert!(add_scaled_look(fixed.as_bytes()).is_err());
        assert!(add_scaled_look(meta.trim_end().as_bytes()).is_err());
    }
}
