//! Resumo, perguntas e tradução pela API de mensagens da Anthropic (Claude), com chave da API.

use crate::summary::{conteudo_resumo, esquema, limpar_raciocinio, mensagem_pergunta, Resumidor, Resumo, INSTRUCOES, INSTRUCOES_PERGUNTA, INSTRUCOES_TRADUCAO, LIMITE_CARACTERES, LIMITE_PERGUNTA};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

type Parcial<'a> = &'a (dyn for<'b> Fn(&'b str) + Send + Sync);

const VERSAO_API: &str = "2023-06-01";
const MAX_TOKENS: u32 = 4096;

pub struct Claude {
    url_base: String,
    http: reqwest::Client,
}

impl Claude {
    pub fn new() -> Self {
        Self::com_url("https://api.anthropic.com")
    }

    pub fn com_url(url: impl Into<String>) -> Self {
        Self { url_base: url.into(), http: reqwest::Client::new() }
    }

    fn pedido(&self, corpo: &Value, chave: &str) -> reqwest::RequestBuilder {
        self.http.post(format!("{}/v1/messages", self.url_base)).header("x-api-key", chave).header("anthropic-version", VERSAO_API).json(corpo)
    }

    fn corpo(sistema: &str, usuario: &str, modelo: &str) -> Value {
        json!({"model": modelo, "max_tokens": MAX_TOKENS, "temperature": 0.2, "system": sistema, "messages": [{"role": "user", "content": usuario}]})
    }

    fn verificar_chave(chave: &str) -> Result<()> {
        if chave.is_empty() { Err(anyhow!("Chave do Claude não configurada.")) } else { Ok(()) }
    }

    async fn erro_http(res: reqwest::Response) -> anyhow::Error {
        let status = res.status().as_u16();
        let json: Value = res.json().await.unwrap_or_default();
        anyhow!("O Claude respondeu {status}: {}", json["error"]["message"].as_str().unwrap_or("erro desconhecido"))
    }

    /// Resposta inteira em texto.
    async fn texto(&self, corpo: &Value, chave: &str) -> Result<String> {
        let res = self.pedido(corpo, chave).send().await?;
        if !res.status().is_success() {
            return Err(Self::erro_http(res).await);
        }
        let json: Value = res.json().await?;
        let t: String = json["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
        match limpar_raciocinio(&t).trim() {
            "" => Err(anyhow!("O Claude não devolveu resposta.")),
            t => Ok(t.to_string()),
        }
    }

    /// Resposta em fluxo (eventos "data: {...}"): chama `parcial` com o texto acumulado.
    async fn fluxo(&self, corpo: &Value, chave: &str, parcial: Parcial<'_>) -> Result<String> {
        let mut corpo = corpo.clone();
        corpo["stream"] = json!(true);
        let mut res = self.pedido(&corpo, chave).send().await?;
        if !res.status().is_success() {
            return Err(Self::erro_http(res).await);
        }
        let (mut pendente, mut acumulado) = (String::new(), String::new());
        while let Some(pedaco) = res.chunk().await? {
            pendente.push_str(&String::from_utf8_lossy(&pedaco));
            while let Some(fim) = pendente.find('\n') {
                let linha: String = pendente.drain(..=fim).collect();
                let Some(dados) = linha.trim().strip_prefix("data:") else { continue };
                let Ok(v) = serde_json::from_str::<Value>(dados.trim()) else { continue };
                if v["type"] == "error" {
                    return Err(anyhow!("O Claude respondeu com erro: {}", v["error"]["message"].as_str().unwrap_or("erro desconhecido")));
                }
                if let Some(t) = v["delta"]["text"].as_str().filter(|_| v["type"] == "content_block_delta") {
                    acumulado.push_str(t);
                    parcial(limpar_raciocinio(&acumulado).trim_start());
                }
            }
        }
        match limpar_raciocinio(&acumulado).trim() {
            "" => Err(anyhow!("O Claude não devolveu resposta.")),
            t => Ok(t.to_string()),
        }
    }
}

impl Default for Claude {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Resumidor for Claude {
    async fn resumir(&self, transcricao: &str, material: &str, foco: &str, chave: &str, modelo: &str) -> Result<Resumo> {
        Self::verificar_chave(chave)?;
        if transcricao.trim().is_empty() {
            return Err(anyhow!("A transcrição está vazia."));
        }
        if transcricao.len() > LIMITE_CARACTERES {
            return Err(anyhow!("A transcrição é longa demais para resumir de uma vez."));
        }
        // o resumo vem como "chamada de ferramenta" obrigatória: é o jeito de a Anthropic garantir o formato
        let mut corpo = Self::corpo(INSTRUCOES, &conteudo_resumo(transcricao, material, foco), modelo);
        corpo["tools"] = json!([{"name": "resumo_reuniao", "description": "Registra o resumo da reunião.", "input_schema": esquema()["schema"]}]);
        corpo["tool_choice"] = json!({"type": "tool", "name": "resumo_reuniao"});
        let res = self.pedido(&corpo, chave).send().await?;
        if !res.status().is_success() {
            return Err(Self::erro_http(res).await);
        }
        let json: Value = res.json().await?;
        let entrada = json["content"].as_array().into_iter().flatten().find(|b| b["type"] == "tool_use").map(|b| b["input"].clone());
        match entrada {
            Some(e) => serde_json::from_value(e).map_err(|e| anyhow!("O Claude devolveu um resumo ilegível: {e}")),
            None => Err(anyhow!("O Claude não devolveu resumo.")),
        }
    }

