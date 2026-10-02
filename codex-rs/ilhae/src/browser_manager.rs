//! Ilhae's client of the common browser service. Browser sessions live in the service.

use browser_rmcp::ErrorData;
use browser_rmcp::model::CallToolRequestParams;
use browser_rmcp::model::CallToolResult;
use browser_rmcp::model::ListToolsResult;
use browser_rmcp::model::PaginatedRequestParams;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::RwLock;
use tokio::sync::Mutex;
use tokio::sync::OnceCell;
use tokio::sync::broadcast;
use ugot_browser_service::Client;

use crate::settings_store::BrowserSettings;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum BrowserStatusEvent {
    Launched(BrowserStatus),
    Stopped,
    Crashed { message: String },
}

/// Cached UI projection of the last observed service status, not a browser session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserStatus {
    pub running: bool,
    pub browser_type: String,
    pub message: String,
    #[serde(default)]
    pub session_connected: bool,
}

impl Default for BrowserStatus {
    fn default() -> Self {
        Self {
            running: false,
            browser_type: "none".to_string(),
            message: "Shared browser service not observed yet".to_string(),
            session_connected: false,
        }
    }
}

pub struct BrowserManager {
    client: OnceCell<Client>,
    status: RwLock<BrowserStatus>,
    // Serializes settings-driven launch requests. This stores settings only;
    // the service owns the selected provider and every session's Stop state.
    launched_settings: Mutex<Option<String>>,
    event_tx: broadcast::Sender<BrowserStatusEvent>,
}

impl BrowserManager {
    pub fn new(_data_dir: &PathBuf) -> Self {
        let (event_tx, _) = broadcast::channel(16);
        Self {
            client: OnceCell::new(),
            status: RwLock::new(BrowserStatus::default()),
            launched_settings: Mutex::new(None),
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BrowserStatusEvent> {
        self.event_tx.subscribe()
    }

    pub fn get_status(&self) -> BrowserStatus {
        self.status.read().unwrap().clone()
    }

    async fn client(&self) -> Result<&Client, String> {
        self.client
            .get_or_try_init(|| async {
                let executable = service_executable(
                    std::env::var_os("UGOT_BROWSER_NATIVE_BIN").as_deref(),
                    std::env::var_os("PATH").as_deref(),
                )?;
                Client::connect_with_executable(&executable)
                    .await
                    .map_err(|error| error.to_string())
            })
            .await
    }

    pub async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
    ) -> Result<ListToolsResult, ErrorData> {
        let client = self.client().await.map_err(protocol_error)?;
        ugot_browser_service::list_tools(client, request).await
    }

    pub async fn call_tool(
        &self,
        request: CallToolRequestParams,
        cfg: &BrowserSettings,
    ) -> Result<CallToolResult, ErrorData> {
        // An explicit lifecycle request supersedes the initial settings launch.
        // Remember the attempt before delivery: an uncertain launch, close or
        // Stop must never cause an implicit relaunch on the next page tool.
        if request.name != "browser_service_status" {
            let settings = serde_json::to_string(cfg).map_err(protocol_error)?;
            let mut launched = self.launched_settings.lock().await;
            let initialize = !matches!(
                request.name.as_ref(),
                "browser_service_stop"
                    | "browser_launch"
                    | "browser_close"
                    | "browser_session_ops"
                    | "browser_profile_ops"
                    | "browser_launch_config"
            ) && launched.as_ref() != Some(&settings);
            *launched = Some(settings);
            if initialize {
                self.launch_inner(cfg).await.map_err(protocol_error)?;
            }
        }
        let client = self.client().await.map_err(protocol_error)?;
        // The native library preserves the complete MCP catalog and result;
        // no local DOM cache, result rewrapping, retries or backend fallback.
        ugot_browser_service::call_tool(client, request).await
    }

    pub async fn react_to_settings(&self, cfg: &BrowserSettings) -> Result<BrowserStatus, String> {
        let result = if cfg.enabled {
            self.launch(cfg).await
        } else {
            self.stop().await
        };
        if let Err(message) = &result {
            tracing::warn!("[BrowserManager] Shared service: {message}");
            let _ = self.event_tx.send(BrowserStatusEvent::Crashed {
                message: message.clone(),
            });
        }
        result
    }

    pub async fn launch(&self, cfg: &BrowserSettings) -> Result<BrowserStatus, String> {
        let settings = serde_json::to_string(cfg).map_err(|error| error.to_string())?;
        let mut launched = self.launched_settings.lock().await;
        let result = self.launch_inner(cfg).await?;
        *launched = Some(settings);
        Ok(result)
    }

    async fn launch_inner(&self, cfg: &BrowserSettings) -> Result<BrowserStatus, String> {
        let engine = launch_engine(cfg)?;
        let configured = settings_launch_request(cfg)?;
        let client = self.client().await?;
        let request = if let Some(request) = configured {
            request
        } else {
            let status = self.refresh_status().await?;
            if status.session_connected {
                return Ok(status);
            }
            if cfg.browser_type.eq_ignore_ascii_case("cef") || status.browser_type != "chrome" {
                return Err(
                    "The selected shared provider is disconnected; select a browser explicitly"
                        .to_string(),
                );
            }
            launch_configuration(cfg, engine.unwrap_or("chrome"))
        };
        let result = ugot_browser_service::call_tool(client, request)
            .await
            .map_err(|error| error.to_string())?;
        if result.is_error == Some(true) {
            return Err(serde_json::to_string(&result).map_err(|error| error.to_string())?);
        }
        let status = self.refresh_status().await?;
        let _ = self
            .event_tx
            .send(BrowserStatusEvent::Launched(status.clone()));
        Ok(status)
    }

    pub async fn refresh_status(&self) -> Result<BrowserStatus, String> {
        let value = self
            .client()
            .await?
            .request("status", json!({}))
            .await
            .map_err(|error| error.to_string())?;
        let status = status_projection(&value);
        *self.status.write().unwrap() = status.clone();
        Ok(status)
    }

    /// Global Stop affects every provider owned by the shared service.
    /// It does not close the browser or resume actions on the next tool call.
    pub async fn stop(&self) -> Result<BrowserStatus, String> {
        let value = self
            .client()
            .await?
            .request("stop", json!({}))
            .await
            .map_err(|error| error.to_string())?;
        if ugot_browser_service::service_result_is_error("stop", &value) {
            return Err(format!("Shared browser Stop was not confirmed: {value}"));
        }
        let mut status = self.get_status();
        status.message = "Shared browser actions stopped; browser remains open".to_string();
        *self.status.write().unwrap() = status.clone();
        let _ = self.event_tx.send(BrowserStatusEvent::Stopped);
        Ok(status)
    }
}

fn protocol_error(message: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(format!("Shared browser service: {message}"), None)
}

fn launch_engine(cfg: &BrowserSettings) -> Result<Option<&'static str>, String> {
    match cfg.browser_type.to_ascii_lowercase().as_str() {
        "auto" | "cef" => Ok(None),
        "chrome" => Ok(Some("chrome")),
        "firefox" => Ok(Some("firefox")),
        "camoufox" => Ok(Some("camoufox")),
        "webkit" => Ok(Some("webkit")),
        other => Err(format!("Unsupported shared browser provider: {other}")),
    }
}

