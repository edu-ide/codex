#[macro_export]
macro_rules! register_admin_notification_handlers {
    ($builder:expr, $state:expr) => {{
        let s = $state.clone();
        $builder
            // ═══ Notification List ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let notif_store = s.infra.notification_store.clone();
                    async move |req: NotificationListRequest,
                                responder: Responder<NotificationListResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!(
                            "ilhae/notification_list RPC offset={} limit={}",
                            req.offset, req.limit
                        );
                        match notif_store.list(req.offset, req.limit) {
                            Ok(notifications) => {
                                responder.respond(NotificationListResponse { notifications })
                            }
                            Err(e) => {
                                warn!("notification_list error: {}", e);
                                responder.respond(NotificationListResponse {
                                    notifications: vec![],
                                })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Notification Stats ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let notif_store = s.infra.notification_store.clone();
                    async move |_req: NotificationStatsRequest,
                                responder: Responder<NotificationStatsResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/notification_stats RPC");
                        match notif_store.stats() {
                            Ok(stats) => responder.respond(NotificationStatsResponse { stats }),
                            Err(e) => {
                                warn!("notification_stats error: {}", e);
                                responder.respond(NotificationStatsResponse {
                                    stats: notification_store::NotificationStats {
                                        total: 0,
                                        unread: 0,
                                    },
                                })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Notification Mark Read ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let notif_store = s.infra.notification_store.clone();
                    async move |req: NotificationMarkReadRequest,
                                responder: Responder<NotificationMarkReadResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/notification_mark_read RPC id={}", req.id);
                        match notif_store.mark_read(&req.id) {
                            Ok(ok) => responder.respond(NotificationMarkReadResponse { ok }),
                            Err(e) => {
                                warn!("notification_mark_read error: {}", e);
                                responder.respond(NotificationMarkReadResponse { ok: false })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
            // ═══ Notification Mark All Read ═══
            .on_receive_request_from(
                sacp::Client,
                {
                    let notif_store = s.infra.notification_store.clone();
                    async move |_req: NotificationMarkAllReadRequest,
                                responder: Responder<NotificationMarkAllReadResponse>,
                                _cx: ConnectionTo<Conductor>| {
                        info!("ilhae/notification_mark_all_read RPC");
                        match notif_store.mark_all_read() {
                            Ok(count) => {
                                responder.respond(NotificationMarkAllReadResponse { count })
                            }
                            Err(e) => {
                                warn!("notification_mark_all_read error: {}", e);
                                responder.respond(NotificationMarkAllReadResponse { count: 0 })
                            }
                        }
                    }
                },
                sacp::on_receive_request!(),
            )
    }};
}
