//! Pedidos do iFood: a segunda pílula da ilha.
//!
//! Uma extensão do navegador (pasta `extension/`) lê o andamento do pedido na
//! aba do iFood e manda para a ilha por `http://127.0.0.1:47615/ifood`. A
//! ilha só escuta no próprio PC (nada vem da internet) e só aceita o que vem
//! de uma extensão do navegador.
//!
//! O que chega é um texto curto, uma informação por linha:
//! `order=<id>`, `stage=shipping`, `store=Lanchonete`, `eta=20:10-20:25`,
//! `code=1234`, `chat=2`. Quando a aba fecha, chega `gone=<id>`.
//!
//! A escuta é uma thread parada esperando conexão: não gasta nada enquanto
//! não há pedido. A extensão repete o estado a cada 30 s; sem notícias por
//! alguns minutos (aba fechada, PC dormiu), a pílula some sozinha.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PORT: u16 = 47615;
/// Sem notícias da aba por esse tempo, o pedido sai da ilha.
const SILENT_LIMIT: Duration = Duration::from_secs(180);
/// Depois de entregue (ou cancelado), ainda aparece por esse tempo.
const FINAL_SHOW: Duration = Duration::from_secs(300);
/// Quanto tempo a pílula fica chamando atenção depois de uma novidade.
pub const HIGHLIGHT: Duration = Duration::from_secs(8);
/// Tamanho máximo do que a extensão manda.
const MAX_BODY: usize = 4096;

/// Etapa do pedido, na ordem em que acontecem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Unknown,
    Placed,
    Confirmed,
    Preparing,
    Driver,
    Ready,
    Pickup,
    Collected,
    Shipping,
    Arrived,
    Delivered,
    Cancelled,
}

impl Stage {
    fn parse(key: &str) -> Stage {
        match key {
            "placed" => Stage::Placed,
            "confirmed" => Stage::Confirmed,
            "preparing" => Stage::Preparing,
            "driver" => Stage::Driver,
            "ready" => Stage::Ready,
            "pickup" => Stage::Pickup,
            "collected" => Stage::Collected,
            "shipping" => Stage::Shipping,
            "arrived" => Stage::Arrived,
            "delivered" => Stage::Delivered,
            "cancelled" => Stage::Cancelled,
            _ => Stage::Unknown,
        }
    }

    /// O texto da pílula.
    pub fn text(self) -> &'static str {
        match self {
            Stage::Unknown => "Acompanhando pedido",
            Stage::Placed => "Pedido enviado",
            Stage::Confirmed => "Pedido confirmado",
            Stage::Preparing => "Em preparo",
            Stage::Driver => "Entregador indo à loja",
            Stage::Ready => "Pronto na loja",
            Stage::Pickup => "Entregador na loja",
            Stage::Collected => "Entregador pegou",
            Stage::Shipping => "A caminho",
            Stage::Arrived => "Entregador chegou!",
            Stage::Delivered => "Entregue",
            Stage::Cancelled => "Cancelado",
        }
    }

    pub fn is_final(self) -> bool {
        matches!(self, Stage::Delivered | Stage::Cancelled)
    }

    /// Daqui para frente o código de entrega interessa (o entregador pede).
    pub fn on_the_way(self) -> bool {
        matches!(self, Stage::Shipping | Stage::Arrived)
    }
}

#[derive(Clone, Debug)]
pub struct Order {
    pub id: String,
    pub stage: Stage,
    pub store: String,
    /// Previsão de entrega como o site mostra: "20:10-20:25" ou "20:25".
    pub eta: String,
    /// Código para dar ao entregador.
    pub code: String,
    /// Quantas mensagens do chat já apareceram.
    pub chats: u32,
    /// Última notícia da aba.
    seen: Instant,
    /// Última novidade (etapa, código, mensagem): a pílula chama atenção.
    pub changed: Option<Instant>,
    /// A última novidade foi uma mensagem nova.
    pub new_chat: bool,
    final_since: Option<Instant>,
}

impl Order {
    fn alive(&self, now: Instant) -> bool {
        now.duration_since(self.seen) < SILENT_LIMIT
            && self.final_since.is_none_or(|since| now.duration_since(since) < FINAL_SHOW)
    }

