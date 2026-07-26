# 潇洒哥的卡台 · 自动发卡网 — 项目计划

> 单商户自动发卡站：买家选卡 → 付款 → 秒发卡密。后端 Rust，单二进制部署，SQLite 单文件存储。

## 一、技术选型

| 层 | 选型 | 理由 |
|---|---|---|
| Web 框架 | Axum 0.8 + Tokio | 生态主流、性能好、类型安全 |
| 模板 | Askama（编译期模板） | 模板错误在编译期暴露，渲染零开销 |
| 数据库 | SQLite（rusqlite, bundled） | 零配置单文件，中小流量完全够用，备份即复制 |
| 密码 | bcrypt | 管理员口令哈希 |
| 支付 | 模拟支付 + 易支付(epay)协议 | 本地即可演示全流程；生产填入易支付商户参数即可收款 |
| 前端 | 原生 CSS/JS（无构建步骤） | 双主题设计令牌，随二进制直接部署 |

部署形态：`cargo build --release` 产出单个二进制 + `static/` + `data.db`，无需 Node/无需外部服务。

## 二、功能清单

**买家端**：首页（公告、分类、商品票券墙）；商品详情与下单（联系方式 + 数量，实时库存）；支付页（模拟支付 / 易支付跳转，状态轮询）；付款后自动发卡，卡密「刮开显示」+ 一键复制；订单查询（订单号 + 联系方式）。

**管理端**（/admin）：登录会话；仪表盘（今日订单/收入、库存告警、最近订单）；分类管理；商品管理（上下架、排序、定价）；卡密管理（批量导入、按状态筛选、删除未售）；订单管理（搜索、手动补单）；站点设置（站名、公告、默认主题、支付模式与易支付参数、改密）。

**核心机制**：下单即锁卡（事务内锁定 N 张卡密，杜绝超卖），15 分钟未付自动过期释放；支付回调验签后锁定卡转已售、订单完成。

## 三、数据库设计

```sql
settings   (key PK, value)
admins     (id, username UNIQUE, password_hash, created_at)
sessions   (token PK, admin_id, expires_at)
categories (id, name, sort)
products   (id, category_id, name, description, price_cents, status, sort, created_at)
cards      (id, product_id, code, status/*0在售 1已售 2锁定*/, order_id, created_at, sold_at)
orders     (id, order_no UNIQUE, product_id, product_name/*快照*/, quantity,
            unit_cents, total_cents, contact, status/*0待付 1完成 2过期*/,
            pay_method, created_at, paid_at)
```

金额一律以「分」存储（整数），显示层格式化，避免浮点误差。

## 四、路由设计

```
公开                                管理（会话中间件保护）
GET  /                首页          GET/POST /admin/login · POST /admin/logout
GET  /p/{id}          商品详情      GET  /admin                仪表盘
POST /order           创建订单      GET/POST /admin/categories[/{id}][/delete]
GET  /pay/{no}        支付页        GET/POST /admin/products[/{id}][/delete|/toggle]
POST /api/pay/mock/{no}  模拟支付   GET  /admin/cards · POST /admin/cards/import
GET  /api/pay/epay/notify 易支付回调 POST /admin/cards/{id}/delete · delete-unsold
GET  /api/order/{no}/status 轮询    GET  /admin/orders · POST /admin/orders/{id}/mark-paid
GET  /order/{no}?c=   取卡结果页    GET/POST /admin/settings · POST /admin/password
GET/POST /query       订单查询      GET  /static/*             静态资源
```

## 五、UI 设计规范 —— 「票据 × 终端」设计语言

发卡站的世界是：票券、串号、刮刮卡、秒发。UI 直接取材于此，而不是通用的霓虹玻璃风。

**设计令牌**

| 令牌 | 暗色「墨夜」(默认) | 亮色「素笺」 |
|---|---|---|
| 背景 bg0/bg1/bg2 | `#0B0F14 / #111722 / #1A2230` | `#F3F5F7 / #FFFFFF / #FAFBFC` |
| 描线 line | `#27313F` | `#E2E7ED` |
| 文字 ink/ink-2 | `#E9EEF4 / #97A3B2` | `#16202B / #5D6B7A` |
| 主色 gold | `#F5B841`（琥珀金，取「金卡」意象） | 同色系，文字级用 `#8A6100` 保对比度 |
| 成功 jade / 危险 red | `#3ECF8E / #F0564A` | 加深适配 |

