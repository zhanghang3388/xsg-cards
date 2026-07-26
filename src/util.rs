use askama::Template;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use rand::Rng;

/// 当前本地时间字符串（可排序格式）
pub fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// N 分钟前的时间字符串（用于过期判断）
pub fn minutes_ago_str(minutes: i64) -> String {
    (chrono::Local::now() - chrono::Duration::minutes(minutes))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// 今天日期前缀，如 2026-07-26
pub fn today_str() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 订单号：时间戳 + 6 位随机数
pub fn gen_order_no() -> String {
    let ts = chrono::Local::now().format("%Y%m%d%H%M%S");
    let n: u32 = rand::thread_rng().gen_range(0..1_000_000);
    format!("{ts}{n:06}")
}

/// 会话令牌：48 位十六进制
pub fn gen_token() -> String {
    let mut rng = rand::thread_rng();
    (0..48)
        .map(|_| {
            let c: u32 = rng.gen_range(0..16);
            char::from_digit(c, 16).unwrap()
        })
        .collect()
}

/// 分 -> "12.50"
pub fn cents_str(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let c = cents.abs();
    format!("{sign}{}.{:02}", c / 100, c % 100)
}

/// "12.5" / "12.50" / "12" -> 1250 分
pub fn yuan_to_cents(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let mut parts = s.splitn(2, '.');
    let whole: i64 = parts.next()?.parse().ok()?;
    let frac = match parts.next() {
        None | Some("") => 0i64,
        Some(f) => {
            if f.len() > 2 || !f.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let v: i64 = f.parse().ok()?;
            if f.len() == 1 {
                v * 10
            } else {
                v
            }
        }
    };
    if whole > 10_000_000 {
        return None;
    }
    Some(whole * 100 + frac)
}

/// 从请求头解析 Cookie
pub fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let mut it = pair.trim().splitn(2, '=');
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 渲染 Askama 模板为响应
pub fn html<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(s) => Html(s).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("模板渲染错误: {e}"),
        )
            .into_response(),
    }
}

/// URL 编码（用于跳转携带中文提示）
pub fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// 联系方式脱敏：foo@bar.com -> f***@bar.com
pub fn mask_contact(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 3 {
        return format!("{}***", chars.first().map(|c| c.to_string()).unwrap_or_default());
    }
    if let Some(at) = s.find('@') {
        let head: String = s[..at].chars().take(1).collect();
        format!("{head}***{}", &s[at..])
    } else {
        let head: String = chars.iter().take(3).collect();
        format!("{head}***{}", chars.last().unwrap())
    }
}
