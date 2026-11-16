// Entende as respostas do iFood: etapa do pedido, loja, previsão, código,
// entregador e os pontos do mapa. Usado pela página do iFood (page.js) e pelo
// acompanhamento em segundo plano (background.js), para os dois lerem igual.
//
// Os nomes dos eventos vieram do projeto iFood PiP
// (github.com/luannsr12/ifood-pip, licença Apache 2.0).
(() => {
  const root = globalThis;
  if (root.__islandbarReader) return;

  const ID = /^[a-f\d]{8}-[a-f\d]{4}-[a-f\d]{4}-[a-f\d]{4}-[a-f\d]{12}$/i;

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

  // A etapa escrita num campo do pedido (lastStatus, status...).
  function readStatus(o, data) {
    if (!data || typeof data !== "object") return;
    for (const key of ["lastStatus", "status", "orderStatus", "currentStatus", "deliveryStatus", "state"]) {
      let v = data[key];
      if (v && typeof v === "object") v = v.code ?? v.value ?? v.status ?? v.name;
      const known = typeof v === "string" ? STAGES[v.toUpperCase()] : null;
      if (known) advance(o, known[1]);
    }
  }

  // Detalhes de um pedido (do site ou da API): loja, mapa, entregador, etapa.
  function readDetails(o, json) {
    scan(o, json);
    const data = json?.data?.order || json?.order || json?.data || json;
    if (!data || typeof data !== "object") return;
    readStatus(o, data);
    const merchant = data.merchant || data.restaurant;
    if (typeof merchant?.name === "string" && merchant.name.length <= 80) o.store = merchant.name.trim();
    // Número do pedido ("4242").
    const number = String(data.orderNumber ?? data.shortId ?? "");
    if (/^\d{2,8}$/.test(number)) o.number = number;
    // Previsão: "deliversAt" até "deliversEndAt" (ou a previsão "esperada").
    const delivery = data.delivery || {};
    const eta = delivery.estimatedTimeOfArrival || {};
    const start = Date.parse(eta.deliversAt || delivery.expectedDeliveryTime || "");
    const end = Date.parse(eta.deliversEndAt || delivery.expectedDeliveryTimeEnd || "");
    if (Number.isFinite(start)) {
      o.etaFromDetails = Number.isFinite(end) && hhmm(end) !== hhmm(start) ? `${hhmm(start)}-${hhmm(end)}` : hhmm(start);
    }
    // Código de entrega (o que você fala para o entregador).
    if (Array.isArray(data.verificationCodes)) {
      const code = data.verificationCodes.find((c) => c?.name === "DELIVERY_CODE");
      if (/^\d{4,6}$/.test(String(code?.value ?? ""))) o.code = String(code.value);
    }
  }

  // Os pedidos dentro de uma resposta da lista (onde quer que estejam).
  function listItems(json) {
    const found = [];
    (function walk(v, depth) {
      if (!v || typeof v !== "object" || depth > 3) return;
      if (Array.isArray(v)) {
        for (const item of v) {
          const id = String(item?.id ?? item?.orderId ?? "").toLowerCase();
          if (ID.test(id)) found.push([id, item]);
        }
        if (found.length) return;
      }
      for (const child of Object.values(v)) {
        walk(child, depth + 1);
        if (found.length) return;
      }
    })(json, 0);
    return found;
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

  // Um pedido novo, ainda sem nada.
  function blank(id) {
    return {
      id,
      stage: "unknown",
      rank: 0,
      store: "",
      number: "",
      eta: "",
      etaFromDetails: "",
      etaFromEvents: "",
      code: "",
      chats: 0,
      driver: "",
      vehicle: "",
      from: null,
      to: null,
      courier: null,
    };
  }

  // O que vai para a ilha.
  function summary(o) {
    return {
      id: o.id,
      stage: o.stage,
      store: o.store,
      number: o.number,
      eta: o.eta || o.etaFromDetails || o.etaFromEvents,
      code: FINAL.has(o.stage) ? "" : o.code,
      chats: o.chats,
      driver: o.driver,
      vehicle: o.vehicle,
      from: o.from,
      to: o.to,
      courier: o.courier,
    };
  }

  root.__islandbarReader = Object.freeze({
    STAGES,
    FINAL,
    ID,
    blank,
    advance,
    hhmm,
    scan,
    readEvents,
    readStatus,
    readDetails,
    listItems,
    summary,
    skeleton,
  });
})();
