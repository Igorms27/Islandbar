//! Clima: temperatura, umidade, vento, chuva e as próximas horas.
//!
//! Onde o PC está:
//! 1. pela localização do Windows (Wi-Fi, mais precisa), se ela estiver ligada
//!    em Configurações → Privacidade → Localização;
//! 2. se não, pela cidade aproximada do endereço da internet.
//!
//! O nome da cidade vem do BigDataCloud e a previsão do Open-Meteo: os dois
//! são gratuitos e não pedem cadastro. As coordenadas vão arredondadas
//! (cerca de 1 km), o bastante para a previsão.
//!
//! Para pesar pouco: a previsão é buscada a cada 15 minutos, e a localização
//! só ao abrir a ilha e de 3 em 3 horas. Tudo numa linha de execução própria,
//! que passa quase todo o tempo dormindo.

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use eframe::egui::{vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const FORECAST: &str = "https://api.open-meteo.com/v1/forecast";
const PLACE: &str = "https://api.bigdatacloud.net/data/reverse-geocode-client";
/// De quanto em quanto tempo busca a previsão.
const FORECAST_EVERY: Duration = Duration::from_secs(15 * 60);
/// De quanto em quanto tempo confere onde o PC está.
const PLACE_EVERY: Duration = Duration::from_secs(3 * 60 * 60);
/// Depois de uma falha (sem internet, por exemplo).
const RETRY_AFTER: Duration = Duration::from_secs(2 * 60);
/// Previsão mais velha que isso não aparece (ex.: o PC ficou dormindo).
const TOO_OLD: Duration = Duration::from_secs(3 * 60 * 60);
/// Quantas horas aparecem depois de "Agora".
pub const HOURS: usize = 5;

/// Como está o céu (o desenho do ícone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sky {
    Clear,
    Partly,
    Cloudy,
    Fog,
    Rain,
    Snow,
    Storm,
}

/// De onde veio a localização.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Windows,
    Internet,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hour {
    /// 0..23, no horário do lugar.
    pub hour: u32,
    pub code: u32,
    pub day: bool,
    pub temp: f32,
    /// Chance de chuva, 0..100.
    pub rain: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    /// Nome da cidade (vazio se não deu para descobrir).
    pub city: String,
    pub source: Source,
    pub temp: f32,
    pub feels: f32,
    /// Umidade do ar, 0..100.
    pub humidity: f32,
    /// Vento em km/h.
    pub wind: f32,
    /// Código do tempo da OMM (o Open-Meteo usa esse padrão).
    pub code: u32,
    pub day: bool,
    pub max: f32,
    pub min: f32,
    /// Maior chance de chuva no dia, 0..100.
    pub rain: Option<f32>,
    pub uv: Option<f32>,
    pub hours: Vec<Hour>,
    pub fetched: SystemTime,
}

impl Weather {
    pub fn sky(&self) -> Sky {
        sky(self.code)
    }

    pub fn description(&self) -> &'static str {
        describe(self.code, self.day)
    }
}

/// O que dizer enquanto ainda não há previsão.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Searching,
    Offline,
    Ready,
}

static NOW: Mutex<Option<Arc<Weather>>> = Mutex::new(None);
static STATE: AtomicU8 = AtomicU8::new(0);

/// A previsão atual (se houver uma recente).
pub fn current() -> Option<Arc<Weather>> {
    let weather = NOW.lock().ok()?.clone()?;
    let age = SystemTime::now().duration_since(weather.fetched).unwrap_or_default();
    (age < TOO_OLD).then_some(weather)
}

pub fn state() -> State {
    match STATE.load(Ordering::Relaxed) {
        1 => State::Offline,
        2 => State::Ready,
        _ => State::Searching,
    }
}

fn publish(weather: Weather) {
    if let Ok(mut now) = NOW.lock() {
        *now = Some(Arc::new(weather));
    }
    STATE.store(2, Ordering::Relaxed);
}

