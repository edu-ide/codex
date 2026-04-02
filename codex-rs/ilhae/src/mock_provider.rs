//! Mock Provider for Headless Testing
//!
//! When `ILHAE_MOCK=true` is set, the proxy uses pre-recorded responses instead
//! of calling the actual LLM. This enables:
//!   - **Fast tests**: No network calls, sub-second responses
//!   - **Deterministic tests**: Same input → same output every time
//!   - **Offline testing**: No API key or internet needed
//!
//! Fixture files are loaded from `tests/fixtures/` directory.
//! Each fixture is a JSON array of prompt→response pairs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static MOCK_ENABLED: AtomicBool = AtomicBool::new(false);
static MOCK_TURN: AtomicUsize = AtomicUsize::new(0);

fn mock_fixture() -> &'static Mutex<Vec<MockTurn>> {
    static FIXTURE: OnceLock<Mutex<Vec<MockTurn>>> = OnceLock::new();
    FIXTURE.get_or_init(|| Mutex::new(Vec::new()))
}

/// A single mock turn: what the agent "responds" when prompted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockTurn {
    /// Optional: match pattern for the incoming prompt (if not set, matches any)
    #[serde(default)]
    pub prompt_contains: Option<String>,
    /// The text response the mock LLM returns
    pub response_text: String,
    /// Tool calls to simulate (each becomes a tool_call in the response)
    #[serde(default)]
    pub tool_calls: Vec<MockToolCall>,
    /// Delay in ms to simulate thinking time (default: 0)
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockToolCall {
    pub tool_name: String,
    pub raw_input: Value,
    pub result_text: String,
}

/// The mock response returned to the caller (not PromptResponse — that's
/// constructed by the caller using this data).
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub text: String,
    pub tool_calls: Vec<MockToolCall>,
}

/// Initialize mock mode from env var or explicit flag.
pub fn init_mock_mode(force: bool) {
    let enabled = force || std::env::var("ILHAE_MOCK").map_or(false, |v| v == "true" || v == "1");
    MOCK_ENABLED.store(enabled, Ordering::SeqCst);

    if enabled {
        tracing::info!("[MockAgent] Mock mode ENABLED");
        let fixture_path = std::env::var("ILHAE_MOCK_FIXTURE")
            .unwrap_or_else(|_| "tests/fixtures/default_mock.json".to_string());
        if let Err(e) = load_fixture(&fixture_path) {
            tracing::warn!(
                "[MockAgent] No fixture loaded from {}: {}. Using built-in defaults.",
                fixture_path,
                e
            );
            load_builtin_defaults();
        }
    }
}

/// Check if mock mode is active.
pub fn is_mock_mode() -> bool {
    MOCK_ENABLED.load(Ordering::SeqCst)
}

/// Load a fixture file (JSON array of MockTurn).
pub fn load_fixture(path: &str) -> Result<(), String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let turns: Vec<MockTurn> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    tracing::info!(
        "[MockAgent] Loaded {} mock turns from {}",
        turns.len(),
        path
    );
    let mut fixture = mock_fixture().lock().unwrap();
    *fixture = turns;
    MOCK_TURN.store(0, Ordering::SeqCst);
    Ok(())
}

/// Load built-in default mock fixture from embedded JSON.
/// The fixture file lives at `tests/fixtures/default_mock.json` and is embedded
/// at compile time for zero-dependency fallback.
fn load_builtin_defaults() {
    const DEFAULT_JSON: &str = include_str!("../tests/fixtures/default_mock.json");
    let defaults: Vec<MockTurn> =
        serde_json::from_str(DEFAULT_JSON).expect("Built-in default_mock.json is invalid");

    let count = defaults.len();
    let mut fixture = mock_fixture().lock().unwrap();
    *fixture = defaults;
    MOCK_TURN.store(0, Ordering::SeqCst);
    tracing::info!("[MockAgent] Loaded {} built-in mock turns", count);
}

/// Get the next mock response. Returns None if mock mode is off or all turns exhausted.
pub fn get_mock_response(_prompt_text: &str) -> Option<MockResponse> {
    if !is_mock_mode() {
        return None;
    }

    let fixture = mock_fixture().lock().unwrap();
    let turn_idx = MOCK_TURN.fetch_add(1, Ordering::SeqCst);

    if turn_idx >= fixture.len() {
        tracing::info!("[MockAgent] All {} mock turns exhausted", fixture.len());
        return Some(MockResponse {
            text: "Mock 응답이 소진되었습니다. 추가 fixture를 로드하세요.".to_string(),
            tool_calls: Vec::new(),
        });
    }

    let turn = &fixture[turn_idx];
    tracing::info!(
        "[MockAgent] Serving mock turn {} (text: {} chars, {} tool calls)",
        turn_idx + 1,
        turn.response_text.len(),
        turn.tool_calls.len()
    );

    if turn.delay_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(turn.delay_ms));
    }

    Some(MockResponse {
        text: turn.response_text.clone(),
        tool_calls: turn.tool_calls.clone(),
    })
}

/// Reset mock state (for multiple test runs).
pub fn reset_mock() {
    MOCK_TURN.store(0, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_defaults() {
        init_mock_mode(true);
        assert!(is_mock_mode());

        // Turn 1
        let resp = get_mock_response("안녕").unwrap();
        assert!(resp.text.contains("작업을 시작"));
        assert_eq!(resp.tool_calls.len(), 2);
        assert_eq!(resp.tool_calls[0].tool_name, "artifact_save");

        // Turn 2
        let resp = get_mock_response("계속").unwrap();
        assert_eq!(resp.tool_calls.len(), 2);
        assert_eq!(resp.tool_calls[1].tool_name, "artifact_edit");

        // Turn 3
        let resp = get_mock_response("저장").unwrap();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].tool_name, "memory_write");

        // Turn 4 (exhausted)
        let resp = get_mock_response("추가").unwrap();
        assert!(resp.text.contains("소진"));
        assert!(resp.tool_calls.is_empty());

        reset_mock();
    }
}
