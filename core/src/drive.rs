//! Conexão com o Google Drive (somente leitura): login pelo navegador, busca e leitura de documentos.
//!
//! O login usa o fluxo de aplicativo instalado do Google (OAuth com PKCE e retorno em 127.0.0.1).
//! A pessoa cria o próprio "ID do cliente" no Google Cloud e informa nos Ajustes.

use crate::material::{extrair_texto, Documento, LIMITE_ARQUIVO};
use crate::settings::Segredos;
use anyhow::{anyhow, Result};
use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ESCOPO: &str = "https://www.googleapis.com/auth/drive.readonly";
const SEGREDO_REFRESH: &str = "driveRefresh";
const GOOGLE_DOC: &str = "application/vnd.google-apps.document";
const GOOGLE_SHEET: &str = "application/vnd.google-apps.spreadsheet";
const GOOGLE_SLIDES: &str = "application/vnd.google-apps.presentation";
const GOOGLE_PASTA: &str = "application/vnd.google-apps.folder";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArquivoDrive {
    pub id: String,
    pub nome: String,
    pub mime: String,
    pub modificado: Option<String>,
}

/// Devolve (ID do cliente, segredo do cliente) lidos dos Ajustes.
pub type CredenciaisGoogle = Arc<dyn Fn() -> (String, String) + Send + Sync>;

pub struct Drive {
    url_api: String,
    url_token: String,
    url_auth: String,
    http: reqwest::Client,
    segredos: Arc<dyn Segredos>,
    credenciais: CredenciaisGoogle,
    /// Token de acesso em uso e quando ele vence (segundos desde 1970).
    acesso: Mutex<Option<(String, i64)>>,
}

pub(crate) fn agora_s() -> i64 {
    crate::agora_ms() / 1000
}

pub(crate) fn aleatorio() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

pub(crate) fn desafio(verificador: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verificador.as_bytes()))
}

/// Aceita um ID solto ou um link do Drive/Docs/Sheets/Slides e devolve o ID do arquivo.
pub fn id_do_link(texto: &str) -> Option<String> {
    let t = texto.trim();
    if !t.starts_with("http") {
        return None;
    }
    let url = reqwest::Url::parse(t).ok()?;
    if !url.host_str()?.ends_with("google.com") {
        return None;
    }
    if let Some((_, id)) = url.query_pairs().find(|(k, _)| k == "id") {
        return Some(id.into_owned());
    }
    let partes: Vec<&str> = url.path_segments()?.collect();
    let pos = partes.iter().position(|p| *p == "d")?;
    partes.get(pos + 1).filter(|id| !id.is_empty()).map(|id| id.to_string())
}

fn texto_pergunta(t: &str) -> String {
    t.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Espera a volta do navegador no endereço local e devolve o código de autorização.
pub(crate) async fn esperar_codigo(listener: tokio::net::TcpListener, estado: &str, limite: Duration, nome: &str) -> Result<String> {
    let esperar = async {
        loop {
            let (mut sock, _) = listener.accept().await?;
            let mut buf = vec![0u8; 8192];
            let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await.ok().and_then(|r| r.ok()).unwrap_or(0);
            let pedido = String::from_utf8_lossy(&buf[..n]).to_string();
            let caminho = pedido.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/").to_string();
            let url = reqwest::Url::parse(&format!("http://127.0.0.1{caminho}"))?;
            let param = |k: &str| url.query_pairs().find(|(c, _)| c == k).map(|(_, v)| v.into_owned());
            let resposta = |msg: &str| {
                let corpo = format!("<!doctype html><meta charset=utf-8><title>Pauta</title><body style=\"font-family:sans-serif;padding:40px\"><h2>{msg}</h2><p>Você já pode fechar esta aba.</p>");
                format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}", corpo.len())
            };
            if let Some(e) = param("error") {
                let _ = sock.write_all(resposta("Conexão cancelada.").as_bytes()).await;
                return Err(anyhow!("O {nome} recusou a conexão: {e}."));
            }
            match (param("code"), param("state")) {
                (Some(codigo), Some(est)) if est == estado => {
                    let _ = sock.write_all(resposta(&format!("{nome} conectado.")).as_bytes()).await;
                    return Ok(codigo);
                }
                _ => {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await; // ex.: favicon
                }
            }
        }
    };
    tokio::time::timeout(limite, esperar).await.map_err(|_| anyhow!("Tempo esgotado esperando o login no {nome}."))?
}

