use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header, Method},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

const MAX_CONCURRENT_ROUTES: usize = 4;
const REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_BODY_BYTES: usize = 64 * 1024;

use crate::graph::{Graph, SpatialIndex};
use crate::graph::cache;
use crate::gpx::writer::write_gpx_to_bytes;
use crate::profile::{load_profile, Profile};
use crate::routing::alternatives::AltConfig;
use crate::profile::RoutingConfig;

struct AppState {
    graph: Arc<Graph>,
    index: Arc<SpatialIndex>,
    profile: Arc<Profile>,
    route_semaphore: Arc<Semaphore>,
}

#[derive(Deserialize)]
struct RouteRequest {
    from: [f64; 2],
    to: [f64; 2],
    #[serde(default)]
    tank_range_km: Option<f32>,
    #[serde(default = "default_true")]
    avoid_paved: bool,
    #[serde(default = "default_true")]
    avoid_fords: bool,
    #[serde(default = "default_prefer_scenic")]
    prefer_scenic: bool,
    #[serde(default = "default_scenic_weight")]
    scenic_weight: f32,
    /// Number of topologically distinct route alternatives to return (default: 1).
    #[serde(default = "default_alternatives")]
    alternatives: usize,
    #[serde(default = "default_diversity")]
    diversity: f32,
    #[serde(default = "default_max_detour")]
    max_detour: f32,
    /// Penalty growth factor for k-alternatives.
    #[serde(default = "default_lambda")]
    lambda: f32,
    /// Fuel reserve fraction — stop is triggered at this fraction of tank range.
    #[serde(default = "default_fuel_buffer")]
    fuel_buffer: f32,
    /// Vehicle profile to apply (default: "high-clearance").
    /// Valid values: "stock-suv", "high-clearance", "4x4", "dirtbike".
    #[serde(default = "default_vehicle")]
    vehicle: String,
}

fn default_true() -> bool { true }
fn default_vehicle() -> String { "high-clearance".to_string() }
fn default_alternatives() -> usize { 1 }
fn default_diversity() -> f32 { AltConfig::default().min_jaccard_distance }
fn default_max_detour() -> f32 { AltConfig::default().max_detour }
fn default_lambda() -> f32 { RoutingConfig::default().lambda }
fn default_fuel_buffer() -> f32 { RoutingConfig::default().fuel_buffer }
fn default_prefer_scenic() -> bool { true }
fn default_scenic_weight() -> f32 { 1.0 }

#[derive(Debug, thiserror::Error)]
enum ValidationError {
    #[error("{label} latitude {value} out of range [-90, 90]")]
    LatOutOfRange { label: &'static str, value: f64 },
    #[error("{label} longitude {value} out of range [-180, 180]")]
    LonOutOfRange { label: &'static str, value: f64 },
    #[error("max_detour must be positive, got {value}")]
    MaxDetourNotPositive { value: f32 },
    #[error("diversity must be between 0.0 and 1.0, got {value}")]
    DiversityOutOfRange { value: f32 },
    #[error("scenic_weight must be between 0.0 and 1.0, got {value}")]
    ScenicWeightOutOfRange { value: f32 },
}

fn validate_request(req: &RouteRequest) -> Result<(), ValidationError> {
    for (label, coords) in [("from", &req.from), ("to", &req.to)] {
        let (lat, lon) = (coords[0], coords[1]);
        if !(-90.0..=90.0).contains(&lat) {
            return Err(ValidationError::LatOutOfRange { label, value: lat });
        }
        if !(-180.0..=180.0).contains(&lon) {
            return Err(ValidationError::LonOutOfRange { label, value: lon });
        }
    }
    if req.max_detour <= 0.0 {
        return Err(ValidationError::MaxDetourNotPositive { value: req.max_detour });
    }
    if req.diversity < 0.0 || req.diversity > 1.0 {
        return Err(ValidationError::DiversityOutOfRange { value: req.diversity });
    }
    if req.scenic_weight < 0.0 || req.scenic_weight > 1.0 {
        return Err(ValidationError::ScenicWeightOutOfRange { value: req.scenic_weight });
    }
    Ok(())
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn handle_health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok", version: env!("CARGO_PKG_VERSION") })
}

async fn handle_route(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RouteRequest>,
) -> Response {
    if let Err(e) = validate_request(&req) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response();
    }

    let _permit = match Arc::clone(&state.route_semaphore).acquire_owned().await {
        Ok(p) => p,
        Err(_) => return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "server at capacity" })),
        ).into_response(),
    };

    let graph = Arc::clone(&state.graph);
    let index = Arc::clone(&state.index);
    let profile = Arc::clone(&state.profile);

    let params = super::RouteParams {
        from_lat: req.from[0],
        from_lon: req.from[1],
        to_lat: req.to[0],
        to_lon: req.to[1],
        alternatives: req.alternatives,
        diversity: req.diversity,
        max_detour: req.max_detour,
        avoid_paved: req.avoid_paved,
        avoid_fords: req.avoid_fords,
        prefer_scenic: req.prefer_scenic,
        scenic_weight: req.scenic_weight,
        tank_range: req.tank_range_km,
        lambda: req.lambda,
        fuel_buffer: req.fuel_buffer,
        vehicle: req.vehicle,
    };

    let result = tokio::task::spawn_blocking(move || {
        super::run_route(&graph, &index, &profile, &params)
            .and_then(|(routes, fuel_stops)| {
                write_gpx_to_bytes(&routes, &graph, "overlandr", &fuel_stops)
            })
    }).await;

    match result {
        Ok(Ok(bytes)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/gpx+xml")],
            bytes,
        ).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = sigterm => {}
    }

    tracing::info!("shutdown signal received; draining in-flight requests");
}

