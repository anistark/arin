//! The `arin-mcp` binary: an MCP server over stdio.
//!
//! Clients launch this as a subprocess and speak MCP to it on stdin and stdout. It
//! connects to the daemon's Unix socket and forwards.

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    // Logs go to stderr: stdout is the MCP transport and must carry nothing else.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arin_mcp=info,arin_core=info".into()),
        )
        .init();

    // Fail here rather than at the first tool call: a client that cannot reach the
    // daemon should find out while it is still starting up.
    let mut client = arin_core::Client::connect()
        .await
        .context("could not reach the arin daemon; is `arin daemon` running?")?;

    let session = client
        .start_session(arin_mcp::CLIENT_NAME)
        .await
        .context("daemon refused the session")?;

    tracing::info!(%session, tools = ?arin_mcp::tools::ALL, "connected to the daemon");

    anyhow::bail!(
        "the MCP server binding is not implemented yet; it lands in 0.2 along with the \
         {} tools listed above",
        arin_mcp::tools::ALL.len()
    )
}