impl Drive {
    pub fn new(segredos: Arc<dyn Segredos>, credenciais: CredenciaisGoogle) -> Self {
        Self::com_urls(
            "https://www.googleapis.com",
            "https://oauth2.googleapis.com/token",
            "https://accounts.google.com/o/oauth2/v2/auth",
            segredos,
            credenciais,
        )
    }

    pub fn com_urls(api: &str, token: &str, auth: &str, segredos: Arc<dyn Segredos>, credenciais: CredenciaisGoogle) -> Self {
        Self {
            url_api: api.into(),
            url_token: token.into(),
            url_auth: auth.into(),
            http: reqwest::Client::new(),
            segredos,
            credenciais,
            acesso: Mutex::new(None),
        }
    }

    pub fn conectado(&self) -> bool {
        self.segredos.ler(SEGREDO_REFRESH).is_some_and(|t| !t.is_empty())
    }

    pub fn desconectar(&self) -> Result<()> {
        *self.acesso.lock().unwrap() = None;
        self.segredos.gravar(SEGREDO_REFRESH, "")
    }

    fn cliente(&self) -> Result<(String, String)> {
        let (id, segredo) = (self.credenciais)();
        if id.trim().is_empty() || segredo.trim().is_empty() {
            return Err(anyhow!("Informe o ID do cliente e o segredo do Google nos Ajustes."));
        }
        Ok((id.trim().to_string(), segredo.trim().to_string()))
    }

