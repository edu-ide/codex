//! MCP relay for the common browser service's runtime tool catalog.

use crate::browser_manager::BrowserManager;
use crate::settings_store::SettingsStore;
use browser_rmcp::ErrorData;
use browser_rmcp::ServerHandler;
use browser_rmcp::ServiceExt;
use browser_rmcp::model::CallToolRequestParams;
use browser_rmcp::model::CallToolResult;
use browser_rmcp::model::ListResourcesResult;
use browser_rmcp::model::ListToolsResult;
use browser_rmcp::model::PaginatedRequestParams;
use browser_rmcp::model::ReadResourceRequestParams;
use browser_rmcp::model::ReadResourceResult;
use browser_rmcp::model::ServerInfo;
use browser_rmcp::service::RequestContext;
use browser_rmcp::service::RoleServer;
use sacp::ByteStreams;
use sacp::Conductor;
use sacp::ConnectTo;
use sacp::DynConnectTo;
use sacp::mcp_server::McpConnectionTo;
use sacp::mcp_server::McpServerConnect;
use sacp::role::mcp;
use std::sync::Arc;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::compat::TokioAsyncWriteCompatExt;

#[derive(Clone)]
pub(crate) struct BrowserTools {
    pub manager: Arc<BrowserManager>,
    pub settings: Arc<SettingsStore>,
}

impl McpServerConnect<Conductor> for BrowserTools {
    fn name(&self) -> String {
        "browser".to_string()
    }

    fn connect(&self, _context: McpConnectionTo<Conductor>) -> DynConnectTo<mcp::Client> {
        DynConnectTo::new(self.clone())
    }
}

impl ConnectTo<mcp::Client> for BrowserTools {
    async fn connect_to(self, client: impl ConnectTo<mcp::Server>) -> Result<(), sacp::Error> {
        let (server_stream, client_stream) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_stream);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let run_client = <ByteStreams<_, _> as ConnectTo<mcp::Client>>::connect_to(
            ByteStreams::new(client_write.compat_write(), client_read.compat()),
            client,
        );
        let run_server = async move {
            self.serve((server_read, server_write))
                .await
                .map_err(sacp::Error::into_internal_error)?
                .waiting()
                .await
                .map(|_| ())
                .map_err(sacp::Error::into_internal_error)
        };
        tokio::try_join!(run_client, run_server)?;
        Ok(())
    }
}

impl ServerHandler for BrowserTools {
    fn get_info(&self) -> ServerInfo {
        ugot_browser_service::server_info()
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if !self.settings.get().browser.enabled {
            return Ok(ListToolsResult::default());
        }
        self.manager.list_tools(request).await
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ugot_browser_service::list_resources())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        ugot_browser_service::read_resource(request)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let settings = self.settings.get().browser;
        if !settings.enabled
            && !matches!(
                request.name.as_ref(),
                "browser_service_stop" | "browser_service_status"
            )
        {
            return Err(ErrorData::invalid_request(
                "Browser tools are disabled in Ilhae settings",
                None,
            ));
        }
        self.manager.call_tool(request, &settings).await
    }
}
