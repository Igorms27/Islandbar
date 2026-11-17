//! Escolhas feitas no cartão inicial: o que aparece na ilha, de qual sensor
//! vem a temperatura e o pet.
//!
//! Ficam guardadas no registro do Windows, na conta do usuário
//! (`HKEY_CURRENT_USER\Software\Islandbar`, valor `Stats`), num texto curto
//! como `temp=gpu;show=cpu,gpu,ram,temp;pet=ghost;weather=on`. Assim continuam valendo quando a
//! ilha abre sozinha com o Windows ou depois de uma atualização.

/// De qual sensor a ilha mostra a temperatura.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TempChoice {
    /// O primeiro que o PC oferecer: placa-mãe, depois placa de vídeo, depois disco.
    #[default]
    Auto,
    Gpu,
    Board,
    Disk,
}

impl TempChoice {
    pub const ALL: [TempChoice; 4] = [TempChoice::Auto, TempChoice::Gpu, TempChoice::Board, TempChoice::Disk];

    fn key(self) -> &'static str {
        match self {
            TempChoice::Auto => "auto",
            TempChoice::Gpu => "gpu",
            TempChoice::Board => "board",
            TempChoice::Disk => "disk",
        }
    }
}

/// O bichinho no lugar das barrinhas do som (veja `pet.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pet {
    /// As barrinhas que dançam com o som (o original).
    #[default]
    Bars,
    Ghost,
    Robot,
}

impl Pet {
    pub const ALL: [Pet; 3] = [Pet::Bars, Pet::Ghost, Pet::Robot];

    fn key(self) -> &'static str {
        match self {
            Pet::Bars => "bars",
            Pet::Ghost => "ghost",
            Pet::Robot => "robot",
        }
    }
}

/// O que aparece na pílula (sem música) e na pílula do modo jogo. O painel
/// aberto sempre mostra tudo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub temp: TempChoice,
    pub pet: Pet,
    pub cpu: bool,
    pub gpu: bool,
    pub ram: bool,
    pub show_temp: bool,
    /// Clima ao lado do relógio (o painel aberto sempre tem a página do clima).
    pub weather: bool,
}

impl Default for Settings {
    /// O mesmo que a ilha mostrava antes desta opção existir.
    fn default() -> Self {
        Self {
            temp: TempChoice::Auto,
            pet: Pet::Bars,
            cpu: true,
            gpu: true,
            ram: true,
            show_temp: false,
            weather: true,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        store::read("Stats").map(|text| Self::parse(&text)).unwrap_or_default()
    }

    pub fn save(&self) {
        store::write("Stats", &self.to_text());
    }

    fn to_text(self) -> String {
        let shown: Vec<&str> = [(self.cpu, "cpu"), (self.gpu, "gpu"), (self.ram, "ram"), (self.show_temp, "temp")]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect();
        let weather = if self.weather { "on" } else { "off" };
        format!("temp={};show={};pet={};weather={weather}", self.temp.key(), shown.join(","), self.pet.key())
    }

    /// Lê o texto guardado. O que não for entendido fica no padrão.
    fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for part in text.split(';') {
            match part.trim().split_once('=') {
                Some(("temp", value)) => {
                    if let Some(choice) = TempChoice::ALL.into_iter().find(|c| c.key() == value.trim()) {
                        settings.temp = choice;
                    }
                }
                Some(("pet", value)) => {
                    if let Some(pet) = Pet::ALL.into_iter().find(|p| p.key() == value.trim()) {
                        settings.pet = pet;
                    }
                }
                // Escolhas guardadas antes do clima existir não têm esta parte: fica ligado.
                Some(("weather", value)) => settings.weather = value.trim() != "off",
                Some(("show", value)) => {
                    let names: Vec<&str> = value.split(',').map(str::trim).collect();
                    settings.cpu = names.contains(&"cpu");
                    settings.gpu = names.contains(&"gpu");
                    settings.ram = names.contains(&"ram");
                    settings.show_temp = names.contains(&"temp");
                }
                _ => {}
            }
        }
        settings
    }
}

/// Textos curtos guardados no registro, em `HKEY_CURRENT_USER\Software\Islandbar`
/// (as escolhas do cartão em `Stats`, o alarme marcado em `Alarm`).
#[cfg(windows)]
pub mod store {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    const KEY: PCWSTR = windows::core::w!("Software\\Islandbar");

    pub fn read(name: &str) -> Option<String> {
        let name = HSTRING::from(name);
        let mut buffer = [0u16; 256];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                KEY,
                &name,
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
    }

    pub fn write(name: &str, text: &str) {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                KEY,
                &HSTRING::from(name),
                REG_SZ.0,
                Some(wide.as_ptr() as *const std::ffi::c_void),
                (wide.len() * 2) as u32,
            )
        };
        crate::perf::line(format!("store: {name} = {text}"));
    }

    pub fn delete(name: &str) {
        let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, KEY, &HSTRING::from(name)) };
        crate::perf::line(format!("store: {name} removed"));
    }
}

/// Fora do Windows não há registro: vale o padrão, ou ISLANDBAR_DEBUG_<NOME>
/// (ex.: ISLANDBAR_DEBUG_STATS).
#[cfg(not(windows))]
pub mod store {
    pub fn read(name: &str) -> Option<String> {
        std::env::var(format!("ISLANDBAR_DEBUG_{}", name.to_uppercase())).ok()
    }

    pub fn write(name: &str, text: &str) {
        crate::perf::line(format!("store: {name} = {text}"));
    }

    pub fn delete(name: &str) {
        crate::perf::line(format!("store: {name} removed"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarda_e_le_de_volta() {
        let settings = Settings {
            temp: TempChoice::Gpu,
            pet: Pet::Robot,
            cpu: true,
            gpu: false,
            ram: true,
            show_temp: true,
            weather: false,
        };
        assert_eq!(settings.to_text(), "temp=gpu;show=cpu,ram,temp;pet=robot;weather=off");
        assert_eq!(Settings::parse(&settings.to_text()), settings);
        // Guardado antes do clima existir: o clima aparece.
        assert!(Settings::parse("temp=gpu;show=cpu;pet=bars").weather);
    }

    #[test]
    fn texto_estranho_fica_no_padrao() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("temp=xyz"), Settings::default());
        let nothing = Settings::parse("show=");
        assert!(!nothing.cpu && !nothing.gpu && !nothing.ram && !nothing.show_temp);
    }
}
