use crate::state::SharedState;
use crate::util::{get_cookie, gen_token, now_str};
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use rusqlite::{Connection, OptionalExtension, Result};
use std::time::{Duration, Instant};

pub const SESSION_COOKIE: &str = "admin_session";
const SESSION_DAYS: i64 = 7;

/// 登录失败上限与锁定时长
const MAX_LOGIN_FAILS: u32 = 5;
const LOCK_DURATION: Duration = Duration::from_secs(600);

/// 取客户端 IP：优先反向代理透传的头部
pub fn client_ip(headers: &HeaderMap) -> String {
    for h in ["x-forwarded-for", "x-real-ip"] {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            let first = v.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "unknown".to_string()
}

/// 该 IP 是否已被锁定；返回剩余锁定分钟数
pub fn login_locked(state: &SharedState, ip: &str) -> Option<u64> {
    let mut guard = state.login_guard.lock().ok()?;
    let (fails, last) = *guard.get(ip)?;
    if fails < MAX_LOGIN_FAILS {
        return None;
    }
    let elapsed = last.elapsed();
    if elapsed >= LOCK_DURATION {
        guard.remove(ip);
        return None;
    }
    Some(((LOCK_DURATION - elapsed).as_secs() / 60) + 1)
}

pub fn record_login_fail(state: &SharedState, ip: &str) {
    if let Ok(mut guard) = state.login_guard.lock() {
        // 顺手清理过期条目，避免内存无限增长
        guard.retain(|_, (_, last)| last.elapsed() < LOCK_DURATION * 2);
        let e = guard.entry(ip.to_string()).or_insert((0, Instant::now()));
        if e.1.elapsed() >= LOCK_DURATION {
            *e = (0, Instant::now());
        }
        e.0 += 1;
        e.1 = Instant::now();
    }
}

pub fn clear_login_fails(state: &SharedState, ip: &str) {
    if let Ok(mut guard) = state.login_guard.lock() {
        guard.remove(ip);
    }
}

#[derive(Clone)]
pub struct AdminUser {
    pub id: i64,
    pub username: String,
}

pub fn verify_login(conn: &Connection, username: &str, password: &str) -> Option<AdminUser> {
    let row: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT id, username, password_hash FROM admins WHERE username = ?1",
            [username],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (id, username, hash) = row?;
    if bcrypt::verify(password, &hash).unwrap_or(false) {
        Some(AdminUser { id, username })
    } else {
        None
    }
}

pub fn create_session(conn: &Connection, admin_id: i64) -> Result<String> {
    let token = gen_token();
    let expires = (chrono::Local::now() + chrono::Duration::days(SESSION_DAYS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    conn.execute(
        "INSERT INTO sessions(token, admin_id, expires_at) VALUES(?1, ?2, ?3)",
        (&token, admin_id, &expires),
    )?;
    // 顺手清理过期会话
    conn.execute("DELETE FROM sessions WHERE expires_at < ?1", [now_str()])?;
    Ok(token)
}

pub fn destroy_session(conn: &Connection, token: &str) {
    let _ = conn.execute("DELETE FROM sessions WHERE token = ?1", [token]);
}

pub fn admin_by_token(conn: &Connection, token: &str) -> Option<AdminUser> {
    conn.query_row(
        "SELECT a.id, a.username FROM sessions s JOIN admins a ON a.id = s.admin_id
         WHERE s.token = ?1 AND s.expires_at > ?2",
        (token, now_str()),
        |r| {
            Ok(AdminUser {
                id: r.get(0)?,
                username: r.get(1)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn change_password(
    conn: &Connection,
    admin_id: i64,
    old: &str,
    new: &str,
) -> std::result::Result<(), String> {
    if new.chars().count() < 6 {
        return Err("新密码至少 6 位".into());
    }
    let hash: String = conn
        .query_row("SELECT password_hash FROM admins WHERE id = ?1", [admin_id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !bcrypt::verify(old, &hash).unwrap_or(false) {
        return Err("原密码不正确".into());
    }
    let new_hash = bcrypt::hash(new, 10).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE admins SET password_hash = ?1 WHERE id = ?2",
        (new_hash, admin_id),
    )
    .map_err(|e| e.to_string())?;
    // 修改密码后清除全部会话
    let _ = conn.execute("DELETE FROM sessions WHERE admin_id = ?1", [admin_id]);
    Ok(())
}

/// 管理端鉴权中间件：无有效会话则跳转登录页
pub async fn require_admin(State(state): State<SharedState>, req: Request, next: Next) -> Response {
    if let Some(token) = get_cookie(req.headers(), SESSION_COOKIE) {
        let admin = {
            let db = state.db.lock().await;
            admin_by_token(&db, &token)
        };
        if let Some(admin) = admin {
            let mut req = req;
            req.extensions_mut().insert(admin);
            return next.run(req).await;
        }
    }
    Redirect::to(&format!("{}/login", crate::config::admin_base())).into_response()
}

/// 登录成功 Set-Cookie
pub fn session_cookie_header(token: &str) -> String {
    // Cookie 作用域限定在后台路径，前台请求不会携带会话
    format!(
        "{SESSION_COOKIE}={token}; Path={}; HttpOnly; SameSite=Lax; Max-Age=604800",
        crate::config::admin_base()
    )
}

pub fn clear_cookie_header() -> String {
    format!(
        "{SESSION_COOKIE}=; Path={}; HttpOnly; SameSite=Lax; Max-Age=0",
        crate::config::admin_base()
    )
}
