//! Pets: o bichinho que fica no lugar das barrinhas do som.
//!
//! Tudo é desenhado com formas simples (círculos, retângulos), sem imagens.
//! Com música, a boca abre no ritmo do som (as mesmas `bars` que fariam as
//! barrinhas dançarem). Parado, de tempos em tempos o pet faz um gesto: pisca,
//! olha para os lados, dá um pulinho.
//!
//! Os gestos seguem um relógio fixo (`gesture`), então a ilha sabe quando
//! acordar para o próximo (`next_gesture_in`) e dorme entre um e outro.
//! Cada gesto dura menos de um segundo e acontece a cada poucos segundos,
//! para o pet parecer vivo sem gastar quase nada.

use eframe::egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

use crate::settings::Pet;

/// A cada quantos segundos o pet pisca, e quanto dura a piscada.
const BLINK_EVERY: f64 = 5.0;
const BLINK_LEN: f64 = 0.18;
/// A cada quantos segundos vem um gesto maior (olhar para os lados, pulinho).
const GESTURE_EVERY: f64 = 16.0;
const GESTURE_LEN: f64 = 1.1;
/// O gesto maior começa no meio do intervalo entre duas piscadas.
const GESTURE_OFFSET: f64 = 7.5;

/// Pose do pet num instante.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    /// 0 = olhos abertos, 1 = fechados.
    pub blink: f32,
    /// Para onde os olhos olham (-1 esquerda .. 1 direita).
    pub look: f32,
    /// Quanto o pet subiu (0..1), no pulinho.
    pub hop: f32,
    /// Luz da antena do robô (0..1).
    pub glow: f32,
}

/// Pose pelo relógio (`time`, em segundos).
pub fn gesture(time: f64) -> Pose {
    let mut pose = Pose::default();
    let blink = time.rem_euclid(BLINK_EVERY);
    if blink < BLINK_LEN {
        pose.blink = (blink / BLINK_LEN * std::f64::consts::PI).sin() as f32;
    }
    let shifted = time - GESTURE_OFFSET;
    let phase = shifted.rem_euclid(GESTURE_EVERY);
    if shifted >= 0.0 && phase < GESTURE_LEN {
        let p = phase / GESTURE_LEN;
        let wave = (p * std::f64::consts::PI).sin() as f32;
        // Os gestos maiores se alternam: olhar para os lados / pulinho.
        if (shifted / GESTURE_EVERY).floor() as i64 % 2 == 0 {
            pose.look = (p * std::f64::consts::TAU).sin() as f32;
            pose.glow = wave;
        } else {
            pose.hop = (p * std::f64::consts::TAU).sin().abs() as f32;
            pose.glow = wave;
        }
    }
    pose
}

/// Segundos até o próximo gesto começar (0 se um está acontecendo agora).
pub fn next_gesture_in(time: f64) -> f64 {
    let blink = time.rem_euclid(BLINK_EVERY);
    let to_blink = if blink < BLINK_LEN { 0.0 } else { BLINK_EVERY - blink };
    let shifted = time - GESTURE_OFFSET;
    let to_gesture = if shifted < 0.0 {
        -shifted
    } else {
        let phase = shifted.rem_euclid(GESTURE_EVERY);
        if phase < GESTURE_LEN {
            0.0
        } else {
            GESTURE_EVERY - phase
        }
    };
    to_blink.min(to_gesture)
}

/// Barrinhas do som (o "pet" original): cinco tracinhos que dançam.
/// `scale` = 1 é o tamanho de sempre (tracinhos de 3 px, a cada 6 px).
pub fn draw_bars(p: &Painter, left_center: Pos2, bars: &[f32], max_height: f32, scale: f32, color: Color32) {
    for (i, level) in bars.iter().enumerate() {
        let height = 3.0 * scale + level * (max_height - 3.0 * scale);
        let x = left_center.x + (1.5 + i as f32 * 6.0) * scale;
        let rect = Rect::from_center_size(pos2(x, left_center.y), vec2(3.0 * scale, height));
        p.rect_filled(rect, 1.5 * scale, color);
    }
}

