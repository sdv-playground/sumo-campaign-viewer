use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::task::JoinHandle;

#[allow(unused_imports)]
use sumo_crypto::RustCryptoBackend;

use sovd_client::flash::{FlashClient, TransferState};
use sovd_client::SovdClient;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcuStatus {
    pub id: String,
    pub name: String,
    /// Raw transfer state from SOVD flash API (e.g. "transferring", "activated")
    pub transfer_state: Option<String>,
    /// Raw activation state from SOVD flash/activation API (e.g. "committed", "activated")
    pub activation_state: Option<String>,
    pub version: Option<String>,
    pub previous_version: Option<String>,
    pub supports_rollback: bool,
    pub progress: Option<f64>,
    pub error: Option<String>,
    /// Diagnostic parameters discovered via list_parameters + read_data
    /// (e.g. active_bank, boot_count, committed — only present if ECU supports them)
    pub diagnostics: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub timestamp: String,
    pub ecu_id: String,
    pub field: String,
    pub value: String,
    pub prev_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignStatus {
    pub ecus: Vec<EcuStatus>,
    pub changes: Vec<StateChange>,
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

// Internal ECU routing info (not sent to frontend)
#[derive(Debug, Clone)]
struct EcuInfo {
    id: String,
    name: String,
    gateway_id: String,
    /// Diagnostic parameter IDs discovered at connect time (e.g. "active_bank", "boot_count")
    diagnostic_params: Vec<String>,
}

// =============================================================================
// App State
// =============================================================================

struct AppState {
    server_url: Mutex<String>,
    gateway_id: Mutex<Option<String>>,
    ecus: Mutex<Vec<EcuInfo>>,
    poll_handle: Mutex<Option<JoinHandle<()>>>,
}

// =============================================================================
// Commands
// =============================================================================

/// Connect to an SOVD server, discover ECUs, and start polling.
#[tauri::command]
async fn connect(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    url: String,
) -> Result<Vec<EcuStatus>, String> {
    // Stop any existing polling
    if let Some(handle) = state.poll_handle.lock().unwrap().take() {
        handle.abort();
    }

    *state.server_url.lock().unwrap() = url.clone();

    let client = sovd_client::SovdClient::new(&url)
        .map_err(|e| format!("connect: {e}"))?;

    // Discover top-level components, then probe each for sub-entities
    let components = client.list_components()
        .await
        .map_err(|e| format!("list components: {e}"))?;

    let mut ecus = Vec::new();
    let mut gateway_id = None;

    // Diagnostic param IDs we care about (if the ECU exposes them)
    const DIAG_PARAMS: &[&str] = &[
        "active_bank", "committed", "boot_count",
        "min_security_ver", "current_security_ver",
        "guest_state", "heartbeat_seq",
    ];

    for comp in &components {
        // Try to discover sub-entity apps (works for gateways)
        match client.list_apps(&comp.id).await {
            Ok(apps) if !apps.is_empty() => {
                // This component is a gateway with sub-entities
                gateway_id = Some(comp.id.clone());
                for app in apps {
                    // Discover which diagnostic params this ECU supports
                    let available = discover_params(&client, &comp.id, &app.id, DIAG_PARAMS).await;
                    ecus.push(EcuInfo {
                        id: app.id.clone(),
                        name: app.name.clone(),
                        gateway_id: comp.id.clone(),
                        diagnostic_params: available,
                    });
                }
            }
            _ => {
                // Direct ECU (no sub-entities)
                let available = discover_params_direct(&client, &comp.id, DIAG_PARAMS).await;
                ecus.push(EcuInfo {
                    id: comp.id.clone(),
                    name: comp.name.clone(),
                    gateway_id: String::new(),
                    diagnostic_params: available,
                });
            }
        }
    }

    let initial_ecus: Vec<EcuStatus> = ecus.iter().map(|e| EcuStatus {
        id: e.id.clone(),
        name: e.name.clone(),
        transfer_state: None,
        activation_state: None,
        version: None,
        previous_version: None,
        supports_rollback: false,
        progress: None,
        error: None,
        diagnostics: HashMap::new(),
    }).collect();

    // Store state
    *state.gateway_id.lock().unwrap() = gateway_id;
    *state.ecus.lock().unwrap() = ecus.clone();

    // Spawn polling task
    let poll_url = url;
    let poll_ecus = ecus;
    let handle = tokio::spawn(async move {
        poll_ecus_loop(app_handle, poll_url, poll_ecus).await;
    });
    *state.poll_handle.lock().unwrap() = Some(handle);

    Ok(initial_ecus)
}

/// Disconnect — stop polling and clear state.
#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = state.poll_handle.lock().unwrap().take() {
        handle.abort();
    }
    *state.ecus.lock().unwrap() = vec![];
    *state.gateway_id.lock().unwrap() = None;
    Ok(())
}

