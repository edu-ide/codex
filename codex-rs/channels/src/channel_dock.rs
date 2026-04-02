//! Channel Dock — extensible channel registry for ilhae-proxy.
//!
//! Uses `strum` for enum string conversion, `inventory` for decentralized
//! auto-registration, `enum_dispatch` for vtable-free trait dispatch,
//! and `async-trait` for the async channel handler trait.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strum_macros::{Display, EnumIter, EnumString, IntoStaticStr};
use tokio::sync::mpsc;

use crate::relay_server::{RelayCommandWithClient, RelayEvent};

// ─── Channel ID ──────────────────────────────────────────────────────────────

/// All supported channel identifiers.
/// `strum` derives give us free `FromStr`, `Display`, and iteration.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Display,
    EnumString,
    EnumIter,
    IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ChannelId {
    Telegram,
    Kakao,
    Discord,
    Slack,
    Whatsapp,
    Line,
    Signal,
    Irc,
}

// ─── Capabilities ────────────────────────────────────────────────────────────

/// What a channel can do.
#[derive(Debug, Clone)]
pub struct ChannelCapabilities {
    /// Supports /command style native commands
    pub native_commands: bool,
    /// Supports group/multi-user chats
    pub group_chat: bool,
    /// Supports message threading / replies
    pub threading: bool,
    /// Supports file/image attachments
    pub file_attachments: bool,
    /// Supports emoji reactions
    pub reactions: bool,
    /// Supports inline keyboard / buttons
    pub inline_buttons: bool,
}

// ─── Outbound config ─────────────────────────────────────────────────────────

/// Outbound message constraints for a channel.
#[derive(Debug, Clone)]
pub struct OutboundConfig {
    /// Max characters per message chunk
    pub text_chunk_limit: usize,
}

// ─── Channel Dock ────────────────────────────────────────────────────────────

/// Metadata + behavior declaration for a chat channel.
/// Does NOT contain connection logic — just the "what" and "how much".
#[derive(Debug, Clone)]
pub struct ChannelDock {
    pub id: ChannelId,
    pub label: &'static str,
    pub emoji: &'static str,
    pub description: &'static str,
    pub capabilities: ChannelCapabilities,
    pub outbound: OutboundConfig,
    /// Whether a client implementation exists in the proxy binary
    pub implemented: bool,
}

// Allow `inventory` to collect ChannelDock submissions from anywhere.
inventory::collect!(ChannelDock);

// ─── Async Channel Handler Trait ─────────────────────────────────────────────

/// Trait for channel client implementations.
/// Each channel (Telegram, Discord, etc.) implements this to handle
/// lifecycle events. Uses `async-trait` for async methods.
#[async_trait]
pub trait ChannelHandler: Send + Sync {
    /// Human-readable name of the channel
    fn channel_id(&self) -> ChannelId;

    /// Start the channel client. Called when the channel is enabled.
    async fn start(
        &self,
        command_tx: mpsc::Sender<RelayCommandWithClient>,
        relay_tx: mpsc::Sender<RelayEvent>,
    ) -> anyhow::Result<()>;

    /// Stop the channel client gracefully.
    async fn stop(&self) -> anyhow::Result<()>;

    /// Whether the channel is currently connected and running.
    fn is_running(&self) -> bool;
}

// ─── Static ChannelDock Submissions ──────────────────────────────────────────
//
// Each channel declares its dock via `inventory::submit!`.
// This means adding a new channel only requires a submit! call in its module.

inventory::submit! {
    ChannelDock {
        id: ChannelId::Telegram,
        label: "Telegram",
        emoji: "🤖",
        description: "Telegram Bot API via teloxide",
        capabilities: ChannelCapabilities {
            native_commands: true,
            group_chat: true,
            threading: true,
            file_attachments: true,
            reactions: true,
            inline_buttons: true,
        },
        outbound: OutboundConfig { text_chunk_limit: 4000 },
        implemented: true,
    }
}

