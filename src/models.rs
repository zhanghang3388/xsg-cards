use crate::util::{cents_str, gen_order_no, minutes_ago_str, now_str};
use rusqlite::{Connection, OptionalExtension, Result, Row};

pub const ORDER_TTL_MIN: i64 = 15; // 待付订单有效期（分钟）

// ---------- 站点设置 ----------

pub fn get_setting(conn: &Connection, key: &str) -> String {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or_default()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )?;
    Ok(())
}

#[derive(Clone)]
pub struct SiteCtx {
    pub name: String,
    pub mark: String,
    pub subtitle: String,
    pub announcement: String,
    pub theme: String,
}

pub fn site_ctx(conn: &Connection) -> SiteCtx {
    SiteCtx {
        name: get_setting(conn, "site_name"),
        mark: get_setting(conn, "site_mark"),
        subtitle: get_setting(conn, "site_subtitle"),
        announcement: get_setting(conn, "announcement"),
        theme: {
            let t = get_setting(conn, "default_theme");
            if t == "light" { t } else { "dark".into() }
        },
    }
}

// ---------- 分类 ----------

pub struct Category {
    pub id: i64,
    pub name: String,
    pub sort: i64,
    pub product_count: i64,
}

pub fn list_categories(conn: &Connection) -> Result<Vec<Category>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name, c.sort,
                (SELECT COUNT(*) FROM products p WHERE p.category_id = c.id) AS pc
         FROM categories c ORDER BY c.sort DESC, c.id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Category {
            id: r.get(0)?,
            name: r.get(1)?,
            sort: r.get(2)?,
            product_count: r.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn create_category(conn: &Connection, name: &str, sort: i64) -> Result<()> {
    conn.execute("INSERT INTO categories(name, sort) VALUES(?1, ?2)", (name, sort))?;
    Ok(())
}

pub fn update_category(conn: &Connection, id: i64, name: &str, sort: i64) -> Result<()> {
    conn.execute(
        "UPDATE categories SET name = ?1, sort = ?2 WHERE id = ?3",
        (name, sort, id),
    )?;
    Ok(())
}

/// 删除分类：仅当分类下没有商品
pub fn delete_category(conn: &Connection, id: i64) -> std::result::Result<(), String> {
    let cnt: i64 = conn
        .query_row("SELECT COUNT(*) FROM products WHERE category_id = ?1", [id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if cnt > 0 {
        return Err(format!("该分类下还有 {cnt} 个商品，请先移除商品"));
    }
    conn.execute("DELETE FROM categories WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- 商品 ----------

#[derive(Clone)]
pub struct Product {
    pub id: i64,
    pub category_id: i64,
    pub name: String,
    pub description: String,
    pub image: String,
    pub price_cents: i64,
    pub status: i64,
    pub sort: i64,
}

impl Product {
    /// 模板用：是否配了图
    pub fn has_image(&self) -> bool {
        !self.image.is_empty()
    }
}

fn product_from_row(r: &Row) -> Result<Product> {
    Ok(Product {
        id: r.get(0)?,
        category_id: r.get(1)?,
        name: r.get(2)?,
        description: r.get(3)?,
        image: r.get(4)?,
        price_cents: r.get(5)?,
        status: r.get(6)?,
        sort: r.get(7)?,
    })
}

/// 商品 + 库存 + 分类名（列表/详情视图）
pub struct ProductView {
    pub p: Product,
    pub stock: i64,
    pub sold: i64,
    pub category_name: String,
    pub price_str: String,
}

const PRODUCT_COLS: &str =
    "p.id, p.category_id, p.name, p.description, p.image, p.price_cents, p.status, p.sort";

fn product_view_from_row(r: &Row) -> Result<ProductView> {
    let p = product_from_row(r)?;
    let stock: i64 = r.get(8)?;
    let sold: i64 = r.get(9)?;
    let cname: String = r.get(10)?;
    let price_str = cents_str(p.price_cents);
    Ok(ProductView { p, stock, sold, category_name: cname, price_str })
}

fn product_view_query(where_clause: &str) -> String {
    format!(
        "SELECT {PRODUCT_COLS},
                (SELECT COUNT(*) FROM cards k WHERE k.product_id = p.id AND k.status = 0) AS stock,
                (SELECT COUNT(*) FROM cards k WHERE k.product_id = p.id AND k.status = 1) AS sold,
                COALESCE(c.name, '未分类') AS cname
         FROM products p LEFT JOIN categories c ON c.id = p.category_id
         {where_clause}
         ORDER BY p.sort DESC, p.id DESC"
    )
}

pub fn list_products_public(conn: &Connection, cat: Option<i64>) -> Result<Vec<ProductView>> {
    match cat {
        Some(cid) => {
            let sql = product_view_query("WHERE p.status = 1 AND p.category_id = ?1");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([cid], product_view_from_row)?;
            rows.collect()
        }
        None => {
            let sql = product_view_query("WHERE p.status = 1");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], product_view_from_row)?;
            rows.collect()
        }
    }
}

pub fn list_products_admin(conn: &Connection) -> Result<Vec<ProductView>> {
    let sql = product_view_query("");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], product_view_from_row)?;
    rows.collect()
}

pub fn get_product_view(conn: &Connection, id: i64) -> Result<Option<ProductView>> {
    let sql = product_view_query("WHERE p.id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([id], product_view_from_row)?;
    rows.next().transpose()
}

pub fn create_product(
    conn: &Connection,
    category_id: i64,
    name: &str,
    description: &str,
    image: &str,
    price_cents: i64,
    sort: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO products(category_id, name, description, image, price_cents, status, sort, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
        (category_id, name, description, image, price_cents, sort, now_str()),
    )?;
    Ok(())
}

pub fn update_product(
    conn: &Connection,
    id: i64,
    category_id: i64,
    name: &str,
    description: &str,
    image: &str,
    price_cents: i64,
    sort: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE products SET category_id=?1, name=?2, description=?3, image=?4, price_cents=?5, sort=?6 WHERE id=?7",
        (category_id, name, description, image, price_cents, sort, id),
    )?;
    Ok(())
}

/// 取商品当前图片地址（用于替换/删除时清理旧文件）
pub fn product_image(conn: &Connection, id: i64) -> String {
    conn.query_row("SELECT image FROM products WHERE id = ?1", [id], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .ok()
    .flatten()
    .unwrap_or_default()
}

pub fn toggle_product(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE products SET status = 1 - status WHERE id = ?1", [id])?;
    Ok(())
}

/// 删除商品：连同未售卡密一起删除；已售卡密保留（历史订单可查）
pub fn delete_product(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM cards WHERE product_id = ?1 AND status != 1", [id])?;
    conn.execute("DELETE FROM products WHERE id = ?1", [id])?;
    Ok(())
}

// ---------- 卡密 ----------

pub struct CardView {
    pub id: i64,
    pub code: String,
    pub status: i64,
    pub order_id: Option<i64>,
    pub created_at: String,
    pub sold_at_str: String,
    pub product_name: String,
}

impl CardView {
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "在售",
            1 => "已售",
            2 => "锁定",
            _ => "未知",
        }
    }
    pub fn status_class(&self) -> &'static str {
        match self.status {
            0 => "done",
            1 => "sold",
            2 => "pending",
            _ => "expired",
        }
    }
    pub fn order_id_str(&self) -> String {
        self.order_id.map(|v| v.to_string()).unwrap_or_else(|| "—".into())
    }
}

