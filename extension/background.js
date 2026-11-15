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

// Ponto do mapa como "lat,lng" (ou vazio).
function spot(p) {
  return Array.isArray(p) && p.length === 2 && p.every(Number.isFinite) ? `${p[0].toFixed(5)},${p[1].toFixed(5)}` : "";
}

function toText(order) {
  return [
    `order=${clean(order.id, 60)}`,
    `stage=${clean(order.stage, 20)}`,
    `store=${clean(order.store)}`,
    `number=${clean(order.number, 12)}`,
    `eta=${clean(order.eta, 20)}`,
    `code=${clean(order.code, 8)}`,
    `chat=${Number(order.chats) || 0}`,
    `driver=${clean(order.driver, 30)}`,
    `vehicle=${clean(order.vehicle, 10)}`,
    `from=${spot(order.from)}`,
    `to=${spot(order.to)}`,
    `courier=${spot(order.courier)}`,
  ].join("\n");
}

async function post(body, stage) {
  let ok = false;
  try {
    ok = (await fetch(ISLAND, { method: "POST", body, headers: { "Content-Type": "text/plain" } })).ok;
  } catch {
    // A ilha está fechada: tudo bem, a próxima repetição tenta de novo.
  }
  // Para a janelinha da extensão mostrar se a ilha está recebendo.
  const status = { ok, at: Date.now() };
  if (stage) status.stage = stage;
  await chrome.storage.session.set({ status });
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
    post(toText(message.order), clean(message.order.stage, 20));
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
