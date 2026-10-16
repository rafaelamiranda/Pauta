//! Identificação de falantes entre os "Outros": a OpenAI separa quem fala em cada trecho e o app
//! guarda um pedaço da voz de cada pessoa, para reconhecê-la nos trechos seguintes.

use crate::{resample::TAXA, wav::pcm16_para_wav, whisper::limpar};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use base64::Engine;
use std::time::Duration;

/// A API aceita até 4 falantes conhecidos.
pub const MAX_FALANTES: usize = 4;
/// Trecho de referência de cada voz: entre 2 e 10 segundos.
const REFERENCIA_MIN: usize = 2 * TAXA as usize;
const REFERENCIA_MAX: usize = 10 * TAXA as usize;
pub const MODELO: &str = "gpt-4o-transcribe-diarize";

/// Uma pessoa já ouvida na reunião e o trecho da voz dela.
#[derive(Debug, Clone, PartialEq)]
pub struct Conhecido {
    pub nome: String,
    pub referencia: Vec<i16>,
}

/// Um pedaço da fala com o rótulo que o modelo deu ("A", "B" ou o nome de um conhecido).
#[derive(Debug, Clone, PartialEq)]
pub struct Trecho {
    pub falante: String,
    pub texto: String,
    pub inicio_s: f64,
    pub fim_s: f64,
}

/// Texto de uma pessoa, já com o nome dela (`None` = não deu para identificar).
#[derive(Debug, Clone, PartialEq)]
pub struct Parte {
    pub falante: Option<String>,
    pub texto: String,
    pub inicio_ms: i64,
}

#[async_trait]
pub trait Diarizador: Send + Sync {
    async fn diarizar(&self, pcm: &[i16], idioma: &str, conhecidos: &[Conhecido]) -> Result<Vec<Trecho>>;
}

/// Dá nome a cada rótulo do modelo. Rótulo novo, com voz suficiente para servir de referência, vira
/// "Pessoa N" e entra em `registro`; sem voz suficiente ou com o registro cheio, fica sem nome.
/// Trechos seguidos da mesma pessoa viram uma parte só.
pub fn atribuir(registro: &mut Vec<Conhecido>, pcm: &[i16], trechos: Vec<Trecho>) -> Vec<Parte> {
    let mut rotulos: Vec<(String, Option<String>)> = Vec::new();
    for t in &trechos {
        if rotulos.iter().any(|(r, _)| *r == t.falante) {
            continue;
        }
        let nome = if registro.iter().any(|c| c.nome == t.falante) {
            Some(t.falante.clone())
        } else {
            let mut voz: Vec<i16> = Vec::new();
            for x in trechos.iter().filter(|x| x.falante == t.falante) {
                let a = ((x.inicio_s.max(0.0) * TAXA as f64) as usize).min(pcm.len());
                let b = ((x.fim_s.max(0.0) * TAXA as f64) as usize).clamp(a, pcm.len());
                voz.extend_from_slice(&pcm[a..b]);
            }
            voz.truncate(REFERENCIA_MAX);
            if registro.len() < MAX_FALANTES && voz.len() >= REFERENCIA_MIN {
                let nome = format!("Pessoa {}", registro.len() + 1);
                registro.push(Conhecido { nome: nome.clone(), referencia: voz });
                Some(nome)
            } else {
                None
            }
        };
        rotulos.push((t.falante.clone(), nome));
    }
    let mut partes: Vec<Parte> = Vec::new();
    for t in trechos {
        let falante = rotulos.iter().find(|(r, _)| *r == t.falante).and_then(|(_, n)| n.clone());
        match partes.last_mut() {
            Some(p) if p.falante == falante => {
                p.texto.push(' ');
                p.texto.push_str(&t.texto);
            }
            _ => partes.push(Parte { falante, texto: t.texto, inicio_ms: (t.inicio_s * 1000.0) as i64 }),
        }
    }
    partes
}

