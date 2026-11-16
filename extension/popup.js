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

async function ask(tab, type) {
  try {
    return await chrome.tabs.sendMessage(tab.id, { type });
  } catch {
    return null;
  }
}

// A extensão consegue consultar o iFood sozinha, sem a aba? Duas tentativas
// da lista de pedidos em andamento (só leitura): só com os cookies do
// navegador e com o crachá que o site está usando. Fica só o resultado
// (número do status), nunca o conteúdo.
const ONGOING = "https://marketplace.ifood.com.br/v4/customers/me/orders?status=ONGOING&page=0&size=5";
async function attempt(init) {
  try {
    const response = await fetch(ONGOING, { cache: "no-store", signal: AbortSignal.timeout(2500), ...init });
    const type = response.headers.get("content-type") || "";
    return { status: response.status, json: type.includes("json") };
  } catch (error) {
    return { erro: error?.name || "falhou" };
  }
}

// Nomes e validade dos cookies do iFood (inclusive os que a página não vê).
async function cookies() {
  try {
    const list = await chrome.cookies.getAll({ domain: "ifood.com.br" });
    const now = Date.now() / 1000;
    return list.slice(0, 60).map((c) => ({
      nome: c.name.replace(/\d{4,}/g, ":n").slice(0, 60),
      dominio: c.domain,
      httpOnly: c.httpOnly,
      dias: c.session ? "sessao" : Math.round((c.expirationDate - now) / 86400),
      temJwt: /eyJ[\w-]{5,}\.[\w-]{5,}\./.test(decodeURIComponent(c.value || "")),
      tamanho: (c.value || "").length,
    }));
  } catch {
    return null;
  }
}

async function diagnostic() {
  const tab = await activeTab();
  if (!tab?.id) return null;
  const [diag, headers, jar] = await Promise.all([ask(tab, "diagnostic"), ask(tab, "headers"), cookies()]);
  if (!diag) return null;
  const [soCookies, comCracha] = await Promise.all([
    attempt({ credentials: "include" }),
    headers ? attempt({ credentials: "omit", headers: new Headers(headers) }) : null,
  ]);
  return { ...diag, cookies: jar, extensaoSozinha: { soCookies, comCracha } };
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
  order.textContent = status?.stage ? STAGES[status.stage] || status.stage : "nenhum pedido em andamento (deixe uma aba do iFood aberta)";

  document.getElementById("copy").addEventListener("click", async () => {
    const done = document.getElementById("done");
    done.textContent = "Testando…";
    const diag = await diagnostic();
    if (!diag) {
      done.textContent = "Abra esta janelinha com uma aba do iFood na frente.";
      return;
    }
    const text = JSON.stringify({ extensao: chrome.runtime.getManifest().version, ilha: status || null, ...diag }, null, 1);
    try {
      await navigator.clipboard.writeText(text);
      done.textContent = "Copiado! Agora é só colar na conversa.";
    } catch {
      // Sem permissão de copiar (demorou demais): mostra para copiar à mão.
      const box = document.getElementById("text");
      box.value = text;
      box.hidden = false;
      box.select();
      done.textContent = "Aperte Ctrl+C para copiar e cole na conversa.";
    }
  });
})();
