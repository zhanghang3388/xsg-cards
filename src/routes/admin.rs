use crate::auth::{self, AdminUser};
use crate::config::{admin_base, sanitize_slug};
use crate::models::{self, CardView, Category, Order, ProductView};
use crate::state::SharedState;
use crate::upload;
use crate::util::{enc, html, yuan_to_cents};
use askama::Template;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use serde::Deserialize;

/// 后台内部跳转：自动带上隐藏路径前缀
fn ard(path: &str) -> Response {
    Redirect::to(&format!("{}{}", admin_base(), path)).into_response()
}

/// 批量表单里回传的筛选条件（形如 `product=3&status=0&page=2`）。
/// 会被拼进 Location 头，所以只保留查询串该有的字符，杜绝换行注入。
fn sanitize_back(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || "=&_-.%".contains(*c))
        .take(200)
        .collect()
}

/// 带回筛选条件的跳转：`kind` 取 "msg" 或 "err"
fn ard_back(path: &str, back: &str, kind: &str, msg: &str) -> Response {
    let b = sanitize_back(back);
    let sep = if b.is_empty() { "" } else { "&" };
    ard(&format!("{path}?{b}{sep}{kind}={}", enc(msg)))
}

/// 把当前筛选条件拼成查询串，随批量表单回传，操作完能停在原页原筛选上。空值自动省略。
fn build_back(parts: &[(&str, String)]) -> String {
    parts
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// 翻页链接的前缀：不含 page 的筛选条件，非空时带尾随 &，模板里直接 `?{{ qbase }}page=N`
fn build_qbase(parts: &[(&str, String)]) -> String {
    let s = build_back(parts);
    if s.is_empty() {
        s
    } else {
        format!("{s}&")
    }
}

/// 解析批量操作表单：同名 checkbox 会重复出现，serde_urlencoded 映射不成 Vec，手动拆。
/// 返回 (去重后的 id 列表, 回跳的查询串)
fn parse_bulk(body: &str) -> (Vec<i64>, String) {
    let mut ids: Vec<i64> = Vec::new();
    let mut back = String::new();
    for pair in body.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let decoded = urlencoding::decode(&v.replace('+', " "))
            .map(|c| c.into_owned())
            .unwrap_or_default();
        match k {
            "ids" => {
                if let Ok(n) = decoded.trim().parse::<i64>() {
                    ids.push(n);
                }
            }
            "back" => back = decoded,
            _ => {}
        }
    }
    ids.sort_unstable();
    ids.dedup();
    (ids, back)
}

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
        .route("/products/bulk-delete", post(products_bulk_delete))
        .route("/cards", get(cards_page))
        .route("/cards/import", post(cards_import))
        .route("/cards/{id}/delete", post(card_delete))
        .route("/cards/delete-unsold", post(cards_delete_unsold))
        .route("/cards/bulk-delete", post(cards_bulk_delete))
        .route("/orders", get(orders_page))
        .route("/orders/{no}/deliver", post(order_deliver))
        .route("/orders/bulk-delete", post(orders_bulk_delete))
        .route("/settings", get(settings_page).post(settings_save))
        .route("/settings/test-mail", post(test_mail))
        .route("/password", post(password_change))
        .route_layer(middleware::from_fn_with_state(state, auth::require_admin))
        // 商品图片上传需要更大的请求体上限
        .layer(DefaultBodyLimit::max(6 * 1024 * 1024));

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
    base: &'static str,
}

