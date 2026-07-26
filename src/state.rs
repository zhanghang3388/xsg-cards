use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    pub db: Mutex<Connection>,
}

pub type SharedState = Arc<AppState>;
