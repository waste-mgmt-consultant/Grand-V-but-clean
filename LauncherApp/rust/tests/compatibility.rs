use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use gta_browser_launcher::{
    config::{self, Settings},
    patches,
    server::{self, Running},
    storage, tls,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio_tungstenite::{
    Connector, connect_async, connect_async_tls_with_config,
    tungstenite::{Message, client::IntoClientRequest},
};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("mirror/playgta5.com");
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("launcher.html"), b"<h1>Fixture</h1>").unwrap();
    std::fs::write(root.join("data/test.bin"), b"0123456789").unwrap();
    std::fs::write(root.join("test.wasm"), b"wasm").unwrap();
    temp
}
fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn settings() -> Settings {
    Settings {
        port: port(),
        ..Default::default()
    }
}
/// `patches::status()` is process-wide and every server start clears it. A test that reads it holds
/// `FIX_STATUS.write()` for its whole run and uses `start_unlocked`; `start` waits for that test.
static FIX_STATUS: RwLock<()> = RwLock::new(());
async fn start(folder: &std::path::Path, settings: Settings, code: &str) -> Running {
    let _guard = FIX_STATUS.read().unwrap_or_else(|e| e.into_inner());
    start_unlocked(folder, settings, code).await
}
async fn start_unlocked(folder: &std::path::Path, settings: Settings, code: &str) -> Running {
    server::start_with_options(
        folder,
        settings,
        code,
        server::StartOptions {
            loopback_only: true,
            advertise: false,
        },
    )
    .await
    .unwrap()
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}
fn base(host: &Running) -> String {
    format!("http://localhost:{}", host.port())
}
async fn ws(
    host: &Running,
    origin: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut req = format!("ws://localhost:{}/relay", host.port())
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("Origin", origin.parse().unwrap());
    connect_async(req).await.unwrap().0
}
async fn send_json<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>, v: Value)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}
async fn receive<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> Message
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
fn hello(kind: &str, key: &str) -> Value {
    json!({"type":kind,"protocol":1,"key":key,"room":"friends","build":"fixture-1","map":"gta5"})
}

#[tokio::test]
async fn tunnel_origin_updates_share_links_and_websocket_checks() {
    let temp = fixture();
    let mut settings = settings();
    settings.mode = "external".into();
    settings.public_origin = "https://pending.invalid".into();
    settings.cloudflare_tunnel = true;
    settings.multiplayer = true;
    let mut host = start(temp.path(), settings, "tunnel-code").await;
    let origin = "https://fixture.trycloudflare.com";
    host.set_public_origin(origin).unwrap();
    assert_eq!(host.url(), format!("{origin}/launcher.html"));
    assert_eq!(host.share_url(), host.url());
    assert_eq!(host.settings.bind_ip(), std::net::Ipv4Addr::LOCALHOST);
    let saved = Settings::load(temp.path()).unwrap();
    assert_eq!(saved.public_origin, origin);
    assert!(saved.cloudflare_tunnel);
    let mut socket = ws(&host, origin).await;
    send_json(&mut socket, hello("check", "tunnel-code")).await;
    assert!(receive(&mut socket).await.is_text());
    socket.close(None).await.unwrap();
    let mut req = format!("ws://localhost:{}/relay", host.port())
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Origin", "https://pending.invalid".parse().unwrap());
    assert!(connect_async(req).await.is_err());
    host.stop().await;
}

#[test]
fn connection_limit_settings() {
    let mut settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.connection_limit(), 0);
    settings.max_connections = 4096;
    settings.https = true;
    assert_eq!(settings.connection_limit(), 4096);
    settings.cloudflare_tunnel = true;
    settings.mode = "external".into();
    assert_eq!(settings.connection_limit(), 200);
    let temp = tempfile::tempdir().unwrap();
    settings.save(temp.path()).unwrap();
    let mut saved = Settings::load(temp.path()).unwrap();
    assert_eq!(saved.connection_limit(), 200);
    saved.cloudflare_tunnel = false;
    assert_eq!(saved.connection_limit(), 4096);
    saved.max_connections = 0;
    assert_eq!(saved.connection_limit(), 0);
}