inventory::submit! {
    ChannelDock {
        id: ChannelId::Kakao,
        label: "KakaoTalk",
        emoji: "💬",
        description: "카카오톡 채널 API",
        capabilities: ChannelCapabilities {
            native_commands: false,
            group_chat: true,
            threading: false,
            file_attachments: true,
            reactions: false,
            inline_buttons: true,
        },
        outbound: OutboundConfig { text_chunk_limit: 1000 },
        implemented: false,
    }
}

inventory::submit! {
    ChannelDock {
        id: ChannelId::Discord,
        label: "Discord",
        emoji: "🎮",
        description: "Discord Bot via serenity/poise",
        capabilities: ChannelCapabilities {
            native_commands: true,
            group_chat: true,
            threading: true,
            file_attachments: true,
            reactions: true,
            inline_buttons: true,
        },
        outbound: OutboundConfig { text_chunk_limit: 2000 },
        implemented: false,
    }
}

inventory::submit! {
    ChannelDock {
        id: ChannelId::Slack,
        label: "Slack",
        emoji: "💼",
        description: "Slack Bot via Web/Events API",
        capabilities: ChannelCapabilities {
            native_commands: true,
            group_chat: true,
            threading: true,
            file_attachments: true,
            reactions: true,
            inline_buttons: true,
        },
        outbound: OutboundConfig { text_chunk_limit: 3000 },
        implemented: false,
    }
}

inventory::submit! {
    ChannelDock {
        id: ChannelId::Whatsapp,
        label: "WhatsApp",
        emoji: "📱",
        description: "WhatsApp Cloud API",
        capabilities: ChannelCapabilities {
            native_commands: false,
            group_chat: true,
            threading: false,
            file_attachments: true,
            reactions: true,
            inline_buttons: true,
        },
        outbound: OutboundConfig { text_chunk_limit: 4096 },
        implemented: false,
    }
}

inventory::submit! {
    ChannelDock {
        id: ChannelId::Line,
        label: "LINE",
        emoji: "🟢",
        description: "LINE Messaging API",
        capabilities: ChannelCapabilities {
            native_commands: false,
            group_chat: true,
            threading: false,
            file_attachments: true,
            reactions: false,
            inline_buttons: true,
        },
        outbound: OutboundConfig { text_chunk_limit: 5000 },
        implemented: false,
    }
}

inventory::submit! {
    ChannelDock {
        id: ChannelId::Signal,
        label: "Signal",
        emoji: "🔒",
        description: "Signal Protocol (presage)",
        capabilities: ChannelCapabilities {
            native_commands: false,
            group_chat: true,
            threading: false,
            file_attachments: true,
            reactions: true,
            inline_buttons: false,
        },
        outbound: OutboundConfig { text_chunk_limit: 6000 },
        implemented: false,
    }
}

inventory::submit! {
    ChannelDock {
        id: ChannelId::Irc,
        label: "IRC",
        emoji: "📟",
        description: "IRC Protocol",
        capabilities: ChannelCapabilities {
            native_commands: true,
            group_chat: true,
            threading: false,
            file_attachments: false,
            reactions: false,
            inline_buttons: false,
        },
        outbound: OutboundConfig { text_chunk_limit: 512 },
        implemented: false,
    }
}

// ─── Lookup helpers ──────────────────────────────────────────────────────────

/// Look up a channel dock by its string ID (uses strum's EnumString).
pub fn get_dock(id: &str) -> Option<&'static ChannelDock> {
    use std::str::FromStr;
    let channel_id = ChannelId::from_str(id).ok()?;
    inventory::iter::<ChannelDock>
        .into_iter()
        .find(|d| d.id == channel_id)
}

/// List all registered channel docks (collected by `inventory`).
pub fn list_docks() -> Vec<&'static ChannelDock> {
    inventory::iter::<ChannelDock>.into_iter().collect()
}

/// List only channels that have a client implementation.
pub fn list_implemented() -> Vec<&'static ChannelDock> {
    inventory::iter::<ChannelDock>
        .into_iter()
        .filter(|d| d.implemented)
        .collect()
}