pub fn list_cards(
    conn: &Connection,
    product_id: Option<i64>,
    status: Option<i64>,
    q: &str,
) -> Result<Vec<CardView>> {
    let mut sql = String::from(
        "SELECT k.id, k.code, k.status, k.order_id, k.created_at, COALESCE(k.sold_at,''), p.name
         FROM cards k JOIN products p ON p.id = k.product_id WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(pid) = product_id {
        sql.push_str(" AND k.product_id = ?");
        params.push(Box::new(pid));
    }
    if let Some(st) = status {
        sql.push_str(" AND k.status = ?");
        params.push(Box::new(st));
    }
    if !q.is_empty() {
        sql.push_str(" AND k.code LIKE ?");
        params.push(Box::new(format!("%{q}%")));
    }
    sql.push_str(" ORDER BY k.id DESC LIMIT 800");
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |r| {
        Ok(CardView {
            id: r.get(0)?,
            code: r.get(1)?,
            status: r.get(2)?,
            order_id: r.get(3)?,
            created_at: r.get(4)?,
            sold_at_str: r.get(5)?,
            product_name: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// 批量导入卡密（按行，自动去重），返回 (导入数, 跳过数)
pub fn import_cards(conn: &mut Connection, product_id: i64, text: &str) -> Result<(usize, usize)> {
    let now = now_str();
    let tx = conn.transaction()?;
    let mut ok = 0usize;
    let mut skip = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO cards(product_id, code, status, created_at) VALUES(?1, ?2, 0, ?3)",
        )?;
        for line in text.lines() {
            let code = line.trim();
            if code.is_empty() {
                continue;
            }
            let n = stmt.execute((product_id, code, &now))?;
            if n > 0 {
                ok += 1;
            } else {
                skip += 1;
            }
        }
    }
    tx.commit()?;
    Ok((ok, skip))
}

pub fn delete_card(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM cards WHERE id = ?1 AND status = 0", [id])?;
    Ok(())
}

pub fn delete_unsold_cards(conn: &Connection, product_id: i64) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM cards WHERE product_id = ?1 AND status = 0",
        [product_id],
    )?;
    Ok(n)
}

// ---------- 订单 ----------

pub struct Order {
    pub id: i64,
    pub order_no: String,
    pub product_id: i64,
    pub product_name: String,
    pub quantity: i64,
    pub unit_cents: i64,
    pub total_cents: i64,
    pub contact: String,
    pub status: i64,
    pub pay_method: String,
    pub created_at: String,
    pub paid_at: Option<String>,
}

impl Order {
    pub fn total_str(&self) -> String {
        cents_str(self.total_cents)
    }
    pub fn unit_str(&self) -> String {
        cents_str(self.unit_cents)
    }
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "待支付",
            1 => "已完成",
            2 => "已过期",
            3 => "已付待补发",
            _ => "未知",
        }
    }
    pub fn status_class(&self) -> &'static str {
        match self.status {
            0 => "pending",
            1 => "done",
            2 => "expired",
            3 => "warn",
            _ => "expired",
        }
    }
    pub fn paid_at_str(&self) -> String {
        self.paid_at.clone().unwrap_or_else(|| "—".into())
    }
    /// 取卡链接（带联系方式校验参数）
    pub fn view_link(&self) -> String {
        format!(
            "/order/{}?c={}",
            self.order_no,
            urlencoding::encode(&self.contact)
        )
    }
    pub fn pay_method_label(&self) -> &'static str {
        match self.pay_method.as_str() {
            "mock" => "模拟支付",
            "manual" => "人工补单",
            "alipay" => "支付宝",
            "wxpay" => "微信支付",
            "" => "—",
            _ => "在线支付",
        }
    }
}

