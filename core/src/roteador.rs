//! Escolhe, a cada chamada, entre a OpenAI (chave da API ou login do ChatGPT) e o modelo local, conforme os Ajustes.

use crate::chatgpt::ChatGpt;
use crate::claude::Claude;
use crate::locais::Locais;
use crate::servidor::{PORTA_LLM, PORTA_WHISPER};
use crate::settings::Config;
use crate::summary::{OpenAi, Resumidor, Resumo};
use crate::transcricao::TranscricaoOpenAi;
use crate::whisper::{ClienteWhisper, Transcritor};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

type LerConfig = Arc<dyn Fn() -> Config + Send + Sync>;

const DICA: &str = "Em Ajustes, escolha a OpenAI para usar a nuvem.";

pub struct TranscritorRoteador {
    nuvem: TranscricaoOpenAi,
    local: ClienteWhisper,
    locais: Arc<Locais>,
    config: LerConfig,
}

impl TranscritorRoteador {
    pub fn new(nuvem: TranscricaoOpenAi, locais: Arc<Locais>, config: LerConfig) -> Self {
        let cfg = config.clone();
        let local = ClienteWhisper::new(format!("http://127.0.0.1:{PORTA_WHISPER}"), "pt").com_idioma(Arc::new(move || cfg().idioma));
        Self { nuvem, local, locais, config }
    }
}

#[async_trait]
impl Transcritor for TranscritorRoteador {
    async fn transcrever(&self, pcm: &[i16]) -> Result<String> {
        let idioma = (self.config)().idioma;
        self.transcrever_em(pcm, &idioma).await
    }

    async fn transcrever_em(&self, pcm: &[i16], idioma: &str) -> Result<String> {
        self.transcrever_dica(pcm, idioma, "").await
    }

    async fn transcrever_dica(&self, pcm: &[i16], idioma: &str, dica: &str) -> Result<String> {
        let c = (self.config)();
        if !c.transcricao_local {
            return self.nuvem.transcrever_dica(pcm, idioma, dica).await;
        }
        self.locais.garantir_whisper(&c).await.map_err(|e| anyhow!("{e} {DICA}"))?;
        self.local.transcrever_dica(pcm, idioma, dica).await.map_err(|e| anyhow!("Transcrição local falhou: {e}. {DICA}"))
    }
}

/// Quem vai atender um pedido, escolhido pelos Ajustes: o modelo local, outro provedor, o Claude,
/// o login do ChatGPT ou a chave da OpenAI.
struct Alvo {
    quem: Arc<dyn Resumidor>,
    chave: String,
    modelo: String,
    local: bool,
}

pub struct ResumidorRoteador {
    nuvem: Arc<OpenAi>,
    chatgpt: Arc<ChatGpt>,
    claude: Arc<Claude>,
    local: Arc<OpenAi>,
    /// Cliente do provedor compatível, refeito só quando o endereço muda.
    compativel: Mutex<Option<(String, Arc<OpenAi>)>>,
    locais: Arc<Locais>,
    config: LerConfig,
}

impl ResumidorRoteador {
    pub fn new(locais: Arc<Locais>, config: LerConfig, chatgpt: Arc<ChatGpt>) -> Self {
        Self {
            nuvem: Arc::new(OpenAi::new()), chatgpt, claude: Arc::new(Claude::new()),
            local: Arc::new(OpenAi::local(format!("http://127.0.0.1:{PORTA_LLM}/v1"))),
            compativel: Mutex::new(None), locais, config,
        }
    }

    fn compativel(&self, url: &str) -> Arc<OpenAi> {
        let mut g = self.compativel.lock().unwrap();
        match g.as_ref() {
            Some((u, c)) if u == url => c.clone(),
            _ => {
                let c = Arc::new(OpenAi::compativel(url));
                *g = Some((url.to_string(), c.clone()));
                c
            }
        }
    }

    /// Escolhe quem atende e prepara o que ele precisa (o modelo local é ligado aqui).
    async fn alvo(&self, chave: &str, modelo: &str) -> Result<Alvo> {
        let c = (self.config)();
        if c.analise_local {
            self.locais.garantir_llm(&c).await.map_err(|e| anyhow!("{e} {DICA}"))?;
            return Ok(Alvo { quem: self.local.clone(), chave: String::new(), modelo: "local".into(), local: true });
        }
        Ok(match c.provedor.as_str() {
            "compat" => {
                if c.compat_url.is_empty() || c.compat_model.is_empty() {
                    return Err(anyhow!("Falta o endereço ou o modelo do provedor. Veja em Ajustes."));
                }
                Alvo { quem: self.compativel(&c.compat_url), chave: c.compat_key, modelo: c.compat_model, local: false }
            }
            "claude" => Alvo { quem: self.claude.clone(), chave: c.claude_key, modelo: c.claude_model, local: false },
            _ if c.chatgpt_conectado => Alvo { quem: self.chatgpt.clone(), chave: chave.into(), modelo: c.chatgpt_model, local: false },
            _ => Alvo { quem: self.nuvem.clone(), chave: chave.into(), modelo: modelo.into(), local: false },
        })
    }
}

fn com_dica(local: bool, e: anyhow::Error) -> anyhow::Error {
    if local { anyhow!("{e}. {DICA}") } else { e }
}

#[async_trait]
impl Resumidor for ResumidorRoteador {
    async fn resumir(&self, transcricao: &str, material: &str, foco: &str, chave: &str, modelo: &str) -> Result<Resumo> {
        let a = self.alvo(chave, modelo).await?;
        a.quem.resumir(transcricao, material, foco, &a.chave, &a.modelo).await.map_err(|e| com_dica(a.local, e))
    }