    /// Ainda chamando atenção? (0..1, 1 = acabou de mudar.)
    pub fn highlight(&self, now: Instant) -> f32 {
        self.changed.map_or(0.0, |at| 1.0 - (now.duration_since(at).as_secs_f32() / HIGHLIGHT.as_secs_f32()).min(1.0))
    }
}

/// Os pedidos que as abas estão mostrando.
#[derive(Default)]
struct Orders(Vec<Order>);

impl Orders {
    /// Aplica o que a extensão mandou. Devolve se algo mudou na tela.
    fn apply(&mut self, text: &str, now: Instant) -> bool {
        let mut fields = std::collections::HashMap::new();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                fields.insert(key.trim(), clean(value));
            }
        }
        if let Some(id) = fields.get("gone") {
            let before = self.0.len();
            self.0.retain(|o| &o.id != id);
            return self.0.len() != before;
        }
        let Some(id) = fields.get("order").filter(|id| !id.is_empty()) else { return false };
        let stage = Stage::parse(fields.get("stage").map_or("", String::as_str));
        let store = fields.get("store").cloned().unwrap_or_default();
        let eta: String = fields
            .get("eta")
            .map(|eta| eta.chars().filter(|c| c.is_ascii_digit() || *c == ':' || *c == '-').take(11).collect())
            .unwrap_or_default();
        let code: String = fields
            .get("code")
            .map(|code| code.chars().filter(char::is_ascii_digit).take(6).collect())
            .unwrap_or_default();
        let chats = fields.get("chat").and_then(|n| n.parse().ok()).unwrap_or(0u32).min(999);

        let Some(order) = self.0.iter_mut().find(|o| &o.id == id) else {
            // Pedido antigo (já entregue quando a aba abriu): não interessa.
            if stage.is_final() {
                return false;
            }
            crate::perf::line(format!("ifood: order {} {stage:?} eta={eta} chats={chats}", short(id)));
            self.0.push(Order {
                id: id.clone(),
                stage,
                store,
                eta,
                code,
                chats,
                seen: now,
                changed: Some(now),
                new_chat: false,
                final_since: None,
            });
            return true;
        };
        let came_back = !order.alive(now);
        order.seen = now;
        // A etapa só avança (uma aba atrasada não faz o pedido "voltar").
        let advanced = stage > order.stage;
        let new_code = !code.is_empty() && code != order.code;
        let new_chat = chats > order.chats;
        let mut changed = advanced || new_code || new_chat || came_back;
        if advanced {
            order.stage = stage;
            if stage.is_final() {
                order.final_since = Some(now);
            }
        }
        if advanced || new_code || new_chat {
            order.changed = Some(now);
            order.new_chat = new_chat && !advanced;
            crate::perf::line(format!(
                "ifood: order {} {:?} code={} chats={chats}",
                short(id),
                order.stage,
                if code.is_empty() { "-" } else { "yes" }
            ));
        }
        if new_code {
            order.code = code;
        }
        order.chats = order.chats.max(chats);
        for (field, value) in [(&mut order.store, store), (&mut order.eta, eta)] {
            if !value.is_empty() && *field != value {
                *field = value;
                changed = true;
            }
        }
        changed
    }

    /// O pedido que aparece: o mais recente que ainda não terminou.
    fn current(&self, now: Instant) -> Option<Order> {
        self.0.iter().filter(|o| o.alive(now)).max_by_key(|o| (!o.stage.is_final(), o.changed)).cloned()
    }

    /// Quando a ilha precisa acordar para algo mudar sozinho (o pedido sumir,
    /// o destaque acabar).
    fn next_change(&self, now: Instant) -> Option<Duration> {
        self.0
            .iter()
            .filter(|o| o.alive(now))
            .flat_map(|o| {
                [
                    Some(o.seen + SILENT_LIMIT),
                    o.final_since.map(|since| since + FINAL_SHOW),
                    o.changed.map(|at| at + HIGHLIGHT).filter(|end| *end > now),
                ]
            })
            .flatten()
            .map(|at| at.saturating_duration_since(now))
            .min()
    }
}