/// Começa a buscar o clima. `wake` acorda a interface quando chega uma previsão.
pub fn start(wake: impl Fn() + Send + 'static) {
    // Para testes: uma previsão fixa, sem internet.
    if std::env::var("ISLANDBAR_DEBUG_WEATHER").as_deref() == Ok("fake") {
        publish(sample());
        return;
    }
    let spawned = std::thread::Builder::new().name("weather".into()).spawn(move || {
        // Deixa a ilha abrir primeiro.
        std::thread::sleep(Duration::from_secs(3));
        let agent = http();
        // A localização, quando foi encontrada e quando procurar de novo.
        let mut place: Option<(Place, SystemTime, Duration)> = None;
        loop {
            let stale = place.as_ref().is_none_or(|(_, at, every)| elapsed(*at) >= *every);
            if stale {
                match locate(&agent) {
                    Ok((found, every)) => place = Some((found, SystemTime::now(), every)),
                    Err(err) => crate::perf::line(format!("weather: no location ({err})")),
                }
            }
            let result = match &place {
                Some((place, _, _)) => forecast(&agent, place),
                None => Err("sem localização".into()),
            };
            // A conexão segura carrega bibliotecas na memória; devolve logo.
            crate::platform::trim_memory();
            let wait = match result {
                Ok(weather) => {
                    crate::perf::line(format!(
                        "weather: {} {:.0}° code={} humidity={:.0}% ({:?})",
                        if weather.city.is_empty() { "?" } else { &weather.city },
                        weather.temp,
                        weather.code,
                        weather.humidity,
                        weather.source
                    ));
                    publish(weather);
                    wake();
                    FORECAST_EVERY
                }
                Err(err) => {
                    crate::perf::line(format!("weather: failed ({err})"));
                    if current().is_none() {
                        STATE.store(1, Ordering::Relaxed);
                        wake();
                    }
                    // Sem internet a cidade pode ter mudado (ex.: notebook): procura de novo.
                    place = place.filter(|_| state() == State::Ready);
                    RETRY_AFTER
                }
            };
            // Dorme em pedaços de 1 minuto, olhando o relógio de verdade: assim,
            // quando o PC acorda da suspensão, a previsão é atualizada logo.
            let since = SystemTime::now();
            while elapsed(since) < wait {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
    });
    if let Err(err) = spawned {
        crate::perf::line(format!("weather: could not start ({err})"));
    }
}

fn elapsed(since: SystemTime) -> Duration {
    SystemTime::now().duration_since(since).unwrap_or_default()
}

fn http() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build(),
        )
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .new_agent()
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<serde_json::Value> {
    let text = agent.get(url).call()?.body_mut().with_config().limit(512 * 1024).read_to_string()?;
    Ok(serde_json::from_str(&text)?)
}

#[derive(Clone, Debug, PartialEq)]
struct Place {
    lat: f64,
    lon: f64,
    city: String,
    source: Source,
}

/// Onde o PC está: Windows primeiro, internet se não der. Devolve também
/// quando procurar de novo.
fn locate(agent: &ureq::Agent) -> Result<(Place, Duration)> {
    let (windows, every) = match ask_windows() {
        Asked::Found(lat, lon) => (Some((lat, lon)), PLACE_EVERY),
        Asked::Unavailable => (None, PLACE_EVERY),
        // O Windows está demorando: por enquanto vale a internet, e daqui a
        // pouco confere a resposta.
        Asked::Waiting => (None, ASK_AGAIN),
    };
    if let Some((lat, lon)) = windows {
        let (lat, lon) = (round(lat), round(lon));
        // Só o nome da cidade; se falhar, a previsão sai do mesmo jeito.
        let city = get_json(agent, &format!("{PLACE}?latitude={lat}&longitude={lon}&localityLanguage=pt"))
            .ok()
            .and_then(|json| read_place(&json))
            .map(|place| place.city)
            .unwrap_or_default();
        return Ok((Place { lat, lon, city, source: Source::Windows }, every));
    }
    // Sem coordenadas, o serviço usa o endereço da internet.
    let json = get_json(agent, &format!("{PLACE}?localityLanguage=pt"))?;
    let place = read_place(&json).ok_or("resposta sem localização")?;
    Ok((place, every))
}

/// Resposta da localização do Windows.
enum Asked {
    Found(f64, f64),
    /// Desligada, negada ou deu erro.
    Unavailable,
    /// Ainda sem resposta.
    Waiting,
}

/// Quanto esperar pela localização do Windows antes de usar a internet.
const ASK_WAIT: Duration = Duration::from_secs(20);
/// Sem resposta do Windows: quando conferir de novo.
const ASK_AGAIN: Duration = Duration::from_secs(5 * 60);

static ASKING: AtomicBool = AtomicBool::new(false);
static ANSWER: Mutex<Option<Option<(f64, f64)>>> = Mutex::new(None);

/// Pergunta ao Windows numa linha de execução à parte, com tempo limite: se
/// ele demorar (ex.: sem sinal de Wi-Fi), o clima não espera e usa a internet;
/// a resposta que chegar depois vale na próxima consulta.
fn ask_windows() -> Asked {
    if !ASKING.swap(true, Ordering::SeqCst) {
        if let Ok(mut answer) = ANSWER.lock() {
            *answer = None;
        }
        let spawned = std::thread::Builder::new().name("weather-location".into()).spawn(|| {
            let found = windows_position();
            if let Ok(mut answer) = ANSWER.lock() {
                *answer = Some(found);
            }
            ASKING.store(false, Ordering::SeqCst);
        });
        if spawned.is_err() {
            ASKING.store(false, Ordering::SeqCst);
            return Asked::Unavailable;
        }
    }
    let since = SystemTime::now();
    while elapsed(since) < ASK_WAIT {
        if let Some(found) = ANSWER.lock().ok().and_then(|mut answer| answer.take()) {
            return found.map_or(Asked::Unavailable, |(lat, lon)| Asked::Found(lat, lon));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    crate::perf::line("weather: windows location has not answered yet".to_string());
    Asked::Waiting
}

/// Duas casas decimais: cerca de 1 km.
fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn read_place(json: &serde_json::Value) -> Option<Place> {
    let lat = json["latitude"].as_f64()?;
    let lon = json["longitude"].as_f64()?;
    if lat.abs() > 90.0 || lon.abs() > 180.0 || (lat, lon) == (0.0, 0.0) {
        return None;
    }
    let city = ["city", "locality", "principalSubdivision"]
        .iter()
        .filter_map(|key| json[*key].as_str())
        .map(|name| name.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string())
        .find(|name| !name.is_empty())
        .unwrap_or_default();
    Some(Place { lat: round(lat), lon: round(lon), city, source: Source::Internet })
}

/// A localização do Windows, se estiver ligada e permitida para a ilha.
#[cfg(windows)]
fn windows_position() -> Option<(f64, f64)> {
    use windows::Devices::Geolocation::Geolocator;
    use windows::Foundation::TimeSpan;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    // A localização do Windows precisa disso na linha de execução.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    // Desligada, pedir a localização faz o Windows abrir uma janela que
    // escurece a tela e espera resposta (ruim no meio de um jogo). Então só
    // pede se ela já estiver ligada para os programas da área de trabalho.
    if !windows_location_on() {
        crate::perf::line("weather: windows location is off, using the internet".to_string());
        return None;
    }
    let locator = Geolocator::new().ok()?;
    // Aceita uma posição de até 30 min atrás; espera no máximo 15 s.
    let ticks = |seconds: i64| TimeSpan { Duration: seconds * 10_000_000 };
    let position = locator
        .GetGeopositionAsyncWithAgeAndTimeout(ticks(30 * 60), ticks(15))
        .and_then(|op| op.join())
        .map_err(|err| crate::perf::line(format!("weather: windows location failed ({err})")))
        .ok()?;
    let point = position.Coordinate().ok()?.Point().ok()?.Position().ok()?;
    let (lat, lon) = (point.Latitude, point.Longitude);
    (lat.abs() <= 90.0 && lon.abs() <= 180.0 && (lat, lon) != (0.0, 0.0)).then_some((lat, lon))
}

/// A localização está ligada em Configurações → Privacidade → Localização:
/// no PC, para a conta e para os programas da área de trabalho.
#[cfg(windows)]
fn windows_location_on() -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    const STORE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\location";
    let read = |root: HKEY, path: &str| -> Option<String> {
        let mut buffer = [0u16; 32];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        unsafe {
            RegGetValueW(
                root,
                &HSTRING::from(path),
                windows::core::w!("Value"),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr() as *mut std::ffi::c_void),
                Some(&mut size),
            )
        }
        .ok()
        .ok()?;
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    };
    let device = read(HKEY_LOCAL_MACHINE, STORE);
    let account = read(HKEY_CURRENT_USER, STORE);
    let desktop = read(HKEY_CURRENT_USER, &format!("{STORE}\\NonPackaged"));
    crate::perf::line(format!("weather: windows location device={device:?} account={account:?} desktop={desktop:?}"));
    device.as_deref() == Some("Allow") && account.as_deref() == Some("Allow") && desktop.as_deref() != Some("Deny")
}

#[cfg(not(windows))]
fn windows_position() -> Option<(f64, f64)> {
    None
}

fn forecast(agent: &ureq::Agent, place: &Place) -> Result<Weather> {
    let url = format!(
        "{FORECAST}?latitude={}&longitude={}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m\
         &hourly=temperature_2m,precipitation_probability,weather_code,is_day\
         &daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,uv_index_max\
         &timezone=auto&forecast_days=2",
        place.lat, place.lon
    );
    let json = get_json(agent, &url)?;
    read_forecast(&json, place).ok_or_else(|| "previsão em formato inesperado".into())
}

fn read_forecast(json: &serde_json::Value, place: &Place) -> Option<Weather> {
    let now = &json["current"];
    let number = |value: &serde_json::Value| value.as_f64().map(|v| v as f32);
    let daily = |key: &str| number(&json["daily"][key][0]);

    // As próximas horas: as que vêm depois da hora de agora.
    let hourly = &json["hourly"];
    let now_time = now["time"].as_str().unwrap_or_default();
    let times = hourly["time"].as_array()?;
    let hours = times
        .iter()
        .enumerate()
        .filter(|(_, time)| time.as_str().is_some_and(|t| t > now_time))
        .take(HOURS)
        .filter_map(|(i, time)| {
            Some(Hour {
                hour: time.as_str()?.get(11..13)?.parse().ok()?,
                code: hourly["weather_code"][i].as_u64()? as u32,
                day: hourly["is_day"][i].as_u64() != Some(0),
                temp: number(&hourly["temperature_2m"][i])?,
                rain: number(&hourly["precipitation_probability"][i]),
            })
        })
        .collect();

    Some(Weather {
        city: place.city.clone(),
        source: place.source,
        temp: number(&now["temperature_2m"])?,
        feels: number(&now["apparent_temperature"]).or_else(|| number(&now["temperature_2m"]))?,
        humidity: number(&now["relative_humidity_2m"])?,
        wind: number(&now["wind_speed_10m"]).unwrap_or(0.0),
        code: now["weather_code"].as_u64()? as u32,
        day: now["is_day"].as_u64() != Some(0),
        max: daily("temperature_2m_max")?,
        min: daily("temperature_2m_min")?,
        rain: daily("precipitation_probability_max"),
        uv: daily("uv_index_max"),
        hours,
        fetched: SystemTime::now(),
    })
}

/// Previsão fixa, para testes (ISLANDBAR_DEBUG_WEATHER=fake).
fn sample() -> Weather {
    let hour = |hour, code, day, temp, rain| Hour { hour, code, day, temp, rain: Some(rain) };
    Weather {
        city: "São Paulo".into(),
        source: Source::Windows,
        temp: 24.2,
        feels: 26.1,
        humidity: 68.0,
        wind: 12.0,
        code: 2,
        day: true,
        max: 28.0,
        min: 19.0,
        rain: Some(80.0),
        uv: Some(7.0),
        hours: vec![
            hour(16, 63, true, 22.0, 70.0),
            hour(17, 95, true, 21.0, 80.0),
            hour(18, 61, true, 21.0, 40.0),
            hour(19, 3, false, 20.0, 10.0),
            hour(20, 2, false, 20.0, 5.0),
        ],
        fetched: SystemTime::now(),
    }
}

/// Códigos da OMM -> desenho.
pub fn sky(code: u32) -> Sky {
    match code {
        0 => Sky::Clear,
        1 | 2 => Sky::Partly,
        3 => Sky::Cloudy,
        45 | 48 => Sky::Fog,
        71..=77 | 85 | 86 => Sky::Snow,
        95..=99 => Sky::Storm,
        51..=67 | 80..=82 => Sky::Rain,
        _ => Sky::Cloudy,
    }
}

/// Códigos da OMM -> texto.
pub fn describe(code: u32, day: bool) -> &'static str {
    match code {
        0 if day => "Ensolarado",
        0 => "Céu limpo",
        1 => "Poucas nuvens",
        2 => "Parcialmente nublado",
        3 => "Nublado",
        45 | 48 => "Neblina",
        51..=57 => "Garoa",
        61 => "Chuva fraca",
        63 | 66 | 67 => "Chuva",
        65 => "Chuva forte",
        71..=77 | 85 | 86 => "Neve",
        80 | 81 => "Pancadas de chuva",
        82 => "Chuva muito forte",
        95 => "Tempestade",
        96..=99 => "Tempestade com granizo",
        _ => "Nublado",
    }
}