    async fn responder(&self, pergunta: &str, material: &str, transcricao: &str, chave: &str, modelo: &str) -> Result<String> {
        Self::verificar_chave(chave)?;
        let usuario = mensagem_pergunta(pergunta, material, transcricao, LIMITE_PERGUNTA)?;
        self.texto(&Self::corpo(INSTRUCOES_PERGUNTA, &usuario, modelo), chave).await
    }

    async fn responder_parcial(&self, pergunta: &str, material: &str, transcricao: &str, chave: &str, modelo: &str, parcial: Parcial<'_>) -> Result<String> {
        Self::verificar_chave(chave)?;
        let usuario = mensagem_pergunta(pergunta, material, transcricao, LIMITE_PERGUNTA)?;
        self.fluxo(&Self::corpo(INSTRUCOES_PERGUNTA, &usuario, modelo), chave, parcial).await
    }

    async fn traduzir(&self, texto: &str, origem: &str, destino: &str, chave: &str, modelo: &str) -> Result<String> {
        Self::verificar_chave(chave)?;
        let sistema = INSTRUCOES_TRADUCAO.replace("{origem}", origem).replace("{destino}", destino);
        self.texto(&Self::corpo(&sistema, texto, modelo), chave).await
    }

    async fn traduzir_parcial(&self, texto: &str, origem: &str, destino: &str, chave: &str, modelo: &str, parcial: Parcial<'_>) -> Result<String> {
        Self::verificar_chave(chave)?;
        let sistema = INSTRUCOES_TRADUCAO.replace("{origem}", origem).replace("{destino}", destino);
        self.fluxo(&Self::corpo(&sistema, texto, modelo), chave, parcial).await
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::whisper::testes::servidor_falso;

    #[tokio::test]
    async fn resumo_vem_da_chamada_de_ferramenta() {
        let corpo = r#"{"content":[{"type":"tool_use","name":"resumo_reuniao","input":{"titulo":"T","resumo":"R","decisoes":["d"],"pendencias":[{"tarefa":"a","responsavel":null,"prazo":"sexta"}],"duvidas":[]}}]}"#;
        let (url, pedidos) = servidor_falso(200, corpo).await;
        let r = Claude::com_url(url).resumir("[00:00:00] Eu: oi", "", "Foco: prazos", "ant-1", "claude-x").await.unwrap();
        assert_eq!((r.titulo.as_str(), r.pendencias[0].prazo.as_deref()), ("T", Some("sexta")));
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.starts_with("POST /v1/messages") && p.to_lowercase().contains("x-api-key: ant-1") && p.contains("anthropic-version"));
        assert!(p.contains("tool_choice") && p.contains("resumo_reuniao") && p.contains("Foco: prazos"));
    }

    #[tokio::test]
    async fn responde_traduz_e_valida() {
        let (url, pedidos) = servidor_falso(200, r#"{"content":[{"type":"text","text":" O prazo é sexta. "}]}"#).await;
        let c = Claude::com_url(url);
        assert_eq!(c.responder("Qual o prazo?", "Prazo: sexta", "", "k", "m").await.unwrap(), "O prazo é sexta.");
        assert_eq!(c.traduzir("Good morning", "inglês", "português", "k", "m").await.unwrap(), "O prazo é sexta.");
        assert!(pedidos.lock().unwrap()[1].contains("de inglês para português"));
        assert!(c.responder("a?", "", "", "", "m").await.unwrap_err().to_string().contains("Chave"));
        assert!(c.responder(" ", "", "", "k", "m").await.unwrap_err().to_string().contains("pergunta"));
        assert!(c.resumir("  ", "", "", "k", "m").await.unwrap_err().to_string().contains("vazia"));
        let (url, _) = servidor_falso(401, r#"{"error":{"message":"chave inválida"}}"#).await;
        let e = Claude::com_url(url).responder("a?", "", "", "k", "m").await.unwrap_err().to_string();
        assert!(e.contains("401") && e.contains("chave inválida"), "{e}");
    }

    #[tokio::test]
    async fn fluxo_mostra_aos_poucos() {
        let corpo = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Bom\"}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" dia\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n";
        let (url, pedidos) = servidor_falso(200, corpo).await;
        let vistos = std::sync::Mutex::new(vec![]);
        let r = Claude::com_url(url).traduzir_parcial("Good morning", "inglês", "português", "k", "m", &|t| vistos.lock().unwrap().push(t.to_string())).await.unwrap();
        assert_eq!(r, "Bom dia");
        assert_eq!(*vistos.lock().unwrap(), vec!["Bom", "Bom dia"]);
        assert!(pedidos.lock().unwrap()[0].contains("\"stream\":true"));
        let (url, _) = servidor_falso(200, "data: {\"type\":\"error\",\"error\":{\"message\":\"sobrecarregado\"}}\n\n").await;
        let e = Claude::com_url(url).responder_parcial("a?", "", "", "k", "m", &|_| {}).await.unwrap_err().to_string();
        assert!(e.contains("sobrecarregado"), "{e}");
    }
}