async fn login_page(State(state): State<SharedState>) -> Response {
    let db = state.db.lock().await;
    let site_name = models::get_setting(&db, "site_name");
    drop(db);
    html(LoginTpl {
        site_name,
        base: admin_base(),
    })
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

async fn login_post(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> Response {
    let ip = auth::client_ip(&headers);
    if let Some(mins) = auth::login_locked(&state, &ip) {
        return ard(&format!(
            "/login?err={}",
            enc(&format!("失败次数过多，请 {mins} 分钟后再试"))
        ));
    }
    let db = state.db.lock().await;
    let admin = auth::verify_login(&db, f.username.trim(), &f.password);
    match admin {
        Some(a) => {
            let token = match auth::create_session(&db, a.id) {
                Ok(t) => t,
                Err(e) => return ard(&format!("/login?err={}", enc(&e.to_string()))),
            };
            drop(db);
            auth::clear_login_fails(&state, &ip);
            let mut resp = ard("/");
            if let Ok(v) = HeaderValue::from_str(&auth::session_cookie_header(&token)) {
                resp.headers_mut().append(header::SET_COOKIE, v);
            }
            resp
        }
        None => {
            drop(db);
            auth::record_login_fail(&state, &ip);
            ard("/login?err=账号或密码不正确")
        }
    }
}

async fn logout(State(state): State<SharedState>, req: axum::extract::Request) -> Response {
    if let Some(token) = crate::util::get_cookie(req.headers(), auth::SESSION_COOKIE) {
        let db = state.db.lock().await;
        auth::destroy_session(&db, &token);
    }
    let mut resp = ard("/login");
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
    base: &'static str,
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
    let mut recent =
        models::list_orders_admin(&db, None, "", &models::Pager::new(1, 10)).unwrap_or_default();
    recent.truncate(10);
    drop(db);
    html(DashTpl {
        username: admin.username,
        active: "dash",
        base: admin_base(),
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
    base: &'static str,
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
        base: admin_base(),
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
        return ard("/categories?err=分类名不能为空");
    }
    let db = state.db.lock().await;
    match models::create_category(&db, name, f.sort.unwrap_or(0)) {
        Ok(_) => ard("/categories?msg=已添加"),
        Err(e) => ard(&format!("/categories?err={}", enc(&e.to_string()))),
    }
}

async fn category_update(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Form(f): Form<CategoryForm>,
) -> Response {
    let db = state.db.lock().await;
    match models::update_category(&db, id, f.name.trim(), f.sort.unwrap_or(0)) {
        Ok(_) => ard("/categories?msg=已保存"),
        Err(e) => ard(&format!("/categories?err={}", enc(&e.to_string()))),
    }
}

async fn category_delete(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    match models::delete_category(&db, id) {
        Ok(_) => ard("/categories?msg=已删除"),
        Err(e) => ard(&format!("/categories?err={}", enc(&e))),
    }
}

// ---------- 商品 ----------

#[derive(Template)]
#[template(path = "admin/products.html")]
struct ProductsTpl {
    username: String,
    active: &'static str,
    base: &'static str,
    products: Vec<ProductView>,
    cats: Vec<Category>,
    pager: models::Pager,
    qbase: String,
    list_path: &'static str,
    back: String,
}

#[derive(Deserialize)]
pub struct PageQ {
    page: Option<i64>,
}

async fn products_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
    Query(qs): Query<PageQ>,
) -> Response {
    let db = state.db.lock().await;
    let pager = models::Pager::new(qs.page.unwrap_or(1), models::count_products(&db));
    let products = models::list_products_admin(&db, &pager).unwrap_or_default();
    let cats = models::list_categories(&db).unwrap_or_default();
    drop(db);
    let back = build_back(&[("page", pager.page.to_string())]);
    html(ProductsTpl {
        username: admin.username,
        active: "products",
        base: admin_base(),
        products,
        cats,
        pager,
        qbase: String::new(),
        list_path: "/products",
        back,
    })
}

struct ProductInput {
    category_id: i64,
    name: String,
    description: String,
    image: String,
    price_cents: i64,
    sort: i64,
}

/// 解析商品表单（multipart：文本字段 + 可选图片文件）
/// 先做完文本校验再落盘图片，避免校验失败留下孤儿文件。
async fn read_product_form(mut mp: Multipart) -> Result<ProductInput, String> {
    let (mut category_id, mut sort) = (0i64, 0i64);
    let (mut name, mut description, mut price) = (String::new(), String::new(), String::new());
    let (mut image_url, mut image_current) = (String::new(), String::new());
    let mut remove_image = false;
    let mut file: Vec<u8> = Vec::new();

    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| format!("表单读取失败：{e}"))?
    {
        let fname = field.name().unwrap_or_default().to_string();
        if fname == "image_file" {
            let data = field
                .bytes()
                .await
                .map_err(|_| "图片上传失败，请确认文件小于 2MB".to_string())?;
            if !data.is_empty() {
                file = data.to_vec();
            }
            continue;
        }
        let v = field
            .text()
            .await
            .map_err(|e| format!("表单读取失败：{e}"))?;
        match fname.as_str() {
            "category_id" => category_id = v.trim().parse().unwrap_or(0),
            "name" => name = v.trim().to_string(),
            "description" => description = v.trim().to_string(),
            "price" => price = v.trim().to_string(),
            "sort" => sort = v.trim().parse().unwrap_or(0),
            "image_url" => image_url = v.trim().to_string(),
            "image_current" => image_current = v.trim().to_string(),
            "remove_image" => remove_image = !v.is_empty(),
            _ => {}
        }
    }

    if name.is_empty() {
        return Err("商品名不能为空".into());
    }
    if category_id <= 0 {
        return Err("请选择商品分类".into());
    }
    let price_cents = yuan_to_cents(&price).ok_or("价格格式不正确，示例：19.90")?;
    if price_cents <= 0 {
        return Err("价格必须大于 0".into());
    }

    // 图片取值优先级：移除 > 新上传 > 外链 > 保持原图
    let image = if remove_image {
        String::new()
    } else if !file.is_empty() {
        upload::save_image(&file)?
    } else if !image_url.is_empty() {
        if !upload::valid_image_ref(&image_url) {
            return Err("图片地址不合法，请填写 http(s):// 开头的完整链接".into());
        }
        image_url
    } else if upload::valid_image_ref(&image_current) {
        image_current
    } else {
        String::new()
    };

    Ok(ProductInput {
        category_id,
        name,
        description,
        image,
        price_cents,
        sort,
    })
}

async fn product_create(State(state): State<SharedState>, mp: Multipart) -> Response {
    let f = match read_product_form(mp).await {
        Ok(v) => v,
        Err(e) => return ard(&format!("/products?err={}", enc(&e))),
    };
    let db = state.db.lock().await;
    match models::create_product(
        &db,
        f.category_id,
        &f.name,
        &f.description,
        &f.image,
        f.price_cents,
        f.sort,
    ) {
        Ok(_) => ard("/products?msg=商品已创建，记得导入卡密"),
        Err(e) => {
            upload::remove_local(&f.image);
            ard(&format!("/products?err={}", enc(&e.to_string())))
        }
    }
}

async fn product_update(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    mp: Multipart,
) -> Response {
    let f = match read_product_form(mp).await {
        Ok(v) => v,
        Err(e) => return ard(&format!("/products?err={}", enc(&e))),
    };
    let db = state.db.lock().await;
    let old_image = models::product_image(&db, id);
    match models::update_product(
        &db,
        id,
        f.category_id,
        &f.name,
        &f.description,
        &f.image,
        f.price_cents,
        f.sort,
    ) {
        Ok(_) => {
            if old_image != f.image {
                upload::remove_local(&old_image);
            }
            ard("/products?msg=已保存")
        }
        Err(e) => ard(&format!("/products?err={}", enc(&e.to_string()))),
    }
}

async fn product_toggle(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    let _ = models::toggle_product(&db, id);
    ard("/products?msg=状态已切换")
}

async fn product_delete(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    let image = models::product_image(&db, id);
    match models::delete_product(&db, id) {
        Ok(_) => {
            upload::remove_local(&image);
            ard("/products?msg=已删除（未售卡密一并清除，已售卡密保留以备查单）")
        }
        Err(e) => ard(&format!("/products?err={}", enc(&e))),
    }
}

async fn products_bulk_delete(State(state): State<SharedState>, body: String) -> Response {
    let (ids, back) = parse_bulk(&body);
    if ids.is_empty() {
        return ard_back("/products", &back, "err", "请先勾选要删除的商品");
    }
    let db = state.db.lock().await;
    let images: Vec<String> = ids.iter().map(|i| models::product_image(&db, *i)).collect();
    let (done, skipped) = models::delete_products_bulk(&db, &ids);
    // 只清理「确实已被删掉」的商品图片：按成功计数猜是哪几个不靠谱，逐个确认商品是否还在。
    for (i, id) in ids.iter().enumerate() {
        let still: i64 = db
            .query_row("SELECT COUNT(*) FROM products WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap_or(1);
        if still == 0 {
            upload::remove_local(&images[i]);
        }
    }
    drop(db);
    let msg = if skipped.is_empty() {
        format!("已删除 {done} 个商品")
    } else {
        format!(
            "已删除 {done} 个商品，{} 个因卡密被待付订单锁定而跳过",
            skipped.len()
        )
    };
    ard_back("/products", &back, "msg", &msg)
}

// ---------- 卡密 ----------

#[derive(Template)]
#[template(path = "admin/cards.html")]
struct CardsTpl {
    username: String,
    active: &'static str,
    base: &'static str,
    cards: Vec<CardView>,
    products: Vec<models::ProductOpt>,
    f_product: i64,
    f_status: i64,
    q: String,
    pager: models::Pager,
    back: String,
    qbase: String,
    list_path: &'static str,
}

#[derive(Deserialize)]
pub struct CardsQ {
    product: Option<i64>,
    status: Option<i64>,
    q: Option<String>,
    page: Option<i64>,
}

async fn cards_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
    Query(qs): Query<CardsQ>,
) -> Response {
    let f_product = qs.product.unwrap_or(0);
    let f_status = qs.status.unwrap_or(-1);
    let q = qs.q.unwrap_or_default();
    let pid = if f_product > 0 { Some(f_product) } else { None };
    let st = if f_status >= 0 { Some(f_status) } else { None };
    let db = state.db.lock().await;
    let pager = models::Pager::new(
        qs.page.unwrap_or(1),
        models::count_cards(&db, pid, st, q.trim()),
    );
    let cards = models::list_cards(&db, pid, st, q.trim(), &pager).unwrap_or_default();
    let products = models::product_options(&db).unwrap_or_default();
    drop(db);
    let filters = [
        ("product", pid.map(|v| v.to_string()).unwrap_or_default()),
        ("status", st.map(|v| v.to_string()).unwrap_or_default()),
        ("q", q.trim().to_string()),
    ];
    let qbase = build_qbase(&filters);
    let mut back_parts = filters.to_vec();
    back_parts.push(("page", pager.page.to_string()));
    let back = build_back(&back_parts);
    html(CardsTpl {
        username: admin.username,
        active: "cards",
        base: admin_base(),
        cards,
        products,
        f_product,
        f_status,
        q,
        pager,
        back,
        qbase,
        list_path: "/cards",
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
            ard(&format!("/cards?product={}&msg={}", f.product_id, enc(&msg)))
        }
        Err(e) => ard(&format!("/cards?err={}", enc(&e.to_string()))),
    }
}

async fn card_delete(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    let _ = models::delete_card(&db, id);
    ard("/cards?msg=已删除（仅可删除未售卡密）")
}

async fn cards_bulk_delete(State(state): State<SharedState>, body: String) -> Response {
    let (ids, back) = parse_bulk(&body);
    if ids.is_empty() {
        return ard_back("/cards", &back, "err", "请先勾选要删除的卡密");
    }
    let db = state.db.lock().await;
    let r = models::delete_cards_bulk(&db, &ids);
    drop(db);
    match r {
        Ok((done, skip)) => {
            let msg = if skip > 0 {
                format!("已删除 {done} 张，{skip} 张已售或锁定中未删")
            } else {
                format!("已删除 {done} 张卡密")
            };
            ard_back("/cards", &back, "msg", &msg)
        }
        Err(e) => ard_back("/cards", &back, "err", &e.to_string()),
    }
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
        Ok(n) => ard(&format!(
            "/cards?product={}&msg={}",
            f.product_id,
            enc(&format!("已清空 {n} 张未售卡密"))
        )),
        Err(e) => ard(&format!("/cards?err={}", enc(&e.to_string()))),
    }
}

