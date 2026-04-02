//! Browser automation MCP tools bridged from browser-use-rs.
//!
//! Provides the `register_browser_tools!` macro that adds browser-use
//! tool registrations to an McpServer builder chain.

use crate::browser_manager::BrowserManager;
use crate::settings_store::SettingsStore;
use action_browser_rs::BrowserSession;
use action_browser_rs::dom::DomTree;
use action_browser_rs::tools::{Tool, ToolContext};
use std::sync::{Arc, Mutex};

/// Shared handle to the browser session, managed by BrowserManager.
pub type BrowserSessionHandle = Arc<Mutex<Option<BrowserSession>>>;

/// Global cache for the last snapshot's DOM tree.
/// When `browser_snapshot` is called, the DOM is stored here.
/// Index-based tools (click, input, hover, select) use this cached DOM
/// so that element indices remain consistent with what the AI saw.
static LAST_SNAPSHOT_DOM: Mutex<Option<DomTree>> = Mutex::new(None);

/// Tools that accept an `index` parameter and should use the cached snapshot DOM.
const INDEX_BASED_TOOLS: &[&str] = &["click", "input", "hover", "select"];

/// Run a browser-use tool against the shared session.
/// For snapshot tool: caches the extracted DOM for subsequent index-based lookups.
/// For index-based tools: injects the cached DOM to ensure index consistency.
/// When session is None: auto-launches browser via BrowserManager (lazy start).
pub fn exec_browser_tool<T: Tool + Default>(
    session_handle: &BrowserSessionHandle,
    params: T::Params,
    browser_mgr: &Arc<BrowserManager>,
    settings_store: &Arc<SettingsStore>,
) -> Result<String, sacp::Error> {
    let guard = session_handle.lock().map_err(|e| {
        let msg = format!("Session lock poisoned: {}", e);
        eprintln!("[browser_tools] {}", msg);
        sacp::Error::new(-32603, msg)
    })?;

    // Lazy launch: if no session connected, launch browser on-demand
    if guard.is_none() {
        drop(guard); // release lock before launch
        let cfg = settings_store.get();
        eprintln!("[browser_tools] No active session, launching browser on-demand...");
        browser_mgr
            .launch(
                &cfg.browser.browser_type,
                cfg.browser.cdp_port,
                cfg.browser.headless,
                cfg.browser.persistent,
                &cfg.browser.server_url,
            )
            .map_err(|e| {
                let msg = format!("Failed to auto-launch browser: {}", e);
                eprintln!("[browser_tools] {}", msg);
                sacp::Error::new(-32603, msg)
            })?;
        // Re-acquire and check
        let guard = session_handle
            .lock()
            .map_err(|e| sacp::Error::new(-32603, format!("Session lock poisoned: {}", e)))?;
        let session = guard.as_ref().ok_or_else(|| {
            let msg =
                "Browser launched but CDP session not connected. Check browser logs.".to_string();
            eprintln!("[browser_tools] {}", msg);
            sacp::Error::new(-32603, msg)
        })?;
        return exec_browser_tool_inner::<T>(session, params);
    }

    let session = guard.as_ref().unwrap();
    exec_browser_tool_inner::<T>(session, params)
}

/// Inner execution logic (session is guaranteed connected).
pub fn exec_browser_tool_inner<T: Tool + Default>(
    session: &BrowserSession,
    params: T::Params,
) -> Result<String, sacp::Error> {
    let tool = T::default();
    let tool_name = tool.name().to_string();

    // For index-based tools, inject the cached snapshot DOM so element
    // indices match what the AI saw in the last snapshot.
    let mut context = if INDEX_BASED_TOOLS.contains(&tool_name.as_str()) {
        if let Ok(cache) = LAST_SNAPSHOT_DOM.lock() {
            if let Some(cached_dom) = cache.as_ref() {
                ToolContext::with_dom(session, cached_dom.clone())
            } else {
                ToolContext::new(session)
            }
        } else {
            ToolContext::new(session)
        }
    } else {
        ToolContext::new(session)
    };

    let result = tool.execute_typed(params, &mut context).map_err(|e| {
        let msg = format!("Browser tool execution failed: {}", e);
        eprintln!("[browser_tools] {}", msg);
        sacp::Error::new(-32603, msg)
    })?;

    // After snapshot or navigate, cache the DOM for subsequent index-based calls
    if tool_name == "snapshot" || tool_name == "navigate" {
        if let Some(dom) = context.dom_tree {
            if let Ok(mut cache) = LAST_SNAPSHOT_DOM.lock() {
                *cache = Some(dom);
            }
        }
    }

    if result.success {
        if let Some(data) = result.data {
            Ok(serde_json::to_string_pretty(&data).unwrap_or_else(|_| data.to_string()))
        } else {
            Ok("Success".to_string())
        }
    } else {
        let msg = format!(
            "Browser tool error: {}",
            result.error.unwrap_or_else(|| "Unknown error".to_string())
        );
        eprintln!("[browser_tools] {}", msg);
        Err(sacp::Error::new(-32603, msg))
    }
}

