//! reader-sync app shell: links the sync core (`core/`) into the app process on every platform.
//! The UI reaches it through `rpc(method, params)` (JSON; see core/src/rpc.rs) plus two
//! commands for book bytes (`import_book` in, `book_file` out), and hears about changes
//! through `core-event` events.

use std::{collections::HashSet, path::PathBuf, sync::Mutex};

use readersync_core::{Event, ImportMeta, ImportSource, Node, NodeConfig};
use serde_json::Value;
use tauri::{DragDropEvent, Emitter, Manager, State, WindowEvent, ipc::Response};

/// Largest book file accepted (matches the core's limit).
const MAX_BOOK_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// The node, or why it couldn't start (the UI shows it instead of the app crashing).
struct AppNode(Result<Node, String>);

/// One node per process. Android can recreate the activity (and rerun setup) inside a live
/// process; a second node on the same files and port must never be opened.
static NODE: std::sync::OnceLock<Node> = std::sync::OnceLock::new();

fn node<'a>(state: &'a State<'_, AppNode>) -> Result<&'a Node, String> {
    state.0.as_ref().map_err(Clone::clone)
}

#[tauri::command]
async fn rpc(state: State<'_, AppNode>, method: String, params: Option<Value>) -> Result<Value, String> {
    readersync_core::rpc::dispatch(node(&state)?, &method, params.unwrap_or(Value::Null)).await.map_err(|e| format!("{e:#}"))
}

/// Adds a book: the file's bytes are the raw request body, and `x-import-meta` carries
/// percent-encoded JSON `{format, originalName, title, author}`.
#[tauri::command]
async fn import_book(state: State<'_, AppNode>, request: tauri::ipc::Request<'_>) -> Result<Value, String> {
    let node = node(&state)?;
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("The book's bytes are missing.".into());
    };
    let meta = request.headers().get("x-import-meta").and_then(|v| v.to_str().ok()).ok_or("The book's details are missing.")?;
    let meta: ImportMeta = serde_json::from_str(&percent_decode(meta)?).map_err(|e| e.to_string())?;
    let (book, added) = node.import_book(ImportSource::Bytes(bytes.clone()), meta).await.map_err(|e| format!("{e:#}"))?;
    Ok(serde_json::json!({ "book": book, "added": added }))
}

/// The bytes of a book this device holds.
#[tauri::command]
async fn book_file(state: State<'_, AppNode>, id: String) -> Result<Response, String> {
    let path = node(&state)?.book_file(&id).map_err(|e| format!("{e:#}"))?.ok_or("This book hasn't arrived on this device yet.")?;
    tokio::task::spawn_blocking(move || std::fs::read(path)).await.map_err(|e| e.to_string())?.map(Response::new).map_err(|e| e.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let builder = tauri::Builder::default();
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    builder
        .manage(DroppedPaths::default())
        .setup(|app| {
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
            if let Ok(node) = &node {
                forward_events(app.handle().clone(), node);
            }
            app.manage(AppNode(node));
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
        .invoke_handler(tauri::generate_handler![rpc, import_book, book_file, read_dropped_file])
        .run(tauri::generate_context!())
        .expect("error while running reader-sync");
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
