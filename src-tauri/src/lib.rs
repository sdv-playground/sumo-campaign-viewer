use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::task::JoinHandle;

#[allow(unused_imports)]
use sumo_crypto::RustCryptoBackend;

use sovd_client::flash::{FlashClient, UpdateStatusBody};
use sovd_client::SovdClient;
use sovd_core::EntityStatus;

/// UDS DID `F189` — "Vehicle Manufacturer ECU Software Version Number"
/// (ISO 14229 / ISO 17978-3 identData). Read as the installed firmware
/// version. The DID hex is itself a valid SOVD param-id (`/data/F189`).
const FW_VERSION_DID: &str = "F189";

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcuStatus {
    pub id: String,
    pub name: String,
    /// Display state derived from the `/updates` `UpdateStatusBody`
    /// (`phase`+`status`+`substate`): one of "idle", "preparing",
    /// "executing", "awaiting-verdict", "committed", "failed".
    pub transfer_state: Option<String>,
    /// Activation state — folded into `transfer_state` on the /updates
    /// wire (no separate activation resource). Mirrors `transfer_state`
    /// once the entry reaches the execute phase, else None.
    pub activation_state: Option<String>,
    /// Installed firmware version, read from identData DID `F189`.
    pub version: Option<String>,
    pub supports_rollback: bool,
    pub progress: Option<f64>,
    pub error: Option<String>,
    /// Diagnostic parameters discovered via list_parameters + read_data
    /// (bank/security DIDs: active_bank, committed, min/current_security_ver —
    /// only present if the ECU supports them). Guest liveness no longer lives
    /// here; it moved to the typed `/status` fields below.
    pub diagnostics: HashMap<String, serde_json::Value>,
    /// Guest runtime health from the converged `/status` endpoint
    /// (ISO 17978-3 §7.19.2): `Some(true)` = `ready`, `Some(false)` =
    /// `notReady`, `None` = `/status` unreachable (no liveness signal).
    pub ready: Option<bool>,
    /// `/status` `x-sumo-runtime.boot_id` — per-guest-lifetime nonce; a
    /// changed value is the canonical (re)boot witness.
    pub boot_id: Option<u32>,
    /// `/status` `x-sumo-runtime.hb_seq` — heartbeat liveness counter
    /// (advances ~1/s while the guest is alive).
    pub hb_seq: Option<u32>,
    /// `/status` `x-sumo-runtime.boot_count` — NV reset metric.
    pub boot_count: Option<u64>,
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

    let client = sovd_client::SovdClient::new(&url).map_err(|e| format!("connect: {e}"))?;

    // Discover top-level components, then probe each for sub-entities
    let components = client
        .list_components()
        .await
        .map_err(|e| format!("list components: {e}"))?;

    let mut ecus = Vec::new();
    let mut gateway_id = None;

    // Bank/security diagnostic DIDs (if the ECU exposes them). Guest liveness
    // (guest_state, heartbeat_seq) and the boot_count metric now come from the
    // converged `/status` endpoint, not per-DID reads.
    const DIAG_PARAMS: &[&str] = &[
        "active_bank",
        "committed",
        "min_security_ver",
        "current_security_ver",
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

    let initial_ecus: Vec<EcuStatus> = ecus
        .iter()
        .map(|e| EcuStatus {
            id: e.id.clone(),
            name: e.name.clone(),
            transfer_state: None,
            activation_state: None,
            version: None,
            supports_rollback: false,
            progress: None,
            error: None,
            diagnostics: HashMap::new(),
            ready: None,
            boot_id: None,
            hb_seq: None,
            boot_count: None,
        })
        .collect();

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
    let envelope =
        sumo_codec::decode::decode_envelope(&data).map_err(|e| format!("decode: {e:?}"))?;

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

/// Get the `/updates` lifecycle status for a component.
///
/// Repurposed for the ISO 17978-3 §7.18 wire: attaches to the latest
/// `/updates` entry on the component and returns its `UpdateStatusBody`
/// (`{phase, status, progress?, step?, error?, x-sumo-substate?}`) as
/// JSON. If the component has no updates, returns JSON `null`.
#[tauri::command]
async fn get_activation(
    state: State<'_, AppState>,
    component_id: String,
) -> Result<serde_json::Value, String> {
    let url = state.server_url.lock().unwrap().clone();
    let gateway_id = state.gateway_id.lock().unwrap().clone();

    let flash_client = match &gateway_id {
        Some(gw) if !gw.is_empty() => FlashClient::for_sovd_sub_entity(&url, gw, &component_id),
        _ => FlashClient::for_sovd(&url, &component_id),
    }
    .map_err(|e| format!("{e}"))?;

    // UpdateStatusBody is Deserialize-only (a wire shape); render the
    // fields the frontend cares about into JSON by hand.
    match latest_update_status(&flash_client).await {
        Some(body) => {
            let display_state = map_update_status(&body);
            Ok(serde_json::json!({
                "phase": body.phase,
                "status": body.status,
                "progress": body.progress,
                "step": body.step,
                "substate": body.substate,
                "error": body.error.map(|e| e.message),
                "display_state": display_state,
            }))
        }
        None => Ok(serde_json::Value::Null),
    }
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

        let payload = CampaignStatus {
            ecus: statuses,
            changes,
        };
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
                prev_value: if p.is_empty() {
                    None
                } else {
                    Some(p.to_string())
                },
            });
        }
    };

    check(
        "Transfer",
        prev.and_then(|p| p.transfer_state.as_deref()),
        next.transfer_state.as_deref(),
    );
    check(
        "Activation",
        prev.and_then(|p| p.activation_state.as_deref()),
        next.activation_state.as_deref(),
    );
    check(
        "Version",
        prev.and_then(|p| p.version.as_deref()),
        next.version.as_deref(),
    );

    // Liveness from `/status` (ISO 17978-3 §7.19.2). The fast-moving heartbeat
    // `hb_seq` is deliberately NOT logged (it advances ~1/s — noise, like the
    // old `heartbeat_seq`); `ready`, the reboot witness `boot_id`, and the NV
    // `boot_count` are rare, meaningful transitions worth a log line.
    let ready_label = |r: Option<bool>| r.map(|b| if b { "ready" } else { "notReady" });
    check(
        "Health",
        ready_label(prev.and_then(|p| p.ready)),
        ready_label(next.ready),
    );

    let prev_boot_id = prev.and_then(|p| p.boot_id).map(|v| v.to_string());
    let next_boot_id = next.boot_id.map(|v| v.to_string());
    check("Boot ID", prev_boot_id.as_deref(), next_boot_id.as_deref());

    let prev_boot_count = prev.and_then(|p| p.boot_count).map(|v| v.to_string());
    let next_boot_count = next.boot_count.map(|v| v.to_string());
    check(
        "Boot Count",
        prev_boot_count.as_deref(),
        next_boot_count.as_deref(),
    );

    // Diagnostics (bank/security DIDs — all slow-changing, so log every change)
    let prev_diag = prev.map(|p| &p.diagnostics);
    for (key, val) in &next.diagnostics {
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
                prev_value: if prev_str.is_empty() || prev_str == "null" {
                    None
                } else {
                    Some(prev_str)
                },
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

    // Lifecycle state ← the latest /updates entry's UpdateStatusBody.
    let status = latest_update_status(&flash_client).await;
    let (transfer_state, activation_state, progress, error, supports_rollback) = match &status {
        Some(body) => {
            let display = map_update_status(body);
            let progress = body.progress.map(|p| p as f64);
            let error = body.error.as_ref().map(|e| e.message.clone());
            // A banked trial paused awaiting a verdict is the only state
            // where commit/rollback is applicable.
            let supports_rollback = body.is_awaiting_verdict();
            // Activation == the execute phase; mirror the display string
            // once execute has begun, else leave None (transfer-only).
            let activation_state = if body.phase == "execute" {
                Some(display.clone())
            } else {
                None
            };
            (
                Some(display),
                activation_state,
                progress,
                error,
                supports_rollback,
            )
        }
        None => (Some("idle".to_string()), None, None, None, false),
    };

    // Installed firmware version ← identData DID F189 (the new wire).
    let version = read_fw_version(sovd_client, ecu).await;

    // Read diagnostic parameters (only for params discovered at connect time)
    let diagnostics = read_diagnostics(sovd_client, ecu).await;

    // Guest liveness ← the converged `/status` endpoint (ISO 17978-3 §7.19.2):
    // standard ready/notReady + the vendor `x-sumo-runtime` block.
    let runtime = read_runtime_status(sovd_client, ecu).await;

    EcuStatus {
        id: ecu.id.clone(),
        name: ecu.name.clone(),
        transfer_state,
        activation_state,
        version,
        supports_rollback,
        progress,
        error,
        diagnostics,
        ready: runtime.ready,
        boot_id: runtime.boot_id,
        hb_seq: runtime.hb_seq,
        boot_count: runtime.boot_count,
    }
}

