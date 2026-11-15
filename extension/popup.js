// Janelinha da extensão: mostra se a ilha está recebendo e copia um
// diagnóstico (só o formato das respostas do site, sem seus dados) para
// ajudar a ajustar a leitura quando o site do iFood muda.
const STAGES = {
  unknown: "acompanhando",
  placed: "enviado",
  confirmed: "confirmado",
  preparing: "em preparo",
  driver: "entregador indo à loja",
  ready: "pronto na loja",
  pickup: "entregador na loja",
  collected: "entregador pegou",
  shipping: "a caminho",
  arrived: "entregador chegou",
  delivered: "entregue",
  cancelled: "cancelado",
};

async function activeTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  return tab;
}

async function diagnostic() {
  const tab = await activeTab();
  if (!tab?.id) return null;
  try {
    return await chrome.tabs.sendMessage(tab.id, { type: "diagnostic" });
  } catch {
    return null;
  }
}

(async () => {
  const { status } = await chrome.storage.session.get("status");
  const island = document.getElementById("island");
  const order = document.getElementById("order");
  if (!status) {
    island.textContent = "ainda sem pedido para mandar";
  } else {
    const seconds = Math.round((Date.now() - status.at) / 1000);
    island.textContent = status.ok ? `recebendo (há ${seconds} s)` : "não respondeu (ela está aberta?)";
  }
  order.textContent = status?.stage ? STAGES[status.stage] || status.stage : "abra a página do pedido";

  document.getElementById("copy").addEventListener("click", async () => {
    const done = document.getElementById("done");
    const diag = await diagnostic();
    if (!diag) {
      done.textContent = "Abra esta janelinha com a aba do pedido do iFood na frente.";
      return;
    }
    const text = JSON.stringify({ extensao: chrome.runtime.getManifest().version, ilha: status || null, ...diag }, null, 1);
    await navigator.clipboard.writeText(text);
    done.textContent = "Copiado! Agora é só colar na conversa.";
  });
})();
