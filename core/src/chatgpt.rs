//! Entrar com a conta do ChatGPT: usa a assinatura da pessoa em vez de uma chave da API.
//!
//! O login é o mesmo fluxo do Codex CLI da OpenAI (OAuth com PKCE e retorno em localhost:1455) e as
//! respostas vêm do endpoint do Codex (API Responses). Não é uma API pública documentada: a OpenAI
//! pode mudar ou bloquear. Serve só para texto (resumo, perguntas e tradução); a transcrição de áudio
//! continua precisando da chave da API ou do modelo local.

use crate::drive::{agora_s, aleatorio, desafio, esperar_codigo};
use crate::settings::{Segredos, SEGREDO_CHATGPT};
use crate::summary::{conteudo_resumo, esquema, mensagem_pergunta, Resumidor, Resumo, INSTRUCOES, INSTRUCOES_PERGUNTA, INSTRUCOES_TRADUCAO, LIMITE_CARACTERES, LIMITE_PERGUNTA};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// ID público do aplicativo Codex CLI na OpenAI (o login do ChatGPT só aceita clientes registrados).
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const PORTA: u16 = 1455;
const ESCOPO: &str = "openid profile email offline_access";

type Parcial<'a> = &'a (dyn for<'b> Fn(&'b str) + Send + Sync);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Credencial {
    acesso: String,
    refresh: String,
    /// ID da conta/plano do ChatGPT, exigido num cabeçalho de cada pedido.
    conta: String,
    email: String,
    /// Quando o token de acesso vence (segundos desde 1970).
    vence: i64,
}