/// Macro to register all browser-use tools onto an McpServer builder.
/// Usage: `let builder = register_browser_tools!(builder, session_handle, browser_mgr, settings_store);`
#[macro_export]
macro_rules! register_browser_tools {
    ($builder:expr, $session:expr, $browser_mgr:expr, $settings_store:expr) => {{
        use action_browser_rs::tools::{self, Tool};
        use $crate::browser_tools::exec_browser_tool;

        macro_rules! bt {
            ($b:expr, $sess:expr, $bmgr:expr, $sstore:expr, $name:literal, $desc:literal, $tool_type:ty) => {{
                let s = $sess.clone();
                let bm = $bmgr.clone();
                let ss = $sstore.clone();
                $b.tool_fn(
                    $name,
                    $desc,
                    {
                        let s = s.clone();
                        let bm = bm.clone();
                        let ss = ss.clone();
                        async move |params: <$tool_type as Tool>::Params, _cx| {
                            exec_browser_tool::<$tool_type>(&s, params, &bm, &ss)
                        }
                    },
                    sacp::tool_fn!(),
                )
            }};
        }

        let b = $builder;
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_navigate",
            "Navigate to a URL. Returns an indexed page snapshot for AI interaction.",
            tools::navigate::NavigateTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_go_back",
            "Go back in browser history",
            tools::go_back::GoBackTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_go_forward",
            "Go forward in browser history",
            tools::go_forward::GoForwardTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_snapshot",
            "Get a snapshot of the current page with indexed interactive elements",
            tools::snapshot::SnapshotTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_screenshot",
            "Capture a PNG screenshot of the current page",
            tools::screenshot::ScreenshotTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_get_markdown",
            "Convert current page content to markdown",
            tools::markdown::GetMarkdownTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_evaluate",
            "Execute JavaScript code in the browser and return results",
            tools::evaluate::EvaluateTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_click",
            "Click an element by CSS selector or index (from browser_snapshot)",
            tools::click::ClickTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_hover",
            "Hover over an element by CSS selector or index",
            tools::hover::HoverTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_select",
            "Select an option in a dropdown element",
            tools::select::SelectTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_input_fill",
            "Type text into an input field by CSS selector or index",
            tools::input::InputTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_press_key",
            "Press a keyboard key (Enter, Tab, Escape, etc.)",
            tools::press_key::PressKeyTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_scroll",
            "Scroll the page up/down by a specified amount",
            tools::scroll::ScrollTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_wait",
            "Wait for an element to appear on the page (CSS selector)",
            tools::wait::WaitTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_new_tab",
            "Open a new browser tab and navigate to a URL",
            tools::new_tab::NewTabTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_tab_list",
            "List all open browser tabs with titles and URLs",
            tools::tab_list::TabListTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_switch_tab",
            "Switch to a specific browser tab by index",
            tools::switch_tab::SwitchTabTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_close_tab",
            "Close the current browser tab",
            tools::close_tab::CloseTabTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_close",
            "Close the browser session when task is complete",
            tools::close::CloseTool);
        // Extended CDP tools
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_console_messages",
            "Get browser console messages (log, warn, error). Auto-starts collector on first call.",
            tools::console::ConsoleTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_network_requests",
            "Get network requests via Performance API. Supports URL filtering.",
            tools::network::NetworkTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_handle_dialog",
            "Accept or dismiss a JavaScript dialog (alert, confirm, prompt)",
            tools::dialog::DialogTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_drag",
            "Drag an element to another position using selectors or coordinates",
            tools::drag::DragTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_file_upload",
            "Upload files to a file input element",
            tools::file_upload::FileUploadTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_pdf",
            "Generate a PDF from the current page (A4, Letter, etc.)",
            tools::pdf::PdfTool);
        let b = bt!(b, $session, $browser_mgr, $settings_store, "browser_resize",
            "Resize the browser viewport for responsive testing or mobile emulation",
            tools::resize::ResizeTool);
        b
    }};
}
