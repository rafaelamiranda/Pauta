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

/// Maior duração gravada num arquivo da reunião (o que passar disso fica de fora).
const DURACAO_MAXIMA_MS: i64 = 12 * 3600 * 1000;
/// Amostras misturadas de cada vez (10 s), para não montar a reunião inteira na memória.
const BLOCO: usize = crate::resample::TAXA as usize * 10;

/// Grava em `destino` um WAV com a reunião inteira: microfone e som do computador juntos, cada fala no seu
/// horário e silêncio entre elas, para o tempo do arquivo bater com o da transcrição. Devolve a duração em ms,
/// ou `None` se a conversa não tem áudio guardado. Só as falas são guardadas (o detector de voz descarta o
/// silêncio e ruídos curtos), então o que ficou de fora delas sai como silêncio.
pub fn salvar_reuniao_wav(raiz: &Path, id: &str, destino: &Path) -> Result<Option<i64>> {
    let mut falas = ler(raiz, id, Fonte::Mic);
    falas.extend(ler(raiz, id, Fonte::Sistema));
    falas.retain(|f| !f.pcm.is_empty());
    let Some(base) = falas.iter().map(|f| f.inicio_ms).min() else { return Ok(None) };
    falas.sort_by_key(|f| f.inicio_ms);
    let taxa = crate::resample::TAXA as i64;
    let inicio = |f: &Fala| ((f.inicio_ms - base) * taxa / 1000) as usize;
    let total = falas.iter().map(|f| inicio(f) + f.pcm.len()).max().unwrap_or(0).min((DURACAO_MAXIMA_MS * taxa / 1000) as usize);

    let mut saida = std::io::BufWriter::new(File::create(destino)?);
    saida.write_all(&crate::wav::cabecalho(total as u32, crate::resample::TAXA))?;
    let mut proxima = 0;
    let mut ativas: Vec<usize> = vec![];
    let mut mistura = vec![0i32; BLOCO];
    let mut bytes = Vec::with_capacity(BLOCO * 2);
    for bloco_ini in (0..total).step_by(BLOCO) {
        let bloco_fim = (bloco_ini + BLOCO).min(total);
        while proxima < falas.len() && inicio(&falas[proxima]) < bloco_fim {
            ativas.push(proxima);
            proxima += 1;
        }
        let m = &mut mistura[..bloco_fim - bloco_ini];
        m.fill(0);
        for &k in &ativas {
            let (ini, pcm) = (inicio(&falas[k]), &falas[k].pcm);
            let de = bloco_ini.max(ini);
            let ate = bloco_fim.min(ini + pcm.len());
            for t in de..ate {
                m[t - bloco_ini] += pcm[t - ini] as i32;
            }
        }
        ativas.retain(|&k| inicio(&falas[k]) + falas[k].pcm.len() > bloco_fim);
        bytes.clear();
        for &v in m.iter() {
            bytes.extend_from_slice(&(v.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
        }
        saida.write_all(&bytes)?;
    }
    saida.flush()?;
    Ok(Some(total as i64 * 1000 / taxa))
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
    fn arquivo_da_reuniao_junta_as_fontes_no_horario_certo() {
        let raiz = pasta();
        let mut mic = Gravador::abrir(&raiz, "c1", Fonte::Mic).unwrap();
        let mut sis = Gravador::abrir(&raiz, "c1", Fonte::Sistema).unwrap();
        mic.gravar(&Fala { pcm: vec![100; 16], inicio_ms: 10_000 }).unwrap(); // começa no zero
        sis.gravar(&Fala { pcm: vec![30_000; 32], inicio_ms: 10_001 }).unwrap(); // 16 amostras depois, sobrepõe
        sis.gravar(&Fala { pcm: vec![-5; 16_000 * 25], inicio_ms: 12_000 }).unwrap(); // atravessa blocos de 10 s
        let destino = raiz.join("r.wav");
        let ms = salvar_reuniao_wav(&raiz, "c1", &destino).unwrap().unwrap();
        assert_eq!(ms, 27_000);
        let b = std::fs::read(&destino).unwrap();
        let amostra = |i: usize| i16::from_le_bytes([b[44 + i * 2], b[45 + i * 2]]);
        assert_eq!(b.len(), 44 + 16_000 * 27 * 2);
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()) as usize, b.len() - 44);
        assert_eq!((amostra(0), amostra(15), amostra(16), amostra(47), amostra(48)), (100, 100, 30_000, 30_000, 0));
        assert_eq!((amostra(32_000), amostra(16_000 * 20), amostra(16_000 * 27 - 1)), (-5, -5, -5));
        assert!(salvar_reuniao_wav(&raiz, "outra", &raiz.join("x.wav")).unwrap().is_none());
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
