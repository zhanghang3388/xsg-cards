use crate::util::now_str;
use rusqlite::{Connection, Result};

pub fn init(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    migrate_columns(&conn)?;
    seed(&conn)?;
    Ok(conn)
}

/// 增量列迁移：老库升级用，已存在则跳过
fn migrate_columns(conn: &Connection) -> Result<()> {
    let has_image: bool = {
        let mut stmt = conn.prepare("PRAGMA table_info(products)")?;
        let cols = stmt.query_map([], |r| r.get::<_, String>(1))?;
        let found = cols.filter_map(|c| c.ok()).any(|c| c == "image");
        found
    };
    if !has_image {
        conn.execute_batch("ALTER TABLE products ADD COLUMN image TEXT NOT NULL DEFAULT ''")?;
        tracing::info!("数据库迁移：products 表已新增 image 列");
    }
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS admins (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            username      TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at    TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sessions (
            token      TEXT PRIMARY KEY,
            admin_id   INTEGER NOT NULL,
            expires_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS categories (
            id   INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            sort INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS products (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            category_id INTEGER NOT NULL,
            name        TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            image       TEXT NOT NULL DEFAULT '',
            price_cents INTEGER NOT NULL DEFAULT 0,
            status      INTEGER NOT NULL DEFAULT 1,
            sort        INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS cards (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            product_id INTEGER NOT NULL,
            code       TEXT NOT NULL,
            status     INTEGER NOT NULL DEFAULT 0, -- 0在售 1已售 2锁定
            order_id   INTEGER,
            created_at TEXT NOT NULL,
            sold_at    TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_cards_unique ON cards(product_id, code);
        CREATE INDEX IF NOT EXISTS idx_cards_pick ON cards(product_id, status);
        CREATE INDEX IF NOT EXISTS idx_cards_order ON cards(order_id);
        CREATE TABLE IF NOT EXISTS orders (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            order_no     TEXT NOT NULL UNIQUE,
            product_id   INTEGER NOT NULL,
            product_name TEXT NOT NULL,
            quantity     INTEGER NOT NULL,
            unit_cents   INTEGER NOT NULL,
            total_cents  INTEGER NOT NULL,
            contact      TEXT NOT NULL,
            status       INTEGER NOT NULL DEFAULT 0, -- 0待付 1完成 2过期 3已付待补发
            pay_method   TEXT NOT NULL DEFAULT '',
            created_at   TEXT NOT NULL,
            paid_at      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_orders_contact ON orders(contact);
        CREATE INDEX IF NOT EXISTS idx_orders_status ON orders(status, created_at);
        "#,
    )
}

fn set_default(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES(?1, ?2)",
        (key, value),
    )?;
    Ok(())
}

fn seed(conn: &Connection) -> Result<()> {
    // 默认站点设置
    set_default(conn, "site_name", "潇洒哥的卡台")?;
    set_default(conn, "site_mark", "XSG·CARDS")?;
    set_default(conn, "site_subtitle", "全自动发卡 · 付款秒到 · 24 小时在线")?;
    set_default(
        conn,
        "announcement",
        "本站全自动发卡：付款成功后卡密立即显示，也可随时在「订单查询」页凭订单号取回。当前为演示模式，支付页点击「模拟支付」即可体验完整流程。",
    )?;
    set_default(conn, "default_theme", "dark")?;
    set_default(conn, "site_url", "http://localhost:8080")?;
    set_default(conn, "pay_mode", "mock")?; // mock | epay
    set_default(conn, "epay_gateway", "")?;
    set_default(conn, "epay_pid", "")?;
    set_default(conn, "epay_key", "")?;
    // 收银台展示哪些支付通道（都关则退回易支付自带收银台让买家自选）
    set_default(conn, "epay_alipay", "1")?;
    set_default(conn, "epay_wxpay", "1")?;

    // 邮件通知（默认关闭，参数由站长在后台填写）
    set_default(conn, "mail_enabled", "0")?;
    set_default(conn, "smtp_host", "")?;
    set_default(conn, "smtp_port", "465")?;
    set_default(conn, "smtp_security", "ssl")?; // ssl | starttls | none
    set_default(conn, "smtp_user", "")?;
    set_default(conn, "smtp_pass", "")?;
    set_default(conn, "mail_from", "")?;
    set_default(conn, "mail_from_name", "")?;
    set_default(conn, "mail_admin_to", "")?;

    // 默认管理员 admin / admin123
    let admin_count: i64 = conn.query_row("SELECT COUNT(*) FROM admins", [], |r| r.get(0))?;
    if admin_count == 0 {
        let hash = bcrypt::hash("admin123", 10).expect("bcrypt 失败");
        conn.execute(
            "INSERT INTO admins(username, password_hash, created_at) VALUES('admin', ?1, ?2)",
            (hash, now_str()),
        )?;
    }

    // 演示数据（仅当商品表为空时）
    let product_count: i64 = conn.query_row("SELECT COUNT(*) FROM products", [], |r| r.get(0))?;
    if product_count == 0 {
        seed_demo(conn)?;
    }
    Ok(())
}

fn seed_demo(conn: &Connection) -> Result<()> {
    use rand::Rng;
    let now = now_str();

    let categories = [("游戏点卡", 30), ("视频会员", 20), ("软件服务", 10)];
    let mut cat_ids = Vec::new();
    for (name, sort) in categories {
        conn.execute(
            "INSERT INTO categories(name, sort) VALUES(?1, ?2)",
            (name, sort),
        )?;
        cat_ids.push(conn.last_insert_rowid());
    }

    // (分类下标, 名称, 描述, 价格分, 库存数)
    let products: [(usize, &str, &str, i64, usize); 8] = [
        (0, "手游月卡 · 直充卡密", "适用于主流手游月卡兑换。\n使用方法：游戏内「设置 → 兑换码」输入卡密即可。\n卡密自动发货，售出后不退不换，请确认区服后购买。", 2980, 35),
        (0, "游戏点卡 100 元面值", "通用点卡，面值 100 元。\n充值入口见发货卡密附带说明链接。\n24 小时自动发货。", 9500, 18),
        (0, "游戏加速器 · 30 天", "全平台加速器月卡兑换码。\n支持电脑 / 主机 / 手机三端，一码通用。\n激活后 30 天有效。", 1900, 6),
        (1, "视频会员 · 月卡", "视频平台会员月卡激活码。\n官网「兑换中心」输入卡密激活，即时到账。\n请勿重复兑换。", 1500, 42),
        (1, "视频会员 · 年卡", "视频平台会员年卡激活码，一次到手 12 个月。\n官网「兑换中心」输入卡密激活。\n年卡专属客服通道。", 12800, 9),
        (1, "音乐会员 · 季卡", "音乐平台豪华会员 3 个月兑换码。\n兑换路径：APP → 我的 → 兑换码。", 3500, 27),
        (2, "系统激活码 · 专业版", "操作系统专业版数字许可证。\n联网自动激活，永久有效，支持重装。\n附激活教程，售后 90 天。", 2500, 15),
        (2, "办公套件 · 年度授权", "办公全家桶年度订阅兑换码。\n支持 5 台设备同时使用，含云存储空间。", 9900, 3),
    ];

    let mut rng = rand::thread_rng();
    let mut used = std::collections::HashSet::new();
    for (ci, name, desc, price, stock) in products {
        conn.execute(
            "INSERT INTO products(category_id, name, description, price_cents, status, sort, created_at)
             VALUES(?1, ?2, ?3, ?4, 1, 0, ?5)",
            (cat_ids[ci], name, desc, price, &now),
        )?;
        let pid = conn.last_insert_rowid();
        for _ in 0..stock {
            // 生成形如 KZ-XXXX-XXXX-XXXX 的演示卡密
            let code = loop {
                let seg = |rng: &mut rand::rngs::ThreadRng| -> String {
                    (0..4)
                        .map(|_| {
                            let chars = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
                            chars[rng.gen_range(0..chars.len())] as char
                        })
                        .collect()
                };
                let c = format!("XS-{}-{}-{}", seg(&mut rng), seg(&mut rng), seg(&mut rng));
                if used.insert(c.clone()) {
                    break c;
                }
            };
            conn.execute(
                "INSERT OR IGNORE INTO cards(product_id, code, status, created_at) VALUES(?1, ?2, 0, ?3)",
                (pid, code, &now),
            )?;
        }
    }
    tracing::info!("已写入演示分类/商品/卡密数据");
    Ok(())
}
