//! 邮件通知：发货后把卡密寄给买家，缺货待补发时提醒店主。
//!
//! 设计要点：发信一律走 tokio::spawn 异步进行，任何失败只记日志，
//! 绝不影响发货主流程——卡密页始终是唯一可靠的交付渠道，邮件只是锦上添花。

use crate::models::{self, Order};
use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use rusqlite::Connection;

#[derive(Clone, Default)]
pub struct MailCfg {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// ssl（隐式 TLS，常见 465）| starttls（常见 587）| none（仅内网自建）
    pub security: String,
    pub user: String,
    pub pass: String,
    pub from: String,
    pub from_name: String,
    pub admin_to: String,
    pub site_name: String,
    pub site_url: String,
}

impl MailCfg {
    /// 配置是否完整到可以发信
    pub fn ready(&self) -> bool {
        self.enabled && !self.host.is_empty() && !self.from.is_empty() && self.port > 0
    }
}

pub fn mail_cfg(conn: &Connection) -> MailCfg {
    let g = |k: &str| models::get_setting(conn, k);
    MailCfg {
        enabled: g("mail_enabled") == "1",
        host: g("smtp_host"),
        port: g("smtp_port").parse().unwrap_or(465),
        security: {
            let s = g("smtp_security");
            if s.is_empty() {
                "ssl".into()
            } else {
                s
            }
        },
        user: g("smtp_user"),
        pass: g("smtp_pass"),
        from: g("mail_from"),
        from_name: {
            let n = g("mail_from_name");
            if n.is_empty() {
                g("site_name")
            } else {
                n
            }
        },
        admin_to: g("mail_admin_to"),
        site_name: g("site_name"),
        site_url: g("site_url").trim_end_matches('/').to_string(),
    }
}

/// 宽松但够用的邮箱判断：联系方式也可能是 QQ 号，非邮箱就不发信。
pub fn is_email(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 6 || s.len() > 254 || s.contains(char::is_whitespace) {
        return false;
    }
    let mut parts = s.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.len() >= 3
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------- 发送 ----------

pub async fn send(cfg: &MailCfg, to: &str, subject: &str, text: String, html: String) -> Result<(), String> {
    if !cfg.ready() {
        return Err("邮件功能未启用或配置不完整".into());
    }
    let from_addr: Mailbox = format!("{} <{}>", cfg.from_name, cfg.from)
        .parse()
        .or_else(|_| cfg.from.parse())
        .map_err(|_| format!("发件人地址不合法：{}", cfg.from))?;
    let to_addr: Mailbox = to
        .trim()
        .parse()
        .map_err(|_| format!("收件人地址不合法：{to}"))?;

    let email = Message::builder()
        .from(from_addr)
        .to(to_addr)
        .subject(subject)
        .multipart(MultiPart::alternative_plain_html(text, html))
        .map_err(|e| format!("邮件构造失败：{e}"))?;

    let builder = match cfg.security.as_str() {
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .map_err(|e| format!("SMTP 连接配置失败：{e}"))?,
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host),
        _ => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
            .map_err(|e| format!("SMTP 连接配置失败：{e}"))?,
    };
    let mut builder = builder.port(cfg.port);
    if !cfg.user.is_empty() {
        builder = builder.credentials(Credentials::new(cfg.user.clone(), cfg.pass.clone()));
    }
    builder
        .build()
        .send(email)
        .await
        .map_err(|e| format!("发送失败：{e}"))?;
    Ok(())
}

/// 后台发信：不阻塞调用方，失败只记日志
pub fn spawn_send(cfg: MailCfg, to: String, subject: String, text: String, html: String) {
    tokio::spawn(async move {
        match send(&cfg, &to, &subject, text, html).await {
            Ok(()) => tracing::info!("邮件已发送 -> {to}：{subject}"),
            Err(e) => tracing::warn!("邮件发送失败 -> {to}：{e}"),
        }
    });
}

// ---------- 邮件内容 ----------

fn wrap_html(cfg: &MailCfg, title: &str, body: String) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"></head>
<body style="margin:0;padding:24px;background:#f4f2ee;font-family:-apple-system,'Segoe UI','Microsoft YaHei',sans-serif;color:#23201c;">
<div style="max-width:560px;margin:0 auto;background:#fffdf9;border:1px solid #e2ddd3;border-radius:12px;overflow:hidden;">
  <div style="padding:18px 24px;border-bottom:2px dashed #e2ddd3;">
    <span style="font-weight:700;font-size:16px;">{site}</span>
    <span style="float:right;color:#8a8178;font-size:12px;letter-spacing:.08em;">{title}</span>
  </div>
  <div style="padding:24px;line-height:1.7;font-size:14px;">{body}</div>
  <div style="padding:14px 24px;border-top:1px solid #eee7dc;color:#8a8178;font-size:12px;">
    本邮件由系统自动发出，请勿直接回复。
  </div>
</div></body></html>"#,
        site = esc(&cfg.site_name),
        title = esc(title),
        body = body
    )
}

