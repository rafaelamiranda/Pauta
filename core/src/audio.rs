//! Áudio das falas guardado no computador (só quando a pessoa liga a opção nos Ajustes).
//!
//! Cada fonte (microfone, som do computador) grava num arquivo `<pasta>/<conversa>/<fonte>.falas`.
//! O arquivo é uma sequência de registros: início da fala em ms (i64), nº de amostras (u32) e as
//! amostras PCM 16 bits, 16 kHz mono, tudo little-endian. Cada fala é gravada inteira assim que
//! termina, então uma queda do app só pode cortar o último registro, que a leitura ignora.

use crate::meeting::Fonte;
use crate::segmenter::Fala;
use anyhow::Result;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn nome_do_arquivo(fonte: Fonte) -> &'static str {
    match fonte {
        Fonte::Mic => "mic.falas",
        Fonte::Sistema => "sistema.falas",
    }
}

fn pasta_da_conversa(raiz: &Path, id: &str) -> PathBuf {
    raiz.join(id)
}

pub struct Gravador {
    arquivo: File,
}

impl Gravador {
    /// Abre (ou continua) o arquivo da fonte na conversa `id`.
    pub fn abrir(raiz: &Path, id: &str, fonte: Fonte) -> Result<Self> {
        let pasta = pasta_da_conversa(raiz, id);
        std::fs::create_dir_all(&pasta)?;
        let arquivo = OpenOptions::new().create(true).append(true).open(pasta.join(nome_do_arquivo(fonte)))?;
        Ok(Self { arquivo })
    }

    pub fn gravar(&mut self, fala: &Fala) -> Result<()> {
        let mut registro = Vec::with_capacity(12 + fala.pcm.len() * 2);
        registro.extend_from_slice(&fala.inicio_ms.to_le_bytes());
        registro.extend_from_slice(&(fala.pcm.len() as u32).to_le_bytes());
        for s in &fala.pcm {
            registro.extend_from_slice(&s.to_le_bytes());
        }
        // um único write: o registro entra inteiro ou não entra
        self.arquivo.write_all(&registro)?;
        Ok(())
    }
}

/// Falas guardadas da fonte, em ordem. Um registro cortado no fim do arquivo é ignorado.
pub fn ler(raiz: &Path, id: &str, fonte: Fonte) -> Vec<Fala> {
    let mut bytes = vec![];
    let Ok(mut f) = File::open(pasta_da_conversa(raiz, id).join(nome_do_arquivo(fonte))) else { return vec![] };
    if f.read_to_end(&mut bytes).is_err() {
        return vec![];
    }
    let mut falas = vec![];
    let mut i = 0;
    while i + 12 <= bytes.len() {
        let inicio_ms = i64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let n = u32::from_le_bytes(bytes[i + 8..i + 12].try_into().unwrap()) as usize;
        let Some(fim) = (i + 12).checked_add(n.saturating_mul(2)).filter(|&f| f <= bytes.len()) else { break };
        let pcm = bytes[i + 12..fim].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        falas.push(Fala { pcm, inicio_ms });
        i = fim;
    }
    falas
}

pub fn existe(raiz: &Path, id: &str) -> bool {
    [Fonte::Mic, Fonte::Sistema].iter().any(|&f| pasta_da_conversa(raiz, id).join(nome_do_arquivo(f)).metadata().is_ok_and(|m| m.len() > 0))
}

pub fn apagar(raiz: &Path, id: &str) {
    if !id.is_empty() && !id.contains(['/', '\\', '.']) {
        let _ = std::fs::remove_dir_all(pasta_da_conversa(raiz, id));
    }
}

/// Apaga o áudio das conversas que não existem mais (apagadas pela pessoa ou pela limpeza automática).
pub fn limpar_orfaos(raiz: &Path, existe_conversa: impl Fn(&str) -> bool) {
    let Ok(pastas) = std::fs::read_dir(raiz) else { return };
    for p in pastas.flatten() {
        let id = p.file_name().to_string_lossy().to_string();
        if p.path().is_dir() && !existe_conversa(&id) {
            apagar(raiz, &id);
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn pasta() -> PathBuf {
        std::env::temp_dir().join(format!("aud-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn grava_e_le_as_falas_na_ordem() {
        let raiz = pasta();
        let mut g = Gravador::abrir(&raiz, "c1", Fonte::Mic).unwrap();
        g.gravar(&Fala { pcm: vec![1, -2, 3], inicio_ms: 500 }).unwrap();
        g.gravar(&Fala { pcm: vec![], inicio_ms: 900 }).unwrap();
        drop(g);
        let mut g = Gravador::abrir(&raiz, "c1", Fonte::Mic).unwrap(); // continua depois de reabrir
        g.gravar(&Fala { pcm: vec![7; 4], inicio_ms: 2000 }).unwrap();
        let l = ler(&raiz, "c1", Fonte::Mic);
        assert_eq!(l.iter().map(|f| (f.inicio_ms, f.pcm.len())).collect::<Vec<_>>(), vec![(500, 3), (900, 0), (2000, 4)]);
        assert_eq!(l[0].pcm, vec![1, -2, 3]);
        assert!(ler(&raiz, "c1", Fonte::Sistema).is_empty() && ler(&raiz, "outra", Fonte::Mic).is_empty());
        assert!(existe(&raiz, "c1") && !existe(&raiz, "outra"));
        let _ = std::fs::remove_dir_all(raiz);
    }

    #[test]
    fn queda_no_meio_do_registro_perde_so_o_ultimo() {
        let raiz = pasta();
        let mut g = Gravador::abrir(&raiz, "c1", Fonte::Sistema).unwrap();
        g.gravar(&Fala { pcm: vec![5; 10], inicio_ms: 1 }).unwrap();
        g.gravar(&Fala { pcm: vec![6; 10], inicio_ms: 2 }).unwrap();
        drop(g);
        let arq = raiz.join("c1").join("sistema.falas");
        let tam = std::fs::metadata(&arq).unwrap().len();
        let f = OpenOptions::new().write(true).open(&arq).unwrap();
        f.set_len(tam - 7).unwrap(); // corta o fim do segundo registro
        let l = ler(&raiz, "c1", Fonte::Sistema);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].inicio_ms, 1);
        // cabeçalho com tamanho absurdo também não derruba a leitura
        std::fs::write(&arq, [0u8, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 2]).unwrap();
        assert!(ler(&raiz, "c1", Fonte::Sistema).is_empty());
        let _ = std::fs::remove_dir_all(raiz);
    }

    #[test]
    fn apaga_a_conversa_e_so_os_orfaos() {
        let raiz = pasta();
        for id in ["a", "b"] {
            Gravador::abrir(&raiz, id, Fonte::Mic).unwrap().gravar(&Fala { pcm: vec![1], inicio_ms: 0 }).unwrap();
        }
        limpar_orfaos(&raiz, |id| id == "a");
        assert!(existe(&raiz, "a") && !raiz.join("b").exists());
        apagar(&raiz, "a");
        assert!(!raiz.join("a").exists());
        apagar(&raiz, "../x"); // nome estranho é ignorado
        let _ = std::fs::remove_dir_all(raiz);
    }
}
