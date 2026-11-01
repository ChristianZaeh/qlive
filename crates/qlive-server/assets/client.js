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

      // M2: {"v":1,"seq":n,"ops":[{"op":"replace","id":"b17","html":"…"}]}
      try {
        var patch = JSON.parse(msg);
        if (patch && patch.ops) {
          applyPatch(patch);
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
