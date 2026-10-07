//! Restrict the final MCP connection set, including plugin and extension registrations.
use std::collections::HashMap;
use std::collections::HashSet;

fn parse_selection(raw: Option<&str>) -> Result<Option<HashSet<String>>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let names: Vec<String> = serde_json::from_str(raw)
        .map_err(|_| "ILHAE_AGENT_MCP_SERVERS must be a JSON array of server names".to_owned())?;
    if names
        .iter()
        .any(|name| name.is_empty() || name.trim() != name)
    {
        return Err("ILHAE_AGENT_MCP_SERVERS contains an empty or padded server name".into());
    }
    Ok(Some(names.into_iter().collect()))
}

fn restrict_servers<T>(
    mut servers: HashMap<String, T>,
    selection: Result<Option<HashSet<String>>, String>,
) -> HashMap<String, T> {
    match selection {
        Ok(None) => {}
        Ok(Some(names)) => servers.retain(|name, _| names.contains(name)),
        Err(message) => {
            tracing::error!("{message}");
            servers.clear();
        }
    }
    servers
}

pub(crate) fn filter_servers<T>(servers: HashMap<String, T>) -> HashMap<String, T> {
    let selection = match std::env::var("ILHAE_AGENT_MCP_SERVERS") {
        Ok(raw) => parse_selection(Some(&raw)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("ILHAE_AGENT_MCP_SERVERS must be UTF-8 JSON".into())
        }
    };
    restrict_servers(servers, selection)
}

#[cfg(test)]
#[path = "agent_profile_tests.rs"]
mod tests;
