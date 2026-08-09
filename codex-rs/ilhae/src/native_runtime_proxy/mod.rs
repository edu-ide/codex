mod client;
mod protocol;
mod server;

pub use client::ensure_remote_native_runtime;
pub use client::stop_remote_native_runtime;
pub use protocol::EnsureNativeRuntimeRequest;
pub use protocol::EnsureNativeRuntimeResponse;
pub use protocol::StopNativeRuntimeRequest;
pub use protocol::StopNativeRuntimeResponse;
pub use server::run_native_runtime_proxy;
