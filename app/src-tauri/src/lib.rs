//! reader-sync app shell. For milestone A1 the UI keeps its library in the webview's
//! IndexedDB (app/src/library.mjs, "local" backend); at integration the sync core is linked
//! here and reached through one `rpc(method, params)` command with the same method names.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running reader-sync");
}
