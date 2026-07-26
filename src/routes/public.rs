use crate::models::{self, Category, ProductView, SiteCtx};
use crate::state::SharedState;
use crate::util::html;
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/", get(index))
        .route("/p/{id}", get(product_page))
        .route("/query", get(query_page))
}

// ---------- 首页 ----------

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTpl {
    site: SiteCtx,
    categories: Vec<Category>,
    products: Vec<ProductView>,
    active_cat: i64,
    on_sale: i64,
    sold_total: i64,
}

#[derive(Deserialize)]
pub struct IndexQ {
    cat: Option<i64>,
}

async fn index(State(state): State<SharedState>, Query(q): Query<IndexQ>) -> Response {
    let db = state.db.lock().await;
    let site = models::site_ctx(&db);
    let categories = models::list_categories(&db).unwrap_or_default();
    let products = models::list_products_public(&db, q.cat).unwrap_or_default();
    let (on_sale, sold_total) = models::public_stats(&db);
    drop(db);
    html(IndexTpl {
        site,
        categories,
        products,
        active_cat: q.cat.unwrap_or(0),
        on_sale,
        sold_total,
    })
}

// ---------- 商品详情 ----------

#[derive(Template)]
#[template(path = "product.html")]
struct ProductTpl {
    site: SiteCtx,
    pv: ProductView,
}

async fn product_page(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    let db = state.db.lock().await;
    let site = models::site_ctx(&db);
    let pv = models::get_product_view(&db, id).ok().flatten();
    drop(db);
    match pv {
        Some(pv) if pv.p.status == 1 => html(ProductTpl { site, pv }),
        _ => Redirect::to("/?err=商品不存在或已下架").into_response(),
    }
}

// ---------- 订单查询 ----------

#[derive(Template)]
#[template(path = "query.html")]
struct QueryTpl {
    site: SiteCtx,
    orders: Vec<models::Order>,
    searched: bool,
    order_no: String,
    contact: String,
}

#[derive(Deserialize)]
pub struct QueryQ {
    order_no: Option<String>,
    contact: Option<String>,
}

async fn query_page(State(state): State<SharedState>, Query(q): Query<QueryQ>) -> Response {
    let order_no = q.order_no.unwrap_or_default().trim().to_string();
    let contact = q.contact.unwrap_or_default().trim().to_string();
    let db = state.db.lock().await;
    let site = models::site_ctx(&db);
    let (orders, searched) = if !contact.is_empty() {
        (
            models::query_orders_by_contact(&db, &order_no, &contact).unwrap_or_default(),
            true,
        )
    } else {
        (Vec::new(), false)
    };
    drop(db);
    html(QueryTpl {
        site,
        orders,
        searched,
        order_no,
        contact,
    })
}

// ---------- 404 ----------

pub async fn not_found(State(state): State<SharedState>) -> Response {
    let db = state.db.lock().await;
    let site = models::site_ctx(&db);
    drop(db);
    #[derive(Template)]
    #[template(path = "404.html")]
    struct NotFoundTpl {
        site: SiteCtx,
    }
    (StatusCode::NOT_FOUND, html(NotFoundTpl { site })).into_response()
}
