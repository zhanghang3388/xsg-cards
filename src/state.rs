use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

pub struct AppState {
    pub db: Mutex<Connection>,
    /// 后台登录失败计数：IP -> (失败次数, 最近一次失败时间)
    pub login_guard: std::sync::Mutex<HashMap<String, (u32, Instant)>>,
}

pub type SharedState = Arc<AppState>;