// ---------- 订单 ----------

#[derive(Template)]
#[template(path = "admin/orders.html")]
struct OrdersTpl {
    username: String,
    active: &'static str,
    base: &'static str,
    orders: Vec<Order>,
    f_status: i64,
    q: String,
    pager: models::Pager,
    back: String,
    qbase: String,
    list_path: &'static str,
}

#[derive(Deserialize)]
pub struct OrdersQ {
    status: Option<i64>,
    q: Option<String>,
    page: Option<i64>,
}

async fn orders_page(
    State(state): State<SharedState>,
    Extension(admin): Extension<AdminUser>,
    Query(qs): Query<OrdersQ>,
) -> Response {
    let f_status = qs.status.unwrap_or(-1);
    let q = qs.q.unwrap_or_default();
    let st = if f_status >= 0 { Some(f_status) } else { None };
    let mut db = state.db.lock().await;
    let _ = models::expire_stale_orders(&mut db);
    let pager = models::Pager::new(
        qs.page.unwrap_or(1),
        models::count_orders_admin(&db, st, q.trim()),
    );
    let orders = models::list_orders_admin(&db, st, q.trim(), &pager).unwrap_or_default();
    drop(db);
    let filters = [
        ("status", st.map(|v| v.to_string()).unwrap_or_default()),
        ("q", q.trim().to_string()),
    ];
    let qbase = build_qbase(&filters);
    let mut back_parts = filters.to_vec();
    back_parts.push(("page", pager.page.to_string()));
    let back = build_back(&back_parts);
    html(OrdersTpl {
        username: admin.username,
        active: "orders",
        base: admin_base(),
        orders,
        f_status,
        q,
        pager,
        back,
        qbase,
        list_path: "/orders",
    })
}

