// Manda o estado do pedido para a Islandbar, que escuta só no próprio PC
// (127.0.0.1).
//
// Com uma aba do iFood aberta e logada, quem acompanha é a própria aba
// (page.js). Sem aba, este script continua sozinho, a cada 1 ou 2 minutos:
// usa o login que o site deixou no navegador (os cookies aAccessToken e
// aRefreshToken de www.ifood.com.br), pergunta ao iFood quais pedidos estão
// em andamento e manda o resumo para a ilha. Quando o crachá vence (dura
// 2 horas), renova do mesmo jeito que o site renova e grava o novo nos mesmos
// cookies, para o site e a extensão continuarem usando o mesmo login.
// Se o login acabou de vez (precisa entrar de novo), avisa a ilha.
//
// O login só é usado para falar com o iFood e nunca sai do navegador.
importScripts("reader.js");
const R = globalThis.__islandbarReader;

const ISLAND = "http://127.0.0.1:47615/ifood";
const SITE = "https://www.ifood.com.br/";
const API = "https://marketplace.ifood.com.br";
const ONGOING = `${API}/v4/customers/me/orders?status=ONGOING&page=0&size=5`;
const RENEW = "https://www.ifood.com.br/site-api/v2/access_tokens";
// Uma aba logada deu notícia há menos que isso: ela cuida de tudo.
const TAB_QUIET_MS = 75000;
// Sem pedido em andamento, a lista é consultada a cada 2 minutos.
const IDLE_MS = 115000;
// O iFood recusou (proteção contra robôs): espera antes de tentar de novo.
const BLOCKED_MS = 10 * 60000;

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

// --- Estado do segundo plano (sobrevive ao "cochilo" do service worker) ---
async function loadState() {
  const { bg } = await chrome.storage.session.get("bg");
  return {
    orders: {},
    listAt: 0,
    blockedUntil: 0,
    tabAt: 0,
    warned: false,
    failedRefresh: "",
    renewals: 0,
    status: "",
    ...bg,
  };
}

async function saveState(bg) {
  await chrome.storage.session.set({ bg });
}

// --- Login ---
function bare(token) {
  return String(token || "").replace(/^Bearer\s+/i, "");
}

// Quando o crachá vence (segundos desde 1970), lido do próprio crachá.
function expiry(token) {
  try {
    const part = bare(token).split(".")[1];
    const b64 = part.replace(/-/g, "+").replace(/_/g, "/");
    return Number(JSON.parse(atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4))).exp) || 0;
  } catch {
    return 0;
  }
}

// O crachá (um JWT) dentro do valor de um cookie, como quer que esteja escrito.
function jwtIn(value) {
  let text = String(value || "");
  try {
    text = decodeURIComponent(text);
  } catch {
    // fica como veio
  }
  return text.match(/eyJ[\w-]+\.[\w-]+\.[\w-]*/)?.[0] || "";
}

// Um cookie do iFood (o site pode guardá-lo "particionado": tenta os dois jeitos).
async function cookie(name) {
  const tries = [{ url: SITE, name }];
  tries.push({ url: SITE, name, partitionKey: { topLevelSite: "https://ifood.com.br" } });
  for (const query of tries) {
    try {
      const [found] = await chrome.cookies.getAll(query);
      if (jwtIn(found?.value)) return found;
    } catch {
      // Navegador antigo sem "partitionKey": segue.
    }
  }
  return null;
}

// Grava um crachá novo num cookie, no mesmo formato e com as mesmas
// características do original (só o crachá muda).
async function rewrite(original, name, jwt) {
  const host = original.domain.replace(/^\./, "");
  const old = jwtIn(original.value);
  const value = old && original.value.includes(old) ? original.value.replace(old, jwt) : jwt;
  const details = {
    url: `https://${host}${original.path || "/"}`,
    name,
    value,
    path: original.path,
    secure: original.secure,
    httpOnly: original.httpOnly,
    sameSite: original.sameSite,
    storeId: original.storeId,
  };
  if (!original.hostOnly) details.domain = original.domain;
  if (!original.session && original.expirationDate) {
    details.expirationDate = Math.max(original.expirationDate, expiry(jwt));
  }
  if (original.partitionKey) details.partitionKey = original.partitionKey;
  try {
    return !!(await chrome.cookies.set(details));
  } catch {
    return false;
  }
}