/// Lê a resposta `diarized_json`. Sem a lista de trechos, usa o texto inteiro, sem falante.
pub fn ler_trechos(json: &serde_json::Value) -> Vec<Trecho> {
    let rotulo = |v: &serde_json::Value| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())).unwrap_or_default();
    if let Some(segmentos) = json["segments"].as_array() {
        return segmentos
            .iter()
            .filter_map(|s| {
                let texto = limpar(s["text"].as_str().unwrap_or(""));
                (!texto.is_empty()).then(|| Trecho {
                    falante: rotulo(&s["speaker"]),
                    texto,
                    inicio_s: s["start"].as_f64().unwrap_or(0.0),
                    fim_s: s["end"].as_f64().unwrap_or(0.0),
                })
            })
            .collect();
    }
    let texto = limpar(json["text"].as_str().unwrap_or(""));
    if texto.is_empty() { vec![] } else { vec![Trecho { falante: String::new(), texto, inicio_s: 0.0, fim_s: 0.0 }] }
}

pub struct DiarizadorOpenAi {
    url_base: String,
    http: reqwest::Client,
    /// Devolve só a chave da API; o modelo é fixo.
    chave: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
    espera_nova_tentativa: Duration,
}

impl DiarizadorOpenAi {
    pub fn new(chave: std::sync::Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        Self::com_url("https://api.openai.com/v1", chave)
    }

    pub fn com_url(url: impl Into<String>, chave: std::sync::Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        Self { url_base: url.into(), http: reqwest::Client::new(), chave, espera_nova_tentativa: Duration::from_secs(1) }
    }

    async fn enviar(&self, wav: &[u8], chave: &str, idioma: &str, conhecidos: &[Conhecido]) -> Result<reqwest::Response> {
        let arquivo = reqwest::multipart::Part::bytes(wav.to_vec()).file_name("fala.wav").mime_str("audio/wav")?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", arquivo)
            .text("model", MODELO)
            .text("response_format", "diarized_json")
            .text("chunking_strategy", "auto");
        if !idioma.is_empty() && idioma != "auto" {
            form = form.text("language", idioma.to_string());
        }
        for c in conhecidos {
            let ref_wav = base64::engine::general_purpose::STANDARD.encode(pcm16_para_wav(&c.referencia, TAXA));
            form = form.text("known_speaker_names[]", c.nome.clone()).text("known_speaker_references[]", format!("data:audio/wav;base64,{ref_wav}"));
        }
        Ok(self.http.post(format!("{}/audio/transcriptions", self.url_base)).bearer_auth(chave).multipart(form).send().await?)
    }
}

#[async_trait]
impl Diarizador for DiarizadorOpenAi {
    async fn diarizar(&self, pcm: &[i16], idioma: &str, conhecidos: &[Conhecido]) -> Result<Vec<Trecho>> {
        let chave = (self.chave)();
        if chave.is_empty() {
            return Err(anyhow!("Chave da OpenAI não configurada."));
        }
        let wav = pcm16_para_wav(pcm, TAXA);
        let mut res = self.enviar(&wav, &chave, idioma, conhecidos).await?;
        if res.status().as_u16() == 429 || res.status().is_server_error() {
            tokio::time::sleep(self.espera_nova_tentativa).await;
            res = self.enviar(&wav, &chave, idioma, conhecidos).await?;
        }
        let status = res.status();
        let json: serde_json::Value = res.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = json["error"]["message"].as_str().unwrap_or("erro desconhecido");
            return Err(anyhow!("OpenAI respondeu {}: {msg}", status.as_u16()));
        }
        Ok(ler_trechos(&json))
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::whisper::testes::servidor_falso;
    use std::sync::Arc;

    fn t(f: &str, texto: &str, a: f64, b: f64) -> Trecho {
        Trecho { falante: f.into(), texto: texto.into(), inicio_s: a, fim_s: b }
    }

