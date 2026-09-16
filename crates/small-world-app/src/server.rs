use crate::dashboard::{DashboardState, SharedDashboardState, prepare_run, run_experiment};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use small_world_distributed::ExperimentConfig;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;

async fn status(State(state): State<SharedDashboardState>) -> Json<DashboardState> {
    Json(state.read().await.clone())
}

async fn start_run(
    State(state): State<SharedDashboardState>,
    Json(config): Json<ExperimentConfig>,
) -> impl IntoResponse {
    if let Err(error) = prepare_run(&state, config).await {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": error })),
        );
    }
    let run_state = state.clone();
    tokio::spawn(async move {
        run_experiment(run_state, config).await;
    });
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "started" })),
    )
}

pub async fn serve(host: &str, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(RwLock::new(DashboardState::default()));
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("frontend")
        .join("dist");
    if !frontend.join("index.html").exists() {
        return Err(format!(
            "frontend build not found at {}; run `cd frontend && npm install && npm run build`",
            frontend.display()
        )
        .into());
    }

    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/run", post(start_run))
        .fallback_service(ServeDir::new(frontend).append_index_html_on_directories(true))
        .with_state(state);
    let listener = TcpListener::bind((host, port)).await?;
    println!("Small World Lab: http://{host}:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}
