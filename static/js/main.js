/* 潇洒哥的卡台 · 前端交互：主题切换 / Toast / 复制 / 刮开显码 / 数量步进 / 收银台轮询 / 对话框 */
(function () {
  "use strict";

  var doc = document.documentElement;

  /* ---------- 主题切换 ---------- */
  var themeBtn = document.getElementById("themeBtn");
  if (themeBtn) {
    themeBtn.addEventListener("click", function () {
      var next = doc.getAttribute("data-theme") === "dark" ? "light" : "dark";
      doc.setAttribute("data-theme", next);
      try { localStorage.setItem("kz-theme", next); } catch (e) {}
    });
  }

  /* ---------- Toast ---------- */
  var toastEl = document.getElementById("toast");
  var toastTimer = null;
  function toast(text, isErr) {
    if (!toastEl || !text) return;
    toastEl.textContent = text;
    toastEl.hidden = false;
    toastEl.classList.toggle("err", !!isErr);
    requestAnimationFrame(function () { toastEl.classList.add("show"); });
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      toastEl.classList.remove("show");
    }, 3600);
  }
  window.kzToast = toast;

  // 从 URL 读取 msg / err 提示（重定向后展示，展示完清理地址栏）
  try {
    var usp = new URLSearchParams(location.search);
    var msg = usp.get("msg"), err = usp.get("err");
    if (msg || err) {
      toast(err || msg, !!err);
      usp.delete("msg"); usp.delete("err");
      var qs = usp.toString();
      history.replaceState(null, "", location.pathname + (qs ? "?" + qs : "") + location.hash);
    }
  } catch (e) {}

  /* ---------- 复制 ---------- */
  function copyText(text, done) {
    if (navigator.clipboard && window.isSecureContext !== false) {
      navigator.clipboard.writeText(text).then(done, function () { fallbackCopy(text, done); });
    } else {
      fallbackCopy(text, done);
    }
  }
  function fallbackCopy(text, done) {
    var ta = document.createElement("textarea");
    ta.value = text;
    ta.style.cssText = "position:fixed;opacity:0;top:0";
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand("copy"); } catch (e) {}
    document.body.removeChild(ta);
    done && done();
  }

  document.addEventListener("click", function (ev) {
    var btn = ev.target.closest("[data-copy]");
    if (!btn) return;
    copyText(btn.getAttribute("data-copy"), function () {
      btn.classList.add("ok");
      var old = btn.textContent;
      btn.textContent = "已复制";
      setTimeout(function () { btn.classList.remove("ok"); btn.textContent = old; }, 1600);
    });
  });

  var copyAll = document.getElementById("copyAll");
  if (copyAll) {
    copyAll.addEventListener("click", function () {
      var codes = Array.prototype.map.call(document.querySelectorAll(".scratch .code"), function (el) {
        return el.textContent.trim();
      });
      if (!codes.length) return;
      copyText(codes.join("\n"), function () { toast("已复制全部 " + codes.length + " 张卡密"); });
    });
  }

  /* ---------- 刮开显码 ---------- */
  function reveal(el) { el.classList.add("revealed"); }
  document.addEventListener("click", function (ev) {
    var s = ev.target.closest(".scratch");
    if (s) reveal(s);
  });
  document.addEventListener("keydown", function (ev) {
    if ((ev.key === "Enter" || ev.key === " ") && ev.target.classList && ev.target.classList.contains("scratch")) {
      ev.preventDefault();
      reveal(ev.target);
    }
  });

  /* ---------- 商品页：数量步进 + 合计 ---------- */
  var qty = document.getElementById("qty");
  var totalEl = document.getElementById("totalPrice");
  function clampQty() {
    var v = parseInt(qty.value, 10) || 1;
    var min = parseInt(qty.min, 10) || 1;
    var max = parseInt(qty.max, 10) || 999;
    qty.value = Math.min(Math.max(v, min), max);
  }
  function updateTotal() {
    if (!totalEl || !qty) return;
    var unit = parseInt(totalEl.getAttribute("data-unit"), 10) || 0;
    var total = unit * (parseInt(qty.value, 10) || 1);
    totalEl.innerHTML = "<i>¥</i>" + (total / 100).toFixed(2);
  }
  if (qty) {
    document.querySelectorAll(".stepper [data-step]").forEach(function (b) {
      b.addEventListener("click", function () {
        qty.value = (parseInt(qty.value, 10) || 1) + parseInt(b.getAttribute("data-step"), 10);
        clampQty();
        updateTotal();
      });
    });
    qty.addEventListener("input", updateTotal);
    qty.addEventListener("change", function () { clampQty(); updateTotal(); });
  }

  /* ---------- 收银台：倒计时 + 状态轮询 + 模拟支付 ---------- */
  var cd = document.getElementById("countdown");
  if (cd) {
    var remain = parseInt(cd.getAttribute("data-remain"), 10) || 0;
    var tick = function () {
      if (remain <= 0) { location.reload(); return; }
      var m = Math.floor(remain / 60), s = remain % 60;
      cd.textContent = "支付剩余时间 " + (m < 10 ? "0" + m : m) + ":" + (s < 10 ? "0" + s : s);
      if (remain <= 60) cd.classList.add("hot");
      remain--;
      setTimeout(tick, 1000);
    };
    tick();
  }

  var orderNo = window.__PAY_ORDER__;
  if (orderNo) {
    var polling = setInterval(function () {
      fetch("/api/order/" + orderNo + "/status")
        .then(function (r) { return r.json(); })
        .then(function (d) {
          if (d.status === 1 || d.status === 3) {
            clearInterval(polling);
            location.href = d.url;
          }
        })
        .catch(function () {});
    }, 2500);
  }

  var mockBtn = document.getElementById("mockPay");
  if (mockBtn) {
    mockBtn.addEventListener("click", function () {
      mockBtn.disabled = true;
      mockBtn.textContent = "正在确认支付…";
      fetch("/api/pay/mock/" + mockBtn.getAttribute("data-no"), { method: "POST" })
        .then(function (r) { return r.json(); })
        .then(function (d) {
          if (d.ok) {
            mockBtn.textContent = "支付成功，正在发货…";
          } else {
            toast(d.msg || "支付失败", true);
            mockBtn.disabled = false;
            mockBtn.textContent = "模拟支付（演示模式）";
          }
        })
        .catch(function () {
          toast("网络异常，请重试", true);
          mockBtn.disabled = false;
          mockBtn.textContent = "模拟支付（演示模式）";
        });
    });
  }

  /* ---------- 对话框 ---------- */
  document.querySelectorAll("[data-dialog]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var dlg = document.getElementById(btn.getAttribute("data-dialog"));
      if (dlg && dlg.showModal) dlg.showModal();
    });
  });
  document.querySelectorAll(".dlg [data-close]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var dlg = btn.closest("dialog");
      if (dlg) dlg.close();
    });
  });
  document.querySelectorAll(".dlg").forEach(function (dlg) {
    dlg.addEventListener("click", function (ev) {
      if (ev.target === dlg) dlg.close(); // 点击遮罩关闭
    });
  });
})();
