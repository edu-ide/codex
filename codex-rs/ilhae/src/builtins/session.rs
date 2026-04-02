#[macro_export]
macro_rules! register_session_tools {
    ($builder:expr, $brain_service:expr, $bt_settings:expr) => {{
        use $crate::{EmptyInput, SessionIdInput, SessionRenameInput, SessionSearchInput};

        $builder
            .tool_fn(
                "session_list",
                "List all chat sessions.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |_input: EmptyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "session_list");
                        match brain.session_list(None) {
                            Ok(sessions) => {
                                let text = serde_json::to_string_pretty(&sessions)
                                    .unwrap_or("[]".to_string());
                                Ok::<String, sacp::Error>(text)
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "session_load",
                "Load all messages for a session by ID.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: SessionIdInput, _cx| {
                        $crate::check_tool_enabled!(bts, "session_load");
                        match brain.session_load(&input.session_id) {
                            Ok(messages) => {
                                let text = serde_json::to_string_pretty(&messages)
                                    .unwrap_or("[]".to_string());
                                Ok::<String, sacp::Error>(text)
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "session_search",
                "Search sessions by title/message text.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: SessionSearchInput, _cx| {
                        $crate::check_tool_enabled!(bts, "session_search");
                        match brain.session_search(&input.query, input.limit) {
                            Ok(sessions) => {
                                let text = serde_json::to_string_pretty(&sessions)
                                    .unwrap_or("[]".to_string());
                                Ok::<String, sacp::Error>(text)
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "session_delete",
                "Delete a session and all its messages.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: SessionIdInput, _cx| {
                        $crate::check_tool_enabled!(bts, "session_delete");
                        match brain.session_delete(&input.session_id) {
                            Ok(()) => Ok::<String, sacp::Error>(format!(
                                "✅ Deleted session {}",
                                input.session_id
                            )),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "session_rename",
                "Rename a session.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: SessionRenameInput, _cx| {
                        $crate::check_tool_enabled!(bts, "session_rename");
                        match brain.session_rename(&input.session_id, &input.title) {
                            Ok(()) => Ok::<String, sacp::Error>(format!(
                                "✅ Renamed session {} → {}",
                                input.session_id, input.title
                            )),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
    }};
}