/// Parse a SUIT manifest envelope and return structured info.
#[tauri::command]
async fn parse_manifest(data: Vec<u8>) -> Result<ManifestInfo, String> {
    let envelope = sumo_codec::decode::decode_envelope(&data)
        .map_err(|e| format!("decode: {e:?}"))?;

    let m = &envelope.manifest;
    let has_install = m.severable.install.is_some();
    let has_validate = m.validate.is_some();
    let has_invoke = m.invoke.is_some();

    let text = m.severable.text.as_ref();
    let tc = text.and_then(|t| t.components.get(&0));

    Ok(ManifestInfo {
        sequence_number: m.sequence_number,
        security_version: None,
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
    let gateway_id = state.gateway_id.lock().unwrap().clone();

    let flash_client = match &gateway_id {
        Some(gw) if !gw.is_empty() => {
            FlashClient::for_sovd_sub_entity(&url, gw, &component_id)
        }
        _ => FlashClient::for_sovd(&url, &component_id),
    }.map_err(|e| format!("{e}"))?;

    let activation = flash_client.get_activation_state().await
        .map_err(|e| format!("{e}"))?;

    serde_json::to_value(&activation).map_err(|e| format!("{e}"))
}

// =============================================================================
// Polling
// =============================================================================

async fn poll_ecus_loop(app_handle: AppHandle, server_url: String, ecus: Vec<EcuInfo>) {
    let sovd_client = match SovdClient::new(&server_url) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut interval = tokio::time::interval(Duration::from_millis(1500));
    let mut prev_states: HashMap<String, EcuStatus> = HashMap::new();

    loop {
        interval.tick().await;

        let mut statuses = Vec::new();
        let mut changes = Vec::new();

        for ecu in &ecus {
            let status = poll_single_ecu(&server_url, &sovd_client, ecu).await;
            let prev = prev_states.get(&ecu.id);
            diff_ecu_status(prev, &status, &mut changes);
            statuses.push(status);
        }

        for s in &statuses {
            prev_states.insert(s.id.clone(), s.clone());
        }

        let payload = CampaignStatus { ecus: statuses, changes };
        if app_handle.emit("campaign-state-update", &payload).is_err() {
            break;
        }
    }
}

fn diff_ecu_status(prev: Option<&EcuStatus>, next: &EcuStatus, changes: &mut Vec<StateChange>) {
    let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();

    let mut check = |field: &str, prev_val: Option<&str>, next_val: Option<&str>| {
        let p = prev_val.unwrap_or("");
        let n = next_val.unwrap_or("");
        if p != n && !n.is_empty() {
            changes.push(StateChange {
                timestamp: ts.clone(),
                ecu_id: next.id.clone(),
                field: field.to_string(),
                value: n.to_string(),
                prev_value: if p.is_empty() { None } else { Some(p.to_string()) },
            });
        }
    };

    check("Transfer", prev.and_then(|p| p.transfer_state.as_deref()), next.transfer_state.as_deref());
    check("Activation", prev.and_then(|p| p.activation_state.as_deref()), next.activation_state.as_deref());
    check("Version", prev.and_then(|p| p.version.as_deref()), next.version.as_deref());
    check("Previous", prev.and_then(|p| p.previous_version.as_deref()), next.previous_version.as_deref());

    // Diagnostics (skip noisy continuously-changing fields)
    const NOISY_DIAG: &[&str] = &["heartbeat_seq"];
    let prev_diag = prev.map(|p| &p.diagnostics);
    for (key, val) in &next.diagnostics {
        if NOISY_DIAG.contains(&key.as_str()) {
            continue;
        }
        let next_str = val.to_string();
        let prev_str = prev_diag
            .and_then(|d| d.get(key))
            .map(|v| v.to_string())
            .unwrap_or_default();
        if next_str != prev_str && next_str != "null" {
            changes.push(StateChange {
                timestamp: ts.clone(),
                ecu_id: next.id.clone(),
                field: key.clone(),
                value: next_str,
                prev_value: if prev_str.is_empty() || prev_str == "null" { None } else { Some(prev_str) },
            });
        }
    }

    // Error
    if let Some(err) = &next.error {
        let prev_err = prev.and_then(|p| p.error.as_deref()).unwrap_or("");
        if err != prev_err {
            changes.push(StateChange {
                timestamp: ts.clone(),
                ecu_id: next.id.clone(),
                field: "Error".to_string(),
                value: err.clone(),
                prev_value: None,
            });
        }
    }
}

async fn poll_single_ecu(server_url: &str, sovd_client: &SovdClient, ecu: &EcuInfo) -> EcuStatus {
    let flash_client = if ecu.gateway_id.is_empty() {
        FlashClient::for_sovd(server_url, &ecu.id)
    } else {
        FlashClient::for_sovd_sub_entity(server_url, &ecu.gateway_id, &ecu.id)
    };

    let flash_client = match flash_client {
        Ok(c) => c,
        Err(_) => return idle_status(ecu),
    };

    // Check activation state
    let activation = flash_client.get_activation_state().await.ok();
    let activation_state = activation.as_ref().map(|a| a.state.clone());
    let version = activation.as_ref().and_then(|a| a.active_version.clone());
    let prev_version = activation.as_ref().and_then(|a| a.previous_version.clone());
    let supports_rollback = activation.as_ref().map(|a| a.supports_rollback).unwrap_or(false);

    // Check flash transfers
    let transfers = flash_client.list_transfers().await.ok();

    let (transfer_state, progress, error) = match transfers {
        Some(list) => {
            let active = list.transfers.iter().rfind(|t| is_active_state(&t.state));
            let latest = active.or_else(|| list.transfers.last());

            match latest {
                Some(t) => {
                    let progress = if matches!(t.state, TransferState::Transferring | TransferState::Running) {
                        flash_client.get_flash_status(&t.transfer_id).await.ok()
                            .and_then(|s| s.progress)
                            .and_then(|p| p.percent)
                    } else {
                        None
                    };
                    let error = if matches!(t.state, TransferState::Failed | TransferState::Error | TransferState::Aborted | TransferState::Invalid) {
                        t.error.as_ref().map(|e| e.message.clone())
                    } else {
                        None
                    };
                    (Some(format!("{:?}", t.state).to_lowercase()), progress, error)
                }
                None => (None, None, None),
            }
        }
        None => (None, None, None),
    };

    // Read diagnostic parameters (only for params discovered at connect time)
    let diagnostics = read_diagnostics(sovd_client, ecu).await;

    EcuStatus {
        id: ecu.id.clone(),
        name: ecu.name.clone(),
        transfer_state,
        activation_state,
        version,
        previous_version: prev_version,
        supports_rollback,
        progress,
        error,
        diagnostics,
    }
}

fn is_active_state(state: &TransferState) -> bool {
    matches!(state,
        TransferState::Queued | TransferState::Preparing | TransferState::Transferring |
        TransferState::Running | TransferState::AwaitingActivation | TransferState::AwaitingReboot
    )
}

fn idle_status(ecu: &EcuInfo) -> EcuStatus {
    EcuStatus {
        id: ecu.id.clone(),
        name: ecu.name.clone(),
        transfer_state: None,
        activation_state: None,
        version: None,
        previous_version: None,
        supports_rollback: false,
        progress: None,
        error: None,
        diagnostics: HashMap::new(),
    }
}

/// Discover which of the requested param IDs are available for a sub-entity ECU.
async fn discover_params(
    client: &SovdClient,
    gateway_id: &str,
    app_id: &str,
    wanted: &[&str],
) -> Vec<String> {
    match client.list_sub_entity_parameters(gateway_id, app_id).await {
        Ok(resp) => {
            let available: Vec<String> = resp.items.iter()
                .filter(|p| wanted.contains(&p.id.as_str()))
                .map(|p| p.id.clone())
                .collect();
            available
        }
        Err(_) => vec![],
    }
}

/// Discover which of the requested param IDs are available for a direct ECU.
async fn discover_params_direct(
    client: &SovdClient,
    component_id: &str,
    wanted: &[&str],
) -> Vec<String> {
    match client.list_parameters(component_id).await {
        Ok(resp) => {
            resp.items.iter()
                .filter(|p| wanted.contains(&p.id.as_str()))
                .map(|p| p.id.clone())
                .collect()
        }
        Err(_) => vec![],
    }
}

/// Read discovered diagnostic parameters for an ECU.
async fn read_diagnostics(client: &SovdClient, ecu: &EcuInfo) -> HashMap<String, serde_json::Value> {
    let mut result = HashMap::new();
    for param_id in &ecu.diagnostic_params {
        let resp = if ecu.gateway_id.is_empty() {
            client.read_data(&ecu.id, param_id).await
        } else {
            client.read_sub_entity_data(&ecu.gateway_id, &ecu.id, param_id).await
        };
        if let Ok(data) = resp {
            result.insert(param_id.clone(), data.value);
        }
    }
    result
}

// =============================================================================
// App
// =============================================================================

pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            server_url: Mutex::new("http://localhost:4000".into()),
            gateway_id: Mutex::new(None),
            ecus: Mutex::new(vec![]),
            poll_handle: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            parse_manifest,
            get_activation,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}