static ORDERS: Mutex<Orders> = Mutex::new(Orders(Vec::new()));

pub fn current(now: Instant) -> Option<Order> {
    ORDERS.lock().ok()?.current(now)
}

pub fn next_change(now: Instant) -> Option<Duration> {
    ORDERS.lock().ok()?.next_change(now)
}

/// Uma linha só, sem caracteres de controle, tamanho limitado.
fn clean(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_string()
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// Começa a escutar a extensão. `wake` acorda a interface quando algo muda.
pub fn start(wake: impl Fn() + Send + 'static) {
    let spawned = std::thread::Builder::new().name("ifood".into()).spawn(move || {
        extension::refresh();
        // A porta pode estar com a ilha anterior por um instante (ex.: logo
        // depois de uma atualização): tenta de novo até ela sair.
        let mut warned = false;
        let listener = loop {
            match TcpListener::bind(("127.0.0.1", PORT)) {
                Ok(listener) => break listener,
                Err(err) => {
                    if !warned {
                        crate::perf::line(format!("ifood: port {PORT} busy ({err}), trying again"));
                        warned = true;
                    }
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        };
        crate::perf::line(format!("ifood: listening on 127.0.0.1:{PORT}"));
        for stream in listener.incoming().flatten() {
            if let Some(body) = receive(stream) {
                let changed = ORDERS.lock().is_ok_and(|mut orders| orders.apply(&body, Instant::now()));
                if changed {
                    wake();
                }
            }
        }
    });
    if let Err(err) = spawned {
        crate::perf::line(format!("ifood: could not start ({err})"));
    }
}

/// Lê um pedido HTTP bem simples. Devolve o texto se veio da extensão.
fn receive(stream: TcpStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut writer = stream.try_clone().ok()?;
    let mut reader = BufReader::new(stream.take(MAX_BODY as u64 + 8192));

    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let (method, path) = (parts.next()?.to_string(), parts.next()?.to_string());
    let (mut origin, mut length) = (String::new(), 0usize);
    for _ in 0..64 {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "origin" => origin = value.trim().to_string(),
                "content-length" => length = value.trim().parse().unwrap_or(usize::MAX),
                _ => {}
            }
        }
    }

    // Só extensões do navegador. Um site qualquer também consegue mandar
    // para 127.0.0.1, mas o navegador põe o endereço do site em "Origin".
    let from_extension =
        ["chrome-extension://", "extension://", "moz-extension://"].iter().any(|scheme| origin.starts_with(scheme));
    let cors = if from_extension {
        format!(
            "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Methods: POST\r\n\
             Access-Control-Allow-Headers: Content-Type\r\nAccess-Control-Allow-Private-Network: true\r\n"
        )
    } else {
        String::new()
    };
    let accepted = from_extension && path == "/ifood";
    let status = match method.as_str() {
        "OPTIONS" if accepted => "204 No Content",
        "POST" if accepted && length <= MAX_BODY => "204 No Content",
        _ => "403 Forbidden",
    };
    let mut body = None;
    if status.starts_with("204") && method == "POST" {
        let mut buffer = vec![0u8; length];
        reader.read_exact(&mut buffer).ok()?;
        body = Some(String::from_utf8_lossy(&buffer).into_owned());
    }
    let _ = writer
        .write_all(format!("HTTP/1.1 {status}\r\n{cors}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
    body
}

/// Os arquivos da extensão vão junto dentro do .exe. A ilha os coloca numa
/// pasta do PC para você carregar no Chrome/Edge ("Carregar sem compactação").
pub mod extension {
    use std::path::PathBuf;

    const FILES: [(&str, &str); 4] = [
        ("manifest.json", include_str!("../extension/manifest.json")),
        ("page.js", include_str!("../extension/page.js")),
        ("bridge.js", include_str!("../extension/bridge.js")),
        ("background.js", include_str!("../extension/background.js")),
    ];

    /// `%LOCALAPPDATA%\Islandbar\extensao-ifood`.
    pub fn folder() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("ISLANDBAR_DEBUG_EXTENSION_DIR") {
            return Some(dir.into());
        }
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(PathBuf::from(base).join("Islandbar").join("extensao-ifood"))
    }

    /// Escreve (ou atualiza) os arquivos na pasta. Só regrava o que mudou.
    pub fn write() -> Option<PathBuf> {
        let dir = folder()?;
        std::fs::create_dir_all(&dir).ok()?;
        for (name, content) in FILES {
            let path = dir.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(content) {
                std::fs::write(&path, content).ok()?;
            }
        }
        Some(dir)
    }

    /// Ao abrir a ilha: se a extensão já foi instalada, deixa os arquivos na
    /// versão desta ilha (o navegador lê de novo quando reinicia).
    pub fn refresh() {
        if folder().is_some_and(|dir| dir.exists()) {
            write();
        }
    }

    /// Botão do cartão inicial: prepara a pasta e abre no Explorador.
    pub fn open() {
        let Some(dir) = write() else {
            crate::perf::line("ifood: could not write the extension folder".to_string());
            return;
        };
        crate::perf::line(format!("ifood: extension folder {}", dir.display()));
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(stage: &str, chats: u32) -> String {
        format!("order=abc123\nstage={stage}\nstore=Lanche Bom\neta=20:10-20:25\ncode=\nchat={chats}")
    }

    #[test]
    fn segue_o_pedido() {
        let mut orders = Orders::default();
        let t0 = Instant::now();
        assert!(orders.apply(&text("preparing", 0), t0));
        let order = orders.current(t0).unwrap();
        assert_eq!(
            (order.stage, order.store.as_str(), order.eta.as_str()),
            (Stage::Preparing, "Lanche Bom", "20:10-20:25")
        );

        // A mesma coisa de novo (a repetição de 30 s): nada muda.
        assert!(!orders.apply(&text("preparing", 0), t0 + Duration::from_secs(30)));
        // Uma aba atrasada não faz voltar a etapa.
        assert!(orders.apply(&text("shipping", 0), t0 + Duration::from_secs(40)));
        assert!(!orders.apply(&text("preparing", 0), t0 + Duration::from_secs(50)));
        assert_eq!(orders.current(t0).unwrap().stage, Stage::Shipping);

        // Mensagem nova chama atenção.
        let t1 = t0 + Duration::from_secs(60);
        assert!(orders.apply(&text("shipping", 1), t1));
        let order = orders.current(t1).unwrap();
        assert!(order.new_chat && order.highlight(t1) > 0.99);
        assert_eq!(order.highlight(t1 + HIGHLIGHT), 0.0);

        // Sem notícias por muito tempo, some.
        assert!(orders.current(t1 + SILENT_LIMIT).is_none());
    }

    #[test]
    fn entregue_some_depois_e_aba_fechada_some_na_hora() {
        let mut orders = Orders::default();
        let t0 = Instant::now();
        orders.apply(&text("shipping", 0), t0);
        orders.apply(&text("delivered", 0), t0);
        assert_eq!(orders.current(t0).unwrap().stage, Stage::Delivered);
        let later = t0 + FINAL_SHOW;
        orders.apply(&text("delivered", 0), later);
        assert!(orders.current(later).is_none());

        orders.apply(&text("preparing", 0).replace("abc123", "other"), t0);
        assert_eq!(orders.current(t0).unwrap().id, "other");
        assert!(orders.apply("gone=other", t0));
        assert_eq!(orders.current(t0).unwrap().id, "abc123");
        // Pedido que já chega entregue (página de um pedido antigo): ignorado.
        assert!(!orders.apply(&text("delivered", 0).replace("abc123", "old"), t0));
    }

    #[test]
    fn limpa_o_que_chega() {
        let mut orders = Orders::default();
        let t0 = Instant::now();
        orders.apply("order=x\nstage=hacker\neta=20:10<b>\ncode=12ab34\nchat=-5", t0);
        let order = orders.current(t0).unwrap();
        assert_eq!(
            (order.stage, order.eta.as_str(), order.code.as_str(), order.chats),
            (Stage::Unknown, "20:10", "1234", 0)
        );
    }
}