/// 买家收到的发货信
fn build_deliver_mail(cfg: &MailCfg, order: &Order, codes: &[String]) -> (String, String, String) {
    let subject = format!("【{}】卡密已发货 · 订单 {}", cfg.site_name, order.order_no);
    let link = if cfg.site_url.is_empty() {
        String::new()
    } else {
        format!("{}{}", cfg.site_url, order.view_link())
    };

    let mut text = format!(
        "感谢购买！以下是你的卡密。\n\n商品：{}\n数量：{}\n订单号：{}\n实付：¥{}\n\n",
        order.product_name,
        order.quantity,
        order.order_no,
        order.total_str()
    );
    for c in codes {
        text.push_str(c);
        text.push('\n');
    }
    if !link.is_empty() {
        text.push_str(&format!("\n随时可在此页面重新取卡：{link}\n"));
    }

    let codes_html = codes
        .iter()
        .map(|c| {
            format!(
                r#"<div style="font-family:'SFMono-Regular',Consolas,monospace;font-size:15px;letter-spacing:.06em;background:#fff;border:1px solid #e2ddd3;border-radius:8px;padding:11px 14px;margin:8px 0;word-break:break-all;">{}</div>"#,
                esc(c)
            )
        })
        .collect::<String>();

    let link_html = if link.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p style="margin:20px 0 0;color:#6b635a;font-size:13px;">卡密也可随时在这里取回：<br><a href="{l}" style="color:#b07c12;word-break:break-all;">{l}</a></p>"#,
            l = esc(&link)
        )
    };

    let body = format!(
        r#"<p style="margin:0 0 4px;">付款成功，以下是你购买的卡密：</p>
<p style="margin:0 0 16px;color:#8a8178;font-size:13px;">{name} × {qty}　订单号 {no}　实付 ¥{total}</p>
{codes}
<p style="margin:18px 0 0;color:#6b635a;font-size:13px;">请尽快兑换并妥善保管，卡密属虚拟商品，售出后不支持退换。</p>
{link}"#,
        name = esc(&order.product_name),
        qty = order.quantity,
        no = esc(&order.order_no),
        total = order.total_str(),
        codes = codes_html,
        link = link_html
    );
    (subject, text, wrap_html(cfg, "发货通知", body))
}

/// 店主收到的缺货告警
fn build_restock_mail(cfg: &MailCfg, order: &Order) -> (String, String, String) {
    let subject = format!("【{}】库存告急：订单 {} 已付款但无卡可发", cfg.site_name, order.order_no);
    let text = format!(
        "订单 {} 已付款，但「{}」库存不足，系统已标记为「已付待补发」。\n\n买家：{}\n数量：{}\n金额：¥{}\n\n请尽快导入卡密后到后台订单页手动补发。\n",
        order.order_no,
        order.product_name,
        order.contact,
        order.quantity,
        order.total_str()
    );
    let body = format!(
        r#"<p style="margin:0 0 12px;"><b>有一笔订单收了钱但发不出卡。</b></p>
<p style="margin:0 0 12px;color:#6b635a;font-size:13px;">订单号 {no}　商品「{name}」× {qty}　金额 ¥{total}<br>买家联系方式：{contact}</p>
<p style="margin:0;">请尽快为该商品导入卡密，然后到后台订单页点「补发」。买家此刻看到的是「已付待补发」状态。</p>"#,
        no = esc(&order.order_no),
        name = esc(&order.product_name),
        qty = order.quantity,
        total = order.total_str(),
        contact = esc(&order.contact)
    );
    (subject, text, wrap_html(cfg, "缺货告警", body))
}

// ---------- 对外入口 ----------

/// 发货完成后调用（持有数据库连接时同步读取内容，随后异步发信）。
/// `status` 为 deliver_order 的返回值：1 已完成，3 已付待补发。
pub fn notify_after_deliver(conn: &Connection, order_no: &str, status: i64) {
    let cfg = mail_cfg(conn);
    if !cfg.ready() {
        return;
    }
    let Ok(Some(order)) = models::get_order_by_no(conn, order_no) else {
        return;
    };

    if status == 1 {
        if !is_email(&order.contact) {
            return; // 联系方式不是邮箱（比如 QQ 号），跳过
        }
        let codes = models::order_codes(conn, order.id).unwrap_or_default();
        if codes.is_empty() {
            return;
        }
        let (subject, text, html) = build_deliver_mail(&cfg, &order, &codes);
        spawn_send(cfg, order.contact.clone(), subject, text, html);
    } else if status == 3 && !cfg.admin_to.is_empty() {
        let (subject, text, html) = build_restock_mail(&cfg, &order);
        let to = cfg.admin_to.clone();
        spawn_send(cfg, to, subject, text, html);
    }
}

/// 后台「发测试信」
pub async fn send_test(cfg: MailCfg, to: String) -> Result<(), String> {
    let body = r#"<p style="margin:0 0 12px;">这是一封测试邮件。</p>
<p style="margin:0;color:#6b635a;font-size:13px;">你能读到它，说明 SMTP 配置正确，买家付款后就能自动收到卡密了。</p>"#;
    let html = wrap_html(&cfg, "测试邮件", body.to_string());
    let subject = format!("【{}】SMTP 配置测试", cfg.site_name);
    send(&cfg, &to, &subject, "这是一封测试邮件，能收到说明 SMTP 配置正确。".into(), html).await
}
