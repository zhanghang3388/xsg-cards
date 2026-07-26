use crate::auth::{self, AdminUser};
use crate::models::{self, Category, CardView, Order, ProductView};
use crate::state::SharedState;
use crate::util::{enc, html, yuan_to_cents};
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use serde::Deserialize;

pub fn router(state: SharedState) -> Router<SharedState> {
    let protected = Router::new()
        .route("/", get(dashboard))
        .route("/categories", get(categories_page).post(category_create))
        .route("/categories/{id}", post(category_update))
        .route("/categories/{id}/delete", post(category_delete))
        .route("/products", get(products_page).post(product_create))
        .route("/products/{id}", post(product_update))
        .route("/products/{id}/toggle", post(product_toggle))
        .route("/products/{id}/delete", post(product_delete))
        .route("/cards", get(cards_page))
        .route("/cards/import", post(cards_import))
        .route("/cards/{id}/delete", post(card_delete))
        .route("/cards/delete-unsold", post(cards_delete_unsold))
        .route("/orders", get(orders_page))
        .route("/orders/{no}/deliver", post(order_deliver))
        .route("/settings", get(settings_page).post(settings_save))
        .route("/password", post(password_change))
        .route_layer(middleware::from_fn_with_state(
            state,
            auth::require_admin,
        ));

    Router::new()
        .route("/login", get(login_page).post(login_post))
        .route("/logout", post(logout))
        .merge(protected)
}

// ---------- 登录 ----------

#[derive(Template)]
#[template(path = "admin/login.html")]
struct LoginTpl {
    site_name: String,
}

async fn login_page(State(state): State<SharedState>) -> Response {
    let db = state.db.lock().await;
    let site_name = models::get_setting(&db, "site_name");
    drop(db);
    html(LoginTpl { site_name })
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

async fn login_post(State(state): State<SharedState>, Form(f): Form<LoginForm>) -> Response {
    let db = state.db.lock().await;
    let admin = auth::verify_login(&db, f.username.trim(), &f.password);
    match admin {
        Some(a) => {
            let token = match auth::create_session(&db, a.id) {
                Ok(t) => t,
                Err(e) => {
                    return Redirect::to(&format!("/admin/login?err={}", enc(&e.to_string())))
                        .into_response()
                }
            };
            drop(db);
            let mut resp = Redirect::to("/admin").into_response();
            if let Ok(v) = HeaderValue::from_str(&auth::session_cookie_header(&token)) {
                resp.headers_mut().append(header::SET_COOKIE, v);
            }
            resp
        }
        None => Redirect::to("/admin/login?err=账号或密码不正确").into_response(),
    }
}

async fn logout(State(state): State<SharedState>, req: axum::extract::Request) -> Response {
    if let Some(token) = crate::util::get_cookie(req.headers(), auth::SESSION_COOKIE) {
        let db = state.db.lock().await;
        auth::destroy_session(&db, &token);
    }
    let mut resp = Redirect::to("/admin/login").into_response();
    if let Ok(v) = HeaderValue::from_str(&auth::clear_cookie_header()) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    resp
}

// ---------- 仪表盘 ----------

#[derive(Template)]
#[template(path = "admin/dashboard.html")]
struct DashTpl {
    username: String,
    active: &'static str,
    stats: models::DashStats,
    low_stock: Vec<ProductView>,
    recent: Vec<Order>,
}

async fn dashboard(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
) -> Response {
    let mut db = state.db.lock().await;
    let _ = models::expire_stale_orders(&mut db);
    let stats = match models::dash_stats(&db) {
        Ok(s) => s,
        Err(e) => return err_page(&e.to_string()),
    };
    let low_stock = models::low_stock_products(&db).unwrap_or_default();
    let mut recent = models::list_orders_admin(&db, None, "").unwrap_or_default();
    recent.truncate(10);
    drop(db);
    html(DashTpl {
        username: admin.username,
        active: "dash",
        stats,
        low_stock,
        recent,
    })
}

fn err_page(e: &str) -> Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("服务器错误: {e}"),
    )
        .into_response()
}

// ---------- 分类 ----------