/// Índice UV em palavras.
pub fn uv_word(uv: f32) -> &'static str {
    match uv.round() as i32 {
        ..=2 => "baixo",
        3..=5 => "médio",
        6..=7 => "alto",
        8..=10 => "muito alto",
        _ => "extremo",
    }
}

// ---------------------------------------------------------------------------
// Ícones (desenhados aqui mesmo, sem imagens)
// ---------------------------------------------------------------------------

const SUN: Color32 = Color32::from_rgb(255, 209, 102);
const MOON: Color32 = Color32::from_rgb(232, 227, 200);
const CLOUD: Color32 = Color32::from_rgb(217, 220, 227);
const RAIN_CLOUD: Color32 = Color32::from_rgb(185, 190, 201);
const STORM_CLOUD: Color32 = Color32::from_rgb(142, 148, 161);
pub const RAIN: Color32 = Color32::from_rgb(100, 181, 255);

fn fade(color: Color32, alpha: f32) -> Color32 {
    color.gamma_multiply(alpha.clamp(0.0, 1.0))
}

/// Converte as medidas de um quadrado de 24x24 para a tela.
#[derive(Clone, Copy)]
struct Grid {
    origin: Pos2,
    scale: f32,
}

impl Grid {
    fn new(center: Pos2, size: f32) -> Self {
        let scale = size / 24.0;
        Self { origin: center - vec2(12.0, 12.0) * scale, scale }
    }

