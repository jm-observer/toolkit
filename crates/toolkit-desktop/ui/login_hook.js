// 注入到 login webview，先于页面脚本运行。
// 抖音前端 SDK 不再把 msToken 写进 document.cookie，而是每次 API 调用时挂在 URL 上：
//   /aweme/v1/web/aweme/post/?msToken=xxxxx&...
//
// 由于 login 窗加载的是远端 origin（www.douyin.com），Tauri 2 默认不允许远端页面调
// invoke。所以这里通过 fetch('http://127.0.0.1:28788/mstoken?value=...') 把抓到的值
// 喂给 desktop 内置的本地 HTTP 桥。no-cors + GET，无需 CORS 预检。
(function () {
  "use strict";
  const BRIDGE = "http://127.0.0.1:28788/mstoken";
  let last = "";

  // ===== 修复"点头像/视频卡片无反应" =====
  // 抖音 feed 里点头像/作品几乎都是 target=_blank 或 window.open，WebView2 默认
  // 拦截弹窗 → 点了没反应。这里把所有"新窗导航"劫成"同窗导航"，让导航生效，
  // 用户跳到博主主页后，URL 变化会被 desktop 桥的 /login-url 接口读到。
  try {
    const _open = window.open;
    window.open = function (url) {
      if (url) { try { window.location.href = url; } catch (e) {} return null; }
      return _open.apply(this, arguments);
    };
  } catch (e) {}
  try {
    document.addEventListener("click", function (ev) {
      let el = ev.target;
      while (el && el !== document) {
        if (el.tagName === "A" && el.target === "_blank" && el.href) {
          ev.preventDefault();
          ev.stopPropagation();
          try { window.location.href = el.href; } catch (e) {}
          return;
        }
        el = el.parentElement;
      }
    }, true);
  } catch (e) {}


  function send(v) {
    if (!v || v === last) return;
    last = v;
    try {
      fetch(BRIDGE + "?value=" + encodeURIComponent(v), {
        mode: "no-cors",
        credentials: "omit",
        cache: "no-store",
      }).catch(function () {});
    } catch (e) {}
  }

  function scan(url) {
    try {
      if (typeof url !== "string") return;
      const i = url.indexOf("msToken=");
      if (i < 0) return;
      const rest = url.slice(i + 8);
      const end = rest.search(/[&#]/);
      const v = end < 0 ? rest : rest.slice(0, end);
      if (v && v.length > 16) send(decodeURIComponent(v));
    } catch (e) {}
  }

  try {
    const _fetch = window.fetch;
    if (_fetch) {
      window.fetch = function (input, init) {
        try {
          const u = typeof input === "string" ? input : input && input.url;
          scan(u);
        } catch (e) {}
        return _fetch.apply(this, arguments);
      };
    }
  } catch (e) {}

  try {
    const _open = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (method, url) {
      try { scan(url); } catch (e) {}
      return _open.apply(this, arguments);
    };
  } catch (e) {}

  try {
    if (navigator.sendBeacon) {
      const _sb = navigator.sendBeacon.bind(navigator);
      navigator.sendBeacon = function (url, data) {
        try { scan(url); } catch (e) {}
        return _sb(url, data);
      };
    }
  } catch (e) {}
})();