/// Serialize dock list to JSON for desktop/mobile UI consumption.
pub fn docks_to_json() -> serde_json::Value {
    let docks: Vec<serde_json::Value> = inventory::iter::<ChannelDock>
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "id": d.id.to_string(),
                "label": d.label,
                "emoji": d.emoji,
                "description": d.description,
                "implemented": d.implemented,
                "capabilities": {
                    "native_commands": d.capabilities.native_commands,
                    "group_chat": d.capabilities.group_chat,
                    "threading": d.capabilities.threading,
                    "file_attachments": d.capabilities.file_attachments,
                    "reactions": d.capabilities.reactions,
                    "inline_buttons": d.capabilities.inline_buttons,
                },
                "outbound": {
                    "text_chunk_limit": d.outbound.text_chunk_limit,
                },
            })
        })
        .collect();
    serde_json::json!(docks)
}

// ─── Channel Registry (runtime) ──────────────────────────────────────────────

/// Runtime registry that holds active ChannelHandler instances.
/// Uses `enum_dispatch`-compatible pattern for efficient dispatch.
pub struct ChannelRegistry {
    pub handlers: Vec<Arc<dyn ChannelHandler>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Register a channel handler.
    pub fn register(&mut self, handler: Arc<dyn ChannelHandler>) {
        self.handlers.push(handler);
    }

    /// Get handler by channel ID.
    pub fn get(&self, id: ChannelId) -> Option<Arc<dyn ChannelHandler>> {
        self.handlers.iter().find(|h| h.channel_id() == id).cloned()
    }

    /// Start all registered and enabled channels.
    pub async fn start_all(
        &self,
        command_tx: mpsc::Sender<RelayCommandWithClient>,
        relay_tx: mpsc::Sender<RelayEvent>,
    ) {
        for handler in &self.handlers {
            let dock = get_dock(&handler.channel_id().to_string());
            let label = dock.map(|d| d.label).unwrap_or("unknown");
            match handler.start(command_tx.clone(), relay_tx.clone()).await {
                Ok(()) => tracing::info!("[{}] Channel started", label),
                Err(e) => tracing::warn!("[{}] Channel start failed: {}", label, e),
            }
        }
    }

    /// List running channels.
    pub fn running(&self) -> Vec<ChannelId> {
        self.handlers
            .iter()
            .filter(|h| h.is_running())
            .map(|h| h.channel_id())
            .collect()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use strum::IntoEnumIterator;

    #[test]
    fn strum_display_and_parse() {
        assert_eq!(ChannelId::Telegram.to_string(), "telegram");
        assert_eq!(ChannelId::from_str("discord"), Ok(ChannelId::Discord));
        assert_eq!(ChannelId::from_str("kakao"), Ok(ChannelId::Kakao));
        assert!(ChannelId::from_str("unknown").is_err());
    }

    #[test]
    fn strum_enum_iter() {
        let all: Vec<ChannelId> = ChannelId::iter().collect();
        assert_eq!(all.len(), 8);
        assert!(all.contains(&ChannelId::Telegram));
        assert!(all.contains(&ChannelId::Irc));
    }

    #[test]
    fn inventory_collects_all_docks() {
        let docks = list_docks();
        assert!(
            docks.len() >= 8,
            "expected at least 8 docks, got {}",
            docks.len()
        );
    }

    #[test]
    fn inventory_get_dock() {
        let dock = get_dock("telegram").expect("telegram dock not found");
        assert_eq!(dock.id, ChannelId::Telegram);
        assert!(dock.implemented);
        assert_eq!(dock.outbound.text_chunk_limit, 4000);
    }

    #[test]
    fn list_implemented_only_has_telegram() {
        let impls = list_implemented();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].id, ChannelId::Telegram);
    }

    #[test]
    fn docks_to_json_serializes() {
        let json = docks_to_json();
        let arr = json.as_array().expect("should be array");
        assert!(arr.len() >= 8);
        // Find telegram entry
        let tg = arr
            .iter()
            .find(|v| v["id"] == "telegram")
            .expect("telegram not in json");
        assert_eq!(tg["emoji"], "🤖");
    }

    #[test]
    fn channel_registry_basic() {
        let registry = ChannelRegistry::new();
        assert_eq!(registry.running().len(), 0);
        assert!(registry.get(ChannelId::Telegram).is_none());
    }
}