    fn at(self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x, y) * self.scale
    }

    fn len(self, value: f32) -> f32 {
        value * self.scale
    }

    /// O mesmo desenho, menor e deslocado (ex.: o sol atrás da nuvem).
    fn part(self, dx: f32, dy: f32, scale: f32) -> Self {
        Self { origin: self.at(dx, dy), scale: self.scale * scale }
    }
}

/// Ícone do tempo, centrado em `center`, num quadrado de `size` pontos.
/// `background` é a cor de trás (para recortar a lua e contornar a nuvem).
pub fn draw_icon(p: &Painter, center: Pos2, size: f32, sky: Sky, day: bool, background: Color32, alpha: f32) {
    let g = Grid::new(center, size);
    let bg = fade(background, alpha);
    match sky {
        Sky::Clear if day => sun(p, g, alpha),
        Sky::Clear => moon(p, g, bg, alpha),
        Sky::Partly => {
            if day {
                sun(p, g.part(-3.2, -3.6, 0.82), alpha);
            } else {
                moon(p, g.part(-3.5, -3.8, 0.8), bg, alpha);
            }
            let cloud_part = g.part(2.2, 2.6, 0.86);
            cloud(p, cloud_part, bg, 1.4);
            cloud(p, cloud_part, fade(CLOUD, alpha), 0.0);
        }
        Sky::Cloudy => cloud(p, g, fade(CLOUD, alpha), 0.0),
        Sky::Fog => {
            let stroke = Stroke::new(g.len(2.0), fade(RAIN_CLOUD, alpha));
            for (y, from, to) in [(8.0, 4.0, 20.0), (12.5, 6.0, 18.0), (17.0, 4.0, 20.0)] {
                p.line_segment([g.at(from, y), g.at(to, y)], stroke);
            }
        }
        Sky::Rain => {
            cloud(p, g.part(0.0, -2.5, 1.0), fade(RAIN_CLOUD, alpha), 0.0);
            let stroke = Stroke::new(g.len(1.8), fade(RAIN, alpha));
            for x in [8.5, 12.5, 16.5] {
                p.line_segment([g.at(x, 17.5), g.at(x - 1.2, 20.8)], stroke);
            }
        }
        Sky::Snow => {
            cloud(p, g.part(0.0, -2.5, 1.0), fade(RAIN_CLOUD, alpha), 0.0);
            for (x, y) in [(8.0, 18.5), (12.5, 20.5), (17.0, 18.5)] {
                p.circle_filled(g.at(x, y), g.len(1.3), fade(Color32::WHITE, alpha));
            }
        }
        Sky::Storm => {
            cloud(p, g.part(0.0, -2.5, 1.0), fade(STORM_CLOUD, alpha), 0.0);
            let bolt = [g.at(13.6, 14.8), g.at(10.6, 18.9), g.at(13.4, 18.9), g.at(11.2, 22.6)];
            p.add(Shape::line(bolt.to_vec(), Stroke::new(g.len(1.7), fade(SUN, alpha))));
        }
    }
}