fn order_from_row(r: &Row) -> Result<Order> {
    Ok(Order {
        id: r.get(0)?,
        order_no: r.get(1)?,
        product_id: r.get(2)?,
        product_name: r.get(3)?,
        quantity: r.get(4)?,
        unit_cents: r.get(5)?,
        total_cents: r.get(6)?,
        contact: r.get(7)?,
        status: r.get(8)?,
        pay_method: r.get(9)?,
        created_at: r.get(10)?,
        paid_at: r.get(11)?,
    })
}

const ORDER_COLS: &str = "id, order_no, product_id, product_name, quantity, unit_cents, total_cents, contact, status, pay_method, created_at, paid_at";

pub fn get_order_by_no(conn: &Connection, order_no: &str) -> Result<Option<Order>> {
    conn.query_row(
        &format!("SELECT {ORDER_COLS} FROM orders WHERE order_no = ?1"),
        [order_no],
        order_from_row,
    )
    .optional()
}

/// 过期释放：待付超时订单 -> 过期，锁定卡密 -> 释放
pub fn expire_stale_orders(conn: &mut Connection) -> Result<usize> {
    let cutoff = minutes_ago_str(ORDER_TTL_MIN);
    let tx = conn.transaction()?;
    let ids: Vec<i64> = {
        let mut stmt =
            tx.prepare("SELECT id FROM orders WHERE status = 0 AND created_at < ?1")?;
        let rows = stmt.query_map([&cutoff], |r| r.get::<_, i64>(0))?;
        rows.collect::<Result<Vec<_>>>()?
    };
    for id in &ids {
        tx.execute(
            "UPDATE cards SET status = 0, order_id = NULL WHERE order_id = ?1 AND status = 2",
            [id],
        )?;
        tx.execute("UPDATE orders SET status = 2 WHERE id = ?1", [id])?;
    }
    let n = ids.len();
    tx.commit()?;
    Ok(n)
}

