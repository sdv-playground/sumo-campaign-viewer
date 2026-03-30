use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::task::JoinHandle;

#[allow(unused_imports)]
use sumo_crypto::RustCryptoBackend;

use sovd_client::flash::{FlashClient, TransferState, ActivationStateResponse};

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
    pub supports_rollback: bool,
    pub progress: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignStatus {
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

// Internal ECU routing info (not sent to frontend)
#[derive(Debug, Clone)]
struct EcuInfo {
    id: String,
    name: String,
    gateway_id: String,
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

    for comp in &components {
        // Try to discover sub-entity apps (works for gateways)
        match client.list_apps(&comp.id).await {
            Ok(apps) if !apps.is_empty() => {
                // This component is a gateway with sub-entities
                gateway_id = Some(comp.id.clone());
                for app in apps {
                    ecus.push(EcuInfo {
                        id: app.id.clone(),
                        name: app.name.clone(),
                        gateway_id: comp.id.clone(),
                    });
                }
            }
            _ => {
                // Direct ECU (no sub-entities)
                ecus.push(EcuInfo {
                    id: comp.id.clone(),
                    name: comp.name.clone(),
                    gateway_id: String::new(),
                });
            }
        }
    }

    let initial_ecus: Vec<EcuStatus> = ecus.iter().map(|e| EcuStatus {
        id: e.id.clone(),
        name: e.name.clone(),
        phase: "idle".into(),
        version: None,
        previous_version: None,
        supports_rollback: false,
        progress: None,
        error: None,
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
    let mut interval = tokio::time::interval(Duration::from_millis(1500));

    loop {
        interval.tick().await;

        let mut statuses = Vec::new();

        for ecu in &ecus {
            let status = poll_single_ecu(&server_url, ecu).await;
            statuses.push(status);
        }

        let payload = CampaignStatus { ecus: statuses };
        if app_handle.emit("campaign-state-update", &payload).is_err() {
            break; // Window closed
        }
    }
}

async fn poll_single_ecu(server_url: &str, ecu: &EcuInfo) -> EcuStatus {
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
    let version = activation.as_ref().and_then(|a| a.active_version.clone());
    let prev_version = activation.as_ref().and_then(|a| a.previous_version.clone());
    let supports_rollback = activation.as_ref().map(|a| a.supports_rollback).unwrap_or(false);

    // Check flash transfers
    let transfers = flash_client.list_transfers().await.ok();

    let (phase, progress, error) = match transfers {
        Some(list) => {
            // Find most recent active transfer, or fall back to latest
            let active = list.transfers.iter().rfind(|t| is_active_state(&t.state));
            let latest = active.or_else(|| list.transfers.last());

            match latest {
                Some(t) => map_transfer_state(
                    &t.state,
                    &t.transfer_id,
                    t.error.as_ref().map(|e| e.message.clone()),
                    &flash_client,
                    &activation,
                ).await,
                None => map_activation_only(&activation),
            }
        }
        None => map_activation_only(&activation),
    };

    EcuStatus {
        id: ecu.id.clone(),
        name: ecu.name.clone(),
        phase,
        version,
        previous_version: prev_version,
        supports_rollback,
        progress,
        error,
    }
}

fn is_active_state(state: &TransferState) -> bool {
    matches!(state,
        TransferState::Queued | TransferState::Preparing | TransferState::Transferring |
        TransferState::Running | TransferState::AwaitingExit | TransferState::AwaitingReset
    )
}

async fn map_transfer_state(
    state: &TransferState,
    transfer_id: &str,
    error: Option<String>,
    flash_client: &FlashClient,
    activation: &Option<ActivationStateResponse>,
) -> (String, Option<f64>, Option<String>) {
    match state {
        TransferState::Queued | TransferState::Pending => ("session".into(), None, None),
        TransferState::Preparing => ("security".into(), None, None),
        TransferState::Transferring | TransferState::Running => {
            let progress = flash_client.get_flash_status(transfer_id).await.ok()
                .and_then(|s| s.progress)
                .and_then(|p| p.percent);
            ("flashing".into(), progress, None)
        }
        TransferState::AwaitingExit => ("finalizing".into(), None, None),
        TransferState::AwaitingReset => ("resetting".into(), None, None),
        TransferState::Activated => ("trial".into(), None, None),
        TransferState::Committed => ("committed".into(), None, None),
        TransferState::RolledBack => ("rolled_back".into(), None, None),
        TransferState::Failed | TransferState::Error | TransferState::Aborted =>
            ("failed".into(), None, error),
        TransferState::Verified => ("verifying".into(), None, None),
        TransferState::Complete | TransferState::Finished => map_activation_only(activation),
        TransferState::Invalid => ("failed".into(), None, Some("verification failed".into())),
    }
}

fn map_activation_only(
    activation: &Option<ActivationStateResponse>,
) -> (String, Option<f64>, Option<String>) {
    match activation {
        Some(a) => {
            let phase = match a.state.as_str() {
                "activated" => "trial",
                "committed" => "committed",
                "rolled_back" => "rolled_back",
                "awaiting_reset" | "awaitingreset" => "resetting",
                _ => "idle",
            };
            (phase.into(), None, None)
        }
        None => ("idle".into(), None, None),
    }
}

fn idle_status(ecu: &EcuInfo) -> EcuStatus {
    EcuStatus {
        id: ecu.id.clone(),
        name: ecu.name.clone(),
        phase: "idle".into(),
        version: None,
        previous_version: None,
        supports_rollback: false,
        progress: None,
        error: None,
    }
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
