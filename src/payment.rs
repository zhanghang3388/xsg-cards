use crate::models::{get_setting, Order};
use rusqlite::Connection;
use std::collections::BTreeMap;

/// 支付配置（来自站点设置）
pub struct PayConfig {
    pub mode: String, // mock | epay
    pub gateway: String,
    pub pid: String,
    pub key: String,
    pub site_url: String,
    pub alipay: bool,
    pub wxpay: bool,
}

/// 收银台上的一个支付按钮
pub struct PayChannel {
    /// 易支付的 type 参数，空串 = 不带 type，由对方收银台自选
    pub code: String,
    pub label: String,
    /// 按钮配色用的 class：alipay | wxpay | other
    pub style: String,
    pub fields: Vec<(String, String)>,
}

fn flag(conn: &Connection, key: &str) -> bool {
    get_setting(conn, key) == "1"
}

pub fn pay_config(conn: &Connection) -> PayConfig {
    PayConfig {
        mode: {
            let m = get_setting(conn, "pay_mode");
            if m == "epay" { m } else { "mock".into() }
        },
        gateway: get_setting(conn, "epay_gateway"),
        pid: get_setting(conn, "epay_pid"),
        key: get_setting(conn, "epay_key"),
        site_url: {
            let u = get_setting(conn, "site_url");
            u.trim_end_matches('/').to_string()
        },
        alipay: flag(conn, "epay_alipay"),
        wxpay: flag(conn, "epay_wxpay"),
    }
}

impl PayConfig {
    pub fn epay_ready(&self) -> bool {
        self.mode == "epay"
            && !self.gateway.is_empty()
            && !self.pid.is_empty()
            && !self.key.is_empty()
    }

    /// 按后台勾选生成收银台按钮。
    /// 一个都没勾时不留白页，退回「其他方式」——提交不带 type 参数，
    /// 由易支付自家收银台列出该商户实际开通的通道让买家选。
    pub fn channels(&self, order: &Order) -> Vec<PayChannel> {
        let mut out = Vec::new();
        if self.alipay {
            out.push(("alipay", "支付宝", "alipay"));
        }
        if self.wxpay {
            out.push(("wxpay", "微信支付", "wxpay"));
        }
        if out.is_empty() {
            out.push(("", "其他方式", "other"));
        }
        out.into_iter()
            .map(|(code, label, style)| PayChannel {
                fields: epay_form_fields(order, self, code),
                code: code.to_string(),
                label: label.to_string(),
                style: style.to_string(),
            })
            .collect()
    }
}

/// 易支付 MD5 签名：参数按键名升序拼接 k=v&...（跳过空值与 sign/sign_type），末尾拼商户密钥
pub fn epay_sign(params: &BTreeMap<String, String>, key: &str) -> String {
    let joined: Vec<String> = params
        .iter()
        .filter(|(k, v)| !v.is_empty() && k.as_str() != "sign" && k.as_str() != "sign_type")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let base = format!("{}{}", joined.join("&"), key);
    format!("{:x}", md5::compute(base.as_bytes()))
}

/// 构造跳转易支付收银台的表单字段（pay_type 可为 alipay / wxpay / 空=收银台自选）
pub fn epay_form_fields(order: &Order, cfg: &PayConfig, pay_type: &str) -> Vec<(String, String)> {
    let mut p = BTreeMap::new();
    p.insert("pid".to_string(), cfg.pid.clone());
    if !pay_type.is_empty() {
        p.insert("type".to_string(), pay_type.to_string());
    }
    p.insert("out_trade_no".to_string(), order.order_no.clone());
    p.insert(
        "notify_url".to_string(),
        format!("{}/api/pay/epay/notify", cfg.site_url),
    );
    p.insert(
        "return_url".to_string(),
        format!(
            "{}/order/{}?c={}",
            cfg.site_url,
            order.order_no,
            urlencoding::encode(&order.contact)
        ),
    );
    p.insert("name".to_string(), order.product_name.clone());
    p.insert("money".to_string(), crate::util::cents_str(order.total_cents));
    let sign = epay_sign(&p, &cfg.key);
    p.insert("sign".to_string(), sign);
    p.insert("sign_type".to_string(), "MD5".to_string());
    p.into_iter().collect()
}

/// 校验易支付异步通知：验签 + 状态 + 金额一致
pub fn verify_epay_notify(
    params: &BTreeMap<String, String>,
    cfg: &PayConfig,
    order: &Order,
) -> Result<(), String> {
    let given_sign = params.get("sign").cloned().unwrap_or_default();
    if given_sign.is_empty() {
        return Err("缺少签名".into());
    }
    let expect = epay_sign(params, &cfg.key);
    if !expect.eq_ignore_ascii_case(&given_sign) {
        return Err("签名校验失败".into());
    }
    if params.get("trade_status").map(String::as_str) != Some("TRADE_SUCCESS") {
        return Err("交易状态非成功".into());
    }
    let money = params.get("money").cloned().unwrap_or_default();
    let paid_cents = crate::util::yuan_to_cents(&money).unwrap_or(-1);
    if paid_cents != order.total_cents {
        return Err(format!(
            "金额不一致：应付 {} 实付 {money}",
            crate::util::cents_str(order.total_cents)
        ));
    }
    Ok(())
}