#[test]
fn bind_address_settings() {
    use std::net::Ipv4Addr;
    let mut settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.bind_ip(), Ipv4Addr::LOCALHOST);
    settings.mode = "external".into();
    settings.public_origin = "https://example.com".into();
    assert_eq!(settings.bind_ip(), Ipv4Addr::LOCALHOST);
    settings.bind = Some(Ipv4Addr::UNSPECIFIED);
    assert_eq!(settings.bind_ip(), Ipv4Addr::UNSPECIFIED);
    settings.validate().unwrap();
    let temp = tempfile::tempdir().unwrap();
    settings.save(temp.path()).unwrap();
    assert_eq!(
        Settings::load(temp.path()).unwrap().bind,
        Some(Ipv4Addr::UNSPECIFIED)
    );
    settings.cloudflare_tunnel = true;
    assert_eq!(settings.bind_ip(), Ipv4Addr::LOCALHOST);
    settings.cloudflare_tunnel = false;
    settings.mode = "lan".into();
    settings.bind = Some(Ipv4Addr::LOCALHOST);
    assert_eq!(settings.bind_ip(), Ipv4Addr::LOCALHOST);
    settings.bind = Some(Ipv4Addr::new(192, 168, 1, 5));
    assert!(settings.validate().is_err());
    let parsed: Settings = serde_json::from_str(r#"{"bind":"0.0.0.0"}"#).unwrap();
    assert_eq!(parsed.bind, Some(Ipv4Addr::UNSPECIFIED));
    assert!(serde_json::from_str::<Settings>(r#"{"bind":"all"}"#).is_err());
}

#[test]
fn validation_and_existing_formats() {
    assert!(config::access_code("  GTA5  ").unwrap() == "GTA5");
    assert!(config::access_code("a\nbc").is_err());
    assert!(storage::valid_path("/userdata/profiles/0/SGTA50000"));
    for p in [
        "/userdata/../escape",
        "/userdata/CON.bin",
        "/userdata/test. ",
        "/userdata/a\\b",
        "/userdata/a:b",
        "/userdata/.tombstones.json",
    ] {
        assert!(!storage::valid_path(p), "{p}");
    }
    assert!(config::origin("https://example.com/", true).is_ok());
    assert!(config::origin("https://user@example.com", true).is_err());
    assert_eq!(
        config::join_url("https://example.com").unwrap(),
        "https://example.com/launcher.html"
    );
    for (r, want) in [
        ("bytes=1-3", Some((1, 3))),
        ("bytes=-3", Some((7, 3))),
        ("bytes=7-", Some((7, 3))),
        ("bytes=11-12", None),
        ("bytes=0-1,3-4", None),
        ("bytes=-0", None),
    ] {
        assert_eq!(server::parse_range(r, 10), want);
    }
    assert_eq!(server::parse_range("bytes=0-0", 0), None);
    let temp = fixture();
    std::fs::write(
        temp.path().join("launcher-settings.json"),
        r#"{"port":8123,"mode":"device","future_option":42}"#,
    )
    .unwrap();
    let s = Settings::load(temp.path()).unwrap();
    s.save(temp.path()).unwrap();
    let v: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("launcher-settings.json")).unwrap())
            .unwrap();
    assert_eq!(v["future_option"], 42);
    config::save_code(temp.path(), "  GTA5 ").unwrap();
    assert_eq!(config::load_code(temp.path()).unwrap(), "GTA5");
}
#[tokio::test]
async fn ranges_batch_gzip_and_headers() {
    let temp = fixture();
    let host = start(temp.path(), settings(), "").await;
    let c = client();
    let url = base(&host);
    let r = c
        .get(format!("{url}/data/test.bin"))
        .header("Range", "bytes=2-5")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.headers()["Content-Range"], "bytes 2-5/10");
    assert_eq!(r.headers()["Cross-Origin-Opener-Policy"], "same-origin");
    assert_eq!(r.headers()["Cross-Origin-Embedder-Policy"], "require-corp");
    assert_eq!(r.bytes().await.unwrap(), "2345");
    let r = c
        .head(format!("{url}/data/test.bin"))
        .header("Range", "bytes=-3")
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["Content-Length"], "3");
    assert!(r.bytes().await.unwrap().is_empty());
    let r = c
        .get(format!("{url}/data/test.bin"))
        .header("Range", "bytes=10-")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 416);
    assert_eq!(r.headers()["Content-Range"], "bytes */10");
    let r = c.get(format!("{url}/test.wasm")).send().await.unwrap();
    assert_eq!(r.headers()["Content-Type"], "application/wasm");
    let r = c
        .post(format!("{url}/data/batch?gz=1"))
        .json(&json!([["test.bin", 1, 2], ["test.bin", 7, 20]]))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["X-Run-Lengths"], "2,3");
    assert_eq!(r.headers()["Content-Encoding"], "gzip");
    let bytes = r.bytes().await.unwrap();
    let mut reader = flate2::read::GzDecoder::new(&bytes[..]);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut out).unwrap();
    assert_eq!(out, b"12789");
    assert_eq!(
        c.post(format!("{url}/data/batch"))
            .json(&json!([["../../launcher-settings.json", 0, 10]]))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        c.post(format!("{url}/log"))
            .body("Fixture log")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("server.log")).unwrap(),
        "Fixture log\n"
    );
    host.stop().await;
}
const HEAP_CHECK: &[u8] =
    b"\x03\x0f\x7e\x03\x7f\x01\x7b\x02\x40\x20\x00\x28\x02\xfc\x03\x22\x10\x41\x1f\x4d";
const NO_ROOM_CHECK: &[u8] =
    b"\x10\xfb\x4d\x0b\x41\x01\x21\x06\x0b\x20\x29\x42\xf0\x00\x7c\x24\x00\x20\x06\x0d\x01";
const WHOLE_LOAD_CHECK: &[u8] =
    b"\x20\x00\x41\x80\x02\x41\x00\x20\x03\x1b\x41\x08\x41\x00\x20\x01\x1b\x72";
