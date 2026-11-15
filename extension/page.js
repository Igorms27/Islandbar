// Lê o andamento do pedido na própria página do iFood.
//
// O site busca as etapas do pedido em
//   /site-api/v1/customers/me/orders/<id>/events
// (uma lista de eventos como CONFIRMED, DISPATCHED...). Este script observa
// essas respostas quando o site as recebe e, se a aba ficar em segundo plano
// sem atualizar, repete a mesma consulta a cada minuto. Das outras respostas
// sobre o pedido tira a loja, o entregador e os pontos do mapa (loja, sua casa
// e, se o site informar, onde o entregador está). Da tela lê a previsão de
// entrega e o número do pedido.
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
  // Qualquer resposta do site sobre um pedido (detalhes, rastreio...).
  const ABOUT_ORDER = /\/orders?\/([a-f\d-]{30,50})(?:\/|$)/i;
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
  const VEHICLES = {
    MOTORCYCLE: "moto",
    MOTORBIKE: "moto",
    MOTO: "moto",
    MOTOCICLETA: "moto",
    BICYCLE: "bike",
    BIKE: "bike",
    BICICLETA: "bike",
    CAR: "carro",
    CARRO: "carro",
  };

  // Pedidos vistos nesta aba: id -> estado.
  const orders = new Map();
  // Diagnóstico: o formato (só os nomes dos campos) das respostas do site.
  const shapes = {};
  const nativeFetch = window.fetch;

  function order(id) {
    let o = orders.get(id);
    if (!o) {
      o = {
        id,
        stage: "unknown",
        rank: 0,
        store: "",
        number: "",
        eta: "",
        etaFromEvents: "",
        code: "",
        chats: 0,
        driver: "",
        vehicle: "",
        from: null,
        to: null,
        courier: null,
        lastPage: 0,
        failures: 0,
        replay: null,
        sent: "",
      };
      orders.set(id, o);
    }
    return o;
  }

  function parse(input) {
    try {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      return url.origin === location.origin ? url : null;
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

  // "João da Silva" -> "João S." (só para mostrar; nada sai do PC).
  function shortName(name) {
    const parts = String(name).trim().split(/\s+/).filter(Boolean);
    if (!parts.length) return "";
    const last = parts.slice(1).find((part) => !/^(d[aeo]s?|e)$/i.test(part));
    return (last ? `${parts[0]} ${last[0].toUpperCase()}.` : parts[0]).slice(0, 30);
  }

  // Um ponto do mapa em qualquer um dos formatos comuns.
  function point(v) {
    if (!v || typeof v !== "object" || Array.isArray(v)) return null;
    if (v.type === "Point" && Array.isArray(v.coordinates)) {
      return point({ longitude: v.coordinates[0], latitude: v.coordinates[1] });
    }
    for (const key of ["coordinates", "location", "geoCoordinates", "geolocation", "position"]) {
      const inner = v[key];
      if (inner && typeof inner === "object" && !Array.isArray(inner)) {
        const p = point(inner);
        if (p) return p;
      }
    }
    const rawLat = v.latitude ?? v.lat;
    const rawLng = v.longitude ?? v.lng ?? v.lon;
    if (rawLat === undefined || rawLat === null || rawLng === undefined || rawLng === null) return null;
    const lat = Number(rawLat);
    const lng = Number(rawLng);
    if (!Number.isFinite(lat) || !Number.isFinite(lng) || Math.abs(lat) > 90 || Math.abs(lng) > 180) return null;
    return lat === 0 && lng === 0 ? null : [lat, lng];
  }

  const DRIVER = /driver|courier|deliveryman|deliveryperson|worker|entregador|rider/;
  const STORE = /merchant|restaurant|store|origin|pickup|loja/;
  const HOME = /customer|destination|deliveryaddress|shippingaddress|dropoff|address|endereco/;

  // Procura na resposta os pontos do mapa e o entregador, pelo nome dos campos.
  function scan(o, json) {
    const seen = new Set();
    (function walk(v, path, depth) {
      if (!v || typeof v !== "object" || depth > 7 || seen.has(v)) return;
      seen.add(v);
      if (!Array.isArray(v)) {
        if (DRIVER.test(path)) {
          if (typeof v.name === "string" && !o.driver) o.driver = shortName(v.name);
          const vehicle = VEHICLES[String(v.modal || v.vehicleType || v.vehicle || "").toUpperCase()];
          if (vehicle) o.vehicle = vehicle;
        }
        const p = point(v);
        if (p) {
          if (DRIVER.test(path)) o.courier = p;
          else if (STORE.test(path)) o.from = p;
          else if (HOME.test(path)) o.to = p;
          return;
        }
        if (/merchant|restaurant/.test(path.split(".").pop() || "") && typeof v.name === "string" && v.name.length <= 80) {
          o.store = v.name.trim();
        }
      }
      for (const [key, child] of Object.entries(v)) walk(child, `${path}.${key.toLowerCase()}`, depth + 1);
    })(json, "", 0);
  }

  // Lista de eventos -> etapa, código de entrega, mensagens, previsão e entregador.
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
      if (typeof meta.driverName === "string") o.driver = shortName(meta.driverName);
      const vehicle = VEHICLES[String(meta.modal || "").toUpperCase()];
      if (vehicle) o.vehicle = vehicle;
      const eta = Date.parse(meta.newEta || "");
      if (Number.isFinite(eta)) o.etaFromEvents = hhmm(eta);
    }
    scan(o, list);
    o.code = validated ? "" : code || o.code;
    o.chats = Math.max(o.chats, chats);
    return true;
  }

  // --- Diagnóstico: só o formato, nunca os valores ---
  function skeleton(v, depth = 0) {
    if (v === null || v === undefined) return null;
    if (Array.isArray(v)) {
      return depth > 6 ? "[...]" : [`lista(${v.length})`, ...v.slice(0, 2).map((x) => skeleton(x, depth + 1))];
    }
    if (typeof v === "object") {
      if (depth > 6) return "{...}";
      const out = {};
      for (const [key, x] of Object.entries(v).slice(0, 60)) out[key] = skeleton(x, depth + 1);
      return out;
    }
    // Códigos do site (CONFIRMED, DELIVERY_DROP_CODE_...) ficam; textos viram só o tamanho.
    if (typeof v === "string") return STAGES[v] || /^[A-Z]+(?:_[A-Z0-9]+)+$/.test(v) ? v : `texto(${v.length})`;
    return typeof v;
  }

  function remember(url, json) {
    const pattern = url.pathname.replace(/[a-f\d-]{30,50}|\d{4,}/gi, ":id");
    if (Array.isArray(json) && json.some((item) => typeof item?.value === "string")) {
      const values = {};
      for (const item of json) {
        if (typeof item?.value !== "string") continue;
        values[item.value] = Object.keys(item.metadata || {}).sort();
      }
      shapes[pattern] = { eventos: values };
    } else {
      shapes[pattern] = skeleton(json);
    }
    sendDiagnostic();
  }

  let diagTimer = 0;
  function sendDiagnostic() {
    if (diagTimer) return;
    diagTimer = setTimeout(() => {
      diagTimer = 0;
      const hosts = new Set();
      for (const el of document.querySelectorAll("iframe[src], script[src], img[src]")) {
        try {
          const host = new URL(el.src).host;
          if (/map|tile|here\.com|waze/i.test(host)) hosts.add(host);
        } catch {
          // ignora
        }
      }
      const summary = [...orders.values()].map((o) => ({
        etapa: o.stage,
        loja: !!o.store,
        numero: !!o.number,
        previsao: !!(o.eta || o.etaFromEvents),
        codigo: !!o.code,
        mensagens: o.chats,
        entregador: !!o.driver,
        veiculo: o.vehicle,
        mapa: { loja: !!o.from, casa: !!o.to, entregador: !!o.courier },
      }));
      const diag = {
        pagina: location.pathname.replace(/[a-f\d-]{30,50}/gi, ":id"),
        canvas: document.querySelectorAll("canvas").length,
        mapas: [...hosts],
        pedidos: summary,
        respostas: shapes,
      };
      window.postMessage({ islandbar: "ifood-diag", diag }, location.origin);
    }, 1500);
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
    const number = text.match(/Pedido\s*(?:N\s*[º°o.]?|#)\s*(\d{2,})/i)?.[1];
    if (number) o.number = number;

    if (!o.store) {
      // O nome da loja fica logo antes de "Pedido nº 1234".
      const label = [...document.querySelectorAll("span,p,small,strong")].find(
        (el) => !el.children.length && /^Pedido\s*(?:N\s*[º°o.]?|#)\s*\d+$/i.test(el.textContent.trim()),
      );
      const name = label?.previousElementSibling?.textContent?.trim();
      if (name && name.length <= 80) o.store = name;
    }
    publish(o);
    sendDiagnostic();
  }

  function publish(o) {
    const summary = {
      id: o.id,
      stage: o.stage,
      store: o.store,
      number: o.number,
      eta: o.eta || o.etaFromEvents,
      code: FINAL.has(o.stage) ? "" : o.code,
      chats: o.chats,
      driver: o.driver,
      vehicle: o.vehicle,
      from: o.from,
      to: o.to,
      courier: o.courier,
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

  // Uma resposta do site chegou: eventos ou outra informação do pedido.
  function handle(url, json, fromPage = true) {
    if (!url) return;
    remember(url, json);
    const eventsId = EVENTS.exec(url.pathname)?.[1]?.toLowerCase();
    if (eventsId) return gotEvents(eventsId, json, fromPage);
    const id = ABOUT_ORDER.exec(url.pathname)?.[1]?.toLowerCase();
    if (id) {
      const o = order(id);
      scan(o, json);
      publish(o);
    }
  }

  function interesting(url) {
    return !!url && (EVENTS.test(url.pathname) || ABOUT_ORDER.test(url.pathname));
  }

  // --- Observa o que o próprio site busca ---
  window.fetch = function (input, init) {
    const response = Reflect.apply(nativeFetch, this, arguments);
    const url = parse(input);
    if (interesting(url)) {
      const eventsId = EVENTS.exec(url.pathname)?.[1]?.toLowerCase();
      if (eventsId) {
        try {
          const request = new Request(input, init);
          if (request.method === "GET") {
            order(eventsId).replay = () => nativeFetch(new Request(request, { cache: "no-store" }));
          }
        } catch {
          // Sem cópia da consulta: só não dá para repetir.
        }
      }
      Promise.resolve(response)
        .then((res) => {
          if (res?.ok) res.clone().json().then((json) => handle(url, json), () => {});
        })
        .catch(() => {});
    }
    return response;
  };

  const xhrOpen = XMLHttpRequest.prototype.open;
  const xhrHeader = XMLHttpRequest.prototype.setRequestHeader;
  const xhrSend = XMLHttpRequest.prototype.send;
  const xhrInfo = new WeakMap();
  XMLHttpRequest.prototype.open = function (method, url) {
    const parsed = String(method).toUpperCase() === "GET" ? parse(url) : null;
    xhrInfo.set(this, interesting(parsed) ? { url: parsed, headers: [] } : null);
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
      const eventsId = EVENTS.exec(info.url.pathname)?.[1]?.toLowerCase();
      if (eventsId) {
        const credentials = xhr.withCredentials ? "include" : "same-origin";
        const headers = info.headers.slice();
        order(eventsId).replay = () =>
          nativeFetch(info.url.href, { method: "GET", cache: "no-store", credentials, headers: new Headers(headers) });
      }
      xhr.addEventListener(
        "load",
        () => {
          if (xhr.status < 200 || xhr.status >= 300) return;
          try {
            handle(info.url, xhr.responseType === "json" ? xhr.response : JSON.parse(xhr.responseText));
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
