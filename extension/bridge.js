// Ponte entre a página do iFood (page.js) e a extensão (background.js).
// O page.js roda "dentro" da página e não pode falar com a extensão; ele
// avisa por window.postMessage e esta ponte repassa.
(() => {
  const latest = new Map();
  let diagnostic = null;

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
      return;
    }
    if (event.data.islandbar !== "ifood") return;
    const order = event.data.order;
    if (!order || typeof order.id !== "string") return;
    latest.set(order.id, order);
    send({ type: "order", order });
  });

  // A janelinha da extensão pede o diagnóstico desta aba.
  chrome.runtime.onMessage.addListener((message, _sender, reply) => {
    if (message?.type === "diagnostic") reply(diagnostic);
  });

  // De tempos em tempos repete o último estado: assim a ilha sabe que a aba
  // continua aberta (e recebe o pedido se ela abriu depois).
  setInterval(() => latest.forEach((order) => send({ type: "order", order })), 30000);

  window.addEventListener("pagehide", () => latest.forEach((order) => send({ type: "gone", id: order.id })));
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) latest.forEach((order) => send({ type: "order", order }));
  });
})();
