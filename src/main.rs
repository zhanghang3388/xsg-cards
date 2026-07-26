mod auth;
mod db;
mod models;
mod payment;
mod routes;
mod state;
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
    let state = Arc::new(AppState {
        db: tokio::sync::Mutex::new(conn),
    });

    let app = Router::new()
        .merge(routes::public::router())
        .merge(routes::order::router())
        .nest("/admin", routes::admin::router(state.clone()))
        .nest_service("/static", ServeDir::new("static"))
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
    tracing::info!("潇洒哥的卡台已启动: http://127.0.0.1:{port}  (管理后台 /admin，默认账号 admin / admin123)");
    axum::serve(listener, app).await.unwrap();
}