/// Lê as informações (claims) de um JWT sem conferir a assinatura: só para saber conta, e-mail e validade.
fn claims(jwt: &str) -> Value {
    jwt.split('.')
        .nth(1)
        .and_then(|p| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// E-mail da conta conectada, a partir do que está guardado no cofre (para mostrar nos Ajustes).
pub fn conta_de(guardado: &str) -> Option<String> {
    serde_json::from_str::<Credencial>(guardado).ok().map(|c| c.email).filter(|e| !e.is_empty())
}

pub struct ChatGpt {
    url_auth: String,
    url_token: String,
    url_api: String,
    http: reqwest::Client,
    segredos: Arc<dyn Segredos>,
    /// Credencial em uso. O cadeado fica preso durante a renovação, para só um pedido renovar por vez.
    cache: tokio::sync::Mutex<Option<Credencial>>,
}

impl ChatGpt {
    pub fn new(segredos: Arc<dyn Segredos>) -> Self {
        Self::com_urls("https://auth.openai.com/oauth/authorize", "https://auth.openai.com/oauth/token", "https://chatgpt.com/backend-api/codex", segredos)
    }

    pub fn com_urls(auth: &str, token: &str, api: &str, segredos: Arc<dyn Segredos>) -> Self {
        Self { url_auth: auth.into(), url_token: token.into(), url_api: api.into(), http: reqwest::Client::new(), segredos, cache: tokio::sync::Mutex::new(None) }
    }

    pub fn conectado(&self) -> bool {
        self.segredos.ler(SEGREDO_CHATGPT).is_some_and(|t| !t.is_empty())
    }

    pub async fn desconectar(&self) -> Result<()> {
        let mut c = self.cache.lock().await;
        *c = None;
        self.segredos.gravar(SEGREDO_CHATGPT, "")
    }

    fn guardar(&self, c: &Credencial) -> Result<()> {
        self.segredos.gravar(SEGREDO_CHATGPT, &serde_json::to_string(c)?)
    }

    /// Monta a credencial a partir da resposta do servidor de login (`anterior` completa o que faltar na renovação).
    fn credencial_de(json: &Value, anterior: Option<&Credencial>) -> Result<Credencial> {
        let acesso = json["access_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| anyhow!("O ChatGPT não devolveu o token de acesso."))?;
        let refresh = json["refresh_token"].as_str().filter(|t| !t.is_empty()).or(anterior.map(|a| a.refresh.as_str())).ok_or_else(|| anyhow!("O ChatGPT não devolveu a permissão de acesso contínuo. Tente entrar de novo."))?;
        let (do_id, do_acesso) = (claims(json["id_token"].as_str().unwrap_or("")), claims(acesso));
        let conta = [&do_id, &do_acesso]
            .iter()
            .find_map(|c| c["https://api.openai.com/auth"]["chatgpt_account_id"].as_str().filter(|s| !s.is_empty()).map(str::to_owned))
            .or_else(|| anterior.map(|a| a.conta.clone()).filter(|s| !s.is_empty()))
            .ok_or_else(|| anyhow!("Esta conta do ChatGPT não tem um plano com acesso ao Codex (é preciso uma assinatura paga)."))?;
        let email = do_id["email"].as_str().map(str::to_owned).or_else(|| anterior.map(|a| a.email.clone())).unwrap_or_default();
        let vence = json["expires_in"].as_i64().map(|s| agora_s() + s).or_else(|| do_acesso["exp"].as_i64()).unwrap_or_else(|| agora_s() + 3600) - 60;
        Ok(Credencial { acesso: acesso.into(), refresh: refresh.into(), conta, email, vence })
    }

    async fn pedir_token(&self, req: reqwest::RequestBuilder) -> Result<Value> {
        let res = req.send().await?;
        let status = res.status();
        let json: Value = res.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = json["error_description"].as_str().or(json["error"]["message"].as_str()).or(json["error"].as_str()).unwrap_or("erro desconhecido");
            return Err(anyhow!("O login do ChatGPT respondeu {}: {msg}", status.as_u16()));
        }
        Ok(json)
    }

    /// Abre o login no navegador (`abrir` recebe o endereço) e guarda a permissão.
    pub async fn conectar(&self, abrir: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        self.conectar_com(abrir, PORTA, Duration::from_secs(180)).await
    }

    async fn conectar_com(&self, abrir: impl FnOnce(&str) -> Result<()>, porta: u16, limite: Duration) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", porta))
            .await
            .map_err(|e| anyhow!("Não consegui abrir a porta {porta} para o login ({e}). Feche o Codex CLI ou outro login em andamento e tente de novo."))?;
        let redirect = format!("http://localhost:{}/auth/callback", listener.local_addr()?.port());
        let (verificador, estado) = (aleatorio(), aleatorio());
        let url = reqwest::Url::parse_with_params(
            &self.url_auth,
            &[
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", redirect.as_str()),
                ("scope", ESCOPO),
                ("code_challenge", desafio(&verificador).as_str()),
                ("code_challenge_method", "S256"),
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
                ("state", estado.as_str()),
                ("originator", "codex_cli_rs"),
            ],
        )?;
        abrir(url.as_str())?;
        let codigo = esperar_codigo(listener, &estado, limite, "ChatGPT").await?;
        let json = self
            .pedir_token(self.http.post(&self.url_token).form(&[
                ("grant_type", "authorization_code"),
                ("code", codigo.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("client_id", CLIENT_ID),
                ("code_verifier", verificador.as_str()),
            ]))
            .await?;
        let cred = Self::credencial_de(&json, None)?;
        self.guardar(&cred)?;
        *self.cache.lock().await = Some(cred);
        Ok(())
    }

    /// Credencial com token válido; renova (e guarda) quando venceu.
    async fn credencial(&self) -> Result<Credencial> {
        let mut g = self.cache.lock().await;
        if g.is_none() {
            *g = self.segredos.ler(SEGREDO_CHATGPT).and_then(|t| serde_json::from_str(&t).ok());
        }
        let mut c = g.clone().ok_or_else(|| anyhow!("ChatGPT não conectado. Entre com o ChatGPT nos Ajustes."))?;
        if c.vence <= agora_s() {
            let json = self
                .pedir_token(self.http.post(&self.url_token).json(&json!({
                    "client_id": CLIENT_ID,
                    "grant_type": "refresh_token",
                    "refresh_token": c.refresh,
                    "scope": "openid profile email",
                })))
                .await
                .map_err(|e| anyhow!("{e}. Entre com o ChatGPT de novo nos Ajustes."))?;
            c = Self::credencial_de(&json, Some(&c))?;
            self.guardar(&c)?;
            *g = Some(c.clone());
        }
        Ok(c)
    }

    /// Pede o texto ao modelo (sempre em fluxo, como o endpoint exige) e chama `parcial` com o acumulado.
    async fn gerar(&self, instrucoes: &str, usuario: &str, formato: Option<Value>, modelo: &str, parcial: Option<Parcial<'_>>) -> Result<String> {
        let cred = self.credencial().await?;
        let mut corpo = json!({
            "model": modelo,
            "instructions": instrucoes,
            "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": usuario}]}],
            "stream": true,
            "store": false,
        });
        if let Some(mut f) = formato {
            f["type"] = json!("json_schema");
            corpo["text"] = json!({ "format": f });
        }
        let mut res = self
            .http
            .post(format!("{}/responses", self.url_api))
            .bearer_auth(&cred.acesso)
            .header("chatgpt-account-id", &cred.conta)
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "codex_cli_rs")
            .header("accept", "text/event-stream")
            .json(&corpo)
            .send()
            .await?;
        let status = res.status();
        if !status.is_success() {
            let texto = res.text().await.unwrap_or_default();
            let json: Value = serde_json::from_str(&texto).unwrap_or_default();
            let msg = json["error"]["message"].as_str().or(json["detail"].as_str()).map(str::to_owned).unwrap_or_else(|| texto.chars().take(200).collect());
            return Err(match status.as_u16() {
                401 => anyhow!("O ChatGPT recusou o acesso (401). Entre com o ChatGPT de novo nos Ajustes."),
                429 => anyhow!("O limite de uso da sua assinatura do ChatGPT foi atingido (429): {msg}"),
                s => anyhow!("O ChatGPT respondeu {s}: {msg}"),
            });
        }
        let (mut pendente, mut acumulado) = (String::new(), String::new());
        while let Some(pedaco) = res.chunk().await? {
            pendente.push_str(&String::from_utf8_lossy(&pedaco));
            while let Some(fim) = pendente.find('\n') {
                let linha: String = pendente.drain(..=fim).collect();
                let Some(dados) = linha.trim().strip_prefix("data:") else { continue };
                let Ok(v) = serde_json::from_str::<Value>(dados.trim()) else { continue };
                match v["type"].as_str() {
                    Some("response.output_text.delta") => {
                        if let Some(d) = v["delta"].as_str() {
                            acumulado.push_str(d);
                            if let Some(p) = parcial {
                                p(acumulado.trim_start());
                            }
                        }
                    }
                    Some("response.failed") | Some("error") => {
                        let msg = v["response"]["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or("erro desconhecido");
                        return Err(anyhow!("O ChatGPT falhou: {msg}"));
                    }
                    _ => {}
                }
            }
        }
        match acumulado.trim() {
            "" => Err(anyhow!("O ChatGPT não devolveu resposta.")),
            t => Ok(t.to_string()),
        }
    }
}

