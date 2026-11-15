// Manda o estado do pedido para a Islandbar, que escuta só no próprio PC
// (127.0.0.1). Nada sai do computador.
const ISLAND = "http://127.0.0.1:47615/ifood";

// Só letras, números e pontuação simples, numa linha só.
function clean(value, max = 80) {
  return String(value ?? "")
    .replace(/[\u0000-\u001f\u007f]/g, " ")
    .trim()
    .slice(0, max);
}

function toText(order) {
  return [
    `order=${clean(order.id, 60)}`,
    `stage=${clean(order.stage, 20)}`,
    `store=${clean(order.store)}`,
    `eta=${clean(order.eta, 20)}`,
    `code=${clean(order.code, 8)}`,
    `chat=${Number(order.chats) || 0}`,
  ].join("\n");
}

async function post(body) {
  try {
    await fetch(ISLAND, { method: "POST", body, headers: { "Content-Type": "text/plain" } });
  } catch {
    // A ilha está fechada: tudo bem, a próxima repetição tenta de novo.
  }
}

// Quais pedidos cada aba mostra (para avisar a ilha quando a aba fecha).
async function remember(tabId, orderId) {
  if (tabId === undefined) return;
  const key = `tab${tabId}`;
  const stored = (await chrome.storage.session.get(key))[key] || [];
  if (!stored.includes(orderId)) {
    await chrome.storage.session.set({ [key]: [...stored, orderId].slice(-5) });
  }
}

chrome.runtime.onMessage.addListener((message, sender) => {
  if (message?.type === "order" && message.order) {
    remember(sender.tab?.id, clean(message.order.id, 60));
    post(toText(message.order));
  } else if (message?.type === "gone") {
    post(`gone=${clean(message.id, 60)}`);
  }
});

chrome.tabs.onRemoved.addListener(async (tabId) => {
  const key = `tab${tabId}`;
  const stored = (await chrome.storage.session.get(key))[key] || [];
  await chrome.storage.session.remove(key);
  for (const id of stored) post(`gone=${id}`);
});
