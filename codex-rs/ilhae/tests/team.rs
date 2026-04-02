//! Team Mode Tests — A2A delegation, brain sessions, persistence

#[path = "common/mod.rs"]
mod common;

#[path = "team/a2a_server.rs"]
mod a2a_server;

#[path = "team/a2a_server_hang.rs"]
mod a2a_server_hang;

#[path = "team/a2a_concurrent.rs"]
mod a2a_concurrent;

#[path = "team/delegation_mock.rs"]
mod delegation_mock;

#[path = "team/brain_session.rs"]
mod brain_session;

#[path = "team/proxy_e2e.rs"]
mod proxy_e2e;

#[path = "team/live_a2a.rs"]
mod live_a2a;

#[path = "team/live_a2a_proxy.rs"]
mod live_a2a_proxy;

#[path = "team/live_a2a_e2e.rs"]
mod live_a2a_e2e;

#[path = "team/delegation_real.rs"]
mod delegation_real;

#[path = "team/persistence.rs"]
mod persistence;

#[path = "team/mcp_forwarding.rs"]
mod mcp_forwarding;

#[path = "team/auto_mode.rs"]
mod auto_mode;

#[path = "team/a2a_protocol.rs"]
mod a2a_protocol;

#[path = "team/a2a_protocol_real.rs"]
mod a2a_protocol_real;

#[path = "team/team_headless_e2e.rs"]
mod team_headless_e2e;
