//! Mapa do pedido do iFood (na janela que abre ao clicar na pílula).
//!
//! O mapa é feito de "azulejos" de 256×256 px do CARTO (estilo escuro, dados
//! do OpenStreetMap). Eles só são baixados com a janela do pedido aberta, e
//! ficam guardados enquanto o pedido existir. Cada azulejo é baixado numa
//! thread curta; quando chega, a interface é acordada para desenhá-lo.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

/// Tamanho de um azulejo, em pixels.
pub const TILE: f64 = 256.0;
/// Zoom máximo (ruas bem de perto) e mínimo (a cidade inteira).
const MAX_ZOOM: u8 = 17;
const MIN_ZOOM: u8 = 3;
/// Créditos que o CARTO e o OpenStreetMap pedem para aparecer no mapa.
pub const CREDITS: &str = "© OpenStreetMap © CARTO";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

enum Slot {
    Loading,
    Ready(TextureHandle),
    Failed,
}

pub struct Map {
    tiles: HashMap<Tile, Slot>,
    tx: Sender<(Tile, Option<ColorImage>)>,
    rx: Receiver<(Tile, Option<ColorImage>)>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Map {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = channel();
        Self { tiles: HashMap::new(), tx, rx, wake: Arc::new(wake) }
    }

    /// Guarda os azulejos que acabaram de chegar.
    pub fn receive(&mut self, ctx: &egui::Context) {
        while let Ok((tile, image)) = self.rx.try_recv() {
            let slot = match image {
                Some(image) => Slot::Ready(ctx.load_texture(
                    format!("map-{}-{}-{}", tile.z, tile.x, tile.y),
                    image,
                    TextureOptions::LINEAR,
                )),
                None => Slot::Failed,
            };
            self.tiles.insert(tile, slot);
        }
    }

    /// O azulejo, se já chegou. Se ainda não foi pedido, começa a baixar.
    pub fn tile(&mut self, tile: Tile) -> Option<&TextureHandle> {
        if !self.tiles.contains_key(&tile) {
            self.tiles.insert(tile, Slot::Loading);
            let (tx, wake) = (self.tx.clone(), self.wake.clone());
            let spawned = std::thread::Builder::new().name("map tile".into()).spawn(move || {
                let image = download(tile);
                if image.is_none() {
                    crate::perf::line(format!("map: tile {}/{}/{} failed", tile.z, tile.x, tile.y));
                }
                let _ = tx.send((tile, image));
                wake();
            });
            if spawned.is_err() {
                self.tiles.insert(tile, Slot::Failed);
            }
        }
        match self.tiles.get(&tile) {
            Some(Slot::Ready(texture)) => Some(texture),
            _ => None,
        }
    }

    /// Algum azulejo ainda baixando?
    pub fn loading(&self) -> bool {
        self.tiles.values().any(|slot| matches!(slot, Slot::Loading))
    }

    /// Esquece os azulejos (o pedido acabou): devolve a memória.
    pub fn clear(&mut self) {
        if !self.tiles.is_empty() && !self.loading() {
            self.tiles.clear();
        }
    }
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        ureq::Agent::config_builder()
            .tls_config(
                TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build(),
            )
            .user_agent("Islandbar")
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .new_agent()
    })
}

fn download(tile: Tile) -> Option<ColorImage> {
    // Para testes: ISLANDBAR_DEBUG_TILES=http://127.0.0.1:8000/{z}/{x}/{y}.png
    let template = std::env::var("ISLANDBAR_DEBUG_TILES").unwrap_or_else(|_| {
        let server = ["a", "b", "c", "d"][((tile.x + tile.y) % 4) as usize];
        format!("https://{server}.basemaps.cartocdn.com/dark_all/{{z}}/{{x}}/{{y}}.png")
    });
    let url = template
        .replace("{z}", &tile.z.to_string())
        .replace("{x}", &tile.x.to_string())
        .replace("{y}", &tile.y.to_string());
    let bytes = agent().get(&url).call().ok()?.body_mut().with_config().limit(2 * 1024 * 1024).read_to_vec().ok()?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// Posição no "mundo" do zoom `z`, em pixels (projeção de Mercator, a dos mapas da web).
pub fn project((lat, lng): (f64, f64), z: u8) -> (f64, f64) {
    let world = TILE * f64::from(1u32 << z);
    let lat = lat.clamp(-85.0, 85.0).to_radians();
    let x = (lng + 180.0) / 360.0 * world;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * world;
    (x, y)
}

/// Escolhe o zoom e o centro para todos os pontos caberem num retângulo de
/// `width`×`height` px, com uma folga nas bordas para os marcadores.
pub fn frame(points: &[(f64, f64)], width: f64, height: f64) -> Option<(u8, (f64, f64))> {
    let first = *points.first()?;
    let margin = 36.0;
    for z in (MIN_ZOOM..=MAX_ZOOM).rev() {
        let projected: Vec<_> = points.iter().map(|p| project(*p, z)).collect();
        let (mut min, mut max) = (projected[0], projected[0]);
        for (x, y) in &projected {
            min = (min.0.min(*x), min.1.min(*y));
            max = (max.0.max(*x), max.1.max(*y));
        }
        // Um ponto só: perto o bastante para ver as ruas.
        if points.len() == 1 && z > 15 {
            continue;
        }
        if max.0 - min.0 + 2.0 * margin <= width && max.1 - min.1 + 2.0 * margin <= height {
            return Some((z, ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0)));
        }
    }
    Some((MIN_ZOOM, project(first, MIN_ZOOM)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projecao_e_enquadramento() {
        // Praça da Sé (São Paulo), zoom 15: azulejo 12139 / 18590.
        let (x, y) = project((-23.5503, -46.6339), 15);
        assert_eq!(((x / TILE) as u32, (y / TILE) as u32), (12139, 18590));

        // Dois pontos a ~1,5 km cabem perto (zoom alto); a ~400 km, longe.
        let near = [(-23.5503, -46.6339), (-23.5610, -46.6560)];
        let (z, _) = frame(&near, 380.0, 170.0).unwrap();
        assert!((13..=15).contains(&z), "zoom {z}");
        let far = [(-23.5503, -46.6339), (-22.9068, -43.1729)];
        assert!(frame(&far, 380.0, 170.0).unwrap().0 <= 8);
        assert_eq!(frame(&near[..1], 380.0, 170.0).unwrap().0, 15);
    }
}
