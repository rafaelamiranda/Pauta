//! Olha o computador (memória, processador, placa de vídeo) e sugere os modelos locais que cabem nele.

use crate::locais::{modelo, CATALOGO};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Maquina {
    pub memoria_mb: u64,
    pub nucleos: usize,
    pub vulkan: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Sugestao {
    pub whisper: String,
    pub llm: String,
    pub aceleracao: String,
    pub motivo: String,
}

/// Memória total em MB. Só o Linux lê (`/proc/meminfo`); nos outros sistemas devolve 0 (desconhecida).
fn memoria_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("MemTotal:").map(str::to_owned)))
            .and_then(|l| l.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()))
            .map_or(0, |kb| kb / 1024);
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// Há driver Vulkan instalado? (O modelo local usa o Vulkan para a placa de vídeo.)
fn tem_vulkan() -> bool {
    #[cfg(target_os = "linux")]
    {
        ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d", "/usr/local/share/vulkan/icd.d"]
            .iter()
            .any(|d| std::fs::read_dir(d).is_ok_and(|mut e| e.any(|f| f.is_ok_and(|f| f.file_name().to_string_lossy().ends_with(".json")))))
    }
    #[cfg(windows)]
    {
        std::path::Path::new(r"C:\Windows\System32\vulkan-1.dll").exists()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        false
    }
}

pub fn detectar() -> Maquina {
    Maquina {
        memoria_mb: memoria_mb(),
        nucleos: std::thread::available_parallelism().map_or(1, |n| n.get()),
        vulkan: tem_vulkan(),
    }
}

/// Sugere modelos para a máquina. Memória desconhecida (0) cai na escolha segura, a recomendada do catálogo.
pub fn sugerir(m: &Maquina) -> Sugestao {
    let padrao = |t| CATALOGO.iter().find(|c| c.tipo == t && c.recomendado).map_or("", |c| c.id).to_string();
    let (whisper, llm, motivo) = match (m.memoria_mb, m.nucleos) {
        (0, _) => (padrao(crate::locais::Tipo::Whisper), padrao(crate::locais::Tipo::Llm), "Não consegui medir a memória deste computador; sugiro os modelos de uso geral.".to_string()),
        (mem, n) if mem < 8_000 || n < 4 => (
            "whisper-base".into(), "qwen3-4b-q4".into(),
            format!("Pouca memória ou poucos núcleos ({} MB, {} núcleos): modelos leves para não travar o computador.", mem, n),
        ),
        (mem, _) if mem < 16_000 => (
            "whisper-small".into(), "qwen3-4b-q4".into(),
            format!("Memória moderada ({} MB): o Whisper small e o Qwen3 4B compactado cabem com folga.", mem),
        ),
        (mem, _) => (
            if m.vulkan { "whisper-turbo-q5" } else { "whisper-small" }.into(),
            "qwen3-4b-q8".into(),
            format!("Bastante memória ({} MB): dá para usar versões menos compactadas.", mem),
        ),
    };
    debug_assert!(modelo(&whisper).is_some() && modelo(&llm).is_some());
    let aceleracao = if m.vulkan { "vulkan" } else { "cpu" }.to_string();
    let motivo = if m.vulkan { format!("{motivo} Há driver Vulkan: o modelo de análise pode usar a placa de vídeo.") } else { motivo };
    Sugestao { whisper, llm, aceleracao, motivo }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn maq(memoria_mb: u64, nucleos: usize, vulkan: bool) -> Maquina {
        Maquina { memoria_mb, nucleos, vulkan }
    }

    #[test]
    fn sugere_por_memoria_e_nucleos_com_ids_que_existem_no_catalogo() {
        for m in [maq(0, 8, false), maq(4_000, 8, false), maq(32_000, 2, true), maq(12_000, 8, false), maq(32_000, 16, true), maq(32_000, 16, false)] {
            let s = sugerir(&m);
            assert!(modelo(&s.whisper).is_some() && modelo(&s.llm).is_some(), "{s:?}");
        }
        assert_eq!(sugerir(&maq(4_000, 8, false)).whisper, "whisper-base");
        assert_eq!(sugerir(&maq(32_000, 2, true)).whisper, "whisper-base", "poucos núcleos pesam mais que a memória");
        assert_eq!(sugerir(&maq(12_000, 8, false)).whisper, "whisper-small");
        let grande = sugerir(&maq(32_000, 16, true));
        assert_eq!((grande.whisper.as_str(), grande.llm.as_str(), grande.aceleracao.as_str()), ("whisper-turbo-q5", "qwen3-4b-q8", "vulkan"));
        assert_eq!(sugerir(&maq(32_000, 16, false)).aceleracao, "cpu");
        assert!(sugerir(&maq(0, 8, false)).motivo.contains("não consegui medir") || sugerir(&maq(0, 8, false)).motivo.contains("Não consegui medir"));
    }

    #[test]
    fn detecta_a_maquina_de_verdade_sem_falhar() {
        let m = detectar();
        assert!(m.nucleos >= 1);
        let _ = sugerir(&m);
    }
}
