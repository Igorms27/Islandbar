// Lê o andamento do pedido na própria página do iFood.
//
// O site busca as etapas do pedido em
//   /site-api/v1/customers/me/orders/<id>/events
// (uma lista de eventos como CONFIRMED, DISPATCHED...). Este script observa
// essas respostas quando o site as recebe e, se a aba ficar em segundo plano
// sem atualizar, repete a mesma consulta a cada minuto. Também lê da tela a
// previsão de entrega e o nome da loja.
//
// O resumo vai para bridge.js por window.postMessage. Nenhum dado de login é
// guardado nem enviado: a consulta repetida usa o próprio fetch da página.
//
// Os nomes dos eventos e os endereços vieram do projeto iFood PiP
// (github.com/luannsr12/ifood-pip, licença Apache 2.0).
(() => {
  if (window.__islandbarIfood) return;
  window.__islandbarIfood = true;

  const EVENTS = /^\/site-api\/v\d+\/customers\/me\/orders\/([a-f\d-]{30,50})\/events\/?$/i;
  const DETAILS = /^\/site-api\/v\d+\/customers\/me\/orders\/([a-f\d-]{30,50})\/?$/i;
  const PAGE = /^\/pedido\/([a-f\d-]{30,50})(?:\/|$)/i;
  const REFRESH_MS = 60000;
  const MAX_FAILURES = 4;

  // Etapas, da primeira à última. A ilha só avança (eventos atrasados não voltam a etapa).
  const STAGES = {
    CREATED: [1, "placed"],
    PLACED: [1, "placed"],
    CONFIRMED: [2, "confirmed"],
    PREPARATION_STARTED: [3, "preparing"],
    SEPARATION_STARTED: [3, "preparing"],
    ASSIGN_DRIVER: [3.5, "driver"],
    DELIVERY_ACCEPTED: [3.5, "driver"],
    GOING_TO_ORIGIN: [4, "driver"],
    DELIVERY_GOING_TO_ORIGIN: [4, "driver"],
    READY_TO_PICKUP: [4.5, "ready"],
    SEPARATION_ENDED: [4.5, "ready"],
    ARRIVED_AT_ORIGIN: [5, "pickup"],
    DELIVERY_ARRIVED_AT_ORIGIN: [5, "pickup"],
    COLLECTED: [5.5, "collected"],
    DELIVERY_COLLECTED: [5.5, "collected"],
    DISPATCHED: [6, "shipping"],
    GOING_TO_DESTINATION: [6, "shipping"],
    DELIVERY_GOING_TO_DESTINATION: [6, "shipping"],
    ARRIVED: [7, "arrived"],
    ARRIVED_AT_DESTINATION: [7, "arrived"],
    DELIVERY_ARRIVED_AT_DESTINATION: [7, "arrived"],
    DELIVERED: [8, "delivered"],
    CONCLUDED: [8, "delivered"],
    COMPLETED: [8, "delivered"],
    DELIVERY_COMPLETED: [8, "delivered"],
    CANCELLED: [9, "cancelled"],
    ORDER_CANCELLED: [9, "cancelled"],
  };
  const RANK = Object.fromEntries(Object.values(STAGES).map(([rank, stage]) => [stage, rank]));
  const FINAL = new Set(["delivered", "cancelled"]);

  // Pedidos vistos nesta aba: id -> estado.
  const orders = new Map();
  const nativeFetch = window.fetch;

  function order(id) {
    let o = orders.get(id);
    if (!o) {
      o = {
        id,
        stage: "unknown",
        rank: 0,
        store: "",
        eta: "",
        etaFromEvents: "",
        code: "",
        chats: 0,
        lastPage: 0,
        failures: 0,
        replay: null,
        sent: "",
      };
      orders.set(id, o);
    }
    return o;
  }

  function idFrom(pattern, input) {
    try {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      if (url.origin !== location.origin) return null;
      return pattern.exec(url.pathname)?.[1]?.toLowerCase() || null;
    } catch {
      return null;
    }
  }

  function advance(o, stage) {
    const rank = RANK[stage] || 0;
    if (rank > o.rank) {
      o.rank = rank;
      o.stage = stage;
    }
  }

  function hhmm(ms) {
    const d = new Date(ms);
    return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  }

  // Lista de eventos -> etapa, código de entrega, mensagens e previsão.
  function readEvents(o, list) {
    if (!Array.isArray(list)) return false;
    let code = "";
    let validated = false;
    let chats = 0;
    for (const item of list) {
      if (!item || typeof item.value !== "string") continue;
      if (item.orderId && String(item.orderId).toLowerCase() !== o.id) continue;
      const value = item.value.toUpperCase();
      const meta = item.metadata || {};
      const known = STAGES[value];
      if (known) advance(o, known[1]);
      if (value.includes("CHAT")) chats += 1;
      if (/^DELIVERY_DROP_CODE_/.test(value) && meta.HANDSHAKE_TYPE === "DELIVERY") {
        if (/^\d{4,6}$/.test(String(meta.HANDSHAKE_CODE || ""))) code = String(meta.HANDSHAKE_CODE);
        if (value === "DELIVERY_DROP_CODE_VALIDATION_SUCCESS") validated = true;
      }
      const eta = Date.parse(meta.newEta || "");
      if (Number.isFinite(eta)) o.etaFromEvents = hhmm(eta);
    }
    o.code = validated ? "" : code || o.code;
    o.chats = Math.max(o.chats, chats);
    return true;
  }

  function readDetails(o, json) {
    const data = json?.data?.order || json?.order || json?.data || json;
    const name = data?.merchant?.name || data?.restaurant?.name;
    if (typeof name === "string" && name.length <= 80) o.store = name.trim();
  }

  // Complementa com o que está escrito na tela (só na página do pedido).
  function readScreen() {
    const id = PAGE.exec(location.pathname)?.[1]?.toLowerCase();
    if (!id || !document.body) return;
    const o = order(id);
    const lines = (document.body.innerText || "")
      .split(/\n+/)
      .map((s) => s.replace(/\s+/g, " ").trim())
      .filter(Boolean);
    const text = lines.join("\n");

    // "Previsão de entrega" seguida de algo como "Hoje, 20:10 - 20:25".
    const at = lines.findIndex((s) => /previs[aã]o de entrega/i.test(s));
    if (at >= 0) {
      const near = lines.slice(at, at + 5).join(" ");
      const range = near.match(/(\d{1,2}:\d{2})\s*(?:[-–—]|a|e)\s*(\d{1,2}:\d{2})/i);
      const single = near.match(/\b(\d{1,2}:\d{2})\b/);
      o.eta = range ? `${range[1]}-${range[2]}` : single ? single[1] : o.eta;
    }

    // Só frases claras da etapa atual ("logo sairá" não conta como saiu).
    const phrases = [
      ["cancelled", /pedido (?:foi |est[aá] )?cancelado/i],
      ["delivered", /pedido (?:foi )?(?:entregue|conclu[ií]do)/i],
      ["arrived", /entregador chegou (?:ao seu endere[çc]o|ao destino)/i],
      ["shipping", /pedido (?:saiu para entrega|est[aá] a caminho)/i],
      ["preparing", /pedido (?:est[aá] sendo preparado|est[aá] em prepara[çc][aã]o|em preparo)/i],
    ];
    for (const [stage, pattern] of phrases) {
      if (pattern.test(text)) {
        advance(o, stage);
        break;
      }
    }

    const code = text.match(/c[oó]digo (?:de|da) entrega\s*[:–-]?\s*(\d{4,6})\b/i)?.[1];
    if (code && !FINAL.has(o.stage)) o.code = code;

    if (!o.store) {
      // O nome da loja fica logo antes de "Pedido nº 1234".
      const number = [...document.querySelectorAll("span,p,small,strong")].find(
        (el) => !el.children.length && /^Pedido\s*(?:N\s*[º°o.]?|#)\s*\d+$/i.test(el.textContent.trim()),
      );
      const name = number?.previousElementSibling?.textContent?.trim();
      if (name && name.length <= 80) o.store = name;
    }
    publish(o);
  }

  function publish(o) {
    const summary = {
      id: o.id,
      stage: o.stage,
      store: o.store,
      eta: o.eta || o.etaFromEvents,
      code: FINAL.has(o.stage) ? "" : o.code,
      chats: o.chats,
    };
    const text = JSON.stringify(summary);
    if (text === o.sent) return;
    o.sent = text;
    window.postMessage({ islandbar: "ifood", order: summary }, location.origin);
  }

  function gotEvents(id, json, fromPage) {
    const o = order(id);
    if (!readEvents(o, json)) return;
    o.failures = 0;
    if (fromPage) o.lastPage = Date.now();
    publish(o);
  }

  function watchResponse(promise, handle) {
    Promise.resolve(promise)
      .then((response) => {
        if (response?.ok) response.clone().json().then(handle, () => {});
      })
      .catch(() => {});
  }

  // --- Observa o que o próprio site busca ---
  window.fetch = function (input, init) {
    const response = Reflect.apply(nativeFetch, this, arguments);
    const eventsId = idFrom(EVENTS, input);
    const detailsId = idFrom(DETAILS, input);
    if (eventsId) {
      try {
        const request = new Request(input, init);
        if (request.method === "GET") order(eventsId).replay = () => nativeFetch(new Request(request, { cache: "no-store" }));
      } catch {
        // Sem cópia da consulta: só não dá para repetir.
      }
      watchResponse(response, (json) => gotEvents(eventsId, json, true));
    } else if (detailsId) {
      watchResponse(response, (json) => {
        readDetails(order(detailsId), json);
        publish(order(detailsId));
      });
    }
    return response;
  };

  const xhrOpen = XMLHttpRequest.prototype.open;
  const xhrHeader = XMLHttpRequest.prototype.setRequestHeader;
  const xhrSend = XMLHttpRequest.prototype.send;
  const xhrInfo = new WeakMap();
  XMLHttpRequest.prototype.open = function (method, url) {
    const get = String(method).toUpperCase() === "GET";
    const eventsId = get ? idFrom(EVENTS, url) : null;
    const detailsId = get ? idFrom(DETAILS, url) : null;
    xhrInfo.set(
      this,
      eventsId || detailsId ? { eventsId, detailsId, url: new URL(String(url), location.href).href, headers: [] } : null,
    );
    return Reflect.apply(xhrOpen, this, arguments);
  };
  XMLHttpRequest.prototype.setRequestHeader = function (name, value) {
    xhrInfo.get(this)?.headers.push([String(name), String(value)]);
    return Reflect.apply(xhrHeader, this, arguments);
  };
  XMLHttpRequest.prototype.send = function () {
    const info = xhrInfo.get(this);
    if (info) {
      const xhr = this;
      if (info.eventsId) {
        const credentials = xhr.withCredentials ? "include" : "same-origin";
        const headers = info.headers.slice();
        order(info.eventsId).replay = () =>
          nativeFetch(info.url, { method: "GET", cache: "no-store", credentials, headers: new Headers(headers) });
      }
      xhr.addEventListener(
        "load",
        () => {
          if (xhr.status < 200 || xhr.status >= 300) return;
          try {
            const json = xhr.responseType === "json" ? xhr.response : JSON.parse(xhr.responseText);
            if (info.eventsId) gotEvents(info.eventsId, json, true);
            else {
              readDetails(order(info.detailsId), json);
              publish(order(info.detailsId));
            }
          } catch {
            // O site continua funcionando normalmente.
          }
        },
        { once: true },
      );
    }
    return Reflect.apply(xhrSend, this, arguments);
  };

  // --- Aba em segundo plano: repete a consulta do site a cada minuto ---
  setInterval(() => {
    for (const o of orders.values()) {
      if (!o.replay || FINAL.has(o.stage) || o.failures >= MAX_FAILURES) continue;
      if (Date.now() - o.lastPage < REFRESH_MS - 5000) continue;
      o.replay()
        .then((response) => (response.ok ? response.json() : Promise.reject(response.status)))
        .then((json) => gotEvents(o.id, json, false))
        .catch(() => {
          o.failures += 1;
        });
    }
  }, REFRESH_MS);

  // --- Lê a tela quando ela muda (com calma: no máximo a cada 2 s) ---
  let pending = 0;
  function soon() {
    if (pending) return;
    pending = setTimeout(() => {
      pending = 0;
      readScreen();
    }, 2000);
  }
  function start() {
    new MutationObserver(soon).observe(document.documentElement, {
      childList: true,
      subtree: true,
      characterData: true,
    });
    soon();
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
  else start();
})();