/// Largura das barrinhas, para quem precisa centralizá-las.
fn bars_width(count: usize, scale: f32) -> f32 {
    (count as f32 * 6.0 - 3.0) * scale
}

/// Desenha o pet escolhido com o centro em `center`. `scale` = 1 é o tamanho
/// da pílula (cerca de 22 px). `bars` é o som (0..1) que faz a boca abrir.
/// `color` (o corpo) e `face` (olhos e boca) chegam opacos; `alpha` esmaece tudo.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    p: &Painter,
    pet: Pet,
    center: Pos2,
    scale: f32,
    bars: &[f32],
    pose: Pose,
    color: Color32,
    face: Color32,
    alpha: f32,
) {
    let paint = Paint { color, face, alpha: alpha.clamp(0.0, 1.0) };
    // Quanto som está saindo: a boca abre com ele.
    let sound = bars.iter().copied().fold(0.0, f32::max);
    match pet {
        Pet::Bars => {
            // Maiores, só ficam mais altas (como sempre foram no painel); menores, encolhem inteiras.
            let max_height = 16.0 * scale;
            let thin = scale.min(1.0);
            let left = center.x - bars_width(bars.len(), thin) / 2.0;
            draw_bars(p, pos2(left, center.y), bars, max_height, thin, paint.body());
        }
        Pet::Ghost => draw_ghost(p, center, scale, sound, pose, paint),
        Pet::Robot => draw_robot(p, center, scale, bars, pose, paint),
    }
}

/// Cores do pet. Os tons mais claros são calculados antes de esmaecer (as
/// cores do egui já vêm multiplicadas pela transparência).
#[derive(Clone, Copy)]
struct Paint {
    color: Color32,
    face: Color32,
    alpha: f32,
}

impl Paint {
    fn body(self) -> Color32 {
        self.color.gamma_multiply(self.alpha)
    }
    fn face(self) -> Color32 {
        self.face.gamma_multiply(self.alpha)
    }
    /// O corpo mais claro (`amount` 0..1), com `extra` de transparência.
    fn light(self, amount: f32, extra: f32) -> Color32 {
        lighten(self.color, amount).gamma_multiply(self.alpha * extra)
    }
}

fn draw_ghost(p: &Painter, center: Pos2, s: f32, sound: f32, pose: Pose, paint: Paint) {
    // Fantasma é clarinho: a cor de destaque bem puxada para o branco.
    let (color, face) = (paint.light(1.0, 1.0), paint.face());
    // Com música ele sobe e desce no ritmo; parado, só no pulinho.
    let lift = (pose.hop * 3.0 + sound * 1.5) * s;
    let c = center - vec2(0.0, lift);
    let r = 8.5 * s;
    // Cabeça redonda, corpo reto e três "ondinhas" embaixo.
    let head = pos2(c.x, c.y - 2.0 * s);
    p.circle_filled(head, r, color);
    p.rect_filled(Rect::from_min_max(pos2(c.x - r, head.y), pos2(c.x + r, c.y + 6.0 * s)), 0.0, color);
    let wave = r / 3.0;
    for i in -1..=1 {
        p.circle_filled(pos2(c.x + i as f32 * 2.0 * wave, c.y + 6.0 * s), wave, color);
    }

    // Olhos: fecham na piscada e acompanham o "olhar".
    let look = pose.look * 1.6 * s;
    let eye_h = (2.2 * (1.0 - pose.blink)).max(0.35) * s;
    for side in [-1.0, 1.0] {
        let eye = pos2(c.x + side * 3.4 * s + look, head.y);
        p.add(Shape::ellipse_filled(eye, vec2(1.7 * s, eye_h), face));
    }

    // Boca: fechadinha parado, abre cantando.
    let mouth = pos2(c.x + look * 0.5, head.y + 4.6 * s);
    let open = (sound * 3.4).min(3.0) * s;
    if open > 0.4 * s {
        p.add(Shape::ellipse_filled(mouth + vec2(0.0, open * 0.3), vec2(1.8 * s + open * 0.25, open), face));
    } else {
        p.add(Shape::ellipse_filled(mouth, vec2(1.5 * s, 0.6 * s), face));
    }
}