pub async fn run_server(graph_path: &Path, host: &str, port: u16) -> anyhow::Result<()> {
    tracing::info!("loading graph from {:?}", graph_path);
    let (g, index, _fp, _ts) = cache::load(graph_path)
        .map_err(|e| anyhow::anyhow!("failed to load graph from {:?}: {}", graph_path, e))?;

    let profile = load_profile(None)
        .map_err(|e| anyhow::anyhow!("failed to load routing profile: {}", e))?;

    let state = Arc::new(AppState {
        graph: Arc::new(g),
        index: Arc::new(index),
        profile: Arc::new(profile),
        route_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_ROUTES)),
    });

    tracing::info!("graph loaded; starting server on {}:{}", host, port);

    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/route", post(handle_route))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::new(Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::CONTENT_TYPE]),
        )
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await
        .map_err(|e| anyhow::anyhow!("failed to bind {}: {}", addr, e))?;
    tracing::info!("listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(from: [f64; 2], to: [f64; 2], max_detour: f32, diversity: f32) -> RouteRequest {
        RouteRequest {
            from,
            to,
            tank_range_km: None,
            avoid_paved: default_true(),
            avoid_fords: default_true(),
            prefer_scenic: default_prefer_scenic(),
            scenic_weight: default_scenic_weight(),
            alternatives: default_alternatives(),
            diversity,
            max_detour,
            lambda: default_lambda(),
            fuel_buffer: default_fuel_buffer(),
            vehicle: default_vehicle(),
        }
    }

    #[test]
    fn valid_request_passes() {
        let r = req([47.0, -116.0], [47.1, -116.1], 1.6, 0.35);
        assert!(validate_request(&r).is_ok());
    }

    #[test]
    fn invalid_latitude_rejected() {
        let r = req([91.0, -116.0], [47.1, -116.1], 1.6, 0.35);
        let err = validate_request(&r).unwrap_err().to_string();
        assert!(err.contains("latitude"), "{err}");
    }

    #[test]
    fn invalid_longitude_rejected() {
        let r = req([47.0, -200.0], [47.1, -116.1], 1.6, 0.35);
        let err = validate_request(&r).unwrap_err().to_string();
        assert!(err.contains("longitude"), "{err}");
    }

    #[test]
    fn negative_detour_rejected() {
        let r = req([47.0, -116.0], [47.1, -116.1], -1.0, 0.35);
        assert!(validate_request(&r).is_err());
    }

    #[test]
    fn diversity_out_of_range_rejected() {
        let r = req([47.0, -116.0], [47.1, -116.1], 1.6, 1.5);
        assert!(validate_request(&r).is_err());
    }

    #[test]
    fn scenic_weight_out_of_range_rejected() {
        let mut r = req([47.0, -116.0], [47.1, -116.1], 1.6, 0.35);
        r.scenic_weight = 1.5;
        assert!(validate_request(&r).is_err());
    }
}
