//! Captura de áudio no Linux (PulseAudio ou PipeWire), usando o `pactl` e o `parec`.
//! Microfone: a fonte escolhida. Som do computador: o "monitor" da saída escolhida.
//! O `parec` já entrega PCM 16 bits, 16 kHz, mono, então não precisa converter.

use crate::meeting::{Fonte, Receptor};
use crate::resample::TRECHO;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

pub struct Captura {
    filhos: Vec<Child>,
    fios: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for Captura {
    fn drop(&mut self) {
        for f in &mut self.filhos {
            let _ = f.kill();
            let _ = f.wait();
        }
        for fio in self.fios.drain(..) {
            let _ = fio.join();
        }
    }
}

/// Nomes (segunda coluna) da saída de `pactl list short`.
fn nomes_da_lista(saida: &str) -> Vec<String> {
    let mut v: Vec<String> = saida.lines().filter_map(|l| l.split('\t').nth(1)).map(str::trim).filter(|n| !n.is_empty()).map(String::from).collect();
    v.sort();
    v.dedup();
    v
}

fn listar(tipo: &str) -> Vec<String> {
    let Ok(o) = Command::new("pactl").args(["list", "short", tipo]).env("LC_ALL", "C").output() else { return vec![] };
    nomes_da_lista(&String::from_utf8_lossy(&o.stdout))
}

/// Nomes dos microfones (sem os monitores) e das saídas de áudio.
pub fn dispositivos() -> (Vec<String>, Vec<String>) {
    let mics = listar("sources").into_iter().filter(|n| !n.ends_with(".monitor")).collect();
    (mics, listar("sinks"))
}

/// Fonte a gravar para o microfone: a escolhida, se existir; senão a padrão.
fn fonte_do_microfone(escolhido: &str, existentes: &[String], avisos: &mut Vec<String>) -> String {
    if escolhido.is_empty() {
        return "@DEFAULT_SOURCE@".into();
    }
    if existentes.iter().any(|n| n == escolhido) {
        return escolhido.into();
    }
    avisos.push(format!("O microfone \"{escolhido}\" não foi encontrado. Usando o padrão do sistema."));
    "@DEFAULT_SOURCE@".into()
}

/// Fonte a gravar para o som do computador: o monitor da saída escolhida, ou o da saída padrão.
fn fonte_do_sistema(escolhida: &str, existentes: &[String], avisos: &mut Vec<String>) -> String {
    if escolhida.is_empty() {
        return "@DEFAULT_MONITOR@".into();
    }
    if existentes.iter().any(|n| n == escolhida) {
        return format!("{escolhida}.monitor");
    }
    avisos.push(format!("A saída de áudio \"{escolhida}\" não foi encontrada. Usando o padrão do sistema."));
    "@DEFAULT_MONITOR@".into()
}

/// PCM 16 bits little-endian para amostras e nível (0..1, com ganho para a barra ficar visível).
fn decodificar(bytes: &[u8]) -> (Vec<i16>, f32) {
    let pcm: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let nivel = if pcm.is_empty() {
        0.0
    } else {
        let soma: f32 = pcm.iter().map(|&s| (s as f32 / 32768.0).powi(2)).sum();
        (soma / pcm.len() as f32).sqrt() * 5.0
    };
    (pcm, nivel)
}

fn abrir(fonte: &str, qual: Fonte, reuniao: Arc<dyn Receptor>) -> Result<(Child, std::thread::JoinHandle<()>), String> {
    let mut filho = Command::new("parec")
        .args([&format!("--device={fonte}"), "--format=s16le", "--rate=16000", "--channels=1", "--latency-msec=50"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("não foi possível abrir o parec ({e}). Instale o pacote pulseaudio-utils (ou pipewire-pulse)"))?;
    // fonte que não existe faz o parec sair na hora: espera um pouco para mostrar o motivo
    std::thread::sleep(Duration::from_millis(250));
    if let Ok(Some(_)) = filho.try_wait() {
        let mut erro = String::new();
        if let Some(mut e) = filho.stderr.take() {
            let _ = e.read_to_string(&mut erro);
        }
        return Err(erro.trim().to_string());
    }
    let mut saida = filho.stdout.take().ok_or("sem saída do parec")?;
    let fio = std::thread::spawn(move || {
        let mut buf = vec![0u8; TRECHO * 2];
        while saida.read_exact(&mut buf).is_ok() {
            let (pcm, nivel) = decodificar(&buf);
            reuniao.informar_nivel(qual, nivel);
            reuniao.receber_audio(qual, &pcm);
        }
    });
    Ok((filho, fio))
}

pub fn iniciar(reuniao: Arc<dyn Receptor>, microfone: String, saida: String) -> Result<(Captura, Vec<String>), String> {
    let (mics, saidas) = dispositivos();
    let mut avisos = Vec::new();
    let mut cap = Captura { filhos: vec![], fios: vec![] };
    let alvos = [
        (Fonte::Mic, fonte_do_microfone(&microfone, &mics, &mut avisos), "Microfone indisponível"),
        (Fonte::Sistema, fonte_do_sistema(&saida, &saidas, &mut avisos), "Som do computador indisponível"),
    ];
    for (qual, fonte, falha) in alvos {
        match abrir(&fonte, qual, reuniao.clone()) {
            Ok((f, fio)) => {
                cap.filhos.push(f);
                cap.fios.push(fio);
            }
            Err(e) => avisos.push(format!("{falha}: {e}")),
        }
    }
    if cap.filhos.is_empty() {
        return Err(avisos.join(" "));
    }
    Ok((cap, avisos))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn lista_curta_do_pactl_vira_nomes_sem_repetir() {
        let s = "51\talsa_output.hdmi\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n57\talsa_output.usb\tPipeWire\ts16le 2ch 48000Hz\tRUNNING\n58\talsa_output.hdmi\tx\n\n";
        assert_eq!(nomes_da_lista(s), vec!["alsa_output.hdmi", "alsa_output.usb"]);
        assert!(nomes_da_lista("").is_empty());
    }

    #[test]
    fn escolhe_a_fonte_ou_cai_no_padrao_com_aviso() {
        let existentes = vec!["alsa_input.usb".to_string()];
        let saidas = vec!["alsa_output.hdmi".to_string()];
        let mut avisos = vec![];
        assert_eq!(fonte_do_microfone("", &existentes, &mut avisos), "@DEFAULT_SOURCE@");
        assert_eq!(fonte_do_microfone("alsa_input.usb", &existentes, &mut avisos), "alsa_input.usb");
        assert_eq!(fonte_do_sistema("", &saidas, &mut avisos), "@DEFAULT_MONITOR@");
        assert_eq!(fonte_do_sistema("alsa_output.hdmi", &saidas, &mut avisos), "alsa_output.hdmi.monitor");
        assert!(avisos.is_empty());
        assert_eq!(fonte_do_microfone("sumiu", &existentes, &mut avisos), "@DEFAULT_SOURCE@");
        assert_eq!(fonte_do_sistema("sumiu", &saidas, &mut avisos), "@DEFAULT_MONITOR@");
        assert_eq!(avisos.len(), 2);
    }

    #[test]
    fn decodifica_pcm_e_calcula_o_nivel() {
        let (pcm, nivel) = decodificar(&[0, 0, 0xff, 0x7f, 0x00, 0x80, 0x01]);
        assert_eq!(pcm, vec![0, 32767, -32768]); // o byte sobrando é ignorado
        assert!(nivel > 1.0);
        assert_eq!(decodificar(&[]), (vec![], 0.0));
        assert_eq!(decodificar(&[0, 0, 0, 0]).1, 0.0);
    }
}