**字体**：拉丁展示字体 Bricolage Grotesque（自托管 woff2，标题与数字有辨识度）；代码/串号/价格用 JetBrains Mono；中文正文走系统栈（PingFang SC / 鸿蒙 / 雅黑），保证国内加载速度。

**签名元素（本站记忆点）**
1. **票券商品卡**：左侧撕票打孔边 + 侧边缺口，价格用等宽字体大号排印，库存以「库存 ×231」串号样式呈现——商品卡本身就像一张待售的充值票。
2. **刮开显码**：付款成功页卡密默认覆盖磨砂「刮层」，点击刮开 + 一键复制，还原刮刮卡的仪式感。

**版式**：首页 = 售票口。左侧标语「选卡 · 付款 · 秒到」+ 等宽字体实时数据行（在售/已发），右侧钉住的公告牌；下方分类页签 + 票券墙。管理后台走克制的侧栏工作台风格，同一套令牌，不玩花活。

```
┌────────────────────────────────────────────┐
│ ◈ 潇洒哥的卡台    首页  订单查询       ◐主题 │
├────────────────────────────────────────────┤
│ 自动发卡 · 24H          ┌─📌 公告 ────────┐ │
│ 选卡 · 付款 · 秒到       │ 支付后自动发货… │ │
│ 在售 12 · 已发 3,481    └─────────────────┘ │
├────────────────────────────────────────────┤
│ [全部] [游戏点卡] [视频会员] [软件]          │
│ ⦿┌────────┐ ⦿┌────────┐ ⦿┌────────┐        │
│  │商品名   │  │        │  │        │        │
│  │¥ 9.90   │  │        │  │        │        │
│  │库存 ×231│  │        │  │        │        │
│  │[立即购买]│  │        │  │        │        │
│  └────────┘  └────────┘  └────────┘        │
└────────────────────────────────────────────┘
```

**动效**：卡片悬停轻抬、刮层揭开过渡、页面淡入三处，尊重 `prefers-reduced-motion`；移动端单列自适应；键盘焦点可见。

## 六、目录结构

```
card/
├── Cargo.toml
├── PLAN.md / README.md
├── src/
│   ├── main.rs            启动、路由装配
│   ├── state.rs           AppState（DB 连接、配置）
│   ├── db.rs              建表迁移、种子数据
│   ├── models.rs          实体与查询
│   ├── auth.rs            管理员会话、中间件
│   ├── payment.rs         模拟支付 + 易支付签名/验签
│   ├── util.rs            金额/时间/订单号/Cookie
│   └── routes/            public.rs · order.rs · admin.rs
├── templates/             Askama 模板（公开 + admin）
└── static/                css / js / fonts / favicon
```

## 七、里程碑

1. 工具链与脚手架 → 2. 数据层与种子数据 → 3. 买家端全流程（含支付） → 4. 双主题 UI → 5. 管理后台 → 6. 构建 + curl 全流程验收 → 7. README 交付

**验收标准**：本地启动后，不碰后台即可完成「浏览 → 下单 → 模拟支付 → 刮开取卡 → 订单查询」闭环；后台可完成「导入卡密 → 商品上架 → 查订单 → 补单」闭环；暗/亮主题切换记忆；首次启动自动创建管理员（文档提示改密）。

---

## 八、上线后的加固（2026-07-26 增补）

原计划把后台固定挂在 `/admin`，实际上线后发现这等于把入口写在门牌上——扫描器只要试一个路径就能找到登录页。本轮改为**随机私密路径**，并顺带补上了商品配图。

**后台私密路径**：新增 `src/config.rs`，用 `OnceLock` 在启动时定一次全局前缀，取值优先级为 环境变量 `ADMIN_PATH` > `settings.admin_path` > 首次启动随机生成 6 位并落库（字符集去掉了 `l/o/0/1` 等易混字符，且排除 `admin`、`static`、`uploads` 等保留字）。路由用 `.nest(config::admin_base(), …)` 装配，模板统一用 `{{ base }}` 拼地址，因此上文路由表里的 `/admin/*` 现均为 `{私密路径}/*`。

