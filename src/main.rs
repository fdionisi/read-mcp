mod prompt_registry;
mod resource_registry;
mod tool_registry;

use std::{env, sync::Arc};

use anyhow::Result;
use context_server::{ContextServer, ContextServerRpcRequest, ContextServerRpcResponse};
use http_client::HttpClient;
use http_client_reqwest::HttpClientReqwest;
use read_mcp_tools::ReadUrlTool;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    prompt_registry::PromptRegistry, resource_registry::ResourceRegistry,
    tool_registry::ToolRegistry,
};

struct ContextServerState {
    rpc: ContextServer,
}

impl ContextServerState {
    fn new(http_client: Arc<dyn HttpClient>) -> Result<Self> {
        let resource_registry = Arc::new(ResourceRegistry::default());
        let tool_registry = Arc::new(ToolRegistry::default());
        tool_registry.register(Arc::new(ReadUrlTool::new(http_client.clone())));

        let prompt_registry = Arc::new(PromptRegistry::default());
        Ok(Self {
            rpc: ContextServer::builder()
                .with_server_info((env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
                .with_resources(resource_registry)
                .with_tools(tool_registry)
                .with_prompts(prompt_registry)
                .build()?,
        })
    }

    async fn process_request(
        &self,
        request: ContextServerRpcRequest,
    ) -> Result<Option<ContextServerRpcResponse>> {
        let response = self.rpc.handle_incoming_message(request).await;
        response
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let http_client = Arc::new(HttpClientReqwest::default());
    let state = ContextServerState::new(http_client)?;

    let mut stdin = BufReader::new(io::stdin()).lines();
    let mut stdout = io::stdout();

    while let Some(line) = stdin.next_line().await? {
        let request: ContextServerRpcRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("Error parsing request: {}", e);
                continue;
            }
        };
        if let Some(response) = state.process_request(request).await? {
            let response_json = serde_json::to_string(&response)?;
            stdout.write_all(response_json.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        } else {
        }
    }

    Ok(())
}