    async fn responder(&self, pergunta: &str, material: &str, transcricao: &str, chave: &str, modelo: &str) -> Result<String> {
        let a = self.alvo(chave, modelo).await?;
        a.quem.responder(pergunta, material, transcricao, &a.chave, &a.modelo).await.map_err(|e| com_dica(a.local, e))
    }

    async fn responder_parcial(&self, pergunta: &str, material: &str, transcricao: &str, chave: &str, modelo: &str, parcial: &(dyn for<'a> Fn(&'a str) + Send + Sync)) -> Result<String> {
        let a = self.alvo(chave, modelo).await?;
        a.quem.responder_parcial(pergunta, material, transcricao, &a.chave, &a.modelo, parcial).await.map_err(|e| com_dica(a.local, e))
    }

    async fn traduzir(&self, texto: &str, origem: &str, destino: &str, chave: &str, modelo: &str) -> Result<String> {
        let a = self.alvo(chave, modelo).await?;
        a.quem.traduzir(texto, origem, destino, &a.chave, &a.modelo).await.map_err(|e| com_dica(a.local, e))
    }

    async fn traduzir_parcial(&self, texto: &str, origem: &str, destino: &str, chave: &str, modelo: &str, parcial: &(dyn for<'a> Fn(&'a str) + Send + Sync)) -> Result<String> {
        let a = self.alvo(chave, modelo).await?;
        a.quem.traduzir_parcial(texto, origem, destino, &a.chave, &a.modelo, parcial).await.map_err(|e| com_dica(a.local, e))
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    struct SemSegredos;
    impl crate::settings::Segredos for SemSegredos {
        fn ler(&self, _: &str) -> Option<String> { None }
        fn gravar(&self, _: &str, _: &str) -> Result<()> { Ok(()) }
    }

    fn chatgpt() -> Arc<ChatGpt> {
        Arc::new(ChatGpt::new(Arc::new(SemSegredos)))
    }

    fn locais() -> (Arc<Locais>, PathBuf) {
        let raiz = std::env::temp_dir().join(format!("rot-{}", uuid::Uuid::new_v4()));
        (Locais::com_urls(raiz.clone(), Arc::new(|_, _| {}), "http://127.0.0.1:1", "http://127.0.0.1:1", Duration::ZERO), raiz)
    }

    #[tokio::test]
    async fn modo_local_sem_modelo_baixado_explica_e_oferece_voltar_para_a_openai() {
        let (l, raiz) = locais();
        let cfg = Config { analise_local: true, transcricao_local: true, ..Default::default() };
        let c2 = cfg.clone();
        let config: LerConfig = Arc::new(move || c2.clone());

        let r = ResumidorRoteador::new(l.clone(), config.clone(), chatgpt());
        let e = r.resumir("[00:00:00] Eu: oi", "", "", "", "m").await.unwrap_err().to_string();
        assert!(e.contains("ainda não foi baixado") && e.contains("escolha a OpenAI"), "{e}");
        let e = r.responder("?", "", "", "", "m").await.unwrap_err().to_string();
        assert!(e.contains("escolha a OpenAI"), "{e}");

        let nuvem = TranscricaoOpenAi::new(Arc::new(|| (String::new(), String::new())));
        let t = TranscritorRoteador::new(nuvem, l, config);
        let e = t.transcrever(&[0i16; 8]).await.unwrap_err().to_string();
        assert!(e.contains("ainda não foi baixado") && e.contains("escolha a OpenAI"), "{e}");
        let _ = std::fs::remove_dir_all(raiz);
    }

    #[tokio::test]
    async fn modo_nuvem_nao_toca_nos_servidores_locais() {
        let (l, raiz) = locais();
        let config: LerConfig = Arc::new(Config::default);
        let r = ResumidorRoteador::new(l, config, chatgpt());
        // sem chave, quem responde é o cliente da OpenAI
        let e = r.resumir("[00:00:00] Eu: oi", "", "", "", "m").await.unwrap_err().to_string();
        assert!(e.contains("Chave da OpenAI"), "{e}");
        let _ = std::fs::remove_dir_all(raiz);
    }

    #[tokio::test]
    async fn provedor_escolhido_nos_ajustes_recebe_o_pedido() {
        use crate::whisper::testes::servidor_falso;
        let (l, raiz) = locais();
        let (url, pedidos) = servidor_falso(200, r#"{"choices":[{"message":{"content":"<think>x</think> Bom dia "}}]}"#).await;
        let cfg = Config { provedor: "compat".into(), compat_url: url, compat_model: "llama-x".into(), compat_key: "gsk-1".into(), ..Default::default() };
        let config: LerConfig = Arc::new(move || cfg.clone());
        let r = ResumidorRoteador::new(l.clone(), config, chatgpt());
        assert_eq!(r.traduzir("Good morning", "inglês", "português", "sk-openai", "gpt-4o-mini").await.unwrap(), "Bom dia");
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.contains("Bearer gsk-1") && p.contains("llama-x") && !p.contains("sk-openai"), "{p}");

        let sem: LerConfig = Arc::new(|| Config { provedor: "compat".into(), ..Default::default() });
        let e = ResumidorRoteador::new(l.clone(), sem, chatgpt()).responder("a?", "", "", "", "m").await.unwrap_err().to_string();
        assert!(e.contains("endereço ou o modelo"), "{e}");
        let claude: LerConfig = Arc::new(|| Config { provedor: "claude".into(), ..Default::default() });
        let e = ResumidorRoteador::new(l, claude, chatgpt()).responder("a?", "", "", "", "m").await.unwrap_err().to_string();
        assert!(e.contains("Claude"), "{e}");
        let _ = std::fs::remove_dir_all(raiz);
    }
}