/// 人工补单 / 补发：待支付或已付待补发的订单直接发货
async fn order_deliver(State(state): State<SharedState>, Path(no): Path<String>) -> Response {
    let mut db = state.db.lock().await;
    let r = models::deliver_order(&mut db, &no, "manual");
    if let Ok(st) = &r {
        crate::mailer::notify_after_deliver(&db, &no, *st);
    }
    drop(db);
    match r {
        Ok(1) => ard("/orders?msg=已发货"),
        Ok(3) => ard("/orders?err=库存不足，已标记待补发，请先导入卡密再重试"),
        Ok(_) => ard("/orders?msg=处理完成"),
        Err(e) => ard(&format!("/orders?err={}", enc(&e))),
    }
}

async fn orders_bulk_delete(State(state): State<SharedState>, body: String) -> Response {
    let (ids, back) = parse_bulk(&body);
    if ids.is_empty() {
        return ard_back("/orders", &back, "err", "请先勾选要删除的订单");
    }
    let mut db = state.db.lock().await;
    let r = models::delete_orders_bulk(&mut db, &ids);
    drop(db);
    match r {
        Ok(n) => ard_back("/orders", &back, "msg", &format!("已删除 {n} 笔订单")),
        Err(e) => ard_back("/orders", &back, "err", &e.to_string()),
    }
}