#[derive(Template)]
#[template(path = "admin/categories.html")]
struct CategoriesTpl {
    username: String,
    active: &'static str,
    cats: Vec<Category>,
}

async fn categories_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
) -> Response {
    let db = state.db.lock().await;
    let cats = models::list_categories(&db).unwrap_or_default();
    drop(db);
    html(CategoriesTpl {
        username: admin.username,
        active: "categories",
        cats,
    })
}

#[derive(Deserialize)]
pub struct CategoryForm {
    name: String,
    sort: Option<i64>,
}

async fn category_create(State(state): State<SharedState>, Form(f): Form<CategoryForm>) -> Response {
    let name = f.name.trim();
    if name.is_empty() {
        return Redirect::to("/admin/categories?err=分类名不能为空").into_response();
    }
    let db = state.db.lock().await;
    match models::create_category(&db, name, f.sort.unwrap_or(0)) {
        Ok(_) => Redirect::to("/admin/categories?msg=已添加").into_response(),
        Err(e) => Redirect::to(&format!("/admin/categories?err={}", enc(&e.to_string()))).into_response(),
    }
}

async fn category_update(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Form(f): Form<CategoryForm>,
) -> Response {
    let db = state.db.lock().await;
    match models::update_category(&db, id, f.name.trim(), f.sort.unwrap_or(0)) {
        Ok(_) => Redirect::to("/admin/categories?msg=已保存").into_response(),
        Err(e) => Redirect::to(&format!("/admin/categories?err={}", enc(&e.to_string()))).into_response(),
    }
}

async fn category_delete(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    match models::delete_category(&db, id) {
        Ok(_) => Redirect::to("/admin/categories?msg=已删除").into_response(),
        Err(e) => Redirect::to(&format!("/admin/categories?err={}", enc(&e))).into_response(),
    }
}

// ---------- 商品 ----------

#[derive(Template)]
#[template(path = "admin/products.html")]
struct ProductsTpl {
    username: String,
    active: &'static str,
    products: Vec<ProductView>,
    cats: Vec<Category>,
}

async fn products_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
) -> Response {
    let db = state.db.lock().await;
    let products = models::list_products_admin(&db).unwrap_or_default();
    let cats = models::list_categories(&db).unwrap_or_default();
    drop(db);
    html(ProductsTpl {
        username: admin.username,
        active: "products",
        products,
        cats,
    })
}

#[derive(Deserialize)]
pub struct ProductForm {
    category_id: i64,
    name: String,
    description: String,
    price: String,
    sort: Option<i64>,
}

fn parse_product_form(f: &ProductForm) -> Result<(String, i64), String> {
    let name = f.name.trim().to_string();
    if name.is_empty() {
        return Err("商品名不能为空".into());
    }
    let cents = yuan_to_cents(&f.price).ok_or("价格格式不正确，示例：19.90")?;
    if cents <= 0 {
        return Err("价格必须大于 0".into());
    }
    Ok((name, cents))
}

async fn product_create(State(state): State<SharedState>, Form(f): Form<ProductForm>) -> Response {
    let (name, cents) = match parse_product_form(&f) {
        Ok(v) => v,
        Err(e) => return Redirect::to(&format!("/admin/products?err={}", enc(&e))).into_response(),
    };
    let db = state.db.lock().await;
    match models::create_product(&db, f.category_id, &name, f.description.trim(), cents, f.sort.unwrap_or(0)) {
        Ok(_) => Redirect::to("/admin/products?msg=商品已创建，记得导入卡密").into_response(),
        Err(e) => Redirect::to(&format!("/admin/products?err={}", enc(&e.to_string()))).into_response(),
    }
}

async fn product_update(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Form(f): Form<ProductForm>,
) -> Response {
    let (name, cents) = match parse_product_form(&f) {
        Ok(v) => v,
        Err(e) => return Redirect::to(&format!("/admin/products?err={}", enc(&e))).into_response(),
    };
    let db = state.db.lock().await;
    match models::update_product(&db, id, f.category_id, &name, f.description.trim(), cents, f.sort.unwrap_or(0)) {
        Ok(_) => Redirect::to("/admin/products?msg=已保存").into_response(),
        Err(e) => Redirect::to(&format!("/admin/products?err={}", enc(&e.to_string()))).into_response(),
    }
}

