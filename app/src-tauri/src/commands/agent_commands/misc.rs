use super::*;

#[derive(serde::Serialize)]
pub struct AppMetadata {
    pub name: String,
    pub version: String,
}

#[tauri::command]
pub fn get_app_metadata() -> Result<AppMetadata, CommandError> {
    Ok(AppMetadata {
        name: "Holt".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
