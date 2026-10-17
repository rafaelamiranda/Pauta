//! Detectar reunião sozinho (Linux): quando um app de chamada passa a gravar o microfone, o app sugere iniciar.
//! Lê os streams de gravação do PulseAudio/PipeWire (`pactl list source-outputs`). Meet, Teams e Zoom
//! pelo navegador aparecem como o navegador; Zoom e Teams instalados, pelo nome do programa.

/// Programas de navegador: gravar o microfone ali costuma ser uma chamada (Meet, Teams ou Zoom na web).
const NAVEGADORES: [&str; 7] = ["chrom", "firefox", "brave", "msedge", "vivaldi", "opera", "zen"];

/// Nome para mostrar ao usuário, se este programa é de chamada.
fn app_de_reuniao(nome: &str, binario: &str) -> Option<&'static str> {
    let texto = format!("{nome} {binario}").to_lowercase();
    if texto.contains("zoom") {
        Some("Zoom")
    } else if texto.contains("teams") {
        Some("Teams")
    } else if NAVEGADORES.iter().any(|n| texto.contains(n)) {
        Some("o navegador (Meet, Teams ou Zoom)")
    } else {
        None
    }
}

fn propriedade<'a>(bloco: &'a str, chave: &str) -> &'a str {
    bloco
        .lines()
        .find_map(|l| l.trim().strip_prefix(chave)?.trim_start().strip_prefix('='))
        .map(|v| v.trim().trim_matches('"'))
        .unwrap_or("")
}

/// App de chamada que está gravando o microfone agora, a partir da saída de `pactl list source-outputs`.
/// Ignora streams parados (corked) e a própria captura do Pauta.
pub fn reuniao_em_curso(saida: &str) -> Option<&'static str> {
    saida.split("Source Output #").skip(1).find_map(|bloco| {
        if bloco.lines().any(|l| l.trim() == "Corked: yes") {
            return None;
        }
        let binario = propriedade(bloco, "application.process.binary");
        if binario == "parec" || binario == "pauta" {
            return None;
        }
        app_de_reuniao(propriedade(bloco, "application.name"), binario)
    })
}

#[cfg(target_os = "linux")]
pub fn consultar() -> Option<&'static str> {
    let o = std::process::Command::new("pactl").args(["list", "source-outputs"]).env("LC_ALL", "C").output().ok()?;
    reuniao_em_curso(&String::from_utf8_lossy(&o.stdout))
}

#[cfg(test)]
mod testes {
    use super::*;

    fn stream(nome: &str, binario: &str, corked: &str) -> String {
        format!("Source Output #7\n\tCorked: {corked}\n\tProperties:\n\t\tapplication.name = \"{nome}\"\n\t\tapplication.process.binary = \"{binario}\"\n")
    }

    #[test]
    fn reconhece_zoom_teams_e_navegador() {
        assert_eq!(reuniao_em_curso(&stream("ZOOM VoiceEngine", "zoom", "no")), Some("Zoom"));
        assert_eq!(reuniao_em_curso(&stream("Microsoft Teams", "teams-for-linux", "no")), Some("Teams"));
        assert_eq!(reuniao_em_curso(&stream("Google Chrome input", "chrome", "no")), Some("o navegador (Meet, Teams ou Zoom)"));
        assert_eq!(reuniao_em_curso(&stream("Firefox", "firefox", "no")), Some("o navegador (Meet, Teams ou Zoom)"));
    }

    #[test]
    fn ignora_parado_proprio_e_outros_programas() {
        assert_eq!(reuniao_em_curso(&stream("Zoom", "zoom", "yes")), None, "stream parado");
        assert_eq!(reuniao_em_curso(&stream("Zoom", "parec", "no")), None, "captura do próprio Pauta");
        assert_eq!(reuniao_em_curso(&stream("OBS", "obs", "no")), None);
        assert_eq!(reuniao_em_curso(""), None);
        // achou o app de chamada mesmo atrás de um stream irrelevante
        let dois = format!("{}{}", stream("OBS", "obs", "no"), stream("Zoom", "zoom", "no"));
        assert_eq!(reuniao_em_curso(&dois), Some("Zoom"));
    }
}
