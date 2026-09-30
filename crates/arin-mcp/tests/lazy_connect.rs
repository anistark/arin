//! The MCP server against a daemon that is not there yet, then is, then restarts.
//!
//! Driven through rmcp's own client over an in-memory pipe, so what is exercised is the
//! handshake and the tool calls an MCP client actually makes, and a real headless daemon
//! on a real socket. Each daemon runs on a runtime of its own, because shutting that
//! runtime down is what closes every connection it accepted, the way a process exiting
//! does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arin_core::{Config, Daemon, NoopCapture, NoopRenderer, Server};
use arin_mcp::{Arin, INSTRUCTIONS, tools};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use rmcp::{ServiceError, ServiceExt};
use serde_json::{Value, json};

/// A headless daemon serving `socket` until stopped or dropped.
struct RunningDaemon(Option<tokio::runtime::Runtime>);

impl RunningDaemon {
    fn start(socket: &Path) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime for the daemon");
        let daemon = Arc::new(Daemon::new(
            Config::with_socket_path(socket),
            Arc::new(NoopRenderer::new()),
            Arc::new(NoopCapture),
        ));
        let server = {
            let _entered = runtime.enter();
            Server::bind(daemon).expect("the daemon binds its socket")
        };
        runtime.spawn(server.run());
        Self(Some(runtime))
    }

    /// Stop it the way a process exiting would, and wait until its socket is released.
    ///
    /// Waiting is the point: dropping a runtime from inside another one can only hand its
    /// tasks to a background thread, and a daemon started before that thread gets to the
    /// listener finds the socket still served.
    async fn stop(mut self) {
        let runtime = self.0.take().expect("running");
        tokio::task::spawn_blocking(move || runtime.shutdown_timeout(Duration::from_secs(5)))
            .await
            .expect("the daemon stops");
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// A socket path short enough for `sun_path`, and not shared with any other test.
fn socket_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("arin-mcp-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

/// An MCP client connected to a fresh server for `socket`.
async fn connect(socket: &Path) -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = Arin::new(socket);
    tokio::spawn(async move {
        let running = server.serve(server_io).await.expect("the server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("the handshake completes")
}

async fn point(client: &RunningService<RoleClient, ()>) -> Result<CallToolResult, ServiceError> {
    let arguments = json!({ "at": "center" }).as_object().cloned().unwrap();
    client
        .call_tool(CallToolRequestParams::new(tools::POINT_AT).with_arguments(arguments))
        .await
}

fn gone(result: &CallToolResult) -> Vec<Value> {
    result
        .structured_content
        .as_ref()
        .and_then(|content| content.get("gone"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn the_server_starts_with_no_daemon_and_says_where_it_looked() {
    let socket = socket_path("absent");
    let client = connect(&socket).await;

    let info = client.peer_info().expect("the server introduced itself");
    assert_eq!(
        info.instructions.as_deref(),
        Some(INSTRUCTIONS),
        "the instructions are what tells a model Arin exists, so they cannot wait for a daemon"
    );
    let advertised = client.list_all_tools().await.expect("tools are listed");
    assert_eq!(advertised.len(), tools::ALL.len());

    let Err(ServiceError::McpError(error)) = point(&client).await else {
        panic!("a call with no daemon has to fail as a tool error");
    };
    assert!(
        error.message.contains(&*socket.to_string_lossy()),
        "the error has to name the socket it tried, got {:?}",
        error.message
    );
    assert!(
        error.message.contains("arin daemon"),
        "the error has to say what to start, got {:?}",
        error.message
    );

    let still_listed = client
        .list_all_tools()
        .await
        .expect("a failed call leaves the server serving");
    assert_eq!(still_listed.len(), tools::ALL.len());
}

#[tokio::test]
async fn a_daemon_started_late_or_restarted_is_picked_up_on_the_next_call() {
    let socket = socket_path("late");
    let client = connect(&socket).await;

    assert!(point(&client).await.is_err(), "nothing is listening yet");

    let daemon = RunningDaemon::start(&socket);
    let drawn = point(&client)
        .await
        .expect("the daemon started after the server, and the same session reaches it");
    assert!(
        gone(&drawn).is_empty(),
        "nothing was drawn before, so nothing went: {:?}",
        gone(&drawn)
    );

    daemon.stop().await;
    let _daemon = RunningDaemon::start(&socket);
    let redrawn = point(&client)
        .await
        .expect("a restarted daemon costs the agent its marks, not the call");
    assert_eq!(
        gone(&redrawn),
        vec![json!({ "annotation_id": null, "reason": "session_end" })],
        "the marks from before the restart are gone, and the agent has to be told"
    );

    let again = point(&client).await.expect("the new connection is kept");
    assert!(
        gone(&again).is_empty(),
        "the loss is reported once: {:?}",
        gone(&again)
    );

    let _ = std::fs::remove_file(&socket);
}