// O crachá para consultar o iFood. Devolve { token } ou { expired } ou {}.
async function login(bg, headers, force) {
  const now = Date.now() / 1000;
  const [access, refresh] = await Promise.all([cookie("aAccessToken"), cookie("aRefreshToken")]);
  const { copy } = await chrome.storage.session.get("copy");
  bg.cookies = { acesso: !!access, renovar: !!refresh, particionado: !!(access?.partitionKey || refresh?.partitionKey) };
  const renewal = jwtIn(refresh?.value);
  // Essa renovação já foi recusada: o login acabou. Só tenta de novo quando o
  // site mudar o cookie (você entrou de novo).
  if (bg.failedRefresh && bg.failedRefresh === renewal.slice(-24)) return { expired: true };
  // O mais novo entre o cookie e a cópia que a aba mandou.
  let token = jwtIn(access?.value);
  if (copy?.token && expiry(copy.token) > expiry(token)) token = bare(copy.token);
  if (!force && token && expiry(token) > now + 120) return { token };

  // Precisa renovar. Só com o cookie de renovação, para gravar o novo no lugar
  // dele: assim o site continua logado com o mesmo login.
  if (!renewal) return {};
  if (expiry(renewal) && expiry(renewal) < now) return { expired: true };
  let response;
  try {
    response = await fetch(RENEW, {
      method: "POST",
      cache: "no-store",
      credentials: "include",
      headers: new Headers([...headers, ["content-type", "application/json"]]),
      body: JSON.stringify({ refresh_token: renewal }),
    });
  } catch {
    return {};
  }
  bg.lastRenew = response.status;
  if (response.status === 400 || response.status === 401) {
    bg.failedRefresh = renewal.slice(-24);
    return { expired: true };
  }
  if (!response.ok) {
    if (response.status === 403 || response.status === 429) bg.blockedUntil = Date.now() + BLOCKED_MS;
    return {};
  }
  let json;
  try {
    json = await response.json();
  } catch {
    return {};
  }
  const fresh = typeof json?.access_token === "string" ? json.access_token : "";
  if (!fresh) return {};
  bg.renewals += 1;
  bg.failedRefresh = "";
  // Grava nos cookies do site (o de acesso, se não existir, nasce igual ao de renovação).
  const wrote = await rewrite(access || refresh, "aAccessToken", fresh);
  if (typeof json.refresh_token === "string" && json.refresh_token) {
    await rewrite(refresh, "aRefreshToken", json.refresh_token);
  }
  bg.wrote = wrote;
  await chrome.storage.session.set({ copy: { token: fresh } });
  return { token: fresh };
}

