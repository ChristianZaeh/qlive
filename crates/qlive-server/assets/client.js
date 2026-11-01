// qlive client, M0.
//
// Keeps the scroll position across reloads, reconnects when the daemon restarts, and
// shows a small badge so you can see the loop working. M2 replaces the reload with
// node patches: the `patch` branch below is where those land.
(function () {
  var KEY = "qlive:scroll";
  var badge = null;
  var viewButton = document.createElement("button");
  viewButton.id = "qlive-view-toggle";
  viewButton.type = "button";
  viewButton.style.cssText = "position:fixed;right:12px;top:12px;z-index:2147483646;font:12px/1.4 system-ui;padding:6px 10px;border:1px solid #aaa;border-radius:6px;background:#fff;color:#222;box-shadow:0 1px 4px #0002";
  viewButton.title = "Toggle continuous paper layout (approximate PDF appearance)";
  var paperStyle = document.createElement("style");
  paperStyle.id = "qlive-paper-style";
  paperStyle.textContent = [
    "html.qlive-paper{background:#e5e7eb!important}",
    "html.qlive-paper body{background:transparent!important}",
    "html.qlive-paper #quarto-content{display:block!important;max-width:none!important;margin:0 auto!important;padding:28px 16px!important}",
    "html.qlive-paper main.content{box-sizing:border-box;width:210mm;max-width:100%;min-height:297mm;margin:0 auto!important;padding:25mm 24mm!important;background:#fff;box-shadow:0 2px 14px #0003;color:#111;font-family:'Libertinus Serif','Times New Roman',serif;font-size:11pt;line-height:1.5}",
    "html.qlive-paper #quarto-header,html.qlive-paper #quarto-sidebar,html.qlive-paper #quarto-margin-sidebar,html.qlive-paper .page-footer{display:none!important}",
    "html.qlive-paper main h1,html.qlive-paper main h2,html.qlive-paper main h3,html.qlive-paper main h4{font-family:inherit;color:inherit}",
    "html.qlive-paper main h1{font-size:24pt}html.qlive-paper main h2{font-size:18pt}html.qlive-paper main h3{font-size:14pt}",
    "html.qlive-paper #title-block-header .title{font-size:24pt}",
    "html.qlive-paper main a{color:inherit}",
    "html.qlive-paper main pre{font-size:9pt}",
    "@media(max-width:650px){html.qlive-paper main.content{padding:12mm 9mm!important}}",
    "@media print{#qlive-view-toggle{display:none!important}}"
  ].join("\n");
  document.head.appendChild(paperStyle);
  document.body.appendChild(viewButton);
  function setView(mode) {
    mode = mode === "paper" ? "paper" : "web";
    document.documentElement.classList.toggle("qlive-paper", mode === "paper");
    viewButton.textContent = mode === "paper" ? "Web view" : "Paper view";
    viewButton.setAttribute("aria-pressed", String(mode === "paper"));
    try { localStorage.setItem("qlive:view", mode); } catch (e) {}
    if (window.parent !== window) window.parent.postMessage({ type: "qlive-view-changed", mode: mode }, "*");
  }
  viewButton.addEventListener("click", function () { setView(document.documentElement.classList.contains("qlive-paper") ? "web" : "paper"); });
  window.addEventListener("message", function (event) {
    if (event.source === window.parent && event.data && event.data.type === "qlive-view") setView(event.data.mode);
  });
  var initialView = new URLSearchParams(location.search).get("qlive-view");
  try { initialView = initialView || localStorage.getItem("qlive:view"); } catch (e) {}
  setView(initialView);

  document.addEventListener("click", function (event) {
    if (!event.target || !event.target.closest) return;
    if (event.target.closest("a, button, input, select, textarea, summary")) return;
    var source = event.target.closest("[data-qlive-source]");
    if (!source) return;
    fetch("/qlive/reveal", { method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: Number(source.getAttribute("data-qlive-source")) }) })
      .then(function (response) { if (response.ok) showBadge("source opened", 900); })
      .catch(function () {});
  });

  function showBadge(text, ms) {
    if (!badge) {
      badge = document.createElement("div");
      badge.style.cssText = [
        "position:fixed", "right:12px", "bottom:12px", "z-index:2147483647",
        "font:12px/1.4 ui-monospace,SFMono-Regular,Menlo,monospace",
        "padding:6px 10px", "border-radius:6px",
        "background:rgba(20,20,20,.85)", "color:#fff",
        "pointer-events:none", "transition:opacity .2s", "opacity:0"
      ].join(";");
      document.body.appendChild(badge);
    }
    badge.textContent = "qlive " + text;
    badge.style.opacity = "1";
    if (ms) {
      setTimeout(function () { badge.style.opacity = "0"; }, ms);
    }
  }

  // Restore scroll after a reload.
  try {
    var saved = sessionStorage.getItem(KEY);
    if (saved !== null) {
      sessionStorage.removeItem(KEY);
      window.scrollTo(0, parseFloat(saved) || 0);
    }
  } catch (e) { /* private mode: not important */ }

  function connect() {
    var proto = location.protocol === "https:" ? "wss:" : "ws:";
    var ws = new WebSocket(proto + "//" + location.host + "/qlive/ws");

    ws.onopen = function () { showBadge("connected", 1200); };

    ws.onmessage = function (event) {
      var msg = event.data;

      if (msg === "reload") {
        showBadge("rendering…");
        try { sessionStorage.setItem(KEY, String(window.scrollY)); } catch (e) {}
        location.reload();
        return;
      }

      try {
        var patch = JSON.parse(msg);
        if (patch && patch.op === "body") {
          replaceBody(patch.html);       // M2: whole body, swapped in place
        } else if (patch && patch.ops) {
          applyPatch(patch);             // later: single blocks by data-qlive-id
        }
      } catch (e) {
        console.warn("qlive: unrecognised message", msg);
      }
    };

    ws.onclose = function () {
      showBadge("daemon gone, retrying");
      setTimeout(connect, 500);
    };
  }

  // Swap everything in <main> after the title block, which belongs to the page
  // shell (pandoc's template) and is not part of the warm body. No reload, so the
  // scroll position and the rest of the page stay exactly where they were.
  function replaceBody(html) {
    var main = document.querySelector("main");
    if (!main) {
      location.reload();
      return;
    }
    var header = main.querySelector("#title-block-header");
    var node = header ? header.nextSibling : main.firstChild;
    while (node) {
      var next = node.nextSibling;
      main.removeChild(node);
      node = next;
    }
    main.insertAdjacentHTML("beforeend", html);

    // Math is typeset client-side; only the new nodes need it.
    if (window.MathJax && window.MathJax.typesetPromise) {
      window.MathJax.typesetPromise([main]).catch(function () {});
    }
    window.__qlivePatches = (window.__qlivePatches || 0) + 1;
    showBadge("updated", 700);
  }

  // Placeholder so the protocol is exercised end to end from M0 onwards.
  function applyPatch(patch) {
    var applied = 0;
    patch.ops.forEach(function (op) {
      var node = document.querySelector('[data-qlive-id="' + op.id + '"]');
      if (!node) { location.reload(); return; }
      if (op.op === "replace" && typeof op.html === "string") {
        node.outerHTML = op.html;
        applied++;
      } else if (op.op === "remove") {
        node.remove();
        applied++;
      }
    });
    showBadge(applied + " block" + (applied === 1 ? "" : "s"), 900);
    window.__qliveBlockPatches = (window.__qliveBlockPatches || 0) + applied;
  }

  connect();
})();