配套三件事：页脚的「管理入口」链接删除（否则等于把密路径印在每一页上）；会话 Cookie 的 `Path` 收紧到私密路径，前台请求不再携带；登录失败按 IP 计数（内存表），5 次锁 10 分钟，反代下从 `X-Forwarded-For` 取真实 IP。

**商品图片**：`products` 表加 `image TEXT`，用 `PRAGMA table_info` 做幂等增量迁移，线上库直接升级不丢数据。新增 `src/upload.rs` 负责落盘：只认 PNG/JPG/WEBP/GIF 的**文件头**（不信扩展名）、单张 ≤ 2MB、请求体 ≤ 6MB、文件名服务端随机生成，存 `uploads/` 由 `ServeDir` 静态托管。后台表单改 `multipart/form-data`，支持上传、外链兜底、移除图片三种操作，替换和删除时旧文件一并清理。前台首页票券顶部与详情页各加一块配图区，无图商品保持原样式。

## 九、运营向补全（2026-07-26 二次增补）

站点跑起来之后暴露出三类日常痛点：卡密攒到几百张后列表拉不动、删东西只能一条一条点、买家付完钱得自己回来查单。本轮针对性补齐。

**邮件通知**：新增 `src/mailer.rs`，用 `lettre` 0.11（`rustls` 而非 native-tls，免掉 OpenSSL 依赖，服务器上编译不用装额外的 dev 包）。SMTP 参数全部走 `settings` 表，后台「设置 → 邮件通知」可视化填写，支持 SSL(465) / STARTTLS(587) / 明文三种模式，密码只写不读、页面永不回显、留空即沿用旧值，另配一个「发送测试邮件」按钮当场验证连通性。

发信时机只有两处，都挂在**发货成功之后**：`deliver_order` 返回状态 1（正常发货）时给买家寄卡密与取卡链接，返回状态 3（已付但库存不足）时给店主寄缺货提醒。三条发货路径（模拟支付、易支付回调、后台人工补发）共用同一个 `notify_after_deliver`，避免哪条路径漏发。发信在 `tokio::spawn` 里异步进行，SMTP 超时或认证失败只记日志，绝不阻塞发货事务——收钱发货是主线，通知是支线，支线断了不能拖累主线。

**列表分页与批量删除**：商品/卡密/订单三张表统一每页 50 条，`Pager` 负责算窗口（首尾页恒显示，中间用 `0` 作省略号哨兵），模板抽成共用的 `templates/admin/_pager.html`。批量删除的坑在于同名 `checkbox` 会重复出现，`serde_urlencoded` 映射不成 `Vec`，所以改用 axum 的 `String` extractor 手动拆 body。筛选条件通过隐藏字段 `back` 往返，删完原样跳回当前页当前筛选；`back` 在拼进 `Location` 前做字符白名单过滤，杜绝响应头注入。

**收银台支付通道可配**：原先支付宝 / 微信两个按钮写死在 pay.html 里，商户没开通哪个就只能让买家点过去报错。改为 settings 里的 `epay_alipay` / `epay_wxpay` 两个开关（默认都开，`INSERT OR IGNORE` 种子保证老库升级后行为不变），后台支付面板勾选。`PayConfig::channels()` 按勾选生成 `Vec<PayChannel>`，模板循环渲染，`.pay-ways` 用 `auto-fit` 让单个按钮自然占满整行。**一个都不勾时不留空页**，退回单个「其他方式」按钮——提交时不带 `type` 参数，由易支付自家收银台列出该商户实际开通的通道；这也正好是签名逻辑早就支持的路径（`epay_sign` 本来就跳过空值参数）。

**两处数据安全修正**：其一，删除商品时旧逻辑把该商品的卡密一并清掉，若其中有正被订单锁定的卡密，买家付完款就取不到货了——现在改为遇到锁定卡密直接拒绝删除并提示先处理订单。其二，订单查询原先只凭联系方式就能列出订单和卡密，等于知道邮箱就能取别人的货——改为**订单号 + 联系方式双凭证**，两者必须同时提供且都对上。