fn draw_robot(p: &Painter, center: Pos2, s: f32, bars: &[f32], pose: Pose, paint: Paint) {
    let (color, face) = (paint.body(), paint.face());
    let sound = bars.iter().copied().fold(0.0, f32::max);
    let lift = pose.hop * 2.5 * s;
    let c = center + vec2(0.0, 2.0 * s - lift);
    // Cabeça quadradinha com orelhas.
    let head = Rect::from_center_size(c, vec2(18.0 * s, 14.0 * s));
    for side in [-1.0, 1.0] {
        let ear = Rect::from_center_size(pos2(c.x + side * 10.0 * s, c.y), vec2(2.5 * s, 6.0 * s));
        p.rect_filled(ear, 1.0 * s, color);
    }
    p.rect_filled(head, 4.0 * s, color);

    // Antena: a luz acende nos gestos e pisca com o som.
    let top = pos2(c.x, head.top());
    let tip = top - vec2(0.0, 4.0 * s);
    p.line_segment([top, tip], Stroke::new(1.3 * s, color));
    let glow = pose.glow.max(sound);
    if glow > 0.05 {
        p.circle_filled(tip, 3.6 * s, paint.light(0.6, 0.35 * glow));
    }
    p.circle_filled(tip, 1.8 * s, paint.light(glow, 1.0));

    // Olhos quadrados que piscam e olham para os lados.
    let look = pose.look * 1.5 * s;
    let eye_h = (3.0 * (1.0 - pose.blink)).max(0.5) * s;
    for side in [-1.0, 1.0] {
        let eye = Rect::from_center_size(pos2(c.x + side * 4.0 * s + look, c.y - 2.0 * s), vec2(3.0 * s, eye_h));
        p.rect_filled(eye, 0.8 * s, face);
    }

    // Boca: uma telinha. Com música, vira um mini equalizador.
    let mouth = Rect::from_center_size(pos2(c.x, c.y + 3.6 * s), vec2(10.0 * s, (3.0 + sound * 1.5) * s));
    p.rect_filled(mouth, 1.0 * s, face);
    if sound > 0.02 {
        let n = bars.len().min(4);
        let w = mouth.width() / n as f32;
        for (i, level) in bars.iter().take(n).enumerate() {
            let h = (mouth.height() - 1.2 * s) * level.clamp(0.15, 1.0);
            let x = mouth.left() + w * (i as f32 + 0.5);
            let bar = Rect::from_center_size(pos2(x, mouth.bottom() - 0.6 * s - h / 2.0), vec2(w * 0.55, h));
            p.rect_filled(bar, 0.0, paint.light(0.5, 1.0));
        }
    }
}

/// Mistura a cor com branco (`amount` 0..1).
fn lighten(color: Color32, amount: f32) -> Color32 {
    let mix = |c: u8| (c as f32 + (255.0 - c as f32) * amount.clamp(0.0, 1.0) * 0.7) as u8;
    Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gestos_seguem_o_relogio() {
        // Logo no começo: piscando.
        assert!(gesture(0.09).blink > 0.9);
        assert_eq!(next_gesture_in(0.05), 0.0);
        // Entre piscadas, nada; a próxima é em 5 s.
        assert_eq!(gesture(2.0), Pose::default());
        assert!((next_gesture_in(2.0) - 3.0).abs() < 1e-9);
        // O gesto maior começa em 7,5 s.
        assert!((next_gesture_in(6.0) - 1.5).abs() < 1e-9);
        assert!(gesture(7.5 + 0.25).look.abs() > 0.5);
    }
}
