use std::collections::HashMap;

use rusqlite::Result as SqlResult;

use crate::session_store::{SessionInfo, SessionMessage, SessionStore};

pub type SessionMap = HashMap<String, SessionInfo>;
pub type MessagesBySession = HashMap<String, Vec<SessionMessage>>;

pub fn load_timeline_inputs(
    store: &SessionStore,
    session_id: &str,
) -> SqlResult<(SessionMap, Vec<SessionMessage>, MessagesBySession)> {
    let session_map = store
        .list_sessions()?
        .into_iter()
        .map(|session| (session.id.clone(), session))
        .collect::<HashMap<_, _>>();

    let source_messages = store.load_session_messages(session_id)?;
    let messages_by_session = source_messages.iter().fold(
        HashMap::<String, Vec<SessionMessage>>::new(),
        |mut acc, message| {
            acc.entry(message.session_id.clone())
                .or_default()
                .push(message.clone());
            acc
        },
    );

    Ok((session_map, source_messages, messages_by_session))
}
