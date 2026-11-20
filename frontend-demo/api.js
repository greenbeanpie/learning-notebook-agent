// Local Notebook Agent — DevTools WebSocket Bridge (Section 13.5 / H3)
(function (global) {
  const params = new URLSearchParams(global.location.search);
  const wsUrl = params.get("ws");
  const token = params.get("token");

  const pending = new Map();
  const listeners = new Set();
  let socket = null;
  let seq = 0;

  function isLive() {
    return Boolean(wsUrl && token);
  }

  function connect() {
    if (!isLive()) return Promise.resolve(false);
    if (socket && socket.readyState === WebSocket.OPEN) return Promise.resolve(true);
    return new Promise((resolve) => {
      const full = wsUrl + (wsUrl.includes("?") ? "&" : "?") + "token=" + encodeURIComponent(token);
      socket = new WebSocket(full);
      socket.onopen = () => resolve(true);
      socket.onerror = () => resolve(false);
      socket.onmessage = (ev) => {
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
  }

  async function call(method, reqParams) {
    if (!isLive()) {
      throw new Error("Offline demo mode");
    }
    await connect();
    const id = "ui-" + (++seq);
    return new Promise((res, rej) => {
      pending.set(id, { res, rej });
      socket.send(JSON.stringify({ id, method, params: reqParams || {} }));
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
