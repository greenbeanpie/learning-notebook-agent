// Local Notebook Agent — DevTools WebSocket Bridge (H3)
(function (global) {
  const params = new URLSearchParams(global.location.search);
  const wsUrl = params.get("ws");
  const token = params.get("token");
  const pending = new Map();
  const listeners = new Set();
  let socket = null;
  let connecting = null;
  let seq = 0;

  function isLive() {
    return Boolean(wsUrl && token);
  }

  function connect() {
    if (!isLive()) return Promise.resolve(false);
    if (socket && socket.readyState === WebSocket.OPEN) return Promise.resolve(true);
    if (connecting) return connecting;
    connecting = new Promise((resolve) => {
      const full = wsUrl + (wsUrl.includes("?") ? "&" : "?") + "token=" + encodeURIComponent(token);
      const ws = new WebSocket(full);
      socket = ws;
      let settled = false;
      const finish = (ok) => {
        if (settled) return;
        settled = true;
        connecting = null;
        resolve(ok);
      };
      const timeoutId = setTimeout(() => {
        try { ws.close(); } catch (_) {}
        finish(false);
      }, 5000);
      ws.onopen = () => { clearTimeout(timeoutId); finish(true); };
      ws.onerror = () => { clearTimeout(timeoutId); finish(false); };
      ws.onclose = () => {
        clearTimeout(timeoutId);
        finish(false);
        pending.forEach(({ rej }) => rej(new Error("DevTools WebSocket disconnected")));
        pending.clear();
      };
      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(ev.data);
          if (msg.id && pending.has(msg.id)) {
            const { res, rej } = pending.get(msg.id);
            pending.delete(msg.id);
            if (msg.error) rej(msg.error);
            else res(msg.result);
          } else if (msg.event) {
            listeners.forEach((fn) => fn(msg));
          }
        } catch (e) {
          console.error("WS parse error", e);
        }
      };
    });
    return connecting;
  }

  async function call(method, reqParams) {
    if (!isLive()) throw new Error("Offline demo mode");
    if (!(await connect()) || !socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error("DevTools WebSocket is not connected");
    }
    const id = "ui-" + (++seq);
    return new Promise((res, rej) => {
      pending.set(id, { res, rej });
      try {
        socket.send(JSON.stringify({ id, method, params: reqParams || {} }));
      } catch (error) {
        pending.delete(id);
        rej(error);
      }
    });
  }

  function onEvent(fn) {
    listeners.add(fn);
    return () => listeners.delete(fn);
  }

  function escapeHtml(str) {
    return String(str ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  global.LnaApi = { isLive, connect, call, onEvent, escapeHtml };
})(window);
