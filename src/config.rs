//! 运行期配置：后台隐藏路径、上传目录。
//! 后台路径优先级：环境变量 ADMIN_PATH > 数据库 settings.admin_path > 首次启动随机生成 6 位。

use rand::Rng;
use rusqlite::Connection;
use std::sync::OnceLock;

static ADMIN_SLUG: OnceLock<String> = OnceLock::new();
static ADMIN_BASE: OnceLock<String> = OnceLock::new();
static UPLOAD_DIR: OnceLock<String> = OnceLock::new();

/// 去掉易混淆字符（0/o/1/l）的字母数字表
const SLUG_CHARS: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";

/// 保留路径：后台路径不能占用这些前缀
const RESERVED: [&str; 9] = [
    "admin", "static", "uploads", "api", "order", "pay", "query", "p", "favicon.ico",
];

/// 生成 6 位随机后台路径
pub fn gen_slug() -> String {
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| SLUG_CHARS[rng.gen_range(0..SLUG_CHARS.len())] as char)
        .collect()
}

/// 校验并规范化后台路径：4-32 位，仅字母数字与 - _，不得使用保留字
pub fn sanitize_slug(raw: &str) -> Option<String> {
    let s = raw.trim().trim_matches('/').to_ascii_lowercase();
    if s.len() < 4 || s.len() > 32 {
        return None;
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    if RESERVED.contains(&s.as_str()) {
        return None;
    }
    Some(s)
}

pub fn init(conn: &Connection) {
    let slug = match std::env::var("ADMIN_PATH").ok().and_then(|v| sanitize_slug(&v)) {
        Some(s) => s,
        None => match sanitize_slug(&crate::models::get_setting(conn, "admin_path")) {
            Some(s) => s,
            None => {
                let s = gen_slug();
                let _ = crate::models::set_setting(conn, "admin_path", &s);
                s
            }
        },
    };
    let _ = ADMIN_BASE.set(format!("/{slug}"));
    let _ = ADMIN_SLUG.set(slug);

    let dir = std::env::var("UPLOAD_DIR").unwrap_or_else(|_| "uploads".to_string());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("上传目录 {dir} 创建失败：{e}");
    }
    let _ = UPLOAD_DIR.set(dir);
}

/// 后台路径（不含斜杠），如 "k7q2xd"
pub fn admin_slug() -> &'static str {
    ADMIN_SLUG.get().map(|s| s.as_str()).unwrap_or("console")
}

/// 后台路径前缀（含前导斜杠），如 "/k7q2xd"
pub fn admin_base() -> &'static str {
    ADMIN_BASE.get().map(|s| s.as_str()).unwrap_or("/console")
}

/// 图片上传目录
pub fn upload_dir() -> &'static str {
    UPLOAD_DIR.get().map(|s| s.as_str()).unwrap_or("uploads")
}