fn sun(p: &Painter, g: Grid, alpha: f32) {
    let color = fade(SUN, alpha);
    let stroke = Stroke::new(g.len(1.8), color);
    let center = g.at(12.0, 12.0);
    for i in 0..8 {
        let angle = i as f32 * std::f32::consts::FRAC_PI_4;
        let dir = vec2(angle.cos(), angle.sin());
        p.line_segment([center + dir * g.len(7.0), center + dir * g.len(9.5)], stroke);
    }
    p.circle_filled(center, g.len(4.6), color);
}

/// Lua minguante: um círculo com outro "mordendo" (da cor do fundo).
fn moon(p: &Painter, g: Grid, background: Color32, alpha: f32) {
    p.circle_filled(g.at(11.0, 12.5), g.len(7.5), fade(MOON, alpha));
    p.circle_filled(g.at(15.5, 9.0), g.len(6.2), background);
}

/// Nuvem: duas bolas e uma base arredondada. `grow` engorda tudo (contorno).
fn cloud(p: &Painter, g: Grid, color: Color32, grow: f32) {
    p.circle_filled(g.at(9.0, 13.0), g.len(4.2 + grow), color);
    p.circle_filled(g.at(14.0, 10.5), g.len(5.2 + grow), color);
    let base = Rect::from_min_max(g.at(4.8 - grow, 12.0 - grow), g.at(20.2 + grow, 17.6 + grow));
    p.rect_filled(base, g.len(2.8 + grow), color);
}

