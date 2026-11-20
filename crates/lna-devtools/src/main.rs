use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use lna_api::CoreRuntime;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(
    name = "lna-devtools",
    about = "Local Notebook Agent CLI & Dev WebSocket Server"
)]
struct Cli {
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start local WebSocket dev server on 127.0.0.1 for frontend-demo/index.html
    Serve {
        #[arg(long, default_value = "127.0.0.1:18492")]
        addr: String,
    },
    /// Execute a single JSON-RPC call from CLI
    Call {
        method: String,
        #[arg(default_value = "{}")]
        params: String,
    },
}

#[derive(Clone)]
struct AppState {
    core: Arc<CoreRuntime>,
    token: String,
}

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let core = CoreRuntime::open(cli.data_dir, true)?;

    match cli.cmd {
        Commands::Call { method, params } => {
            let params_val: serde_json::Value = serde_json::from_str(&params)?;
            let req = serde_json::json!({
                "id": "cli-1",
                "method": method,
                "params": params_val
            });
            println!("{}", core.handle_json(&req.to_string()));
            core.shutdown();
        }
        Commands::Serve { addr } => {
            let token = uuid::Uuid::new_v4().to_string();
            println!("Local Notebook Agent DevTools WS running at ws://{addr}/ws?token={token}");
            println!("Open frontend-demo/index.html?ws=ws://{addr}/ws&token={token}");

            let state = AppState {
                core: core.clone(),
                token,
            };
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            rt.block_on(async move {
                let app = Router::new()
                    .route("/ws", get(ws_handler))
                    .with_state(state);
                let listener = tokio::net::TcpListener::bind(&addr).await?;
                axum::serve(listener, app).await?;
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
            core.shutdown();
        }
    }
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if q.token.as_deref() != Some(&state.token) {
        return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let mut rx = state.core.events.subscribe();

    let ev_tx = out_tx.clone();
    let ev_task = tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            if let Ok(txt) = serde_json::to_string(&ev) {
                if ev_tx.send(txt).await.is_err() {
                    break;
                }
            }
        }
    });

    let write_task = tokio::spawn(async move {
        while let Some(txt) = out_rx.recv().await {
            if sender.send(Message::Text(txt)).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(Message::Text(txt))) = receiver.next().await {
        let core = state.core.clone();
        let tx = out_tx.clone();
        tokio::spawn(async move {
            if let Ok(reply) = tokio::task::spawn_blocking(move || core.handle_json(&txt)).await {
                let _ = tx.send(reply).await;
            }
        });
    }

    ev_task.abort();
    write_task.abort();
}
