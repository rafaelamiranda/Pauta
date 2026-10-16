//! Cliente do whisper-server (whisper.cpp).

use crate::{resample::TAXA, wav::pcm16_para_wav};
use anyhow::{anyhow, Result};
use async_trait::async_trait;

#[async_trait]
pub trait Transcritor: Send + Sync {
    /// Devolve o texto da fala, ou vazio se só havia ruído.
    async fn transcrever(&self, pcm: &[i16]) -> Result<String>;
    /// Como `transcrever`, mas com o idioma do áudio dado ("auto" detecta sozinho).
    async fn transcrever_em(&self, pcm: &[i16], _idioma: &str) -> Result<String> {
        self.transcrever(pcm).await
    }
    /// Como `transcrever_em`, com uma dica de nomes e siglas para o modelo acertar a grafia.
    async fn transcrever_dica(&self, pcm: &[i16], idioma: &str, _dica: &str) -> Result<String> {
        self.transcrever_em(pcm, idioma).await
    }
}

/// Descarta a resposta quando o modelo só repetiu a dica de volta (acontece com ruído ou silêncio).
pub fn sem_eco(texto: String, dica: &str) -> String {
    if !dica.is_empty() && texto.to_lowercase().contains(&crate::material::DICA_ROTULO.to_lowercase()) { String::new() } else { texto }
}

/// O whisper costuma "inventar" marcações como "[BLANK_AUDIO]" ou "(música)" ao ouvir só ruído.
pub fn limpar(texto: &str) -> String {
    let t = texto.split_whitespace().collect::<Vec<_>>().join(" ");
    let so_pontuacao = t.chars().all(|c| c.is_whitespace() || ".-–—".contains(c));
    let marcacao = t.chars().count() >= 2 && t.starts_with(['[', '(', '*']) && t.ends_with([']', ')', '*']);
    if so_pontuacao || marcacao { String::new() } else { t }
}

pub struct ClienteWhisper {
    url_base: String,
    idioma: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
    http: reqwest::Client,
    fila: tokio::sync::Mutex<()>, // o servidor atende um pedido por vez
}

impl ClienteWhisper {
    pub fn new(url_base: impl Into<String>, idioma: impl Into<String>) -> Self {
        let idioma: String = idioma.into();
        Self { url_base: url_base.into(), idioma: std::sync::Arc::new(move || idioma.clone()), http: reqwest::Client::new(), fila: Default::default() }
    }

    /// Idioma lido a cada fala, para acompanhar os Ajustes.
    pub fn com_idioma(mut self, idioma: std::sync::Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        self.idioma = idioma;
        self
    }
}

#[async_trait]
impl Transcritor for ClienteWhisper {
    async fn transcrever(&self, pcm: &[i16]) -> Result<String> {
        let idioma = (self.idioma)();
        self.transcrever_em(pcm, &idioma).await
    }

    async fn transcrever_em(&self, pcm: &[i16], idioma: &str) -> Result<String> {
        self.transcrever_dica(pcm, idioma, "").await
    }

    async fn transcrever_dica(&self, pcm: &[i16], idioma: &str, dica: &str) -> Result<String> {
        let _vez = self.fila.lock().await;
        let arquivo = reqwest::multipart::Part::bytes(pcm16_para_wav(pcm, TAXA)).file_name("fala.wav").mime_str("audio/wav")?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", arquivo)
            .text("response_format", "json")
            .text("temperature", "0.0")
            .text("language", idioma.to_string());
        if !dica.is_empty() {
            form = form.text("prompt", dica.to_string());
        }
        let res = self.http.post(format!("{}/inference", self.url_base)).multipart(form).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("whisper-server respondeu {}", res.status().as_u16()));
        }
        let json: serde_json::Value = res.json().await?;
        Ok(limpar(json["text"].as_str().unwrap_or("")))
    }
}

#[cfg(test)]
pub mod testes {
    use super::*;

    #[test]
    fn eco_da_dica_e_descartado() {
        let dica = "Nomes e termos desta reunião: Alfa.";
        assert_eq!(sem_eco("Nomes e termos desta reunião: Alfa.".into(), dica), "");
        assert_eq!(sem_eco("Vamos falar da Alfa".into(), dica), "Vamos falar da Alfa");
        assert_eq!(sem_eco("Nomes e termos desta reunião".into(), ""), "Nomes e termos desta reunião");
    }

    #[test]
    fn filtra_ruido() {
        assert_eq!(limpar(" Olá  mundo "), "Olá mundo");
        assert_eq!(limpar("[BLANK_AUDIO]"), "");
        assert_eq!(limpar("(música)"), "");
        assert_eq!(limpar(" ... "), "");
        assert_eq!(limpar("Preço (R$ 10) ok"), "Preço (R$ 10) ok");
    }

    /// Servidor HTTP mínimo: responde `corpo` a cada pedido e guarda o texto recebido.
    pub async fn servidor_falso(status: u16, corpo: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let pedidos = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let guardados = pedidos.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let guardados = guardados.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 8192];
                    // lê até ter os cabeçalhos e o corpo inteiro (Content-Length)
                    loop {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 { break; }
                        buf.extend_from_slice(&tmp[..n]);
                        let txt = String::from_utf8_lossy(&buf).to_string();
                        if let Some(fim) = txt.find("\r\n\r\n") {
                            let tam = txt[..fim].to_lowercase().split("content-length:").nth(1)
                                .and_then(|r| r.split("\r\n").next()).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                            if buf.len() >= fim + 4 + tam { break; }
                        }
                    }
                    guardados.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                    let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}", corpo.len());
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (url, pedidos)
    }

    #[tokio::test]
    async fn envia_o_audio_e_devolve_o_texto() {
        let (url, pedidos) = servidor_falso(200, r#"{"text":" Olá  mundo "}"#).await;
        let c = ClienteWhisper::new(url, "pt");
        assert_eq!(c.transcrever(&[0i16; 160]).await.unwrap(), "Olá mundo");
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.starts_with("POST /inference"));
        assert!(p.contains("name=\"language\"") && p.contains("pt"));
    }

    #[tokio::test]
    async fn dica_vai_como_prompt_so_quando_existe() {
        let (url, pedidos) = servidor_falso(200, r#"{"text":"ok"}"#).await;
        let c = ClienteWhisper::new(url, "pt");
        c.transcrever_dica(&[0i16; 160], "pt", "Nomes e termos desta reunião: Alfa.").await.unwrap();
        c.transcrever_dica(&[0i16; 160], "pt", "").await.unwrap();
        let p = pedidos.lock().unwrap().clone();
        assert!(p[0].contains("name=\"prompt\"") && p[0].contains("Alfa"));
        assert!(!p[1].contains("name=\"prompt\""));
    }

    #[tokio::test]
    async fn erro_http_vira_erro() {
        let (url, _) = servidor_falso(500, "{}").await;
        let e = ClienteWhisper::new(url, "pt").transcrever(&[0i16; 8]).await.unwrap_err();
        assert!(e.to_string().contains("500"));
    }
}
