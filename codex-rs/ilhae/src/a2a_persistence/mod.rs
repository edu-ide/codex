//! A2A Persistence Proxy — modularized into sub-modules.
//!
//! - `forwarding_executor`: `ForwardingExecutor` + `AgentExecutor` impl
//! - `schedule_store`: `PersistenceScheduleStore` dual-write
//! - `router`: A2A reverse proxy router + delegation tap

pub mod delegation_tracker;
pub mod events;
pub mod forwarding_executor;
pub mod router;
pub mod schedule_store;

// Re-export main types for backward compatibility
pub use forwarding_executor::DelegationResponseCache;
pub use forwarding_executor::ForwardingExecutor;
pub use forwarding_executor::delegation_cache_read;
pub use forwarding_executor::delegation_cache_write;
pub use router::RoutingMap;
pub use router::build_proxy_router;
pub use router::build_routing_map;
pub use router::build_routing_table;
pub use router::update_routing_map;
pub use schedule_store::PersistenceScheduleStore;
