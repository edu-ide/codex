#[macro_export]
macro_rules! register_admin_memory_handlers {
    ($builder:expr, $state:expr) => {{
        let s = $state.clone();
        $builder
            // ═══ Memory Search ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let brain = s.infra.brain.clone();
                    async move |req: MemorySearchRequest,
                                responder: Responder<MemorySearchResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/memory_search RPC query={}", req.query);
                        match brain.memory_search(&req.query, req.limit, None) {
                            Ok(chunks) => responder.respond(MemorySearchResponse { chunks }),
                            Err(e) => {
                                warn!("memory_search error: {}", e);
                                responder.respond(MemorySearchResponse { chunks: vec![] })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Memory List ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let brain = s.infra.brain.clone();
                    async move |req: MemoryListRequest,
                                responder: Responder<MemoryListResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/memory_list RPC offset={} limit={}",
                            req.offset, req.limit
                        );
                        match brain.memory_chunk_list(req.offset, req.limit) {
                            Ok(chunks) => responder.respond(MemoryListResponse { chunks }),
                            Err(e) => {
                                warn!("memory_list error: {}", e);
                                responder.respond(MemoryListResponse { chunks: vec![] })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Memory Store ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let brain = s.infra.brain.clone();
                    async move |req: MemoryStoreRequest,
                                responder: Responder<MemoryStoreResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/memory_store RPC");
                        match brain.memory_chunk_store(&req.text, &req.source) {
                            Ok(id) => responder.respond(MemoryStoreResponse { id }),
                            Err(e) => {
                                warn!("memory_store error: {}", e);
                                responder.respond(MemoryStoreResponse { id: -1 })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Memory Forget ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let brain = s.infra.brain.clone();
                    async move |req: MemoryForgetRequest,
                                responder: Responder<MemoryForgetResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/memory_forget RPC chunk_id={}", req.chunk_id);
                        match brain.memory_chunk_forget(req.chunk_id) {
                            Ok(ok) => responder.respond(MemoryForgetResponse { ok }),
                            Err(e) => {
                                warn!("memory_forget error: {}", e);
                                responder.respond(MemoryForgetResponse { ok: false })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Memory Stats ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let mem_store = s.infra.brain.memory().clone();
                    async move |_req: MemoryStatsRequest,
                                responder: Responder<MemoryStatsResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/memory_stats RPC");
                        match mem_store.stats() {
                            Ok(stats) => responder.respond(MemoryStatsResponse { stats }),
                            Err(e) => {
                                warn!("memory_stats error: {}", e);
                                responder.respond(MemoryStatsResponse {
                                    stats: memory_store::MemoryStats {
                                        total_chunks: 0,
                                        total_files: 0,
                                        db_size_bytes: 0,
                                        last_indexed_at: None,
                                    },
                                })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Memory Pin ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let brain = s.infra.brain.clone();
                    async move |req: MemoryPinRequest,
                                responder: Responder<MemoryPinResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/memory_pin RPC chunk_id={} pinned={}",
                            req.chunk_id, req.pinned
                        );
                        match brain.memory_chunk_set_pinned(req.chunk_id, req.pinned) {
                            Ok(ok) => responder.respond(MemoryPinResponse { ok }),
                            Err(e) => {
                                warn!("memory_pin error: {}", e);
                                responder.respond(MemoryPinResponse { ok: false })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Context Read ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let brain = s.infra.brain.clone();
                    async move |_req: ReadContextRequest,
                                responder: Responder<ReadContextResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/read_context RPC");
                        let memory_text = brain.memory_pinned_text();
                        responder.respond(ReadContextResponse {
                            system: crate::memory_provider::read_global("system"),
                            identity: crate::memory_provider::read_global("identity"),
                            soul: crate::memory_provider::read_global("soul"),
                            user: crate::memory_provider::read_global("user"),
                            memory: memory_text,
                        })
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Context Write ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let brain = s.infra.brain.clone();
                    async move |req: WriteContextRequest,
                                responder: Responder<WriteContextResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/write_context RPC file={}", req.file);
                        if req.file == "memory" {
                            match brain.memory_chunk_store_pinned(&req.content, "ui", true) {
                                Ok(_) => responder.respond(WriteContextResponse { ok: true }),
                                Err(e) => responder
                                    .respond_with_error(sacp::util::internal_error(e.to_string())),
                            }
                        } else {
                            match crate::memory_provider::write_section(&req.file, &req.content) {
                                Ok(_) => responder.respond(WriteContextResponse { ok: true }),
                                Err(e) => {
                                    responder.respond_with_error(sacp::util::internal_error(e))
                                }
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
    }};
}
