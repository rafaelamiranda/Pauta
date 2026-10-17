//! Escolhe, a cada chamada, entre a OpenAI (chave da API ou login do ChatGPT) e o modelo local, conforme os Ajustes.

use crate::chatgpt::ChatGpt;
use crate::locais::Locais;
use crate::servidor::{PORTA_LLM, PORTA_WHISPER};
use crate::settings::Config;
use crate::summary::{OpenAi, Resumidor, Resumo};
use crate::transcricao::TranscricaoOpenAi;
use crate::whisper::{ClienteWhisper, Transcritor};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;

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

pub struct ResumidorRoteador {
    nuvem: OpenAi,
    chatgpt: Arc<ChatGpt>,
    local: OpenAi,
    locais: Arc<Locais>,
    config: LerConfig,
}

impl ResumidorRoteador {
    pub fn new(locais: Arc<Locais>, config: LerConfig, chatgpt: Arc<ChatGpt>) -> Self {
        Self { nuvem: OpenAi::new(), chatgpt, local: OpenAi::local(format!("http://127.0.0.1:{PORTA_LLM}/v1")), locais, config }
    }
}

#[async_trait]
impl Resumidor for ResumidorRoteador {
    async fn resumir(&self, transcricao: &str, material: &str, chave: &str, modelo: &str) -> Result<Resumo> {
        let c = (self.config)();
        if !c.analise_local {
            if c.chatgpt_conectado {
                return self.chatgpt.resumir(transcricao, material, chave, &c.chatgpt_model).await;
            }
            return self.nuvem.resumir(transcricao, material, chave, modelo).await;
        }
        self.locais.garantir_llm(&c).await.map_err(|e| anyhow!("{e} {DICA}"))?;
        self.local.resumir(transcricao, material, "", "local").await.map_err(|e| anyhow!("{e}. {DICA}"))
    }

    async fn responder(&self, pergunta: &str, material: &str, transcricao: &str, chave: &str, modelo: &str) -> Result<String> {
        let c = (self.config)();
        if !c.analise_local {
            if c.chatgpt_conectado {
                return self.chatgpt.responder(pergunta, material, transcricao, chave, &c.chatgpt_model).await;
            }
            return self.nuvem.responder(pergunta, material, transcricao, chave, modelo).await;
        }
        self.locais.garantir_llm(&c).await.map_err(|e| anyhow!("{e} {DICA}"))?;
        self.local.responder(pergunta, material, transcricao, "", "local").await.map_err(|e| anyhow!("{e}. {DICA}"))
    }

    async fn responder_parcial(&self, pergunta: &str, material: &str, transcricao: &str, chave: &str, modelo: &str, parcial: &(dyn for<'a> Fn(&'a str) + Send + Sync)) -> Result<String> {
        let c = (self.config)();
        if !c.analise_local {
            if c.chatgpt_conectado {
                return self.chatgpt.responder_parcial(pergunta, material, transcricao, chave, &c.chatgpt_model, parcial).await;
            }
            return self.nuvem.responder_parcial(pergunta, material, transcricao, chave, modelo, parcial).await;
        }
        self.locais.garantir_llm(&c).await.map_err(|e| anyhow!("{e} {DICA}"))?;
        self.local.responder_parcial(pergunta, material, transcricao, "", "local", parcial).await.map_err(|e| anyhow!("{e}. {DICA}"))
    }

    async fn traduzir(&self, texto: &str, origem: &str, destino: &str, chave: &str, modelo: &str) -> Result<String> {
        let c = (self.config)();
        if !c.analise_local {
            if c.chatgpt_conectado {
                return self.chatgpt.traduzir(texto, origem, destino, chave, &c.chatgpt_model).await;
            }
            return self.nuvem.traduzir(texto, origem, destino, chave, modelo).await;
        }
        self.locais.garantir_llm(&c).await.map_err(|e| anyhow!("{e} {DICA}"))?;
        self.local.traduzir(texto, origem, destino, "", "local").await.map_err(|e| anyhow!("{e}. {DICA}"))
    }

    async fn traduzir_parcial(&self, texto: &str, origem: &str, destino: &str, chave: &str, modelo: &str, parcial: &(dyn for<'a> Fn(&'a str) + Send + Sync)) -> Result<String> {
        let c = (self.config)();
        if !c.analise_local {
            if c.chatgpt_conectado {
                return self.chatgpt.traduzir_parcial(texto, origem, destino, chave, &c.chatgpt_model, parcial).await;
            }
            return self.nuvem.traduzir_parcial(texto, origem, destino, chave, modelo, parcial).await;
        }
        self.locais.garantir_llm(&c).await.map_err(|e| anyhow!("{e} {DICA}"))?;
        self.local.traduzir_parcial(texto, origem, destino, "", "local", parcial).await.map_err(|e| anyhow!("{e}. {DICA}"))
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
        let e = r.resumir("[00:00:00] Eu: oi", "", "", "m").await.unwrap_err().to_string();
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
        let e = r.resumir("[00:00:00] Eu: oi", "", "", "m").await.unwrap_err().to_string();
        assert!(e.contains("Chave da OpenAI"), "{e}");
        let _ = std::fs::remove_dir_all(raiz);
    }
}