    async fn pedir_token(&self, campos: &[(&str, &str)]) -> Result<serde_json::Value> {
        let res = self.http.post(&self.url_token).form(campos).send().await?;
        let status = res.status();
        let json: serde_json::Value = res.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = json["error_description"].as_str().or(json["error"].as_str()).unwrap_or("erro desconhecido");
            return Err(anyhow!("O Google respondeu {}: {msg}", status.as_u16()));
        }
        Ok(json)
    }

    /// Abre o login no navegador (`abrir` recebe o endereço) e guarda a permissão de leitura.
    pub async fn conectar(&self, abrir: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        self.conectar_com_limite(abrir, Duration::from_secs(180)).await
    }

    async fn conectar_com_limite(&self, abrir: impl FnOnce(&str) -> Result<()>, limite: Duration) -> Result<()> {
        let (id, segredo) = self.cliente()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let (verificador, estado) = (aleatorio(), aleatorio());
        let url = reqwest::Url::parse_with_params(
            &self.url_auth,
            &[
                ("client_id", id.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("response_type", "code"),
                ("scope", ESCOPO),
                ("access_type", "offline"),
                ("prompt", "consent"),
                ("state", estado.as_str()),
                ("code_challenge", desafio(&verificador).as_str()),
                ("code_challenge_method", "S256"),
            ],
        )?;
        abrir(url.as_str())?;
        let codigo = esperar_codigo(listener, &estado, limite, "Google Drive").await?;
        let json = self
            .pedir_token(&[
                ("grant_type", "authorization_code"),
                ("code", codigo.as_str()),
                ("client_id", id.as_str()),
                ("client_secret", segredo.as_str()),
                ("code_verifier", verificador.as_str()),
                ("redirect_uri", redirect.as_str()),
            ])
            .await?;
        let refresh = json["refresh_token"].as_str().ok_or_else(|| anyhow!("O Google não devolveu a permissão de acesso contínuo. Tente conectar de novo."))?;
        self.segredos.gravar(SEGREDO_REFRESH, refresh)?;
        *self.acesso.lock().unwrap() = json["access_token"].as_str().map(|t| (t.to_string(), agora_s() + json["expires_in"].as_i64().unwrap_or(3600) - 60));
        Ok(())
    }

    async fn token(&self) -> Result<String> {
        if let Some((t, vence)) = self.acesso.lock().unwrap().clone() {
            if vence > agora_s() {
                return Ok(t);
            }
        }
        let refresh = self.segredos.ler(SEGREDO_REFRESH).filter(|t| !t.is_empty()).ok_or_else(|| anyhow!("Google Drive não conectado. Conecte nos Ajustes."))?;
        let (id, segredo) = self.cliente()?;
        let json = self
            .pedir_token(&[("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()), ("client_id", id.as_str()), ("client_secret", segredo.as_str())])
            .await
            .map_err(|e| anyhow!("{e}. Se a permissão foi revogada, reconecte nos Ajustes."))?;
        let t = json["access_token"].as_str().ok_or_else(|| anyhow!("O Google não devolveu o token de acesso."))?.to_string();
        *self.acesso.lock().unwrap() = Some((t.clone(), agora_s() + json["expires_in"].as_i64().unwrap_or(3600) - 60));
        Ok(t)
    }

    async fn get(&self, caminho: &str, consulta: &[(&str, &str)]) -> Result<reqwest::Response> {
        let token = self.token().await?;
        let res = self.http.get(format!("{}{caminho}", self.url_api)).bearer_auth(token).query(consulta).send().await?;
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status().as_u16();
        let json: serde_json::Value = res.json().await.unwrap_or_default();
        let msg = json["error"]["message"].as_str().unwrap_or("erro desconhecido");
        Err(match status {
            401 => anyhow!("O Google recusou o acesso (401). Reconecte o Drive nos Ajustes."),
            403 => anyhow!("Sem permissão para ler este arquivo no Drive (403): {msg}"),
            404 => anyhow!("Arquivo não encontrado no Drive."),
            _ => anyhow!("O Google Drive respondeu {status}: {msg}"),
        })
    }

    fn ler_arquivo(v: &serde_json::Value) -> Option<ArquivoDrive> {
        Some(ArquivoDrive {
            id: v["id"].as_str()?.to_string(),
            nome: v["name"].as_str().unwrap_or("Sem nome").to_string(),
            mime: v["mimeType"].as_str().unwrap_or("").to_string(),
            modificado: v["modifiedTime"].as_str().map(str::to_string),
        })
    }

    /// Procura por nome. Texto vazio lista os mais recentes. Um link ou ID do Drive abre direto o arquivo.
    pub async fn buscar(&self, texto: &str) -> Result<Vec<ArquivoDrive>> {
        let texto = texto.trim();
        let campos = "id,name,mimeType,modifiedTime";
        if let Some(id) = id_do_link(texto) {
            let json: serde_json::Value = self.get(&format!("/drive/v3/files/{id}"), &[("fields", campos), ("supportsAllDrives", "true")]).await?.json().await?;
            return Ok(Self::ler_arquivo(&json).into_iter().collect());
        }
        let mut q = format!("trashed = false and mimeType != '{GOOGLE_PASTA}'");
        if !texto.is_empty() {
            q += &format!(" and (name contains '{0}' or fullText contains '{0}')", texto_pergunta(texto));
        }
        let fields = format!("files({campos})");
        let json: serde_json::Value = self
            .get(
                "/drive/v3/files",
                &[("q", q.as_str()), ("fields", fields.as_str()), ("pageSize", "20"), ("orderBy", "modifiedTime desc"), ("supportsAllDrives", "true"), ("includeItemsFromAllDrives", "true")],
            )
            .await?
            .json()
            .await?;
        Ok(json["files"].as_array().map(|a| a.iter().filter_map(Self::ler_arquivo).collect()).unwrap_or_default())
    }

    /// Baixa o arquivo e devolve o documento com o texto extraído.
    pub async fn baixar(&self, id: &str) -> Result<Documento> {
        let meta: serde_json::Value = self
            .get(&format!("/drive/v3/files/{id}"), &[("fields", "id,name,mimeType,size"), ("supportsAllDrives", "true")])
            .await?
            .json()
            .await?;
        let arq = Self::ler_arquivo(&meta).ok_or_else(|| anyhow!("O Drive não devolveu os dados do arquivo."))?;
        let texto = match arq.mime.as_str() {
            GOOGLE_DOC | GOOGLE_SLIDES | GOOGLE_SHEET => {
                let formato = if arq.mime == GOOGLE_SHEET { "text/csv" } else { "text/plain" };
                let bytes = self.get(&format!("/drive/v3/files/{id}/export"), &[("mimeType", formato)]).await?.bytes().await?;
                extrair_texto(&format!("{}.txt", arq.nome), "text/plain", &bytes)?
            }
            m if m.starts_with("application/vnd.google-apps.") => {
                return Err(anyhow!("O tipo de arquivo do Google ({}) não pode ser lido como texto.", m.trim_start_matches("application/vnd.google-apps.")));
            }
            _ => {
                if meta["size"].as_str().and_then(|s| s.parse::<usize>().ok()).is_some_and(|s| s > LIMITE_ARQUIVO) {
                    return Err(anyhow!("{} é grande demais (limite de {} MB).", arq.nome, LIMITE_ARQUIVO / 1024 / 1024));
                }
                let bytes = self.get(&format!("/drive/v3/files/{id}"), &[("alt", "media"), ("supportsAllDrives", "true")]).await?.bytes().await?;
                extrair_texto(&arq.nome, &arq.mime, &bytes)?
            }
        };
        Ok(Documento { id: format!("drive:{}", arq.id), nome: arq.nome, origem: "drive".into(), texto })
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::whisper::testes::servidor_falso;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Memoria(Mutex<HashMap<String, String>>);
    impl Segredos for Memoria {
        fn ler(&self, n: &str) -> Option<String> {
            self.0.lock().unwrap().get(n).cloned()
        }
        fn gravar(&self, n: &str, v: &str) -> Result<()> {
            self.0.lock().unwrap().insert(n.into(), v.into());
            Ok(())
        }
    }

    fn drive(api: &str, token: &str) -> (Drive, Arc<Memoria>) {
        let m = Arc::new(Memoria::default());
        let d = Drive::com_urls(api, token, "https://exemplo.test/auth", m.clone(), Arc::new(|| ("cid".into(), "csec".into())));
        (d, m)
    }

    const TOKEN: &str = r#"{"access_token":"acesso-1","expires_in":3600}"#;

    #[test]
    fn extrai_id_de_links_do_google() {
        assert_eq!(id_do_link("https://docs.google.com/document/d/ABC123/edit?usp=sharing").as_deref(), Some("ABC123"));
        assert_eq!(id_do_link("https://drive.google.com/file/d/XYZ_9-/view").as_deref(), Some("XYZ_9-"));
        assert_eq!(id_do_link("https://drive.google.com/open?id=QQQ").as_deref(), Some("QQQ"));
        assert_eq!(id_do_link("contrato alfa"), None);
        assert_eq!(id_do_link("https://outro.com/d/ABC"), None);
    }

    #[test]
    fn pkce_confere_com_o_exemplo_da_rfc() {
        assert_eq!(desafio("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[tokio::test]
    async fn sem_credenciais_nao_tenta_conectar() {
        let m = Arc::new(Memoria::default());
        let d = Drive::new(m, Arc::new(|| (String::new(), String::new())));
        let e = d.conectar(|_| Ok(())).await.unwrap_err().to_string();
        assert!(e.contains("ID do cliente"), "{e}");
        assert!(!d.conectado());
    }

    #[tokio::test]
    async fn login_completo_guarda_a_permissao() {
        let (token_url, pedidos_token) = servidor_falso(200, r#"{"access_token":"a1","refresh_token":"r1","expires_in":3600}"#).await;
        let (d, segredos) = drive("http://127.0.0.1:1", &token_url_com_caminho(&token_url));
        // o "navegador" abre o endereço e volta ao retorno local com o código
        let abrir = |url: &str| -> Result<()> {
            let u = reqwest::Url::parse(url)?;
            assert_eq!(u.host_str(), Some("exemplo.test"));
            let q: HashMap<_, _> = u.query_pairs().into_owned().collect();
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["scope"], ESCOPO);
            assert_eq!(q["client_id"], "cid");
            let retorno = format!("{}/?code=COD&state={}", q["redirect_uri"], q["state"]);
            tokio::spawn(async move {
                let _ = reqwest::get(retorno).await;
            });
            Ok(())
        };
        d.conectar(abrir).await.unwrap();
        assert!(d.conectado());
        assert_eq!(segredos.ler(SEGREDO_REFRESH).as_deref(), Some("r1"));
        let p = pedidos_token.lock().unwrap()[0].clone();
        assert!(p.contains("grant_type=authorization_code") && p.contains("code=COD") && p.contains("code_verifier=") && p.contains("client_secret=csec"), "{p}");
        d.desconectar().unwrap();
        assert!(!d.conectado());
    }

    fn token_url_com_caminho(base: &str) -> String {
        format!("{base}/token")
    }

    #[tokio::test]
    async fn estado_errado_no_retorno_e_ignorado_ate_o_limite() {
        let (d, _) = drive("http://127.0.0.1:1", "http://127.0.0.1:1/token");
        let abrir = |url: &str| -> Result<()> {
            let u = reqwest::Url::parse(url)?;
            let redirect = u.query_pairs().find(|(k, _)| k == "redirect_uri").unwrap().1.into_owned();
            tokio::spawn(async move {
                let _ = reqwest::get(format!("{redirect}/?code=X&state=falso")).await;
            });
            Ok(())
        };
        let e = d.conectar_com_limite(abrir, Duration::from_millis(400)).await.unwrap_err().to_string();
        assert!(e.contains("Tempo esgotado"), "{e}");
        assert!(!d.conectado());
    }

    #[tokio::test]
    async fn busca_usa_o_token_renovado_e_le_os_arquivos() {
        let (token_url, pedidos_token) = servidor_falso(200, TOKEN).await;
        let (api, pedidos_api) = servidor_falso(200, r#"{"files":[{"id":"1","name":"Preços 2026","mimeType":"application/vnd.google-apps.document","modifiedTime":"2026-01-02T00:00:00Z"}]}"#).await;
        let (d, segredos) = drive(&api, &token_url);
        assert!(d.buscar("preços").await.unwrap_err().to_string().contains("não conectado"));
        segredos.gravar(SEGREDO_REFRESH, "r1").unwrap();
        let r = d.buscar("preço's").await.unwrap();
        assert_eq!(r[0].nome, "Preços 2026");
        d.buscar("outra").await.unwrap();
        assert_eq!(pedidos_token.lock().unwrap().len(), 1, "o token é reaproveitado");
        let p = pedidos_api.lock().unwrap()[0].clone();
        assert!(p.starts_with("GET /drive/v3/files?") && p.contains("Bearer acesso-1"));
        assert!(p.contains("preço%5C%27s") || p.contains("pre%C3%A7o%5C%27s"), "aspas escapadas: {p}");
        assert!(pedidos_token.lock().unwrap()[0].contains("grant_type=refresh_token"));
    }

    #[tokio::test]
    async fn erros_do_drive_viram_mensagens_claras() {
        let (token_url, _) = servidor_falso(200, TOKEN).await;
        let (api, _) = servidor_falso(404, r#"{"error":{"message":"File not found"}}"#).await;
        let (d, segredos) = drive(&api, &token_url);
        segredos.gravar(SEGREDO_REFRESH, "r1").unwrap();
        assert!(d.baixar("nada").await.unwrap_err().to_string().contains("não encontrado"));
    }

    #[tokio::test]
    async fn baixa_arquivo_de_texto_do_drive() {
        let (token_url, _) = servidor_falso(200, TOKEN).await;
        // o servidor falso devolve o mesmo corpo para qualquer rota: serve para os dois passos só com texto simples
        let (api, pedidos) = servidor_falso(200, r#"{"id":"9","name":"notas.txt","mimeType":"text/plain","size":"20"}"#).await;
        let (d, segredos) = drive(&api, &token_url);
        segredos.gravar(SEGREDO_REFRESH, "r1").unwrap();
        let doc = d.baixar("9").await.unwrap();
        assert_eq!(doc.id, "drive:9");
        assert_eq!(doc.origem, "drive");
        assert!(doc.texto.contains("notas.txt"));
        assert!(pedidos.lock().unwrap()[1].contains("alt=media"));
    }
}