// ---------- 设置 ----------

#[derive(Template)]
#[template(path = "admin/settings.html")]
struct SettingsTpl {
    username: String,
    active: &'static str,
    base: &'static str,
    site_name: String,
    site_mark: String,
    site_subtitle: String,
    announcement: String,
    default_theme: String,
    site_url: String,
    admin_path: String,
    pay_mode: String,
    epay_gateway: String,
    epay_pid: String,
    epay_key: String,
    epay_alipay: bool,
    epay_wxpay: bool,
    mail_enabled: bool,
    smtp_host: String,
    smtp_port: String,
    smtp_security: String,
    smtp_user: String,
    smtp_pass_set: bool,
    mail_from: String,
    mail_from_name: String,
    mail_admin_to: String,
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
        base: admin_base(),
        site_name: g("site_name"),
        site_mark: g("site_mark"),
        site_subtitle: g("site_subtitle"),
        announcement: g("announcement"),
        default_theme: g("default_theme"),
        site_url: g("site_url"),
        admin_path: crate::config::admin_slug().to_string(),
        pay_mode: g("pay_mode"),
        epay_gateway: g("epay_gateway"),
        epay_pid: g("epay_pid"),
        epay_key: g("epay_key"),
        epay_alipay: g("epay_alipay") == "1",
        epay_wxpay: g("epay_wxpay") == "1",
        mail_enabled: g("mail_enabled") == "1",
        smtp_host: g("smtp_host"),
        smtp_port: g("smtp_port"),
        smtp_security: g("smtp_security"),
        smtp_user: g("smtp_user"),
        // 密码永不回显，只告诉页面「已设置过」
        smtp_pass_set: !g("smtp_pass").is_empty(),
        mail_from: g("mail_from"),
        mail_from_name: g("mail_from_name"),
        mail_admin_to: g("mail_admin_to"),
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
    admin_path: String,
    pay_mode: String,
    epay_gateway: String,
    epay_pid: String,
    epay_key: String,
    epay_alipay: Option<String>,
    epay_wxpay: Option<String>,
    mail_enabled: Option<String>,
    smtp_host: String,
    smtp_port: String,
    smtp_security: String,
    smtp_user: String,
    smtp_pass: String,
    mail_from: String,
    mail_from_name: String,
    mail_admin_to: String,
}

