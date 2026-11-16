// Ponte entre a página do iFood (page.js) e a extensão (background.js).
// O page.js roda "dentro" da página e não pode falar com a extensão; ele
// avisa por window.postMessage e esta ponte repassa.
(() => {
  const latest = new Map();
  let diagnostic = null;
  // Respostas que a janelinha da extensão está esperando.
  const waiting = { diag: [], auth: [] };

  function send(message) {
    try {
      chrome.runtime.sendMessage(message);
    } catch {
      // A extensão foi recarregada: esta aba só volta a falar depois de um F5.
    }
  }

  window.addEventListener("message", (event) => {
    if (event.source !== window || !event.data) return;
    if (event.data.islandbar === "ifood-diag") {
      diagnostic = event.data.diag;
      waiting.diag.splice(0).forEach((reply) => reply(diagnostic));
      return;
    }
    if (event.data.islandbar === "ifood-auth") {
      const headers = Array.isArray(event.data.headers) ? event.data.headers : null;
      waiting.auth.splice(0).forEach((reply) => reply(headers));
      return;
    }
    if (event.data.islandbar === "ifood-gone") {
      const id = String(event.data.id || "");
      if (latest.delete(id)) send({ type: "gone", id });
      return;
    }
    if (event.data.islandbar !== "ifood") return;
    const order = event.data.order;
    if (!order || typeof order.id !== "string") return;
    latest.set(order.id, order);
    send({ type: "order", order });
  });

  // A janelinha da extensão pede o diagnóstico desta aba (o de agora), ou os
  // cabeçalhos do site para o teste "a extensão consegue sozinha?".
  function request(kind, ask, fallback, reply) {
    waiting[kind].push(reply);
    window.postMessage({ islandbar: ask }, location.origin);
    setTimeout(() => {
      const at = waiting[kind].indexOf(reply);
      if (at >= 0) waiting[kind].splice(at, 1)[0](fallback());
    }, 1000);
  }
  chrome.runtime.onMessage.addListener((message, _sender, reply) => {
    if (message?.type === "diagnostic") {
      request("diag", "ifood-diag-request", () => diagnostic, reply);
      return true;
    }
    if (message?.type === "headers") {
      request("auth", "ifood-auth-request", () => null, reply);
      return true;
    }
    return false;
  });

  // De tempos em tempos repete o último estado: assim a ilha sabe que a aba
  // continua aberta (e recebe o pedido se ela abriu depois).
  setInterval(() => latest.forEach((order) => send({ type: "order", order })), 30000);

  window.addEventListener("pagehide", () => latest.forEach((order) => send({ type: "gone", id: order.id })));
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) latest.forEach((order) => send({ type: "order", order }));
  });
})();