async fn product_toggle(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    let _ = models::toggle_product(&db, id);
    Redirect::to("/admin/products?msg=状态已切换").into_response()
}

async fn product_delete(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    match models::delete_product(&db, id) {
        Ok(_) => Redirect::to("/admin/products?msg=已删除（未售卡密一并清除）").into_response(),
        Err(e) => Redirect::to(&format!("/admin/products?err={}", enc(&e.to_string()))).into_response(),
    }
}

// ---------- 卡密 ----------

#[derive(Template)]
#[template(path = "admin/cards.html")]
struct CardsTpl {
    username: String,
    active: &'static str,
    cards: Vec<CardView>,
    products: Vec<ProductView>,
    f_product: i64,
    f_status: i64,
    q: String,
}

#[derive(Deserialize)]
pub struct CardsQ {
    product: Option<i64>,
    status: Option<i64>,
    q: Option<String>,
}

async fn cards_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
    Query(qs): Query<CardsQ>,
) -> Response {
    let f_product = qs.product.unwrap_or(0);
    let f_status = qs.status.unwrap_or(-1);
    let q = qs.q.unwrap_or_default();
    let db = state.db.lock().await;
    let cards = models::list_cards(
        &db,
        if f_product > 0 { Some(f_product) } else { None },
        if f_status >= 0 { Some(f_status) } else { None },
        q.trim(),
    )
    .unwrap_or_default();
    let products = models::list_products_admin(&db).unwrap_or_default();
    drop(db);
    html(CardsTpl {
        username: admin.username,
        active: "cards",
        cards,
        products,
        f_product,
        f_status,
        q,
    })
}

#[derive(Deserialize)]
pub struct ImportForm {
    product_id: i64,
    codes: String,
}

async fn cards_import(State(state): State<SharedState>, Form(f): Form<ImportForm>) -> Response {
    let mut db = state.db.lock().await;
    match models::import_cards(&mut db, f.product_id, &f.codes) {
        Ok((ok, skip)) => {
            let msg = if skip > 0 {
                format!("导入 {ok} 张，跳过重复 {skip} 张")
            } else {
                format!("导入 {ok} 张")
            };
            Redirect::to(&format!("/admin/cards?product={}&msg={}", f.product_id, enc(&msg))).into_response()
        }
        Err(e) => Redirect::to(&format!("/admin/cards?err={}", enc(&e.to_string()))).into_response(),
    }
}

async fn card_delete(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    let _ = models::delete_card(&db, id);
    Redirect::to("/admin/cards?msg=已删除（仅可删除未售卡密）").into_response()
}

#[derive(Deserialize)]
pub struct DeleteUnsoldForm {
    product_id: i64,
}

async fn cards_delete_unsold(
    State(state): State<SharedState>,
    Form(f): Form<DeleteUnsoldForm>,
) -> Response {
    let db = state.db.lock().await;
    match models::delete_unsold_cards(&db, f.product_id) {
        Ok(n) => Redirect::to(&format!("/admin/cards?product={}&msg={}", f.product_id, enc(&format!("已清空 {n} 张未售卡密")))).into_response(),
        Err(e) => Redirect::to(&format!("/admin/cards?err={}", enc(&e.to_string()))).into_response(),
    }
}

// ---------- 订单 ----------

#[derive(Template)]
#[template(path = "admin/orders.html")]
struct OrdersTpl {
    username: String,
    active: &'static str,
    orders: Vec<Order>,
    f_status: i64,
    q: String,
}

#[derive(Deserialize)]
pub struct OrdersQ {
    status: Option<i64>,
    q: Option<String>,
}

