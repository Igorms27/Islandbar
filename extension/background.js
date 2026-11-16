// Manda o estado do pedido para a Islandbar, que escuta só no próprio PC
// (127.0.0.1).
//
// Com uma aba do iFood aberta e logada, quem descobre pedidos novos é a
// própria aba (page.js). Sem aba, este script faz isso sozinho, a cada minuto:
// usa o login que o site deixou no navegador (os cookies aAccessToken e
// aRefreshToken de www.ifood.com.br), pergunta ao iFood quais pedidos estão
// em andamento e manda o resumo para a ilha. Quando o crachá vence (dura
// 2 horas), renova do mesmo jeito que o site renova e grava o novo nos mesmos
// cookies, para o site e a extensão continuarem usando o mesmo login.
// Se o login acabou de vez (precisa entrar de novo), avisa a ilha.
//
// Com um pedido em andamento (com ou sem aba), este script consulta as
// etapas e as mensagens a cada 15 s e a posição do entregador a cada 30 s: o
// navegador "freia" abas de fundo (uma consulta por minuto), mas não ele.
//
// O login só é usado para falar com o iFood e nunca sai do navegador.
importScripts("reader.js");
const R = globalThis.__islandbarReader;

const ISLAND = "http://127.0.0.1:47615/ifood";
const SITE = "https://www.ifood.com.br/";
const API = "https://marketplace.ifood.com.br";
const ONGOING = `${API}/v4/customers/me/orders?status=ONGOING&page=0&size=5`;
const RENEW = "https://www.ifood.com.br/site-api/v2/access_tokens";
// Uma aba logada deu notícia há menos que isso: ela procura os pedidos novos
// e renova o login (este script não renova junto, para não atropelar o site).
const TAB_QUIET_MS = 75000;
// A lista de pedidos (para achar pedido novo): a cada minuto.
const LIST_MS = 55000;
// Com pedido em andamento: etapas e mensagens a cada 15 s...
const FAST_MS = 15000;
// ...e a posição do entregador a cada 30 s.
const DETAILS_MS = 30000;
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
// Fica na memória enquanto este script está acordado (todos usam o mesmo
// objeto) e numa cópia na sessão do navegador, para quando ele acordar de novo.
let memo = null;
async function loadState() {
  if (memo) return memo;
  const { bg } = await chrome.storage.session.get("bg");
  memo = {
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
  return memo;
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
async function login(bg, headers, force, mayRenew = true) {
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
  // Com a aba aberta, quem renova é o site: espera o crachá novo dele.
  if (!mayRenew) return token && expiry(token) > now ? { token } : {};

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
    return true;
  } catch (error) {
    // Sem as etapas, a lista e os detalhes já bastam.
    return error.status || false;
  }
}

// O entregador já está envolvido (a posição dele no mapa interessa).
function moving(o) {
  return ["driver", "ready", "pickup", "collected", "shipping", "arrived"].includes(o.stage);
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
let quick = 0;
async function poll() {
  if (running) return;
  running = true;
  const bg = await loadState();
  try {
    await round(bg);
  } finally {
    running = false;
    await saveState(bg);
    // Pedido em andamento: a próxima consulta já em 15 s (o alarme de 30 s fica de reserva).
    clearTimeout(quick);
    if (active(bg)) quick = setTimeout(poll, FAST_MS);
  }
}

function active(bg) {
  return Object.values(bg.orders).some((o) => o.listed && !R.FINAL.has(o.stage));
}

async function round(bg) {
  const now = Date.now();
  const tab = now - bg.tabAt < TAB_QUIET_MS;
  if (now < bg.blockedUntil) return;
  // A lista (pedido novo) só sem aba: com aba, a própria aba procura.
  const listDue = !tab && now - bg.listAt >= LIST_MS;
  if (!active(bg) && !listDue) {
    if (tab) bg.status = "aba do iFood aberta: ela procura os pedidos";
    return;
  }
  // O alarme de reserva e o relógio de 15 s caíram juntos: uma consulta basta.
  if (!listDue && now - (bg.quickAt || 0) < FAST_MS - 3000) return;
  bg.quickAt = now;

  const { headers } = await chrome.storage.local.get("headers");
  if (!Array.isArray(headers)) {
    bg.status = "abra o site do iFood uma vez, logado";
    return;
  }
  let auth = await login(bg, headers, false, !tab);
  if (auth.expired && !tab) return warn(bg, true);
  if (!auth.token) {
    bg.status = bg.cookies?.renovar ? "não deu para renovar o login agora" : "sem login do iFood no navegador";
    return;
  }
  if (!listDue) return followQuick(bg, auth.token, headers, tab);

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
    // Com o entregador a caminho, os detalhes (posição no mapa); antes, de vez em quando.
    const wait = moving(o) ? DETAILS_MS : 5 * 60000;
    if (Date.now() - (o.detailsAt || 0) >= wait - 2000) await details(o, auth.token, headers);
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
  bg.status = ongoing.size ? `acompanhando ${ongoing.size} pedido(s) a cada 15 s` : "sem pedido em andamento";
}

// Entre uma lista e outra: só as etapas e mensagens (e o entregador no mapa)
// dos pedidos em andamento.
async function followQuick(bg, token, headers, tab) {
  let failed = false;
  for (const o of Object.values(bg.orders)) {
    if (!o.listed || R.FINAL.has(o.stage)) continue;
    const ok = await events(o, token, headers);
    if (ok !== true) failed = true;
    if (ok === 403 || ok === 429) bg.blockedUntil = Date.now() + BLOCKED_MS;
    if (moving(o) && Date.now() - (o.detailsAt || 0) >= DETAILS_MS - 2000) await details(o, token, headers);
    await publish(o);
    // Chegou ao fim pelas etapas: a lista confirma na próxima rodada.
    if (R.FINAL.has(o.stage)) o.listed = false;
  }
  // As etapas não vieram: a lista (que também traz a etapa) vai na próxima rodada.
  if (failed && !tab) bg.listAt = 0;
  const count = Object.values(bg.orders).filter((o) => o.listed && !R.FINAL.has(o.stage)).length;
  bg.status = count ? `acompanhando ${count} pedido(s) a cada 15 s` : "sem pedido em andamento";
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

// Um pedido que a aba achou: o segundo plano passa a seguir as etapas dele a
// cada 15 s (a aba de fundo, freada pelo navegador, só consegue a cada minuto).
async function follow(id, stage) {
  if (!R.ID.test(id)) return;
  const bg = await loadState();
  bg.tabAt = Date.now();
  if (R.FINAL.has(stage) || bg.orders[id]) return;
  bg.orders[id] = { ...R.blank(id), listed: true };
  R.advance(bg.orders[id], stage);
  await saveState(bg);
  if (!running) setTimeout(poll, 1000);
}

async function tabAlive() {
  const bg = await loadState();
  bg.tabAt = Date.now();
  await saveState(bg);
}

chrome.runtime.onMessage.addListener((message, sender) => {
  if (message?.type === "order" && message.order) {
    const id = clean(message.order.id, 60);
    remember(sender.tab?.id, id);
    post(toText(message.order), clean(message.order.stage, 20));
    follow(id, clean(message.order.stage, 20));
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
    if (!alarm || alarm.periodInMinutes !== 0.5) chrome.alarms.create("pedidos", { periodInMinutes: 0.5 });
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