const MEMORY_NAME_CHECK: &[u8] = b"\x20\x01\x42\xc4\x8e\xb9\x01\x42\x07\x10\xbe\xc6\x05\x0d\x00\x20\x01\x42\x08\x7c\x21\x00\x03\x40";
/// Offsets of the heap count, the no-room flag, the whole-load flag and the memory file name start in `heap_wasm`.
const HEAP_AT: usize = 4 + 18;
const NO_ROOM_AT: usize = 4 + 20 + 3 + 5;
const WHOLE_LOAD_AT: usize = 4 + 20 + 3 + 21 + 3 + 4;
const MEMORY_NAME_AT: usize = 4 + 20 + 3 + 21 + 3 + 18 + 4 + 18;
fn heap_wasm() -> Vec<u8> {
    [
        &b"head"[..],
        HEAP_CHECK,
        b"mid",
        NO_ROOM_CHECK,
        b"and",
        WHOLE_LOAD_CHECK,
        b"tail",
        MEMORY_NAME_CHECK,
    ]
    .concat()
}
#[test]
fn photo_fix_changes_one_byte_and_adds_the_pipeline_once() {
    let mut wasm = heap_wasm();
    patches::fix_memory_files(&mut wasm).unwrap();
    let changed: Vec<_> = (0..wasm.len())
        .filter(|&i| wasm[i] != heap_wasm()[i])
        .collect();
    assert_eq!(changed, [MEMORY_NAME_AT]);
    assert_eq!(wasm[MEMORY_NAME_AT], 10);
    for mut unknown in [
        b"no check here".to_vec(),
        [heap_wasm(), heap_wasm()].concat(),
    ] {
        assert!(patches::fix_memory_files(&mut unknown).is_err());
    }
    let seed = patches::add_photo_pipeline(r#"[{"vs":"other"}]"#).unwrap();
    let pipelines: Vec<Value> = serde_json::from_str(&seed).unwrap();
    assert_eq!(pipelines.len(), 2);
    assert_eq!(pipelines[1]["rt"], json!(["rgba8unorm"]));
    assert_eq!(patches::add_photo_pipeline(&seed).unwrap(), seed);
    assert!(patches::add_photo_pipeline("{}").is_err());
}
#[tokio::test]
async fn photo_export_hooks_loaders_and_saves_jpegs() {
    let _guard = FIX_STATUS.write().unwrap_or_else(|e| e.into_inner());
    let temp = fixture();
    let build = temp.path().join("mirror/playgta5.com/b/x");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("loader.js"), "x({\n\tpreRun: [() => {}],\n});").unwrap();
    std::fs::write(build.join("loader.movie.js"), "unknown loader").unwrap();
    let host = start_unlocked(temp.path(), settings(), "").await;
    let (c, url) = (client(), base(&host));
    let get = |name: &str| c.get(format!("{url}/b/x/{name}")).send();
    let loader = get("loader.js").await.unwrap().text().await.unwrap();
    assert_eq!(loader.matches("onRuntimeInitialized").count(), 1);
    assert!(loader.ends_with("preRun: [() => {}],\n});"));
    assert_eq!(patches::add_photo_export(&loader).unwrap(), loader);
    let movie = get("loader.movie.js").await.unwrap().text().await.unwrap();
    assert_eq!(movie, "unknown loader");
    let jpeg = b"\xff\xd8\xff\xe0 photo \xff\xd9";
    for (name, body, status) in [
        ("PGTA5902240930.jpg", &jpeg[..], 204),
        ("PGTA5902240930.png", &jpeg[..], 400),
        ("x/PGTA51.jpg", &jpeg[..], 400),
        ("PGTA5.jpg", &jpeg[..], 400),
        ("PGTA51.jpg", &b"not a jpeg"[..], 400),
    ] {
        let r = c
            .put(format!("{url}/photos/{name}"))
            .body(body.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status, "{name}");
    }
    let photos: Vec<_> = std::fs::read_dir(temp.path().join("photos"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(photos, ["PGTA5902240930.jpg"]);
    assert_eq!(
        std::fs::read(temp.path().join("photos/PGTA5902240930.jpg")).unwrap(),
        jpeg
    );
    let status = patches::status().join("\n");
    assert!(
        status.contains("loader.js: photo export applied"),
        "{status}"
    );
    assert!(
        status.contains("loader.movie.js: photo export not applied"),
        "{status}"
    );
    host.stop().await;
    let mut public = settings();
    public.public_origin = "https://grandv.example".into();
    let host = start_unlocked(temp.path(), public, "").await;
    let r = c
        .put(format!("{}/photos/PGTA51.jpg", base(&host)))
        .body(jpeg.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(!temp.path().join("photos/PGTA51.jpg").exists());
    let loader = c
        .get(format!("{}/b/x/loader.js", base(&host)))
        .send()
        .await
        .unwrap();
    assert!(
        !loader
            .text()
            .await
            .unwrap()
            .contains("onRuntimeInitialized")
    );
    host.stop().await;
}
#[test]
fn memory_limit_changes_only_its_three_bytes() {
    let mut wasm = heap_wasm();
    patches::limit_heaps(&mut wasm, 6).unwrap();
    let changed: Vec<_> = (0..wasm.len())
        .filter(|&i| wasm[i] != heap_wasm()[i])
        .collect();
    assert_eq!(changed, [HEAP_AT, NO_ROOM_AT, WHOLE_LOAD_AT]);
    assert_eq!(
        (wasm[HEAP_AT], wasm[NO_ROOM_AT], wasm[WHOLE_LOAD_AT]),
        (5, 0, 0)
    );
    for mut unknown in [
        b"no check here".to_vec(),
        [heap_wasm(), heap_wasm()].concat(),
        [&b"head"[..], HEAP_CHECK].concat(),
        [&b"head"[..], HEAP_CHECK, NO_ROOM_CHECK].concat(),
    ] {
        let before = unknown.clone();
        assert!(patches::limit_heaps(&mut unknown, 6).is_err());
        assert_eq!(unknown, before);
    }
    assert!(patches::limit_heaps(&mut heap_wasm(), 0).is_err());
    assert!(patches::limit_heaps(&mut heap_wasm(), 32).is_err());
    let mut s = Settings::default();
    assert!(s.memory_limit && s.memory_limit_heaps == 6);
    s.memory_limit_heaps = 32;
    assert!(s.validate().is_err());
}
fn leb(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}
fn sized(bytes: &[u8]) -> Vec<u8> {
    [leb(bytes.len()), bytes.to_vec()].concat()
}
/// A game file with the functions and values the cash hotkey looks for, plus the memory limit checks.
/// `inserted` goes at the start of CStatsMgr::Update.
fn cash_wasm(inserted: &[u8]) -> Vec<u8> {
    // i64.const 6583436, the keyboard block.
    let keyboard = b"\x8c\xe9\x91\x03";
    // The five {SP0,SP1,SP2,MP0,MP1}_TOTAL_CASH hashes, the cash stat 25469204 and the constructor (function 6).
    let cash = b"\x20\x00\x41\x9d\x86\x93\x19\x36\x02\x1c\x20\x00\x41\x82\xd3\xf5\xa5\x04\x36\x02\x18\x20\x00\x41\xfd\x88\xd4\xeb\x78\x36\x02\x14\x20\x00\x41\x8b\xd0\xb9\x88\x78\x36\x02\x10\x20\x00\x41\x86\xce\x8c\x91\x04\x36\x02\x0c\x42\x94\xc2\x92\x0c\x20\x00\x42\x1c\x7c\x20\x00\x42\x18\x7c\x20\x00\x42\x14\x7c\x20\x00\x42\x10\x7c\x20\x00\x42\x0c\x7c\x10\x06";
    let functions: [(&str, Vec<u8>); 10] = [
        (
            "wasm_input_test_key",
            [&b"\x00\x42\xff\x01\x83\x42"[..], keyboard, b"\x7c\x0b"].concat(),
        ),
        (
            "CStatsMgr::Update()",
            [
                &b"\x01\x01\x7f"[..],
                inserted,
                b"\x23\x00\x42\x40\x7c\x22\x05\x24\x00\x0b",
            ]
            .concat(),
        ),
        ("StatId_char::GetStatId() const", b"\x00\x0b".to_vec()),
        (
            "CStatsDataMgr::IsKeyValid(StatId const&) const",
            b"\x00\x0b".to_vec(),
        ),
        (
            "CStatsMgr::IncrementStat(StatId const&, float, unsigned int)",
            b"\x00\x0b".to_vec(),
        ),
        (
            "StatId_char::StatId_char(char const*)",
            b"\x00\x0b".to_vec(),
        ),
        ("__wasm_call_ctors", [&b"\x00"[..], cash, b"\x0b"].concat()),
        ("GrowHeap", [HEAP_CHECK, b"\x0b"].concat()),
        (
            "LoadRequestedObjects",
            [&b"\x00"[..], NO_ROOM_CHECK, b"\x0b"].concat(),
        ),
        (
            "AddImageToList",
            [&b"\x00"[..], WHOLE_LOAD_CHECK, b"\x0b"].concat(),
        ),
    ];
    // Imports: one function (index 0), a memory and a global.
    let imports = b"\x03\x03env\x01f\x00\x00\x03env\x01m\x02\x01\x01\x02\x03env\x01g\x03\x7f\x00";
    let mut code = leb(functions.len());
    let mut names = leb(functions.len());
    for (i, (name, body)) in functions.iter().enumerate() {
        code.extend(sized(body));
        names.extend([leb(i + 1), sized(name.as_bytes())].concat());
    }
    let name_section = [sized(b"name"), vec![1], sized(&names)].concat();
    [
        &b"\0asm\x01\0\0\0"[..],
        &[2],
        &sized(imports),
        &[10],
        &sized(&code),
        &[0],
        &sized(&name_section),
    ]
    .concat()
}
/// What the original cash_hotkey.py inserts into `cash_wasm`.
const CASH_HOTKEY: &[u8] = b"\x42\xf4\xea\x91\x03\x2d\x00\x00\x04\x40\x42\xf4\xea\x91\x03\x41\x00\x3a\x00\x00\x23\x00\x42\x10\x7d\x24\x00\x23\x00\x42\x08\x7c\x42\x94\xc2\x92\x0c\x10\x03\x23\x00\x42\x08\x7c\x10\x04\x04\x40\x23\x00\x42\x08\x7c\x43\xf0\x23\x74\x49\x41\x03\x10\x05\x0b\x23\x00\x42\x10\x7c\x24\x00\x0b";
const PAGE: &str =
    "<script>\n\t\te.preventDefault();\n\t\tif(!pressedPhysical.has(e.code)) press(e);\n</script>";
const PAGE_HOTKEY: &str = "Atomics.store(keys, 0xE8, 1)";
#[test]
fn cash_hotkey_patches_known_game_only() {
    let fixed = patches::add_cash_hotkey(&cash_wasm(b"")).unwrap();
    assert_eq!(fixed, cash_wasm(CASH_HOTKEY));
    assert_eq!(patches::add_cash_hotkey(&fixed).unwrap(), fixed);
    let mut renamed = cash_wasm(b"");
    let at = renamed
        .windows(10)
        .position(|w| w == b"IsKeyValid")
        .unwrap();
    renamed[at..at + 10].copy_from_slice(b"IsKeyKnown");
    for (unknown, why) in [
        (heap_wasm(), "not a wasm file"),
        (renamed, "CStatsDataMgr::IsKeyValid(StatId const&) const"),
        (cash_wasm(b"")[..100].to_vec(), "section size"),
    ] {
        let error = format!("{:#}", patches::add_cash_hotkey(&unknown).unwrap_err());
        assert_eq!(
            error,
            format!("game file differs from the known version: {why}")
        );
    }
    let page = patches::add_cash_key(PAGE).unwrap();
    assert_eq!(page.matches(PAGE_HOTKEY).count(), 1);
    assert!(page.find(PAGE_HOTKEY) < page.find("if(!pressedPhysical"));
    assert_eq!(patches::add_cash_key(&page).unwrap(), page);
    assert!(patches::add_cash_key("<script></script>").is_err());
    assert!(!Settings::default().cash_hotkey);
}
#[tokio::test]
async fn memory_limit_serves_fixed_copy_and_keeps_assets() {
    let _guard = FIX_STATUS.write().unwrap_or_else(|e| e.into_inner());
    let temp = fixture();
    let build = temp.path().join("mirror/playgta5.com/b/x");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("game.wasm"), heap_wasm()).unwrap();
    std::fs::write(build.join("game.other.wasm"), b"unknown build").unwrap();
    std::fs::create_dir_all(build.join("shaders")).unwrap();
    std::fs::write(build.join("shaders/pipelines.json"), r#"[{"vs": "other"}]"#).unwrap();
    let fixed = temp.path().join("patched/b/x/game.wasm");
    let c = client();
    for (enabled, heaps, expected) in [(true, 6, 5), (true, 8, 7), (false, 6, 0x1f)] {
        let mut s = settings();
        s.memory_limit = enabled;
        s.memory_limit_heaps = heaps;
        let host = start_unlocked(temp.path(), s, "").await;
        let url = base(&host);
        let body = c
            .get(format!("{url}/b/x/game.wasm"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let mut want = heap_wasm();
        want[HEAP_AT] = expected;
        want[MEMORY_NAME_AT] = 10;
        if enabled {
            want[NO_ROOM_AT] = 0;
            want[WHOLE_LOAD_AT] = 0;
        }
        assert_eq!(body, want);
        let r = c
            .get(format!("{url}/b/x/game.wasm"))
            .header("Range", "bytes=22-23")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.bytes().await.unwrap(), [expected, 0x4d][..]);
        let other = c
            .get(format!("{url}/b/x/game.other.wasm"))
            .send()
            .await
            .unwrap();
        assert_eq!(other.bytes().await.unwrap(), "unknown build");
        assert_eq!(std::fs::read(build.join("game.wasm")).unwrap(), heap_wasm());
        assert!(fixed.is_file());
        let seed: Vec<Value> = c
            .get(format!("{url}/b/x/shaders/pipelines.json"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!((seed.len(), &seed[1]["rt"]), (2, &json!(["rgba8unorm"])));
        assert_eq!(
            std::fs::read_to_string(build.join("shaders/pipelines.json")).unwrap(),
            r#"[{"vs": "other"}]"#
        );
        let status = patches::status().join("\n");
        assert!(
            status.contains("pipelines.json: photo fix applied"),
            "{status}"
        );
        if enabled {
            assert!(
                status.contains("game.wasm: memory limit applied, photo fix applied"),
                "{status}"
            );
            assert!(
                status.contains("game.other.wasm: memory limit not applied"),
                "{status}"
            );
            assert!(!temp.path().join("patched/b/x/game.other.wasm").exists());
        } else {
            assert!(status.contains("game.wasm: photo fix applied"), "{status}");
            assert!(
                status.contains("game.other.wasm: photo fix not applied"),
                "{status}"
            );
        }
        host.stop().await;
    }
    // A restart with the same asset and settings reuses the fixed copy.
    let mut built = None;
    for _ in 0..2 {
        let host = start_unlocked(temp.path(), settings(), "").await;
        let url = base(&host);
        c.get(format!("{url}/b/x/game.wasm")).send().await.unwrap();
        let modified = std::fs::metadata(&fixed).unwrap().modified().unwrap();
        assert_eq!(*built.get_or_insert(modified), modified);
        assert_eq!(
            patches::status(),
            ["game.wasm: memory limit applied, photo fix applied"]
        );
        host.stop().await;
    }
}
#[tokio::test]
async fn cash_hotkey_serves_fixed_game_and_page() {
    let _guard = FIX_STATUS.write().unwrap_or_else(|e| e.into_inner());
    let temp = fixture();
    let root = temp.path().join("mirror/playgta5.com");
    std::fs::create_dir_all(root.join("b/x")).unwrap();
    std::fs::write(root.join("game.html"), PAGE).unwrap();
    std::fs::write(root.join("b/x/game.wasm"), cash_wasm(b"")).unwrap();
    std::fs::write(root.join("b/x/game.other.wasm"), heap_wasm()).unwrap();
    let c = client();
    for memory_limit in [true, false] {
        let mut s = settings();
        s.cash_hotkey = true;
        s.memory_limit = memory_limit;
        let host = start_unlocked(temp.path(), s, "").await;
        let url = base(&host);
        let get = |path: &str| {
            let request = c.get(format!("{url}/{path}"));
            async move { request.send().await.unwrap().bytes().await.unwrap() }
        };
        let mut want = cash_wasm(CASH_HOTKEY);
        let mut other = heap_wasm();
        patches::fix_memory_files(&mut other).unwrap();
        if memory_limit {
            patches::limit_heaps(&mut want, 6).unwrap();
            patches::limit_heaps(&mut other, 6).unwrap();
        }
        assert_eq!(get("b/x/game.wasm").await, want);
        assert_eq!(get("b/x/game.other.wasm").await, other);
        let page = String::from_utf8(get("game.html").await.to_vec()).unwrap();
        assert_eq!(page.matches(PAGE_HOTKEY).count(), 1, "{page}");
        assert_eq!(
            std::fs::read_to_string(root.join("game.html")).unwrap(),
            PAGE
        );
        assert_eq!(
            std::fs::read(root.join("b/x/game.wasm")).unwrap(),
            cash_wasm(b"")
        );
        let status = patches::status().join("\n");
        let (both, other) = if memory_limit {
            (
                "game.wasm: memory limit applied, cash hotkey applied",
                "game.other.wasm: memory limit applied, cash hotkey not applied (game file differs",
            )
        } else {
            (
                "game.wasm: cash hotkey applied",
                "game.other.wasm: cash hotkey not applied (game file differs",
            )
        };
        for line in [both, other, "game.html: cash hotkey applied"] {
            assert!(status.contains(line), "{status}");
        }
        host.stop().await;
    }
    let host = start_unlocked(
        temp.path(),
        Settings {
            memory_limit: false,
            ..settings()
        },
        "",
    )
    .await;
    let page = c
        .get(format!("{}/game.html", base(&host)))
        .send()
        .await
        .unwrap();
    assert!(!page.text().await.unwrap().contains(PAGE_HOTKEY));
    c.get(format!("{}/b/x/game.wasm", base(&host)))
        .send()
        .await
        .unwrap();
    assert!(!temp.path().join("patched/b/x/game.wasm").exists());
    host.stop().await;
}
#[tokio::test]
async fn save_restore_delete_rewrite_and_limits() {
    let temp = fixture();
    let mut s = settings();
    let host = start(temp.path(), s.clone(), "").await;
    let c = client();
    let url = base(&host);
    let profile = "a".repeat(32);
    let path = format!(
        "/userdata/{profile}/{}",
        URL_SAFE_NO_PAD.encode("/userdata/profiles/0/SGTA50000")
    );
    assert_eq!(
        c.put(format!("{url}{path}"))
            .header("X-Userdata-Mtime", "1700000000123")
            .body("save bytes")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let result: Value = c
        .get(format!("{url}/userdata/{profile}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["files"][0]["data"], "c2F2ZSBieXRlcw==");
    assert_eq!(result["files"][0]["mtime"], 1700000000123u64);
    assert_eq!(
        c.delete(format!("{url}{path}"))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    host.stop().await;
    s.port = port();
    let host = start(temp.path(), s, "").await;
    let url = base(&host);
    let result: Value = c
        .get(format!("{url}/userdata/{profile}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(result["files"].as_array().unwrap().is_empty());
    assert!(result["tombstones"][0]["mtime"].as_u64().unwrap() > 0);
    assert_eq!(
        c.put(format!("{url}{path}"))
            .body("new bytes")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let result: Value = c
        .get(format!("{url}/userdata/{profile}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(result["tombstones"].as_array().unwrap().is_empty());
    let bad = format!(
        "{url}/userdata/{profile}/{}",
        URL_SAFE_NO_PAD.encode("/userdata/../outside")
    );
    assert_eq!(c.put(bad).body("bad").send().await.unwrap().status(), 400);
    assert_eq!(
        c.put(format!("{url}{path}"))
            .body(vec![0; storage::MAX_FILE + 1])
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    host.stop().await;
}
#[tokio::test]
async fn capability_port_change_preserves_busy_listener_and_shutdown() {
    let temp = fixture();
    let host = start(temp.path(), settings(), "").await;
    let c = client();
    let url = base(&host);
    let token = host
        .url()
        .split("local-launch=")
        .nth(1)
        .unwrap()
        .to_string();
    let endpoint = format!("{url}/__local/port");
    assert_eq!(
        c.post(&endpoint)
            .json(&json!({"port":port()}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    assert_eq!(
        c.post(&endpoint)
            .header("Origin", &url)
            .header("X-Launcher-Token", &token)
            .json(&json!({"port":busy_port}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        c.get(format!("{url}/launcher.html"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let replacement = port();
    let result: Value = c
        .post(&endpoint)
        .header("Origin", &url)
        .header("X-Launcher-Token", token)
        .json(&json!({"port":replacement}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        result["url"]
            .as_str()
            .unwrap()
            .contains(&replacement.to_string())
    );
    assert_eq!(host.port(), replacement);
    assert_eq!(Settings::load(temp.path()).unwrap().port, replacement);
    assert_eq!(
        c.get(format!("{}/launcher.html", base(&host)))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let old_port = url.rsplit(':').next().unwrap().parse::<u16>().unwrap();
    host.stop().await;
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", replacement))
            .await
            .is_err()
    );
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", old_port))
            .await
            .is_err()
    );
    assert!(busy.local_addr().is_ok());
}
#[tokio::test]
async fn aggregate_bandwidth_and_ip_admission() {
    let temp = fixture();
    std::fs::write(
        temp.path().join("mirror/playgta5.com/payload.bin"),
        vec![0; 65536],
    )
    .unwrap();
    let mut s = settings();
    s.speed_mib = 0.125;
    s.max_clients = 1;
    let host = start(temp.path(), s, "").await;
    let url = format!("http://127.0.0.1:{}/payload.bin", host.port());
    let c = client();
    let before = Instant::now();
    let fetch = || async {
        c.get(&url)
            .header("Range", "bytes=0-32767")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
    };
    let (a, b) = tokio::join!(fetch(), fetch());
    assert_eq!(a.len() + b.len(), 65536);
    assert!(before.elapsed() >= Duration::from_millis(450));
    let other = reqwest::Client::builder()
        .no_proxy()
        .local_address("127.0.0.2".parse::<std::net::IpAddr>().unwrap())
        .build()
        .unwrap();
    assert_eq!(other.get(&url).send().await.unwrap().status(), 503);
    host.stop().await;
}
#[tokio::test]
async fn relay_preflight_capacity_pose_identity_and_cleanup() {
    let temp = fixture();
    let mut s = settings();
    s.multiplayer = true;
    let host = start(temp.path(), s, "GTA5").await;
    let origin = base(&host);
    let mut probe = ws(&host, &origin).await;
    send_json(&mut probe, hello("check", "GTA5")).await;
    let reply: Value = serde_json::from_str(receive(&mut probe).await.to_text().unwrap()).unwrap();
    assert_eq!(reply, json!({"type":"checked","players":0,"capacity":2}));
    let mut bad = ws(&host, &origin).await;
    send_json(&mut bad, hello("check", "gta5")).await;
    assert!(
        matches!(receive(&mut bad).await,Message::Close(Some(c)) if u16::from(c.code)==1008 && c.reason=="Join key rejected")
    );
    let mut a = ws(&host, &origin).await;
    send_json(&mut a, hello("join", "GTA5")).await;
    let welcome: Value = serde_json::from_str(receive(&mut a).await.to_text().unwrap()).unwrap();
    let id = welcome["id"].clone();
    assert_eq!(welcome["type"], "welcome");
    let mut b = ws(&host, &origin).await;
    send_json(&mut b, hello("join", "GTA5")).await;
    let welcome: Value = serde_json::from_str(receive(&mut b).await.to_text().unwrap()).unwrap();
    assert_eq!(welcome["peers"], json!([id]));
    assert!(receive(&mut a).await.to_text().unwrap().contains("joined"));
    let mut full = ws(&host, &origin).await;
    send_json(&mut full, hello("check", "GTA5")).await;
    assert!(matches!(receive(&mut full).await,Message::Close(Some(c)) if c.reason=="Room full"));
    send_json(&mut a,json!({"type":"pose","seq":1,"model":123,"position":[1,2,3],"rotation":[0,90,0],"id":"forged"})).await;
    let pose: Value = serde_json::from_str(receive(&mut b).await.to_text().unwrap()).unwrap();
    assert_eq!(pose["id"], id);
    assert_eq!(pose["seq"], 1);
    send_json(
        &mut a,
        json!({"type":"pose","seq":2,"model":123,"position":[99999,2,3],"rotation":[0,0,0]}),
    )
    .await;
    assert!(matches!(receive(&mut a).await,Message::Close(Some(c)) if u16::from(c.code)==1008));
    assert!(receive(&mut b).await.to_text().unwrap().contains("left"));
    host.stop().await;
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(2), b.next()).await,
        Err(_)
    ));
}
#[tokio::test]
async fn origin_rejection_and_public_proxy_configuration() {
    let temp = fixture();
    let mut s = settings();
    s.mode = "external".into();
    s.public_origin = "https://school.example".into();
    s.multiplayer = true;
    let host = start(temp.path(), s, "GTA5").await;
    assert_eq!(host.url(), "https://school.example/launcher.html");
    let mut req = format!("ws://localhost:{}/relay", host.port())
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Origin", "https://evil.example".parse().unwrap());
    assert!(connect_async(req).await.is_err());
    let mut good = ws(&host, "https://school.example").await;
    send_json(&mut good, hello("check", "GTA5")).await;
    assert!(
        receive(&mut good)
            .await
            .to_text()
            .unwrap()
            .contains("checked")
    );
    assert_eq!(
        client()
            .post(format!("{}/__local/port", base(&host)))
            .json(&json!({"port":port()}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    host.stop().await;
}
#[tokio::test]
async fn verified_https_wss_certificate_bootstrap_and_stable_root() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let temp = fixture();
    let first = tls::generate(temp.path()).unwrap();
    let root_path = temp.path().join("lan-tls/lan-root-ca.cer");
    let original = std::fs::read(&root_path).unwrap();
    let key = std::fs::read(temp.path().join("lan-tls/lan-root-ca.key")).unwrap();
    tls::generate(temp.path()).unwrap();
    assert_eq!(original, std::fs::read(&root_path).unwrap());
    assert_eq!(
        key,
        std::fs::read(temp.path().join("lan-tls/lan-root-ca.key")).unwrap()
    );
    let mut s = settings();
    s.mode = "lan".into();
    s.https = true;
    s.tls_cert = first.0;
    s.tls_key = first.1;
    s.multiplayer = true;
    let leaf = std::fs::read(&s.tls_cert).unwrap();
    let host = start(temp.path(), s, "GTA5").await;
    assert_eq!(leaf, std::fs::read(&host.settings.tls_cert).unwrap());
    let c = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(reqwest::Certificate::from_der(&original).unwrap())
        .build()
        .unwrap();
    let url = format!("https://localhost:{}", host.port());
    assert_eq!(
        c.get(format!("{url}/launcher.html"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let r = c
        .get(format!("{url}/__lan/certificate.cer"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.bytes().await.unwrap().as_ref(), &original);
    let info: Value = c
        .get(format!("{url}/__lan/certificate-info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["sha256"], host.fingerprint());
    let r = client()
        .get(format!("{}/__lan/certificate.cer", base(&host)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["Connection"], "close");
    assert_eq!(r.bytes().await.unwrap().as_ref(), &original);
    let raw = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    assert_eq!(
        raw.get(format!("{}/launcher.html", base(&host)))
            .send()
            .await
            .unwrap()
            .status(),
        302
    );
    assert_eq!(
        raw.post(format!("{}/data/batch", base(&host)))
            .body("[]")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let mut roots = rustls::RootCertStore::empty();
    roots.add(original.clone().into()).unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut request = format!("wss://localhost:{}/relay", host.port())
        .into_client_request()
        .unwrap();
    request.headers_mut().insert("Origin", url.parse().unwrap());
    let (mut socket, _) = connect_async_tls_with_config(
        request,
        None,
        false,
        Some(Connector::Rustls(Arc::new(config))),
    )
    .await
    .unwrap();
    send_json(&mut socket, hello("check", "GTA5")).await;
    assert!(
        receive(&mut socket)
            .await
            .to_text()
            .unwrap()
            .contains("checked")
    );
    host.stop().await;
}
#[test]
fn old_tombstone_list_is_preserved_as_timestamp_zero() {
    let temp = fixture();
    let id = "a".repeat(32);
    let root = temp.path().join("userdata").join(&id);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".tombstones.json"), r#"["save.bin"]"#).unwrap();
    std::fs::write(root.join("save.bin"), "deleted file").unwrap();
    let v = storage::restore(temp.path(), &id).unwrap();
    assert_eq!(v["files"], json!([]));
    assert_eq!(
        v["tombstones"],
        json!([{"path":"/userdata/save.bin","mtime":0}])
    );
}
#[tokio::test]
async fn discovery_nonce_protocol_and_socket_cleanup() {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(8123u16);
    let stop = tokio_util::sync::CancellationToken::new();
    let cancel = stop.clone();
    let task = tokio::spawn(async move {
        gta_browser_launcher::discovery::advertise(socket, rx, true, "fingerprint".into(), cancel)
            .await
    });
    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client
        .send_to(
            json!({"marker":"GTA5LAN1","nonce":"abcdefghijklmnop"})
                .to_string()
                .as_bytes(),
            ("127.0.0.1", port),
        )
        .await
        .unwrap();
    let mut buf = [0; 1024];
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), client.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let v: Value = serde_json::from_slice(&buf[..n]).unwrap();
    assert_eq!(v["port"], 8123);
    assert_eq!(v["nonce"], "abcdefghijklmnop");
    assert_eq!(v["https"], true);
    assert!(v.get("key").is_none());
    tx.send(8124).unwrap();
    stop.cancel();
    task.await.unwrap().unwrap();
    assert!(
        tokio::net::UdpSocket::bind(("127.0.0.1", port))
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn manifest_version_follows_game_files() {
    use gta_browser_launcher::integrity::manifest_with_version;
    let temp = fixture();
    let data = temp.path().join("mirror/playgta5.com/data");
    let manifest = r#"{"mount":"/game/","version":"grandv-test-20261016-1","files":[["test.bin",10,123],["gone.bin",4,123]]}"#;
    std::fs::write(data.join("manifest.json"), manifest).unwrap();
    let version = |text: &str| {
        serde_json::from_str::<Value>(text).unwrap()["version"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = manifest_with_version(manifest, &data).unwrap();
    assert_eq!(first, manifest_with_version(manifest, &data).unwrap());
    assert_eq!(
        first.replace(&version(&first), "grandv-test-20261016-1"),
        manifest
    );
    // A same-size repair: only the change time differs.
    filetime::set_file_mtime(
        data.join("test.bin"),
        filetime::FileTime::from_unix_time(1_800_000_000, 0),
    )
    .unwrap();
    let repaired = manifest_with_version(manifest, &data).unwrap();
    assert_ne!(version(&first), version(&repaired));
    for text in [&first, &repaired] {
        // The game keeps the digits as a double.
        let digits: String = version(text).chars().filter(char::is_ascii_digit).collect();
        assert!(digits.len() <= 15 && digits.parse::<u64>().unwrap() < 1 << 53);
    }
    let host = start(temp.path(), settings(), "").await;
    let c = client();
    let url = base(&host);
    let served = c
        .get(format!("{url}/data/manifest.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(served.headers()["Content-Type"], "application/json");
    assert_eq!(served.text().await.unwrap(), repaired);
    let r = c
        .get(format!("{url}/data/test.bin?v={}", version(&repaired)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().await.unwrap(), "0123456789");
    host.stop().await;
}