/// Ícones pequenos dos quadradinhos do painel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Humidity,
    Wind,
    Rain,
    Uv,
}

pub fn draw_detail(p: &Painter, center: Pos2, size: f32, detail: Detail, alpha: f32) {
    let g = Grid::new(center, size);
    let gray = fade(Color32::from_rgb(201, 204, 212), alpha);
    match detail {
        Detail::Humidity => drop(p, g, fade(RAIN, alpha)),
        Detail::Wind => {
            let stroke = Stroke::new(g.len(1.8), gray);
            for (y, from, to) in [(8.0, 3.0, 15.0), (12.5, 5.0, 20.0), (17.0, 3.0, 13.0)] {
                p.line_segment([g.at(from, y), g.at(to, y)], stroke);
            }
        }
        Detail::Rain => {
            // Guarda-chuva: meia bola e o cabo.
            let points: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let angle = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
                    g.at(12.0 + 8.5 * angle.cos(), 12.0 + 8.5 * angle.sin())
                })
                .collect();
            p.add(Shape::convex_polygon(points, fade(RAIN, alpha), Stroke::NONE));
            let stroke = Stroke::new(g.len(1.8), gray);
            p.line_segment([g.at(12.0, 12.0), g.at(12.0, 18.5)], stroke);
            p.line_segment([g.at(12.0, 18.5), g.at(9.5, 19.5)], stroke);
        }
        Detail::Uv => sun(p, g, alpha),
    }
}

/// Gota: um triângulo em cima de uma bola.
fn drop(p: &Painter, g: Grid, color: Color32) {
    p.circle_filled(g.at(12.0, 14.0), g.len(5.2), color);
    p.add(Shape::convex_polygon(vec![g.at(12.0, 3.5), g.at(16.9, 12.3), g.at(7.1, 12.3)], color, Stroke::NONE));
}

