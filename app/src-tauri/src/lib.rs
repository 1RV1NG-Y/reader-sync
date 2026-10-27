//! reader-sync app shell. For milestone A1 the UI keeps its library in the webview's
//! IndexedDB (app/src/library.mjs, "local" backend); at integration the sync core is linked
//! here and reached through one `rpc(method, params)` command with the same method names.

use std::{collections::HashSet, path::PathBuf, sync::Mutex};

use tauri::{DragDropEvent, Manager, State, WindowEvent, ipc::Response};

/// Largest book file accepted (matches the core's limit).
const MAX_BOOK_SIZE: u64 = 2 * 1024 * 1024 * 1024;

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
    tauri::Builder::default()
        .manage(DroppedPaths::default())
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                let state = window.state::<DroppedPaths>();
                let mut dropped = state.0.lock().unwrap();
                // Only the latest drop can be read.
                dropped.clear();
                dropped.extend(paths.iter().cloned());
            }
        })
        .invoke_handler(tauri::generate_handler![read_dropped_file])
        .run(tauri::generate_context!())
        .expect("error while running reader-sync");
}