#[async_trait]
impl Resumidor for ChatGpt {
    async fn resumir(&self, transcricao: &str, material: &str, _chave: &str, modelo: &str) -> Result<Resumo> {
        if transcricao.trim().is_empty() {
            return Err(anyhow!("A transcrição está vazia."));
        }
        if transcricao.len() > LIMITE_CARACTERES {
            return Err(anyhow!("A transcrição é longa demais para resumir de uma vez."));
        }
        let texto = self.gerar(INSTRUCOES, &conteudo_resumo(transcricao, material), Some(esquema()), modelo, None).await?;
        serde_json::from_str(&texto).map_err(|e| anyhow!("O ChatGPT devolveu um resumo ilegível: {e}"))
    }

    async fn responder(&self, pergunta: &str, material: &str, transcricao: &str, chave: &str, modelo: &str) -> Result<String> {
        self.responder_parcial(pergunta, material, transcricao, chave, modelo, &|_| {}).await
    }

    async fn responder_parcial(&self, pergunta: &str, material: &str, transcricao: &str, _chave: &str, modelo: &str, parcial: Parcial<'_>) -> Result<String> {
        let usuario = mensagem_pergunta(pergunta, material, transcricao, LIMITE_PERGUNTA)?;
        self.gerar(INSTRUCOES_PERGUNTA, &usuario, None, modelo, Some(parcial)).await
    }

    async fn traduzir(&self, texto: &str, origem: &str, destino: &str, chave: &str, modelo: &str) -> Result<String> {
        self.traduzir_parcial(texto, origem, destino, chave, modelo, &|_| {}).await
    }