/// Alfinete do mapa (ao lado do nome da cidade).
pub fn draw_pin(p: &Painter, center: Pos2, size: f32, color: Color32, background: Color32) {
    let g = Grid::new(center, size);
    p.circle_filled(g.at(12.0, 10.0), g.len(6.5), color);
    p.add(Shape::convex_polygon(vec![g.at(6.6, 13.5), g.at(17.4, 13.5), g.at(12.0, 22.0)], color, Stroke::NONE));
    p.circle_filled(g.at(12.0, 10.0), g.len(2.6), background);
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &str = r#"{
        "latitude": -23.5, "longitude": -46.625, "timezone": "America/Sao_Paulo",
        "current": {"time": "2026-11-17T15:30", "interval": 900, "temperature_2m": 24.2,
            "relative_humidity_2m": 68, "apparent_temperature": 26.1, "is_day": 1,
            "weather_code": 2, "wind_speed_10m": 12.4},
        "hourly": {
            "time": ["2026-11-17T14:00", "2026-11-17T15:00", "2026-11-17T16:00", "2026-11-17T17:00",
                     "2026-11-17T18:00", "2026-11-17T19:00", "2026-11-17T20:00", "2026-11-17T21:00"],
            "temperature_2m": [25.0, 24.5, 22.0, 21.0, 21.0, 20.0, 20.0, 19.5],
            "precipitation_probability": [0, 10, 70, 80, null, 10, 5, 0],
            "weather_code": [1, 2, 63, 95, 61, 3, 2, 0],
            "is_day": [1, 1, 1, 1, 1, 0, 0, 0]},
        "daily": {"time": ["2026-11-17", "2026-11-18"], "temperature_2m_max": [28.1, 27.0],
            "temperature_2m_min": [19.0, 18.2], "precipitation_probability_max": [80, 30],
            "uv_index_max": [7.2, 6.0]}
    }"#;

    #[test]
    fn le_a_previsao() {
        let place = Place { lat: -23.55, lon: -46.63, city: "São Paulo".into(), source: Source::Internet };
        let json: serde_json::Value = serde_json::from_str(RESPONSE).unwrap();
        let w = read_forecast(&json, &place).unwrap();
        assert_eq!((w.temp, w.feels, w.humidity, w.code, w.day), (24.2, 26.1, 68.0, 2, true));
        assert_eq!((w.max, w.min, w.rain, w.uv), (28.1, 19.0, Some(80.0), Some(7.2)));
        assert_eq!(w.description(), "Parcialmente nublado");
        // Só as horas depois de agora (15:30), no máximo cinco.
        let hours: Vec<u32> = w.hours.iter().map(|h| h.hour).collect();
        assert_eq!(hours, [16, 17, 18, 19, 20]);
        assert_eq!(w.hours[1].code, 95);
        assert_eq!(w.hours[2].rain, None);
        assert!(!w.hours[3].day);
    }

    #[test]
    fn le_a_cidade() {
        let json = serde_json::json!({"latitude": -22.9064, "longitude": -47.0616, "city": "Campinas",
            "locality": "Centro", "principalSubdivision": "São Paulo", "lookupSource": "ipGeolocation"});
        let place = read_place(&json).unwrap();
        assert_eq!((place.city.as_str(), place.lat, place.lon), ("Campinas", -22.91, -47.06));
        let json = serde_json::json!({"latitude": -22.9, "longitude": -47.0, "city": "", "locality": "Barão Geraldo"});
        assert_eq!(read_place(&json).unwrap().city, "Barão Geraldo");
        assert!(read_place(&serde_json::json!({"latitude": 0.0, "longitude": 0.0})).is_none());
        assert!(read_forecast(&serde_json::json!({}), &place).is_none());
    }

    #[test]
    fn codigos_do_tempo() {
        assert_eq!(sky(0), Sky::Clear);
        assert_eq!(sky(63), Sky::Rain);
        assert_eq!(sky(81), Sky::Rain);
        assert_eq!(sky(96), Sky::Storm);
        assert_eq!(sky(45), Sky::Fog);
        assert_eq!(describe(0, false), "Céu limpo");
        assert_eq!(uv_word(7.2), "alto");
        assert_eq!(uv_word(11.0), "extremo");
    }
}
