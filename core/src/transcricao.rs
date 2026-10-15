//! Transcrição pela API da OpenAI (gpt-4o-transcribe).

use crate::{resample::TAXA, wav::pcm16_para_wav, whisper::{limpar, Transcritor}};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Devolve (chave da API, modelo de transcrição), lidos a cada fala para acompanhar os Ajustes.
pub type Credenciais = Arc<dyn Fn() -> (String, String) + Send + Sync>;

pub struct TranscricaoOpenAi {
    url_base: String,
    idioma: Arc<dyn Fn() -> String + Send + Sync>,
    http: reqwest::Client,
    credenciais: Credenciais,
    espera_nova_tentativa: Duration,
}

impl TranscricaoOpenAi {
    pub fn new(credenciais: Credenciais) -> Self {
        Self::com_url("https://api.openai.com/v1", credenciais)
    }

    pub fn com_url(url: impl Into<String>, credenciais: Credenciais) -> Self {
        Self {
            url_base: url.into(),
            idioma: Arc::new(|| "pt".into()),
            http: reqwest::Client::new(),
            credenciais,
            espera_nova_tentativa: Duration::from_secs(1),
        }
    }

    /// Idioma lido a cada fala, para acompanhar os Ajustes.
    pub fn com_idioma(mut self, idioma: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        self.idioma = idioma;
        self
    }

    #[cfg(test)]
    fn sem_espera(mut self) -> Self {
        self.espera_nova_tentativa = Duration::ZERO;
        self
    }

    async fn enviar(&self, wav: &[u8], chave: &str, modelo: &str) -> Result<reqwest::Response> {
        let arquivo = reqwest::multipart::Part::bytes(wav.to_vec()).file_name("fala.wav").mime_str("audio/wav")?;
        let form = reqwest::multipart::Form::new()
            .part("file", arquivo)
            .text("model", modelo.to_string())
            .text("language", (self.idioma)())
            .text("response_format", "json");
        Ok(self.http.post(format!("{}/audio/transcriptions", self.url_base)).bearer_auth(chave).multipart(form).send().await?)
    }
}

#[async_trait]
impl Transcritor for TranscricaoOpenAi {
    async fn transcrever(&self, pcm: &[i16]) -> Result<String> {
        let (chave, modelo) = (self.credenciais)();
        if chave.is_empty() {
            return Err(anyhow!("Chave da OpenAI não configurada."));
        }
        let wav = pcm16_para_wav(pcm, TAXA);
        let mut res = self.enviar(&wav, &chave, &modelo).await?;
        // uma nova tentativa para limite de uso (429) ou falha do servidor (5xx)
        if res.status().as_u16() == 429 || res.status().is_server_error() {
            tokio::time::sleep(self.espera_nova_tentativa).await;
            res = self.enviar(&wav, &chave, &modelo).await?;
        }
        let status = res.status();
        let json: serde_json::Value = res.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = json["error"]["message"].as_str().unwrap_or("erro desconhecido");
            return Err(anyhow!("OpenAI respondeu {}: {msg}", status.as_u16()));
        }
        Ok(limpar(json["text"].as_str().unwrap_or("")))
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::whisper::testes::servidor_falso;

    fn cred(chave: &str) -> Credenciais {
        let chave = chave.to_string();
        Arc::new(move || (chave.clone(), "gpt-4o-transcribe".into()))
    }

    #[tokio::test]
    async fn envia_audio_com_modelo_idioma_e_chave() {
        let (url, pedidos) = servidor_falso(200, r#"{"text":" Olá  mundo "}"#).await;
        let t = TranscricaoOpenAi::com_url(url, cred("sk-1"));
        assert_eq!(t.transcrever(&[0i16; 160]).await.unwrap(), "Olá mundo");
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.starts_with("POST /audio/transcriptions"));
        assert!(p.contains("Bearer sk-1"));
        assert!(p.contains("gpt-4o-transcribe") && p.contains("name=\"language\"") && p.contains("name=\"model\""));
    }

    #[tokio::test]
    async fn sem_chave_nao_envia_nada() {
        let (url, pedidos) = servidor_falso(200, "{}").await;
        let e = TranscricaoOpenAi::com_url(url, cred("")).transcrever(&[0i16; 8]).await.unwrap_err();
        assert!(e.to_string().contains("Chave"));
        assert!(pedidos.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn erro_mostra_a_mensagem_da_openai_e_tenta_de_novo_em_5xx() {
        let (url, pedidos) = servidor_falso(429, r#"{"error":{"message":"limite atingido"}}"#).await;
        let e = TranscricaoOpenAi::com_url(url, cred("k")).sem_espera().transcrever(&[0i16; 8]).await.unwrap_err().to_string();
        assert!(e.contains("429") && e.contains("limite atingido"), "{e}");
        assert_eq!(pedidos.lock().unwrap().len(), 2, "uma nova tentativa");

        let (url, pedidos) = servidor_falso(401, r#"{"error":{"message":"chave inválida"}}"#).await;
        let e = TranscricaoOpenAi::com_url(url, cred("k")).sem_espera().transcrever(&[0i16; 8]).await.unwrap_err().to_string();
        assert!(e.contains("401"));
        assert_eq!(pedidos.lock().unwrap().len(), 1, "401 não repete");
    }

    #[tokio::test]
    async fn idioma_vem_dos_ajustes_a_cada_fala() {
        let (url, pedidos) = servidor_falso(200, r#"{"text":"hello"}"#).await;
        let t = TranscricaoOpenAi::com_url(url, cred("k")).com_idioma(Arc::new(|| "en".into()));
        t.transcrever(&[0i16; 8]).await.unwrap();
        let p = pedidos.lock().unwrap()[0].clone();
        let depois = p.split("name=\"language\"").nth(1).unwrap();
        assert!(depois.trim_start().starts_with("en"), "{p}");
    }
}