async fn orders_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
    Query(qs): Query<OrdersQ>,
) -> Response {
    let f_status = qs.status.unwrap_or(-1);
    let q = qs.q.unwrap_or_default();
    let mut db = state.db.lock().await;
    let _ = models::expire_stale_orders(&mut db);
    let orders = models::list_orders_admin(
        &db,
        if f_status >= 0 { Some(f_status) } else { None },
        q.trim(),
    )
    .unwrap_or_default();
    drop(db);
    html(OrdersTpl {
        username: admin.username,
        active: "orders",
        orders,
        f_status,
        q,
    })
}

/// 人工补单 / 补发：待支付或已付待补发的订单直接发货
async fn order_deliver(State(state): State<SharedState>, Path(no): Path<String>) -> Response {
    let mut db = state.db.lock().await;
    match models::deliver_order(&mut db, &no, "manual") {
        Ok(1) => Redirect::to("/admin/orders?msg=已发货").into_response(),
        Ok(3) => Redirect::to("/admin/orders?err=库存不足，已标记待补发，请先导入卡密再重试").into_response(),
        Ok(_) => Redirect::to("/admin/orders?msg=处理完成").into_response(),
        Err(e) => Redirect::to(&format!("/admin/orders?err={}", enc(&e))).into_response(),
    }
}

// ---------- 设置 ----------

#[derive(Template)]
#[template(path = "admin/settings.html")]
struct SettingsTpl {
    username: String,
    active: &'static str,
    site_name: String,
    site_mark: String,
    site_subtitle: String,
    announcement: String,
    default_theme: String,
    site_url: String,
    pay_mode: String,
    epay_gateway: String,
    epay_pid: String,
    epay_key: String,
}

async fn settings_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
) -> Response {
    let db = state.db.lock().await;
    let g = |k: &str| models::get_setting(&db, k);
    let tpl = SettingsTpl {
        username: admin.username,
        active: "settings",
        site_name: g("site_name"),
        site_mark: g("site_mark"),
        site_subtitle: g("site_subtitle"),
        announcement: g("announcement"),
        default_theme: g("default_theme"),
        site_url: g("site_url"),
        pay_mode: g("pay_mode"),
        epay_gateway: g("epay_gateway"),
        epay_pid: g("epay_pid"),
        epay_key: g("epay_key"),
    };
    drop(db);
    html(tpl)
}

#[derive(Deserialize)]
pub struct SettingsForm {
    site_name: String,
    site_mark: String,
    site_subtitle: String,
    announcement: String,
    default_theme: String,
    site_url: String,
    pay_mode: String,
    epay_gateway: String,
    epay_pid: String,
    epay_key: String,
}

async fn settings_save(State(state): State<SharedState>, Form(f): Form<SettingsForm>) -> Response {
    let theme = if f.default_theme == "light" { "light" } else { "dark" };
    let pay_mode = if f.pay_mode == "epay" { "epay" } else { "mock" };
    let db = state.db.lock().await;
    let pairs: [(&str, &str); 10] = [
        ("site_name", f.site_name.trim()),
        ("site_mark", f.site_mark.trim()),
        ("site_subtitle", f.site_subtitle.trim()),
        ("announcement", f.announcement.trim()),
        ("default_theme", theme),
        ("site_url", f.site_url.trim().trim_end_matches('/')),
        ("pay_mode", pay_mode),
        ("epay_gateway", f.epay_gateway.trim()),
        ("epay_pid", f.epay_pid.trim()),
        ("epay_key", f.epay_key.trim()),
    ];
    for (k, v) in pairs {
        if let Err(e) = models::set_setting(&db, k, v) {
            return Redirect::to(&format!("/admin/settings?err={}", enc(&e.to_string()))).into_response();
        }
    }
    Redirect::to("/admin/settings?msg=已保存").into_response()
}

#[derive(Deserialize)]
pub struct PasswordForm {
    old_password: String,
    new_password: String,
}

async fn password_change(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
    Form(f): Form<PasswordForm>,
) -> Response {
    let db = state.db.lock().await;
    match auth::change_password(&db, admin.id, &f.old_password, &f.new_password) {
        Ok(_) => Redirect::to("/admin/login?msg=密码已修改，请重新登录").into_response(),
        Err(e) => Redirect::to(&format!("/admin/settings?err={}", enc(&e))).into_response(),
    }
}