/// Guest liveness extracted from the converged `/status` endpoint.
#[derive(Default)]
struct RuntimeStatus {
    ready: Option<bool>,
    boot_id: Option<u32>,
    hb_seq: Option<u32>,
    boot_count: Option<u64>,
}

/// Read a component's runtime status from the converged `/status` endpoint
/// (ISO 17978-3 §7.19.2): the standard `ready`/`notReady` plus the vendor
/// `x-sumo-runtime` block (`boot_id`, `hb_seq`, `boot_count`).
///
/// `/status` is a top-level entity resource — there is no sub-entity
/// `GET /apps/{id}/status` read on the SOVD wire — so this addresses the
/// component by its own id (the managed-cvc guests are registered as
/// top-level components). A gateway child, or an unreachable guest, yields a
/// default (all-`None`) status and the liveness fields simply stay absent.
async fn read_runtime_status(client: &SovdClient, ecu: &EcuInfo) -> RuntimeStatus {
    let body = match client.read_status(&ecu.id).await {
        Ok(b) => b,
        Err(_) => return RuntimeStatus::default(),
    };
    let runtime = body.extensions.get("x-sumo-runtime");
    let field_u64 = |key: &str| runtime.and_then(|r| r.get(key)).and_then(|v| v.as_u64());
    RuntimeStatus {
        ready: Some(matches!(body.status, EntityStatus::Ready)),
        boot_id: field_u64("boot_id").map(|v| v as u32),
        hb_seq: field_u64("hb_seq").map(|v| v as u32),
        boot_count: field_u64("boot_count"),
    }
}