/// 下单：事务内锁定 N 张卡密，防超卖
pub fn create_order(
    conn: &mut Connection,
    product_id: i64,
    quantity: i64,
    contact: &str,
) -> std::result::Result<Order, String> {
    if !(1..=100).contains(&quantity) {
        return Err("购买数量需在 1 - 100 之间".into());
    }
    let contact = contact.trim();
    if contact.chars().count() < 4 {
        return Err("请填写有效的联系方式（至少 4 个字符）".into());
    }
    expire_stale_orders(conn).map_err(|e| e.to_string())?;

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let prod: Option<(String, i64, i64)> = tx
        .query_row(
            "SELECT name, price_cents, status FROM products WHERE id = ?1",
            [product_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((pname, price, pstatus)) = prod else {
        return Err("商品不存在".into());
    };
    if pstatus != 1 {
        return Err("商品已下架".into());
    }

    let order_no = gen_order_no();
    let now = now_str();
    tx.execute(
        "INSERT INTO orders(order_no, product_id, product_name, quantity, unit_cents, total_cents, contact, status, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
        (&order_no, product_id, &pname, quantity, price, price * quantity, contact, &now),
    )
    .map_err(|e| e.to_string())?;
    let order_id = tx.last_insert_rowid();

    let locked = tx
        .execute(
            "UPDATE cards SET status = 2, order_id = ?1
             WHERE id IN (SELECT id FROM cards WHERE product_id = ?2 AND status = 0 ORDER BY id LIMIT ?3)",
            (order_id, product_id, quantity),
        )
        .map_err(|e| e.to_string())?;
    if locked as i64 != quantity {
        // 库存不足，整体回滚
        drop(tx);
        return Err("库存不足，请减少数量或稍后再试".into());
    }
    tx.commit().map_err(|e| e.to_string())?;

    get_order_by_no(conn, &order_no)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "订单创建失败".into())
}

/// 发货：支付成功后调用。锁定卡 -> 已售，订单 -> 已完成。
/// 若订单已过期被释放（迟到的回调），尝试重新锁卡补发；不足则标记「已付待补发」。
pub fn deliver_order(
    conn: &mut Connection,
    order_no: &str,
    pay_method: &str,
) -> std::result::Result<i64, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let order = tx
        .query_row(
            &format!("SELECT {ORDER_COLS} FROM orders WHERE order_no = ?1"),
            [order_no],
            order_from_row,
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(order) = order else {
        return Err("订单不存在".into());
    };
    if order.status == 1 {
        return Ok(1); // 幂等：已完成
    }
    let now = now_str();

    // 已锁定的卡数量
    let locked: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM cards WHERE order_id = ?1 AND status = 2",
            [order.id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    if locked < order.quantity {
        // 迟到回调：尝试补锁
        let need = order.quantity - locked;
        let got = tx
            .execute(
                "UPDATE cards SET status = 2, order_id = ?1
                 WHERE id IN (SELECT id FROM cards WHERE product_id = ?2 AND status = 0 ORDER BY id LIMIT ?3)",
                (order.id, order.product_id, need),
            )
            .map_err(|e| e.to_string())?;
        if (got as i64) < need {
            // 库存不够补发：记录已付待补发，等管理员处理
            tx.execute(
                "UPDATE orders SET status = 3, pay_method = ?1, paid_at = ?2 WHERE id = ?3",
                (pay_method, &now, order.id),
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(3);
        }
    }

    tx.execute(
        "UPDATE cards SET status = 1, sold_at = ?1 WHERE order_id = ?2 AND status = 2",
        (&now, order.id),
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE orders SET status = 1, pay_method = ?1, paid_at = ?2 WHERE id = ?3",
        (pay_method, &now, order.id),
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(1)
}

/// 订单卡密（完成的订单）
pub fn order_codes(conn: &Connection, order_id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT code FROM cards WHERE order_id = ?1 AND status = 1 ORDER BY id",
    )?;
    let rows = stmt.query_map([order_id], |r| r.get::<_, String>(0))?;
    rows.collect()
}

pub fn query_orders_by_contact(
    conn: &Connection,
    order_no: &str,
    contact: &str,
) -> Result<Vec<Order>> {
    if !order_no.is_empty() {
        let mut stmt = conn.prepare(&format!(
            "SELECT {ORDER_COLS} FROM orders WHERE order_no = ?1 AND contact = ?2"
        ))?;
        let rows = stmt.query_map([order_no, contact], order_from_row)?;
        rows.collect()
    } else {
        let mut stmt = conn.prepare(&format!(
            "SELECT {ORDER_COLS} FROM orders WHERE contact = ?1 ORDER BY id DESC LIMIT 20"
        ))?;
        let rows = stmt.query_map([contact], order_from_row)?;
        rows.collect()
    }
}

pub fn list_orders_admin(
    conn: &Connection,
    status: Option<i64>,
    q: &str,
) -> Result<Vec<Order>> {
    let mut sql = format!("SELECT {ORDER_COLS} FROM orders WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(st) = status {
        sql.push_str(" AND status = ?");
        params.push(Box::new(st));
    }
    if !q.is_empty() {
        sql.push_str(" AND (order_no LIKE ? OR contact LIKE ? OR product_name LIKE ?)");
        let like = format!("%{q}%");
        params.push(Box::new(like.clone()));
        params.push(Box::new(like.clone()));
        params.push(Box::new(like));
    }
    sql.push_str(" ORDER BY id DESC LIMIT 300");
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), order_from_row)?;
    rows.collect()
}

// ---------- 仪表盘统计 ----------

pub struct DashStats {
    pub today_orders: i64,
    pub today_revenue_str: String,
    pub total_sold: i64,
    pub stock_total: i64,
    pub pending_resend: i64,
}

pub fn dash_stats(conn: &Connection) -> Result<DashStats> {
    let today = crate::util::today_str();
    let like = format!("{today}%");
    let today_orders: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orders WHERE status = 1 AND paid_at LIKE ?1",
        [&like],
        |r| r.get(0),
    )?;
    let today_revenue: i64 = conn.query_row(
        "SELECT COALESCE(SUM(total_cents),0) FROM orders WHERE status = 1 AND paid_at LIKE ?1",
        [&like],
        |r| r.get(0),
    )?;
    let total_sold: i64 =
        conn.query_row("SELECT COUNT(*) FROM cards WHERE status = 1", [], |r| r.get(0))?;
    let stock_total: i64 =
        conn.query_row("SELECT COUNT(*) FROM cards WHERE status = 0", [], |r| r.get(0))?;
    let pending_resend: i64 =
        conn.query_row("SELECT COUNT(*) FROM orders WHERE status = 3", [], |r| r.get(0))?;
    Ok(DashStats {
        today_orders,
        today_revenue_str: cents_str(today_revenue),
        total_sold,
        stock_total,
        pending_resend,
    })
}

/// 库存告警（在售但库存 < 10）
pub fn low_stock_products(conn: &Connection) -> Result<Vec<ProductView>> {
    let all = list_products_admin(conn)?;
    Ok(all
        .into_iter()
        .filter(|pv| pv.p.status == 1 && pv.stock < 10)
        .collect())
}

/// 公开统计：在售商品数 + 累计发卡数
pub fn public_stats(conn: &Connection) -> (i64, i64) {
    let on_sale: i64 = conn
        .query_row("SELECT COUNT(*) FROM products WHERE status = 1", [], |r| r.get(0))
        .unwrap_or(0);
    let sold: i64 = conn
        .query_row("SELECT COUNT(*) FROM cards WHERE status = 1", [], |r| r.get(0))
        .unwrap_or(0);
    (on_sale, sold)
}
