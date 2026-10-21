//! Compara os detectores de voz (volume x Silero) e o RNNoise em um arquivo de áudio seu.
//!
//!   cargo run --release -p busca_core --example comparar_audio -- reuniao.mp3 [pasta_de_saida]
//!
//! Escreve na pasta de saída (padrão: ./comparacao) quatro WAVs para ouvir:
//!   volume-falas.wav   só as falas que o detector por volume achou
//!   silero-falas.wav   só as falas que o Silero achou
//!   ruido-reduzido.wav o áudio inteiro depois do RNNoise
//!   original.wav       o áudio inteiro, sem mexer (16 kHz mono)

use busca_core::decodificar::decodificar;
use busca_core::preparo::reduzir_ruido;
use busca_core::resample::TAXA;
use busca_core::segmenter::{Fala, Segmentador};
use busca_core::wav::pcm16_para_wav;
use std::path::PathBuf;
use std::time::Instant;

fn segmentar(mut seg: Segmentador, pcm: &[i16]) -> Vec<Fala> {
    let mut falas = vec![];
    for (i, trecho) in pcm.chunks_exact(1600).enumerate() {
        falas.extend(seg.push(trecho, i as i64 * 100));
    }
    falas.extend(seg.flush());
    falas
}

fn mostrar(nome: &str, falas: &[Fala], total_s: f64, tempo: f64) {
    let falado: f64 = falas.iter().map(|f| f.pcm.len() as f64 / TAXA as f64).sum();
    println!("\n== {nome}: {} falas, {:.0}% do áudio ({:.1} s de {:.1} s). Tempo de cálculo: {tempo:.2} s", falas.len(), falado / total_s * 100.0, falado, total_s);
    for f in falas.iter().take(40) {
        let ini = f.inicio_ms as f64 / 1000.0;
        println!("   {:>7.1} s  →  {:>7.1} s   ({:.1} s)", ini, ini + f.pcm.len() as f64 / TAXA as f64, f.pcm.len() as f64 / TAXA as f64);
    }
    if falas.len() > 40 {
        println!("   ... e mais {}", falas.len() - 40);
    }
}

fn juntar(falas: &[Fala]) -> Vec<i16> {
    let pausa = vec![0i16; TAXA as usize * 3 / 10];
    falas.iter().flat_map(|f| f.pcm.iter().copied().chain(pausa.iter().copied())).collect()
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(entrada) = args.next() else {
        eprintln!("Uso: comparar_audio <arquivo de áudio> [pasta de saída]");
        std::process::exit(2);
    };
    let saida = PathBuf::from(args.next().unwrap_or_else(|| "comparacao".into()));
    std::fs::create_dir_all(&saida)?;

    let mut pcm: Vec<i16> = vec![];
    decodificar(std::path::Path::new(&entrada), |t| pcm.extend(t))?;
    let total_s = pcm.len() as f64 / TAXA as f64;
    println!("Arquivo: {entrada} ({total_s:.1} s)");

    let t = Instant::now();
    let volume = segmentar(Segmentador::new(), &pcm);
    mostrar("Detector por volume", &volume, total_s, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let silero = segmentar(Segmentador::new().com_silero(), &pcm);
    mostrar("Silero (rede neural)", &silero, total_s, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let limpo = reduzir_ruido(&pcm);
    println!("\n== RNNoise: {:.2} s de cálculo para {total_s:.1} s de áudio", t.elapsed().as_secs_f64());

    let gravar = |nome: &str, p: &[i16]| std::fs::write(saida.join(nome), pcm16_para_wav(p, TAXA));
    gravar("original.wav", &pcm)?;
    gravar("ruido-reduzido.wav", &limpo)?;
    gravar("volume-falas.wav", &juntar(&volume))?;
    gravar("silero-falas.wav", &juntar(&silero))?;
    println!("\nArquivos para ouvir em {}/", saida.display());
    Ok(())
}