/// Attach to the most-recent `/updates` entry on this component and fetch
/// its `UpdateStatusBody`. Returns `None` when the component has no updates
/// (idle) or any step of the list/attach/status round-trip fails.
async fn latest_update_status(flash_client: &FlashClient) -> Option<UpdateStatusBody> {
    let updates = flash_client.list_updates().await.ok()?;
    let last_id = updates.last()?;
    flash_client.attach(last_id).await.ok()?;
    flash_client.spec_status().await.ok()
}

/// Map an `/updates` `UpdateStatusBody` to the viewer's lowercase display
/// state string. The frontend keys its colour/stepper logic off these:
///   no updates                          → "idle"   (caller; not reached here)
///   prepare / *                         → "preparing"
///   execute + inProgress + awaiting-verdict → "awaiting-verdict" (trial)
///   execute + inProgress                → "executing"
///   execute + completed                 → "committed"
///   * + failed                          → "failed"
fn map_update_status(body: &UpdateStatusBody) -> String {
    if body.status == "failed" {
        return "failed".to_string();
    }
    match body.phase.as_str() {
        "prepare" => "preparing".to_string(),
        "execute" => {
            if body.is_awaiting_verdict() {
                "awaiting-verdict".to_string()
            } else if body.status == "completed" {
                "committed".to_string()
            } else {
                "executing".to_string()
            }
        }
        // Unknown phase: fall back to the raw status token, lowercased.
        _ => body.status.to_lowercase(),
    }
}

/// Read the installed firmware version from identData DID `F189`.
/// Routes via `GET .../data/F189` (top-level) or the gateway apps path
/// (sub-entity). Returns the value as a display string, or `None` if the
/// ECU doesn't expose F189.
async fn read_fw_version(client: &SovdClient, ecu: &EcuInfo) -> Option<String> {
    let resp = if ecu.gateway_id.is_empty() {
        client.read_data(&ecu.id, FW_VERSION_DID).await
    } else {
        client
            .read_sub_entity_data(&ecu.gateway_id, &ecu.id, FW_VERSION_DID)
            .await
    }
    .ok()?;

    // identData is normally an ASCII version string; fall back to the raw
    // JSON rendering (trimmed of quotes) for non-string encodings.
    match resp.value {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn idle_status(ecu: &EcuInfo) -> EcuStatus {
    EcuStatus {
        id: ecu.id.clone(),
        name: ecu.name.clone(),
        transfer_state: None,
        activation_state: None,
        version: None,
        supports_rollback: false,
        progress: None,
        error: None,
        diagnostics: HashMap::new(),
        ready: None,
        boot_id: None,
        hb_seq: None,
        boot_count: None,
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
            let available: Vec<String> = resp
                .items
                .iter()
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
        Ok(resp) => resp
            .items
            .iter()
            .filter(|p| wanted.contains(&p.id.as_str()))
            .map(|p| p.id.clone())
            .collect(),
        Err(_) => vec![],
    }
}

/// Read discovered diagnostic parameters for an ECU.
async fn read_diagnostics(
    client: &SovdClient,
    ecu: &EcuInfo,
) -> HashMap<String, serde_json::Value> {
    let mut result = HashMap::new();
    for param_id in &ecu.diagnostic_params {
        let resp = if ecu.gateway_id.is_empty() {
            client.read_data(&ecu.id, param_id).await
        } else {
            client
                .read_sub_entity_data(&ecu.gateway_id, &ecu.id, param_id)
                .await
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
