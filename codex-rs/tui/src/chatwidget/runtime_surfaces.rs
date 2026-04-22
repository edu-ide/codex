use crate::history_cell;
use crate::history_cell::LoopLifecycleCell;
use crate::history_cell::WebSearchCell;
use codex_protocol::protocol::LoopLifecycleCompletedEvent;
use codex_protocol::protocol::LoopLifecycleProgressEvent;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_protocol::protocol::WebSearchEndEvent;

use super::ChatWidget;

pub(super) fn web_search_action_to_core(
    action: codex_app_server_protocol::WebSearchAction,
) -> codex_protocol::models::WebSearchAction {
    match action {
        codex_app_server_protocol::WebSearchAction::Search { query, queries } => {
            codex_protocol::models::WebSearchAction::Search { query, queries }
        }
        codex_app_server_protocol::WebSearchAction::OpenPage { url } => {
            codex_protocol::models::WebSearchAction::OpenPage { url }
        }
        codex_app_server_protocol::WebSearchAction::FindInPage { url, pattern } => {
            codex_protocol::models::WebSearchAction::FindInPage { url, pattern }
        }
        codex_app_server_protocol::WebSearchAction::Other => {
            codex_protocol::models::WebSearchAction::Other
        }
    }
}

impl ChatWidget {
    pub(super) fn on_web_search_begin(&mut self, ev: WebSearchBeginEvent) {
        self.flush_answer_stream_with_separator();
        self.flush_active_cell();
        self.active_cell = Some(Box::new(history_cell::new_active_web_search_call(
            ev.call_id,
            String::new(),
            self.config.animations,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(super) fn on_web_search_end(&mut self, ev: WebSearchEndEvent) {
        self.flush_answer_stream_with_separator();
        let WebSearchEndEvent {
            call_id,
            query,
            action,
        } = ev;
        let mut handled = false;
        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<WebSearchCell>())
            && cell.call_id() == call_id
        {
            cell.update(action.clone(), query.clone());
            cell.complete();
            self.bump_active_cell_revision();
            self.flush_active_cell();
            handled = true;
        }

        if !handled {
            self.add_to_history(history_cell::new_web_search_call(call_id, query, action));
        }
        self.had_work_activity = true;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_loop_lifecycle_begin(
        &mut self,
        item_id: String,
        kind: codex_protocol::protocol::LoopLifecycleKind,
        title: String,
        summary: String,
        detail: Option<String>,
        reason: Option<String>,
        counts: Option<std::collections::BTreeMap<String, i64>>,
        target_profile: Option<String>,
    ) {
        self.flush_answer_stream_with_separator();
        self.flush_active_cell();
        self.active_cell = Some(Box::new(history_cell::new_active_loop_lifecycle_call(
            item_id,
            kind,
            title,
            summary,
            detail,
            reason,
            counts,
            target_profile,
            self.config.animations,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(super) fn on_loop_lifecycle_progress(&mut self, ev: LoopLifecycleProgressEvent) {
        self.flush_answer_stream_with_separator();
        let LoopLifecycleProgressEvent {
            item_id,
            summary,
            detail,
            counts,
            ..
        } = ev;

        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<LoopLifecycleCell>())
            && cell.call_id() == item_id
        {
            cell.update_progress(summary, detail, counts);
            self.bump_active_cell_revision();
            self.request_redraw();
        }
    }

    pub(super) fn on_loop_lifecycle_end(&mut self, event: LoopLifecycleCompletedEvent) {
        self.flush_answer_stream_with_separator();
        let item = event.item;
        let final_status = item.status.clone();
        let mut handled = false;
        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<LoopLifecycleCell>())
            && cell.call_id() == item.id
        {
            cell.complete(
                item.summary.clone(),
                item.detail.clone(),
                item.reason.clone(),
                item.counts.clone(),
                item.error.clone(),
                item.duration_ms,
                item.target_profile.clone(),
                final_status.clone(),
            );
            self.bump_active_cell_revision();
            handled = true;
        }

        if !handled {
            self.add_to_history(history_cell::new_loop_lifecycle_call(
                item.id,
                item.kind,
                item.title,
                item.summary,
                item.detail,
                item.reason,
                item.counts,
                item.error,
                item.duration_ms,
                item.target_profile,
                final_status,
            ));
            self.request_redraw();
            return;
        }

        self.flush_active_cell();
        self.request_redraw();
    }
}
