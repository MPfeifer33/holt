//! Native dialog commands. Currently exposes a folder picker used by the
//! TUI project switcher; can grow if more dialog flows land in the frontend.

use rfd::AsyncFileDialog;

/// Open a native folder picker. Returns the chosen path as a string, or
/// `None` if the user cancelled.
#[tauri::command]
pub async fn pick_directory(title: Option<String>) -> Result<Option<String>, String> {
    let mut dialog = AsyncFileDialog::new();
    if let Some(t) = title.as_deref() {
        dialog = dialog.set_title(t);
    }
    let picked = dialog.pick_folder().await;
    Ok(picked.map(|f| f.path().to_string_lossy().into_owned()))
}
