use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

#[allow(unused_imports)]
use sumo_crypto::RustCryptoBackend;

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
    // Decode the CBOR envelope (no signature validation — inspection only)
    let envelope = sumo_codec::decode::decode_envelope(&data)
        .map_err(|e| format!("decode: {e:?}"))?;

    let m = &envelope.manifest;
    let has_install = m.severable.install.is_some();
    let has_validate = m.validate.is_some();
    let has_invoke = m.invoke.is_some();

    // Extract text fields if present
    let text = m.severable.text.as_ref();
    let tc = text.and_then(|t| t.components.get(&0));

    Ok(ManifestInfo {
        sequence_number: m.sequence_number,
        security_version: None, // Would need parameter extraction
        component_count: m.common.components.len(),
        dependency_count: m.common.dependencies.len(),
        has_install,
        has_validate,
        has_invoke,
        has_firmware: has_install || has_validate,
        text_version: tc.and_then(|c| c.version.clone()),
        text_vendor_name: tc.and_then(|c| c.vendor_name.clone()),
        text_model_name: tc.and_then(|c| c.model_name.clone()),
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