    async fn traduzir_parcial(&self, texto: &str, origem: &str, destino: &str, _chave: &str, modelo: &str, parcial: Parcial<'_>) -> Result<String> {
        let instrucoes = INSTRUCOES_TRADUCAO.replace("{origem}", origem).replace("{destino}", destino);
        self.gerar(&instrucoes, texto, None, modelo, Some(parcial)).await
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::whisper::testes::servidor_falso;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Memoria(Mutex<HashMap<String, String>>);
    impl Segredos for Memoria {
        fn ler(&self, n: &str) -> Option<String> { self.0.lock().unwrap().get(n).cloned() }
        fn gravar(&self, n: &str, v: &str) -> Result<()> { self.0.lock().unwrap().insert(n.into(), v.into()); Ok(()) }
    }

    fn jwt(payload: Value) -> String {
        let b = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        format!("{}.{}.assinatura", b(b"{}"), b(payload.to_string().as_bytes()))
    }

    fn tokens(conta: &str) -> &'static str {
        let id = jwt(json!({"email": "eu@exemplo.com", "https://api.openai.com/auth": {"chatgpt_account_id": conta}}));
        Box::leak(format!(r#"{{"access_token":"acc1","refresh_token":"ref1","id_token":"{id}","expires_in":3600}}"#).into_boxed_str())
    }

    fn cliente(auth: &str, token: &str, api: &str) -> (ChatGpt, Arc<Memoria>) {
        let m = Arc::new(Memoria::default());
        (ChatGpt::com_urls(auth, token, api, m.clone()), m)
    }

    fn guardar(m: &Memoria, vence: i64) {
        let c = Credencial { acesso: "acc0".into(), refresh: "ref0".into(), conta: "conta-1".into(), email: "eu@exemplo.com".into(), vence };
        m.gravar(SEGREDO_CHATGPT, &serde_json::to_string(&c).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn login_completo_guarda_conta_e_permissao() {
        let (token_url, pedidos) = servidor_falso(200, tokens("conta-9")).await;
        let (c, m) = cliente("https://auth.exemplo.test/authorize", &format!("{token_url}/token"), "http://127.0.0.1:1");
        let abrir = |url: &str| -> Result<()> {
            let u = reqwest::Url::parse(url)?;
            assert_eq!(u.host_str(), Some("auth.exemplo.test"));
            let q: HashMap<_, _> = u.query_pairs().into_owned().collect();
            assert_eq!(q["client_id"], CLIENT_ID);
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["scope"], ESCOPO);
            assert!(q["redirect_uri"].starts_with("http://localhost:") && q["redirect_uri"].ends_with("/auth/callback"));
            let retorno = format!("{}?code=COD&state={}", q["redirect_uri"].replace("localhost", "127.0.0.1"), q["state"]);
            tokio::spawn(async move {
                let _ = reqwest::get(retorno).await;
            });
            Ok(())
        };
        c.conectar_com(abrir, 0, Duration::from_secs(5)).await.unwrap();
        assert!(c.conectado());
        let guardado = m.ler(SEGREDO_CHATGPT).unwrap();
        assert_eq!(conta_de(&guardado).as_deref(), Some("eu@exemplo.com"));
        let cred: Credencial = serde_json::from_str(&guardado).unwrap();
        assert_eq!((cred.acesso.as_str(), cred.refresh.as_str(), cred.conta.as_str()), ("acc1", "ref1", "conta-9"));
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.contains("grant_type=authorization_code") && p.contains("code=COD") && p.contains("code_verifier=") && p.contains(CLIENT_ID), "{p}");
        c.desconectar().await.unwrap();
        assert!(!c.conectado());
    }

    #[tokio::test]
    async fn conta_sem_plano_e_recusada_com_mensagem_clara() {
        let json = json!({"access_token": jwt(json!({})), "refresh_token": "r"});
        let e = ChatGpt::credencial_de(&json, None).unwrap_err().to_string();
        assert!(e.contains("assinatura"), "{e}");
    }

    #[tokio::test]
    async fn token_vencido_e_renovado_e_guardado() {
        let (token_url, pedidos) = servidor_falso(200, tokens("conta-1")).await;
        let (c, m) = cliente("x", &format!("{token_url}/token"), "http://127.0.0.1:1");
        guardar(&m, 0);
        let cred = c.credencial().await.unwrap();
        assert_eq!(cred.acesso, "acc1");
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.contains("\"grant_type\":\"refresh_token\"") && p.contains("ref0"), "{p}");
        assert!(m.ler(SEGREDO_CHATGPT).unwrap().contains("acc1"));
        // ainda válido: não renova de novo
        c.credencial().await.unwrap();
        assert_eq!(pedidos.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn sem_login_pede_para_entrar() {
        let (c, _) = cliente("x", "http://127.0.0.1:1/token", "http://127.0.0.1:1");
        let e = c.responder("oi?", "", "", "", "m").await.unwrap_err().to_string();
        assert!(e.contains("Entre com o ChatGPT"), "{e}");
    }

    #[tokio::test]
    async fn responde_em_fluxo_com_cabecalhos_do_codex() {
        let corpo = "data: {\"type\":\"response.created\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"O prazo\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\" é sexta.\"}\n\ndata: {\"type\":\"response.completed\"}\n\n";
        let (api, pedidos) = servidor_falso(200, corpo).await;
        let (c, m) = cliente("x", "http://127.0.0.1:1/token", &api);
        guardar(&m, agora_s() + 3600);
        let vistos = Mutex::new(vec![]);
        let r = c.responder_parcial("Qual o prazo?", "Prazo: sexta", "", "", "gpt-5", &|t| vistos.lock().unwrap().push(t.to_string())).await.unwrap();
        assert_eq!(r, "O prazo é sexta.");
        assert_eq!(*vistos.lock().unwrap(), vec!["O prazo", "O prazo é sexta."]);
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.starts_with("POST /responses"), "{p}");
        let minusculo = p.to_lowercase();
        assert!(minusculo.contains("authorization: bearer acc0") && minusculo.contains("chatgpt-account-id: conta-1"), "{p}");
        assert!(p.contains("\"stream\":true") && p.contains("\"store\":false") && p.contains("input_text") && p.contains("Qual o prazo?") && p.contains("\"model\":\"gpt-5\""), "{p}");
    }

    #[tokio::test]
    async fn resumo_usa_o_esquema_e_le_o_json() {
        let json_resumo = r#"{\"titulo\":\"T\",\"resumo\":\"R\",\"decisoes\":[],\"pendencias\":[],\"duvidas\":[]}"#;
        let corpo: &'static str = Box::leak(format!("data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{json_resumo}\"}}\n\n").into_boxed_str());
        let (api, pedidos) = servidor_falso(200, corpo).await;
        let (c, m) = cliente("x", "http://127.0.0.1:1/token", &api);
        guardar(&m, agora_s() + 3600);
        let r = c.resumir("[00:00:00] Eu: oi", "", "", "gpt-5").await.unwrap();
        assert_eq!(r.titulo, "T");
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.contains("\"json_schema\"") && p.contains("resumo_reuniao"), "{p}");
        assert!(c.resumir("  ", "", "", "m").await.unwrap_err().to_string().contains("vazia"));
    }

    #[tokio::test]
    async fn traduz_e_erros_viram_mensagens_claras() {
        let corpo = "data: {\"type\":\"response.output_text.delta\",\"delta\":\" Bom dia \"}\n\n";
        let (api, pedidos) = servidor_falso(200, corpo).await;
        let (c, m) = cliente("x", "http://127.0.0.1:1/token", &api);
        guardar(&m, agora_s() + 3600);
        assert_eq!(c.traduzir("Good morning", "inglês", "português", "", "m").await.unwrap(), "Bom dia");
        assert!(pedidos.lock().unwrap()[0].contains("de inglês para português"));

        for (status, corpo, esperado) in [(401, "{}", "Entre com o ChatGPT de novo"), (429, r#"{"error":{"message":"muitas"}}"#, "limite de uso"), (500, r#"{"detail":"quebrou"}"#, "quebrou")] {
            let (api, _) = servidor_falso(status, corpo).await;
            let (c, m) = cliente("x", "http://127.0.0.1:1/token", &api);
            guardar(&m, agora_s() + 3600);
            let e = c.responder("a?", "", "", "", "m").await.unwrap_err().to_string();
            assert!(e.contains(esperado), "{status}: {e}");
        }
        let (api, _) = servidor_falso(200, "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"modelo inválido\"}}}\n\n").await;
        let (c, m) = cliente("x", "http://127.0.0.1:1/token", &api);
        guardar(&m, agora_s() + 3600);
        assert!(c.responder("a?", "", "", "", "m").await.unwrap_err().to_string().contains("modelo inválido"));
    }
}
