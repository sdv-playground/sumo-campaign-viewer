use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcuStatus {
    pub id: String,
    pub name: String,
    pub phase: String,
    pub version: Option<String>,
    pub previous_version: Option<String>,
    pub security_version: Option<u64>,
    pub progress: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignStatus {
    pub status: String,
    pub ecus: Vec<EcuStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestInfo {
    pub sequence_number: u64,
    pub security_version: Option<u64>,
    pub component_count: usize,
    pub dependency_count: usize,
    pub has_install: bool,
    pub has_validate: bool,
    pub has_invoke: bool,
    pub has_firmware: bool,
    pub text_version: Option<String>,
    pub text_vendor_name: Option<String>,
    pub text_model_name: Option<String>,
}

// =============================================================================
// App State
// =============================================================================

struct AppState {
    server_url: Mutex<String>,
}

// =============================================================================
// Commands
// =============================================================================

/// Connect to an SOVD server and discover components.
#[tauri::command]
async fn connect(state: State<'_, AppState>, url: String) -> Result<Vec<EcuStatus>, String> {
    *state.server_url.lock().unwrap() = url.clone();

    let client = sovd_client::SovdClient::new(&url)
        .map_err(|e| format!("connect: {e}"))?;

    let components = client.list_components()
        .await
        .map_err(|e| format!("list components: {e}"))?;

    Ok(components.iter().map(|c| EcuStatus {
        id: c.id.clone(),
        name: c.name.clone(),
        phase: "idle".into(),
        version: None,
        previous_version: None,
        security_version: None,
        progress: None,
        error: None,
    }).collect())
}

/// Parse a SUIT manifest envelope and return structured info.
#[tauri::command]
async fn parse_manifest(data: Vec<u8>) -> Result<ManifestInfo, String> {
    let crypto = sumo_crypto::RustCryptoBackend::new();
    // Parse without validation (no trust anchor needed for inspection)
    let envelope = sumo_codec::decode::decode_envelope(&data)
        .map_err(|e| format!("decode: {e:?}"))?;

    let manifest = sumo_onboard::Manifest::from_envelope(envelope);

    Ok(ManifestInfo {
        sequence_number: manifest.sequence_number(),
        security_version: manifest.security_version(0),
        component_count: manifest.component_count(),
        dependency_count: manifest.dependency_count(),
        has_install: manifest.has_install(),
        has_validate: manifest.has_validate(),
        has_invoke: manifest.has_invoke(),
        has_firmware: manifest.has_firmware(),
        text_version: manifest.text_version(0).map(|s| s.to_string()),
        text_vendor_name: manifest.text_vendor_name(0).map(|s| s.to_string()),
        text_model_name: manifest.text_model_name(0).map(|s| s.to_string()),
    })
}

/// Get activation state for a component.
#[tauri::command]
async fn get_activation(
    state: State<'_, AppState>,
    component_id: String,
) -> Result<serde_json::Value, String> {
    let url = state.server_url.lock().unwrap().clone();
    let client = sovd_client::SovdClient::new(&url)
        .map_err(|e| format!("{e}"))?;
    // TODO: read activation via flash client
    Ok(serde_json::json!({"component": component_id, "state": "unknown"}))
}

// =============================================================================
// App
// =============================================================================

pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            server_url: Mutex::new("http://localhost:4000".into()),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            parse_manifest,
            get_activation,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}
