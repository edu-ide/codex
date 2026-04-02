//! Channel bot integrations for Ilhae.
//!
//! Each channel is behind a feature flag:
//! - `telegram` (default)
//! - `discord` (default)
//! - `slack` (default)

#[cfg(feature = "telegram")]
pub mod telegram_client;

#[cfg(feature = "discord")]
pub mod discord_client;

#[cfg(feature = "slack")]
pub mod slack_client;

// Always-available (lightweight)
pub mod kakao_client;
pub mod line_client;
pub mod whatsapp_client;

pub mod approval_manager;
pub mod channel_bots;
pub mod channel_dock;
