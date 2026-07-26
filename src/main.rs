mod auth;
mod config;
mod db;
mod mailer;
mod models;
mod payment;
mod routes;
mod state;
mod upload;
mod util;

use crate::state::AppState;
use axum::Router;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| "data.db".to_string());
    let conn = db::init(&db_path).expect("数据库初始化失败");
    config::init(&conn);
    let state = Arc::new(AppState {
        db: tokio::sync::Mutex::new(conn),
        login_guard: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let app = Router::new()
        .merge(routes::public::router())
        .merge(routes::order::router())
        .nest(config::admin_base(), routes::admin::router(state.clone()))
        .nest_service("/static", ServeDir::new("static"))
        .nest_service("/uploads", ServeDir::new(config::upload_dir()))
        .fallback(routes::public::not_found)
        .with_state(state);

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8080);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("端口绑定失败");
    tracing::info!("潇洒哥的卡台已启动: http://127.0.0.1:{port}");
    tracing::info!(
        "管理后台入口: http://127.0.0.1:{port}{}  （该路径为私密地址，请勿外泄）",
        config::admin_base()
    );
    axum::serve(listener, app).await.unwrap();
}
