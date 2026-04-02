fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .expect("Failed to build tokio runtime");
    runtime.block_on(codex_ilhae::run_ilhae_proxy())
}