fn settings_launch_request(cfg: &BrowserSettings) -> Result<Option<CallToolRequestParams>, String> {
    let engine = launch_engine(cfg)?;
    if !cfg.server_url.trim().is_empty()
        || (engine.is_none() && cfg.cdp_port != BrowserSettings::default().cdp_port)
    {
        let endpoint = if cfg.server_url.trim().is_empty() {
            format!("http://127.0.0.1:{}", cfg.cdp_port)
        } else {
            cfg.server_url.trim().to_string()
        };
        let mut request = CallToolRequestParams::new("browser_launch_config");
        request.arguments = json!({ "action": "attach", "cdpUrl": endpoint })
            .as_object()
            .cloned();
        return Ok(Some(request));
    }
    Ok(engine
        .map(|engine| launch_configuration(cfg, engine))
        .or_else(|| (!cfg.persistent).then(|| launch_configuration(cfg, "chrome"))))
}

fn launch_configuration(cfg: &BrowserSettings, engine: &str) -> CallToolRequestParams {
    let mut request = CallToolRequestParams::new("browser_launch_config");
    let mut arguments = json!({
        "action": "configure", "engine": engine, "headless": cfg.headless,
        "persistent": cfg.persistent, "relaunch": true,
    });
    if engine == "chrome" {
        arguments["cdpPort"] = cfg.cdp_port.into();
    }
    request.arguments = arguments.as_object().cloned();
    request
}

fn status_projection(value: &Value) -> BrowserStatus {
    let connected = value
        .get("browserConnected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let browser_type = value
        .get("activeEngine")
        .filter(|value| value.is_string())
        .or_else(|| value.get("selectedBackend"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    BrowserStatus {
        running: connected,
        browser_type,
        message: if value.get("stopped").and_then(Value::as_bool) == Some(true) {
            "Shared browser actions stopped; browser remains open".to_string()
        } else if connected {
            "Connected to the shared browser service".to_string()
        } else {
            "Shared browser provider is disconnected".to_string()
        },
        session_connected: connected,
    }
}

fn service_executable(explicit: Option<&OsStr>, path: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(explicit) = explicit {
        let executable = PathBuf::from(explicit);
        if !executable.is_absolute() || !native_executable(&executable) {
            return Err("UGOT_BROWSER_NATIVE_BIN must be an existing absolute native browser-service executable".to_string());
        }
        return executable.canonicalize().map_err(|error| error.to_string());
    }
    let binary = if cfg!(windows) {
        "ugot-browser.exe"
    } else {
        "ugot-browser"
    };
    if let Some(path) = path {
        for directory in std::env::split_paths(path) {
            if let Ok(executable) = directory.join(binary).canonicalize()
                && native_executable(&executable)
            {
                return Ok(executable);
            }
        }
    }
    Err("Native shared browser service was not found. Set UGOT_BROWSER_NATIVE_BIN to its absolute executable path".to_string())
}

fn native_executable(path: &Path) -> bool {
    if !path.is_file()
        || matches!(
            path.extension().and_then(OsStr::to_str),
            Some("js" | "mjs" | "cjs")
        )
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !path
            .metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        {
            return false;
        }
    }
    let mut magic = [0; 4];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    magic == *b"\x7fELF"
        || magic.starts_with(b"MZ")
        || matches!(
            magic,
            [0xfe, 0xed, 0xfa, 0xce | 0xcf]
                | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe | 0xbf]
                | [0xbe | 0xbf, 0xba, 0xfe, 0xca]
        )
}

#[cfg(test)]
#[path = "browser_manager_tests.rs"]
mod tests;
