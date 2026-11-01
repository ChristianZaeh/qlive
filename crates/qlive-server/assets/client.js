// qlive client, M0.
//
// Keeps the scroll position across reloads, reconnects when the daemon restarts, and
// shows a small badge so you can see the loop working. M2 replaces the reload with
// node patches: the `patch` branch below is where those land.
(function () {
  var KEY = "qlive:scroll";
  var badge = null;

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
      if (!node) return;
      if (op.op === "replace" && typeof op.html === "string") {
        node.outerHTML = op.html;
        applied++;
      } else if (op.op === "remove") {
        node.remove();
        applied++;
      }
    });
    showBadge(applied + " block" + (applied === 1 ? "" : "s"), 900);
  }

  connect();
})();