async fn settings_save(State(state): State<SharedState>, Form(f): Form<SettingsForm>) -> Response {
    let theme = if f.default_theme == "light" { "light" } else { "dark" };
    let pay_mode = if f.pay_mode == "epay" { "epay" } else { "mock" };
    let new_slug = match sanitize_slug(&f.admin_path) {
        Some(s) => s,
        None => {
            return ard("/settings?err=后台路径需为 4-32 位字母数字（可含 - _），且不能使用 admin 等保留字")
        }
    };
    let mail_on = if f.mail_enabled.is_some() { "1" } else { "0" };
    // 未勾选的 checkbox 根本不会出现在表单里，所以 None 就是关
    let alipay_on = if f.epay_alipay.is_some() { "1" } else { "0" };
    let wxpay_on = if f.epay_wxpay.is_some() { "1" } else { "0" };
    let security = match f.smtp_security.as_str() {
        "starttls" => "starttls",
        "none" => "none",
        _ => "ssl",
    };
    let port = f.smtp_port.trim().parse::<u16>().unwrap_or(0);
    if mail_on == "1" && port == 0 {
        return ard("/settings?err=SMTP 端口需为 1-65535 的数字（SSL 常用 465，STARTTLS 常用 587）");
    }
    let port_str = if port == 0 {
        "465".to_string()
    } else {
        port.to_string()
    };
    let db = state.db.lock().await;
    let pairs: [(&str, &str); 21] = [
        ("site_name", f.site_name.trim()),
        ("site_mark", f.site_mark.trim()),
        ("site_subtitle", f.site_subtitle.trim()),
        ("announcement", f.announcement.trim()),
        ("default_theme", theme),
        ("site_url", f.site_url.trim().trim_end_matches('/')),
        ("admin_path", new_slug.as_str()),
        ("pay_mode", pay_mode),
        ("epay_gateway", f.epay_gateway.trim()),
        ("epay_pid", f.epay_pid.trim()),
        ("epay_key", f.epay_key.trim()),
        ("epay_alipay", alipay_on),
        ("epay_wxpay", wxpay_on),
        ("mail_enabled", mail_on),
        ("smtp_host", f.smtp_host.trim()),
        ("smtp_port", port_str.as_str()),
        ("smtp_security", security),
        ("smtp_user", f.smtp_user.trim()),
        ("mail_from", f.mail_from.trim()),
        ("mail_from_name", f.mail_from_name.trim()),
        ("mail_admin_to", f.mail_admin_to.trim()),
    ];
    for (k, v) in pairs {
        if let Err(e) = models::set_setting(&db, k, v) {
            return ard(&format!("/settings?err={}", enc(&e.to_string())));
        }
    }
    // 密码留空 = 沿用原值，这样保存其它设置时不用重新输一遍授权码
    if !f.smtp_pass.is_empty() {
        if let Err(e) = models::set_setting(&db, "smtp_pass", f.smtp_pass.trim()) {
            return ard(&format!("/settings?err={}", enc(&e.to_string())));
        }
    }
    drop(db);
    if new_slug != crate::config::admin_slug() {
        return ard(&format!(
            "/settings?msg={}",
            enc(&format!("已保存。后台路径将在服务重启后变为 /{new_slug}"))
        ));
    }
    ard("/settings?msg=已保存")
}

#[derive(Deserialize)]
pub struct TestMailForm {
    to: String,
}

/// 发一封测试信，验证 SMTP 参数是否可用。这里同步等待发送结果，
/// 因为站长需要看到真实的错误原因（认证失败 / 端口不通 / 发件人被拒）。
async fn test_mail(State(state): State<SharedState>, Form(f): Form<TestMailForm>) -> Response {
    let to = f.to.trim().to_string();
    if !crate::mailer::is_email(&to) {
        return ard("/settings?err=请填写一个有效的收件邮箱");
    }
    let db = state.db.lock().await;
    let mut cfg = crate::mailer::mail_cfg(&db);
    drop(db);
    // 测试信不受「启用」开关限制，方便先测通再打开
    cfg.enabled = true;
    if cfg.host.is_empty() || cfg.from.is_empty() {
        return ard("/settings?err=请先填写 SMTP 服务器与发件人地址并保存，再发测试信");
    }
    match crate::mailer::send_test(cfg, to.clone()).await {
        Ok(()) => ard(&format!(
            "/settings?msg={}",
            enc(&format!("测试邮件已发往 {to}，请查收（也看看垃圾箱）"))
        )),
        Err(e) => ard(&format!("/settings?err={}", enc(&e))),
    }
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
        Ok(_) => ard("/login?msg=密码已修改，请重新登录"),
        Err(e) => ard(&format!("/settings?err={}", enc(&e))),
    }
}
