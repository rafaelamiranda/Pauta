//! Lê um arquivo de áudio (mp3, m4a, wav, flac, ogg, mkv e outros) e entrega PCM 16 bits, 16 kHz mono,
//! em trechos de 100 ms: o mesmo formato que as capturas ao vivo produzem.

use crate::resample::ParaPcm16k;
use anyhow::{anyhow, Result};
use std::path::Path;
use symphonia::core::{
    audio::SampleBuffer, codecs::{DecoderOptions, CODEC_TYPE_NULL}, errors::Error, formats::FormatOptions,
    io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

/// Chama `cada` com cada trecho de 100 ms. Devolve a duração total, em segundos.
pub fn decodificar(caminho: &Path, mut cada: impl FnMut(Vec<i16>)) -> Result<f64> {
    let arquivo = std::fs::File::open(caminho).map_err(|e| anyhow!("Não foi possível abrir o arquivo: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(arquivo), Default::default());
    let mut dica = Hint::new();
    if let Some(ext) = caminho.extension().and_then(|e| e.to_str()) {
        dica.with_extension(ext);
    }
    let lido = symphonia::default::get_probe()
        .format(&dica, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|_| anyhow!("Formato de áudio não reconhecido. Use mp3, m4a, wav, flac, ogg ou mkv."))?;
    let mut formato = lido.format;
    let faixa = formato.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or_else(|| anyhow!("O arquivo não tem áudio."))?;
    let id = faixa.id;
    let mut decodificador = symphonia::default::get_codecs()
        .make(&faixa.codec_params, &DecoderOptions::default())
        .map_err(|_| anyhow!("Este tipo de áudio não é suportado."))?;

    let mut conversor: Option<ParaPcm16k> = None;
    let mut trechos = 0usize;
    loop {
        let pacote = match formato.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(anyhow!("Falha ao ler o áudio: {e}")),
        };
        if pacote.track_id() != id {
            continue;
        }
        match decodificador.decode(&pacote) {
            Ok(buf) => {
                let spec = *buf.spec();
                let mut amostras = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                amostras.copy_interleaved_ref(buf);
                let c = conversor.get_or_insert_with(|| ParaPcm16k::new(spec.rate, spec.channels.count()));
                for trecho in c.feed(amostras.samples()) {
                    trechos += 1;
                    cada(trecho);
                }
            }
            Err(Error::DecodeError(_)) => continue, // um pacote ruim não derruba o arquivo
            Err(e) => return Err(anyhow!("Falha ao decodificar o áudio: {e}")),
        }
    }
    if trechos == 0 {
        return Err(anyhow!("Não encontrei áudio neste arquivo."));
    }
    Ok(trechos as f64 / 10.0)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn wav(taxa: u32, canais: u16, segundos: u32) -> Vec<u8> {
        let n = (taxa * segundos) as usize;
        let mut w = vec![];
        let dados = (n * canais as usize * 2) as u32;
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + dados).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&canais.to_le_bytes());
        w.extend_from_slice(&taxa.to_le_bytes());
        w.extend_from_slice(&(taxa * canais as u32 * 2).to_le_bytes());
        w.extend_from_slice(&(canais * 2).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&dados.to_le_bytes());
        for i in 0..n {
            let v = ((i as f32 * 0.05).sin() * 8000.0) as i16;
            for _ in 0..canais { w.extend_from_slice(&v.to_le_bytes()); }
        }
        w
    }

    fn arquivo(nome: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(nome);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn wav_de_qualquer_taxa_vira_16k_mono_em_trechos_de_100ms() {
        for (taxa, canais) in [(16000, 1), (44100, 2), (48000, 1)] {
            let p = arquivo("a.wav", &wav(taxa, canais, 3));
            let mut trechos = vec![];
            let dur = decodificar(&p, |t| trechos.push(t)).unwrap();
            assert!((29..=30).contains(&trechos.len()), "{taxa}/{canais}: {}", trechos.len());
            assert!(trechos.iter().all(|t| t.len() == 1600));
            assert!((dur - 3.0).abs() < 0.2);
            assert!(trechos.iter().flatten().any(|&s| s.abs() > 1000), "o sinal chegou");
            let _ = std::fs::remove_dir_all(p.parent().unwrap());
        }
    }

    #[test]
    fn arquivo_ruim_da_erro_claro() {
        let p = arquivo("x.mp3", b"isto nao e audio nenhum, so texto repetido ".repeat(50).as_slice());
        let e = decodificar(&p, |_| {}).unwrap_err().to_string();
        assert!(e.contains("não reconhecido") || e.contains("não suportado") || e.contains("Falha") || e.contains("Não encontrei"), "{e}");
        assert!(decodificar(&p.with_file_name("nao-existe.wav"), |_| {}).unwrap_err().to_string().contains("abrir"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
