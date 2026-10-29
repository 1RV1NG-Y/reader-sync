//! reader-sync app shell: links the sync core (`core/`) into the app process on every platform.
//! The UI reaches it through `rpc(method, params)` (JSON; see core/src/rpc.rs) plus two
//! commands for book bytes (`import_book` in, `book_file` out), and hears about changes
//! through `core-event` events.

use std::{collections::HashSet, path::PathBuf, sync::Mutex};

use readersync_core::{Event, ImportMeta, ImportSource, Node, NodeConfig};

#[cfg(target_os = "android")]
mod android;
#[cfg(desktop)]
mod background;
#[cfg(any(target_os = "android", test))]
mod node_slot;
use serde_json::Value;
use tauri::{DragDropEvent, Emitter, Manager, State, WindowEvent, ipc::Response};

/// Largest book file accepted (matches the core's limit).
const MAX_BOOK_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// The node, or why it couldn't start (the UI shows it instead of the app crashing).
#[cfg(not(target_os = "android"))]
struct AppNode(Result<Node, String>);
// The Android runtime owns the node. A clone here would keep a shut-down node's file lock
// alive and prevent a later service/activity restart from reopening the library.
#[cfg(target_os = "android")]
struct AppNode;

/// One node per process. Android can recreate the activity (and rerun setup) inside a live
/// process; a second node on the same files and port must never be opened.
#[cfg(not(target_os = "android"))]
static NODE: std::sync::OnceLock<Node> = std::sync::OnceLock::new();

fn node(state: &State<'_, AppNode>) -> Result<Node, String> {
    #[cfg(target_os = "android")]
    { let _ = state; android::node() }
    #[cfg(not(target_os = "android"))]
    { state.0.as_ref().cloned().map_err(Clone::clone) }
}

#[tauri::command]
async fn rpc(state: State<'_, AppNode>, method: String, params: Option<Value>) -> Result<Value, String> {
    readersync_core::rpc::dispatch(&node(&state)?, &method, params.unwrap_or(Value::Null)).await.map_err(|e| format!("{e:#}"))
}

/// Adds a book. Desktop: the file's bytes are the raw request body, and `x-import-meta`
/// carries percent-encoded JSON `{format, originalName, title, author}`. Android's IPC can't
/// carry raw bodies, so there the body is JSON `{meta, data}` with the bytes in base64.
#[tauri::command]
async fn import_book(state: State<'_, AppNode>, request: tauri::ipc::Request<'_>) -> Result<Value, String> {
    let node = node(&state)?;
    let (bytes, meta): (Vec<u8>, ImportMeta) = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => {
            let meta = request.headers().get("x-import-meta").and_then(|v| v.to_str().ok()).ok_or("The book's details are missing.")?;
            (bytes.clone(), serde_json::from_str(&percent_decode(meta)?).map_err(|e| e.to_string())?)
        }
        tauri::ipc::InvokeBody::Json(body) => {
            let data = body.get("data").and_then(Value::as_str).ok_or("The book's bytes are missing.")?;
            let bytes = data_encoding::BASE64.decode(data.as_bytes()).map_err(|e| format!("The book's bytes are damaged: {e}"))?;
            (bytes, serde_json::from_value(body.get("meta").cloned().unwrap_or_default()).map_err(|e| e.to_string())?)
        }
    };
    let (book, added) = node.import_book(ImportSource::Bytes(bytes), meta).await.map_err(|e| format!("{e:#}"))?;
    Ok(serde_json::json!({ "book": book, "added": added }))
}

/// The bytes of a book this device holds.
#[tauri::command]
async fn book_file(state: State<'_, AppNode>, id: String) -> Result<Response, String> {
    let path = node(&state)?.book_file(&id).map_err(|e| format!("{e:#}"))?.ok_or("This book hasn't arrived on this device yet.")?;
    tokio::task::spawn_blocking(move || std::fs::read(path)).await.map_err(|e| e.to_string())?.map(Response::new).map_err(|e| e.to_string())
}

/// The bytes of a book this device holds, in base64 (Android, whose IPC can't carry raw bytes).
#[tauri::command]
async fn book_file_base64(state: State<'_, AppNode>, id: String) -> Result<String, String> {
    let path = node(&state)?.book_file(&id).map_err(|e| format!("{e:#}"))?.ok_or("This book hasn't arrived on this device yet.")?;
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(path)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    Ok(data_encoding::BASE64.encode(&bytes))
}

/// Paths the user has dropped onto the window and the page hasn't read yet. The page can read
/// a file only by naming one of these, so it can't use the command to read anything else.
#[derive(Default)]
struct DroppedPaths(Mutex<HashSet<PathBuf>>);

