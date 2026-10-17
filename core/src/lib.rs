//! Lógica do app de reuniões, sem dependência de interface.

pub mod baixar;
pub mod chatgpt;
#[cfg(target_os = "linux")]
pub mod captura_pulse;
pub mod deteccao;
pub mod drive;
pub mod exportar;
pub mod falantes;
pub mod locais;
pub mod mail;
pub mod material;
pub mod meeting;
pub mod resample;
pub mod roteador;
pub mod segmenter;
#[cfg(target_os = "linux")]
pub mod segredos_linux;
pub mod settings;
pub mod significado;
pub mod store;
pub mod summary;
pub mod traducao;
pub mod transcricao;
pub mod wav;
pub mod whisper;
pub mod servidor;

pub use anyhow::{anyhow, Result};

/// Hora atual em milissegundos desde 1970.
pub fn agora_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
