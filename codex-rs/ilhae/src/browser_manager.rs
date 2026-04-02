use action_browser_rs::browser::{
    BrowserSession, ConnectionOptions, PlaywrightBackend, PlaywrightBrowserKind,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::settings_store::BrowserSettings;

/// Event emitted when browser status changes.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum BrowserStatusEvent {
    Launched(BrowserStatus),
    Stopped,
    Crashed { message: String },
}

/// Current state of the managed browser.
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
            message: "Not started".to_string(),
            session_connected: false,
        }
    }
}

/// Manages the browser process lifecycle (Playwright internal or Remote CDP connection).
pub struct BrowserManager {
    pub status: RwLock<BrowserStatus>,
    pub session: Arc<Mutex<Option<BrowserSession>>>,
    pub data_dir: PathBuf,
    pub event_tx: broadcast::Sender<BrowserStatusEvent>,
}

impl BrowserManager {
    pub fn new(data_dir: &PathBuf) -> Self {
        let (event_tx, _) = broadcast::channel(16);
        Self {
            status: RwLock::new(BrowserStatus::default()),
            session: Arc::new(Mutex::new(None)),
            data_dir: data_dir.clone(),
            event_tx,
        }
    }

    /// Subscribe to browser status change events.
    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<BrowserStatusEvent> {
        self.event_tx.subscribe()
    }

    /// React to settings changes: auto-launch if enabled, auto-stop if disabled.
    pub fn react_to_settings(&self, cfg: &BrowserSettings) {
        if cfg.enabled {
            let status = self.get_status();
            if !status.running {
                info!("[BrowserManager] Settings enabled=true → auto-launching/connecting");
                match self.launch(
                    &cfg.browser_type,
                    cfg.cdp_port,
                    cfg.headless,
                    cfg.persistent,
                    &cfg.server_url,
                ) {
                    Ok(s) => info!("[BrowserManager] Successfully connected: {}", s.message),
                    Err(e) => warn!("[BrowserManager] Connect failed: {}", e),
                }
            }
        } else {
            let status = self.get_status();
            if status.running {
                info!("[BrowserManager] Settings enabled=false → stopping");
                let _ = self.stop();
            }
        }
    }

    /// Get the BrowserSession for tool execution.
    pub fn get_session(&self) -> Arc<Mutex<Option<BrowserSession>>> {
        self.session.clone()
    }

    /// Disconnect the Playwright/CDP session gracefully.
    fn disconnect_session(&self) {
        let mut s = self.session.lock().unwrap();
        if let Some(session) = s.take() {
            let _ = session.close();
            info!("[BrowserManager] Browser Session closed");
        }

        if let Ok(mut status) = self.status.write() {
            status.session_connected = false;
        }
    }

    /// Get current browser status.
    pub fn get_status(&self) -> BrowserStatus {
        let status = self.status.read().unwrap();
        // action-browser-rs internally handles health.
        status.clone()
    }

    /// Fetch webSocketDebuggerUrl from /json/version
    fn fetch_cdp_url(cdp_port: u16) -> Result<String, String> {
        let version_url = format!("http://127.0.0.1:{}/json/version", cdp_port);
        match std::process::Command::new("curl")
            .args(["-s", "--max-time", "1", &version_url])
            .output()
        {
            Ok(output) if output.status.success() => {
                let body = String::from_utf8_lossy(&output.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(url) = json.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
                        return Ok(url.to_string());
                    }
                }
                Err("webSocketDebuggerUrl missing in /json/version".into())
            }
            _ => Err("Could not reach /json/version".into()),
        }
    }

    /// Establish connection to existing CDP browser (e.g. from Ilhae Desktop).
    fn connect_cdp(&self, cdp_port: u16) -> Result<BrowserSession, String> {
        info!(
            "[BrowserManager] Connecting to external CDP port {}",
            cdp_port
        );

        for attempt in 1..=3 {
            let delay_ms = if attempt == 1 { 500 } else { 1500 };
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));

            match Self::fetch_cdp_url(cdp_port) {
                Ok(ws_url) => {
                    info!("[BrowserManager] Got debugger URL: {}", ws_url);
                    match BrowserSession::connect(ConnectionOptions::new(ws_url).timeout(30000)) {
                        Ok(session) => return Ok(session),
                        Err(e) => warn!("[BrowserManager] CDP connection failed: {}", e),
                    }
                }
                Err(e) => warn!(
                    "[BrowserManager] CDP target fetch failed ({}): {}",
                    attempt, e
                ),
            }
        }
        Err(format!(
            "Could not connect to external Browser CDP on port {}",
            cdp_port
        ))
    }

    /// Connect or Launch based on settings.
    pub fn launch(
        &self,
        browser_type: &str,
        cdp_port: u16,
        headless: bool,
        _persistent: bool,
        _server_url: &str,
    ) -> Result<BrowserStatus, String> {
        {
            let status = self.status.read().unwrap();
            if status.running {
                return Err("Browser already running/connected".to_string());
            }
        }

        let browser_type_lower = browser_type.to_lowercase();

        let session = if browser_type_lower == "camoufox" || browser_type_lower == "firefox" {
            // Internal Playwright
            info!(
                "[BrowserManager] Local Launch Playwright (Camoufox/Firefox) headless={}",
                headless
            );
            let backend = PlaywrightBackend::launch(PlaywrightBrowserKind::Firefox, headless)
                .map_err(|e| format!("Failed to launch Playwright backend: {}", e))?;
            BrowserSession::with_backend(Box::new(backend))
        } else {
            // CDP connection: Ilhae Desktop or external Chrome manages the process on `cdp_port`
            info!(
                "[BrowserManager] Externally managed browser requested. Attempting CDP connect on port {}.",
                cdp_port
            );
            self.connect_cdp(cdp_port)?
        };

        let use_type = if browser_type_lower.contains("camoufox") {
            "Playwright (Camoufox)"
        } else {
            "CDP (Chrome/External)"
        };

        let mut s = self.session.lock().unwrap();
        *s = Some(session);

        let result = BrowserStatus {
            running: true,
            browser_type: use_type.to_string(),
            message: format!("{} running successfully", use_type),
            session_connected: true,
        };

        let mut status = self.status.write().unwrap();
        *status = result.clone();
        drop(status);

        let final_status = self.get_status();
        let _ = self
            .event_tx
            .send(BrowserStatusEvent::Launched(final_status.clone()));

        Ok(final_status)
    }

    /// Stop observing the browser running state.
    pub fn stop(&self) -> Result<BrowserStatus, String> {
        self.disconnect_session();

        let mut status = self.status.write().unwrap();
        if !status.running {
            return Err("Browser is not running".to_string());
        }

        *status = BrowserStatus {
            message: "Browser stopped/disconnected".to_string(),
            ..BrowserStatus::default()
        };
        let result = status.clone();
        drop(status);
        let _ = self.event_tx.send(BrowserStatusEvent::Stopped);
        Ok(result)
    }
}