// --- Consultas ao iFood ---
async function ask(url, token, headers, sameSite) {
  const response = await fetch(url, {
    method: "GET",
    cache: "no-store",
    credentials: sameSite ? "include" : "omit",
    headers: new Headers([...headers, ["authorization", `Bearer ${token}`]]),
  });
  if (!response.ok) {
    const error = new Error(`HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  return response.json();
}

// Etapas, mensagens e código (a mesma consulta da página do pedido).
async function events(o, token, headers) {
  try {
    const json = await ask(`${SITE}site-api/v1/customers/me/orders/${o.id}/events`, token, headers, true);
    R.readEvents(o, json);
  } catch {
    // Sem as etapas, a lista e os detalhes já bastam.
  }
}

// Detalhes (o entregador no mapa, se o iFood informar).
async function details(o, token, headers) {
  o.detailsAt = Date.now();
  try {
    R.readDetails(o, await ask(`${API}/v3/customers/me/orders/${o.id}`, token, headers));
  } catch {
    // Fica com o que a lista trouxe.
  }
}

async function publish(o) {
  const text = toText(R.summary(o));
  // Repete a cada consulta mesmo sem mudança: é o "ainda estou aqui" para a ilha.
  o.sent = text;
  await post(text, o.stage);
}

// Avisa (ou desavisa) a ilha que o login do iFood acabou.
async function warn(bg, on) {
  if (on) {
    bg.warned = true;
    bg.status = "login vencido: entre no iFood de novo";
    await post("login=expired");
  } else if (bg.warned) {
    bg.warned = false;
    await post("login=ok");
  }
}

// --- Uma rodada do acompanhamento sem aba ---
let running = false;
async function poll() {
  if (running) return;
  running = true;
  const bg = await loadState();
  try {
    await round(bg);
  } finally {
    running = false;
    await saveState(bg);
  }
}

async function round(bg) {
  const now = Date.now();
  if (now - bg.tabAt < TAB_QUIET_MS) {
    bg.status = "aba do iFood aberta: ela acompanha";
    return;
  }
  if (now < bg.blockedUntil) return;
  const active = Object.values(bg.orders).some((o) => o.listed && !R.FINAL.has(o.stage));
  if (!active && !bg.warned && now - bg.listAt < IDLE_MS) return;

  const { headers } = await chrome.storage.local.get("headers");
  if (!Array.isArray(headers)) {
    bg.status = "abra o site do iFood uma vez, logado";
    return;
  }
  let auth = await login(bg, headers, false);
  if (auth.expired) return warn(bg, true);
  if (!auth.token) {
    bg.status = bg.cookies?.renovar ? "não deu para renovar o login agora" : "sem login do iFood no navegador";
    return;
  }

  let json;
  try {
    json = await ask(ONGOING, auth.token, headers);
  } catch (error) {
    if (error.status === 401) {
      // O crachá foi recusado antes da hora: renova e tenta uma vez mais.
      auth = await login(bg, headers, true);
      if (auth.expired) return warn(bg, true);
      if (auth.token) json = await ask(ONGOING, auth.token, headers).catch(() => null);
    } else if (error.status === 403 || error.status === 429) {
      bg.blockedUntil = Date.now() + BLOCKED_MS;
    }
    bg.lastList = error.status || "falhou";
    if (!json) {
      bg.status = `o iFood não respondeu (${bg.lastList})`;
      return;
    }
  }
  bg.listAt = Date.now();
  bg.lastList = 200;
  await warn(bg, false);

  const ongoing = new Set();
  for (const [id, item] of R.listItems(json)) {
    const known = bg.orders[id];
    const o = known || R.blank(id);
    R.readDetails(o, item);
    if (R.FINAL.has(o.stage)) {
      // Acabou: a ilha mostra "Entregue" por alguns minutos.
      if (known?.listed) {
        known.listed = false;
        await publish(o);
      }
      continue;
    }
    ongoing.add(id);
    o.listed = true;
    bg.orders[id] = o;
    await events(o, auth.token, headers);
    // Com o entregador a caminho, os detalhes a cada rodada (posição no mapa).
    const moving = ["driver", "ready", "pickup", "collected", "shipping", "arrived"].includes(o.stage);
    if (moving || Date.now() - (o.detailsAt || 0) > 5 * 60000) await details(o, auth.token, headers);
    await publish(o);
  }
  // Saiu da lista sem a gente ver o fim: confere uma última vez.
  for (const o of Object.values(bg.orders)) {
    if (!o.listed || ongoing.has(o.id)) continue;
    o.listed = false;
    await details(o, auth.token, headers);
    if (R.FINAL.has(o.stage)) await publish(o);
    else await post(`gone=${clean(o.id, 60)}`);
  }
  // Esquece os que já acabaram.
  for (const [id, o] of Object.entries(bg.orders)) if (!o.listed) delete bg.orders[id];
  bg.status = ongoing.size ? `acompanhando ${ongoing.size} pedido(s) sem aba` : "sem pedido em andamento";
}

// Uma aba fechou (ou saiu do iFood) com pedidos: se der para seguir sem ela, o
// segundo plano assume; senão a pílula some.
async function tabGone(ids) {
  const { headers } = await chrome.storage.local.get("headers");
  if (!Array.isArray(headers)) {
    for (const id of ids) await post(`gone=${id}`);
    return;
  }
  const bg = await loadState();
  bg.tabAt = 0;
  for (const id of ids) {
    if (!bg.orders[id]) bg.orders[id] = { ...R.blank(id), listed: true };
    else bg.orders[id].listed = true;
  }
  await saveState(bg);
  setTimeout(poll, 3000);
}

// Quais pedidos cada aba mostra (para saber o que fazer quando ela fecha).
async function remember(tabId, orderId) {
  if (tabId === undefined) return;
  const key = `tab${tabId}`;
  const stored = (await chrome.storage.session.get(key))[key] || [];
  if (!stored.includes(orderId)) {
    await chrome.storage.session.set({ [key]: [...stored, orderId].slice(-5) });
  }
}

async function tabAlive() {
  const bg = await loadState();
  bg.tabAt = Date.now();
  await saveState(bg);
}

chrome.runtime.onMessage.addListener((message, sender) => {
  if (message?.type === "order" && message.order) {
    remember(sender.tab?.id, clean(message.order.id, 60));
    post(toText(message.order), clean(message.order.stage, 20));
  } else if (message?.type === "gone") {
    tabGone([clean(message.id, 60)]);
  } else if (message?.type === "tab") {
    tabAlive();
  } else if (message?.type === "login" && typeof message.token === "string" && Array.isArray(message.headers)) {
    // A aba logada mandou o crachá (fica só na memória do navegador) e os
    // cabeçalhos do site (guardados para consultar sem a aba).
    const headers = message.headers
      .filter((h) => Array.isArray(h) && h.length === 2 && h[0] !== "authorization")
      .map(([name, value]) => [String(name).slice(0, 60), String(value).slice(0, 4000)])
      .slice(0, 40);
    chrome.storage.local.set({ headers });
    chrome.storage.session.set({ copy: { token: message.token } });
    loadState().then(async (bg) => {
      bg.tabAt = Date.now();
      bg.failedRefresh = "";
      await warn(bg, false);
      bg.status = "aba do iFood aberta: ela acompanha";
      await saveState(bg);
    });
  }
});

// A cada minuto (o navegador acorda este script só para isso).
function schedule() {
  chrome.alarms.get("pedidos", (alarm) => {
    if (!alarm) chrome.alarms.create("pedidos", { periodInMinutes: 1 });
  });
}
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === "pedidos") poll();
});
schedule();

// Quando o navegador abre (ou a extensão é instalada/atualizada), dá um "oi"
// para a ilha: assim ela sabe que a extensão está instalada.
chrome.runtime.onStartup.addListener(() => {
  post("hello=1");
  setTimeout(poll, 5000);
});
chrome.runtime.onInstalled.addListener(() => post("hello=1"));

chrome.tabs.onRemoved.addListener(async (tabId) => {
  const key = `tab${tabId}`;
  const stored = (await chrome.storage.session.get(key))[key] || [];
  await chrome.storage.session.remove(key);
  if (stored.length) tabGone(stored);
});
