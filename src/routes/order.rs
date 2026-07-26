use crate::models::{self, Order, SiteCtx, ORDER_TTL_MIN};
use crate::payment;
use crate::state::SharedState;
use crate::util::{enc, html, mask_contact};
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::collections::BTreeMap;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/order", post(create_order))
        .route("/pay/{order_no}", get(pay_page))
        .route("/api/pay/mock/{order_no}", post(mock_pay))
        .route("/api/pay/epay/notify", get(epay_notify))
        .route("/api/order/{order_no}/status", get(order_status))
        .route("/order/{order_no}", get(order_result))
}

// ---------- 创建订单 ----------

#[derive(Deserialize)]
pub struct CreateOrderForm {
    product_id: i64,
    quantity: i64,
    contact: String,
}

async fn create_order(
    State(state): State<SharedState>,
    axum::Form(f): axum::Form<CreateOrderForm>,
) -> Response {
    let mut db = state.db.lock().await;
    match models::create_order(&mut db, f.product_id, f.quantity, &f.contact) {
        Ok(order) => Redirect::to(&format!("/pay/{}", order.order_no)).into_response(),
        Err(e) => Redirect::to(&format!("/p/{}?err={}", f.product_id, enc(&e))).into_response(),
    }
}

// ---------- 支付页 ----------

#[derive(Template)]
#[template(path = "pay.html")]
struct PayTpl {
    site: SiteCtx,
    order: Order,
    remain_secs: i64,
    pay_mode: String,
    epay_ready: bool,
    epay_gateway: String,
    channels: Vec<payment::PayChannel>,
}

fn remain_secs(order: &Order) -> i64 {
    let t = chrono::NaiveDateTime::parse_from_str(&order.created_at, "%Y-%m-%d %H:%M:%S");
    match t {
        Ok(t) => {
            let elapsed = (chrono::Local::now().naive_local() - t).num_seconds();
            (ORDER_TTL_MIN * 60 - elapsed).max(0)
        }
        Err(_) => 0,
    }
}

async fn pay_page(State(state): State<SharedState>, Path(order_no): Path<String>) -> Response {
    let mut db = state.db.lock().await;
    let _ = models::expire_stale_orders(&mut db);
    let site = models::site_ctx(&db);
    let Some(order) = models::get_order_by_no(&db, &order_no).ok().flatten() else {
        return Redirect::to("/?err=订单不存在").into_response();
    };
    if order.status == 1 || order.status == 3 {
        let link = order.view_link();
        return Redirect::to(&link).into_response();
    }
    let cfg = payment::pay_config(&db);
    let channels = if cfg.epay_ready() {
        cfg.channels(&order)
    } else {
        Vec::new()
    };
    drop(db);
    let remain = remain_secs(&order);
    html(PayTpl {
        site,
        remain_secs: remain,
        pay_mode: cfg.mode.clone(),
        epay_ready: cfg.epay_ready(),
        epay_gateway: cfg.gateway.clone(),
        channels,
        order,
    })
}

// ---------- 模拟支付（演示模式） ----------

async fn mock_pay(State(state): State<SharedState>, Path(order_no): Path<String>) -> Response {
    let mut db = state.db.lock().await;
    let cfg = payment::pay_config(&db);
    if cfg.mode != "mock" {
        return Json(serde_json::json!({"ok": false, "msg": "当前不是演示支付模式"})).into_response();
    }
    match models::deliver_order(&mut db, &order_no, "mock") {
        Ok(st) => {
            crate::mailer::notify_after_deliver(&db, &order_no, st);
            Json(serde_json::json!({"ok": true, "status": st})).into_response()
        }
        Err(e) => Json(serde_json::json!({"ok": false, "msg": e})).into_response(),
    }
}

// ---------- 易支付异步通知 ----------

async fn epay_notify(
    State(state): State<SharedState>,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    let out_trade_no = params.get("out_trade_no").cloned().unwrap_or_default();
    let mut db = state.db.lock().await;
    let cfg = payment::pay_config(&db);
    let Some(order) = models::get_order_by_no(&db, &out_trade_no).ok().flatten() else {
        return "fail: order not found".into_response();
    };
    if order.status == 1 {
        return "success".into_response();
    }
    if let Err(e) = payment::verify_epay_notify(&params, &cfg, &order) {
        tracing::warn!("易支付回调校验失败 {out_trade_no}: {e}");
        return format!("fail: {e}").into_response();
    }
    let pay_type = params.get("type").cloned().unwrap_or_else(|| "epay".into());
    match models::deliver_order(&mut db, &out_trade_no, &pay_type) {
        Ok(st) => {
            crate::mailer::notify_after_deliver(&db, &out_trade_no, st);
            "success".into_response()
        }
        Err(e) => {
            tracing::error!("发货失败 {out_trade_no}: {e}");
            format!("fail: {e}").into_response()
        }
    }
}

// ---------- 订单状态轮询 ----------

async fn order_status(State(state): State<SharedState>, Path(order_no): Path<String>) -> Response {
    let db = state.db.lock().await;
    let order = models::get_order_by_no(&db, &order_no).ok().flatten();
    drop(db);
    match order {
        Some(o) => Json(serde_json::json!({
            "status": o.status,
            "url": o.view_link(),
        }))
        .into_response(),
        None => Json(serde_json::json!({"status": -1})).into_response(),
    }
}

// ---------- 取卡 / 订单结果页 ----------

#[derive(Template)]
#[template(path = "result.html")]
struct ResultTpl {
    site: SiteCtx,
    order: Order,
    codes: Vec<String>,
    verified: bool,
    masked: String,
}

#[derive(Deserialize)]
pub struct ResultQ {
    c: Option<String>,
}

async fn order_result(
    State(state): State<SharedState>,
    Path(order_no): Path<String>,
    Query(q): Query<ResultQ>,
) -> Response {
    let mut db = state.db.lock().await;
    let _ = models::expire_stale_orders(&mut db);
    let site = models::site_ctx(&db);
    let Some(order) = models::get_order_by_no(&db, &order_no).ok().flatten() else {
        return Redirect::to("/?err=订单不存在").into_response();
    };
    let given = q.c.unwrap_or_default();
    let verified = !given.is_empty() && given == order.contact;
    if verified && order.status == 0 {
        return Redirect::to(&format!("/pay/{}", order.order_no)).into_response();
    }
    let codes = if verified && order.status == 1 {
        models::order_codes(&db, order.id).unwrap_or_default()
    } else {
        Vec::new()
    };
    drop(db);
    let masked = mask_contact(&order.contact);
    html(ResultTpl {
        site,
        order,
        codes,
        verified,
        masked,
    })
}