    #[test]
    fn rotulo_novo_vira_pessoa_com_a_voz_guardada_e_o_conhecido_mantem_o_nome() {
        let pcm = vec![100i16; TAXA as usize * 12];
        let mut reg = vec![];
        let p = atribuir(&mut reg, &pcm, vec![t("A", "Oi", 0.0, 3.0), t("B", "Olá", 3.0, 6.0), t("A", "Tudo bem?", 6.0, 8.0)]);
        assert_eq!(p.iter().map(|x| (x.falante.as_deref(), x.texto.as_str())).collect::<Vec<_>>(), [(Some("Pessoa 1"), "Oi"), (Some("Pessoa 2"), "Olá"), (Some("Pessoa 1"), "Tudo bem?")]);
        assert_eq!(reg.len(), 2);
        assert_eq!(reg[0].referencia.len(), 5 * TAXA as usize, "voz de A: 3 s + 2 s");

        // no trecho seguinte o modelo já devolve o nome; trechos seguidos da mesma pessoa se juntam
        let p = atribuir(&mut reg, &pcm, vec![t("Pessoa 2", "Sim", 0.0, 1.0), t("Pessoa 2", "claro", 1.0, 2.0)]);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].falante.as_deref(), p[0].texto.as_str()), (Some("Pessoa 2"), "Sim claro"));
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn voz_curta_ou_registro_cheio_ficam_sem_nome() {
        let pcm = vec![100i16; TAXA as usize * 20];
        let mut reg = vec![];
        let p = atribuir(&mut reg, &pcm, vec![t("A", "Ok", 0.0, 1.0)]);
        assert_eq!(p[0].falante, None);
        assert!(reg.is_empty(), "1 s não serve de referência");

        for i in 0..MAX_FALANTES {
            reg.push(Conhecido { nome: format!("Pessoa {}", i + 1), referencia: vec![1; REFERENCIA_MIN] });
        }
        let p = atribuir(&mut reg, &pcm, vec![t("Z", "Quinto", 0.0, 5.0)]);
        assert_eq!(p[0].falante, None);
        assert_eq!(reg.len(), MAX_FALANTES);
        assert_eq!(p[0].inicio_ms, 0);
    }

    #[test]
    fn le_trechos_da_resposta_e_aceita_so_texto() {
        let j = serde_json::json!({"segments": [
            {"speaker": "A", "text": " Bom dia ", "start": 0.0, "end": 1.5},
            {"speaker": "B", "text": "[BLANK_AUDIO]", "start": 1.5, "end": 2.0},
            {"speaker": 2, "text": "Olá", "start": 2.0, "end": 3.0}]});
        let v = ler_trechos(&j);
        assert_eq!(v.iter().map(|x| (x.falante.as_str(), x.texto.as_str())).collect::<Vec<_>>(), [("A", "Bom dia"), ("2", "Olá")]);
        assert_eq!(ler_trechos(&serde_json::json!({"text": "só texto"}))[0].falante, "");
        assert!(ler_trechos(&serde_json::json!({})).is_empty());
    }

    #[tokio::test]
    async fn envia_o_modelo_o_formato_e_as_referencias_dos_conhecidos() {
        let (url, pedidos) = servidor_falso(200, r#"{"segments":[{"speaker":"Pessoa 1","text":"oi","start":0,"end":1}]}"#).await;
        let d = DiarizadorOpenAi::com_url(url, Arc::new(|| "sk-1".into()));
        let conhecidos = [Conhecido { nome: "Pessoa 1".into(), referencia: vec![5; REFERENCIA_MIN] }];
        let r = d.diarizar(&[0i16; 160], "pt", &conhecidos).await.unwrap();
        assert_eq!(r[0].falante, "Pessoa 1");
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.starts_with("POST /audio/transcriptions") && p.contains("Bearer sk-1"));
        for trecho in ["gpt-4o-transcribe-diarize", "diarized_json", "chunking_strategy", "known_speaker_names[]", "known_speaker_references[]", "data:audio/wav;base64,"] {
            assert!(p.contains(trecho), "falta {trecho}");
        }
    }

    #[tokio::test]
    async fn sem_chave_nao_envia_e_erro_mostra_a_mensagem() {
        let (url, pedidos) = servidor_falso(401, r#"{"error":{"message":"chave inválida"}}"#).await;
        assert!(DiarizadorOpenAi::com_url(url.clone(), Arc::new(String::new)).diarizar(&[0; 8], "pt", &[]).await.unwrap_err().to_string().contains("Chave"));
        assert!(pedidos.lock().unwrap().is_empty());
        let e = DiarizadorOpenAi::com_url(url, Arc::new(|| "k".into())).diarizar(&[0; 8], "pt", &[]).await.unwrap_err().to_string();
        assert!(e.contains("401") && e.contains("chave inválida"), "{e}");
    }
}