/// Returns the bytes of a book file the user just dropped onto the window.
#[tauri::command]
fn read_dropped_file(path: PathBuf, dropped: State<'_, DroppedPaths>) -> Result<Response, String> {
    if !dropped.0.lock().unwrap().remove(&path) {
        return Err("That file wasn't dropped onto the window.".into());
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("epub" | "pdf")) {
        return Err("Only EPUB and PDF files can be added.".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_BOOK_SIZE {
        return Err("That isn't a book file this app can add.".into());
    }
    std::fs::read(&path).map(Response::new).map_err(|e| e.to_string())
}


/// Opens Android's native LAN listening controls; absent from browser/desktop UI.
#[tauri::command]
async fn android_sync_controls(_app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let handle = _app.state::<android::Controls>();
        handle.0.run_mobile_plugin_async::<Value>("showControls", Value::Null)
            .await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let builder = tauri::Builder::default();
    // First: a second launch (menu, login item) shows this app's window instead of starting
    // another node on the same library. `READER_SYNC_MULTI_INSTANCE` turns that off for test
    // instances with their own data dirs (app/tests/e2e).
    #[cfg(desktop)]
    let builder = if std::env::var_os("READER_SYNC_MULTI_INSTANCE").is_some() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| background::open_window(app)))
    };
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec![background::BACKGROUND_ARG])));
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android::plugin());
    builder
        .manage(DroppedPaths::default())
        .setup(|app| {
            #[cfg(target_os = "android")]
            if let Err(error) = android::attach(app.handle().clone(), app.path().app_data_dir()?.join("library")) {
                tracing::error!("the Android library could not start: {error}");
            }
            #[cfg(not(target_os = "android"))]
            let node = match NODE.get() {
                Some(node) => Ok(node.clone()),
                None => {
                    let data_dir = app.path().app_data_dir()?.join("library");
                    tauri::async_runtime::block_on(Node::open(NodeConfig {
                        data_dir,
                        device_name: device_name(),
                        platform: std::env::consts::OS.into(),
                        port: readersync_core::DEFAULT_PORT,
                        mdns: true,
                    }))
                    .map(|node| NODE.get_or_init(|| node).clone())
                    .map_err(|e| {
                        tracing::error!("the library could not start: {e:#}");
                        format!("{e:#}")
                    })
                }
            };
            #[cfg(not(target_os = "android"))]
            if let Ok(node) = &node {
                forward_events(app.handle().clone(), node);
                background::setup(app.handle(), node.clone())?;
            }
            // Without a node there is nothing to keep running: just show the error.
            #[cfg(not(target_os = "android"))]
            if node.is_err() && app.get_webview_window("main").is_none() {
                if let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main") {
                    tauri::WebviewWindowBuilder::from_config(app.handle(), config)?.build()?;
                }
            }
            #[cfg(not(target_os = "android"))]
            app.manage(AppNode(node));
            #[cfg(target_os = "android")]
            app.manage(AppNode);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                let state = window.state::<DroppedPaths>();
                let mut dropped = state.0.lock().unwrap();
                // Only the latest drop can be read.
                dropped.clear();
                dropped.extend(paths.iter().cloned());
            }
        })
        .invoke_handler(tauri::generate_handler![
            rpc,
            import_book,
            book_file,
            book_file_base64,
            read_dropped_file,
            android_sync_controls,
            #[cfg(desktop)]
            background::keeps_running_in_background,
            #[cfg(desktop)]
            background::start_at_login,
            #[cfg(desktop)]
            background::set_start_at_login,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting reader-sync")
        .run(|_app, _event| {
            // Closing the last window doesn't end the app: it keeps syncing in the tray, or
            // finishes sending before it quits (background.rs). `app.exit()` does end it.
            #[cfg(desktop)]
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = _event {
                api.prevent_exit();
                background::window_closed(_app);
            }
        });
}

fn forward_events(handle: tauri::AppHandle, node: &Node) {
    let mut events = node.subscribe();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match events.recv().await {
                Ok(event) => {
                    let _ = handle.emit("core-event", event);
                }
                // Missed events only mean the UI should refresh everything.
                Err(RecvError::Lagged(_)) => {
                    let _ = handle.emit("core-event", Event::LibraryChanged);
                    let _ = handle.emit("core-event", Event::StatusChanged);
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

fn percent_decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3).ok_or("bad percent-encoding")?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "bad percent-encoding")?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

fn init_logging() {
    use tracing_subscriber::{EnvFilter, prelude::*};
    let filter = EnvFilter::try_from_env("READER_SYNC_LOG").unwrap_or_else(|_| EnvFilter::new("readersync_core=info,iroh=warn"));
    #[cfg(target_os = "android")]
    let layer = tracing_android::layer("reader-sync").ok();
    #[cfg(not(target_os = "android"))]
    let layer = Some(tracing_subscriber::fmt::layer());
    let _ = tracing_subscriber::registry().with(filter).with(layer).try_init();
}

#[cfg(target_os = "android")]
fn device_name() -> String {
    fn prop(name: &str) -> Option<String> {
        let name = std::ffi::CString::new(name).ok()?;
        let mut buf = [0 as libc::c_char; 92]; // PROP_VALUE_MAX
        // SAFETY: `buf` is PROP_VALUE_MAX bytes, the documented maximum for this call.
        let len = unsafe { libc::__system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
        if len <= 0 {
            return None;
        }
        let bytes: Vec<u8> = buf[..len as usize].iter().map(|&c| c as u8).collect();
        String::from_utf8(bytes).ok().filter(|s| !s.trim().is_empty())
    }
    prop("ro.product.marketname").or_else(|| prop("ro.product.model")).unwrap_or_else(|| "Android phone".into())
}

/// "<computer name> (Linux)": a dual-boot PC is two devices with the same computer name, so
/// the OS is part of the name. `READER_SYNC_DEVICE_NAME` overrides it (for running test
/// instances side by side).
#[cfg(not(target_os = "android"))]
fn device_name() -> String {
    if let Ok(name) = std::env::var("READER_SYNC_DEVICE_NAME") {
        return name;
    }
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty());
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    match host {
        Some(host) => format!("{host} ({os})"),
        None => os.to_owned(),
    }
}
