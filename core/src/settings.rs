//! Configurações em arquivo JSON. Chave e senha ficam fora dele, no cofre do sistema.

use crate::locais::Tipo;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Onde guardar segredos (no Windows: Gerenciador de Credenciais).
pub trait Segredos: Send + Sync {
    fn ler(&self, nome: &str) -> Option<String>;
    fn gravar(&self, nome: &str, valor: &str) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub openai_key: String,
    pub openai_model: String,
    /// Modelo usado quando a pessoa entra com a conta do ChatGPT (assinatura) em vez da chave da API.
    pub chatgpt_model: String,
    /// Há login do ChatGPT guardado no cofre (não vai para o arquivo).
    #[serde(skip)]
    pub chatgpt_conectado: bool,
    pub transcricao_model: String,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub destinatario: String,
    pub enviar_automatico: bool,
    /// Idioma falado nas reuniões (código de 2 letras, como "pt").
    pub idioma: String,
    /// Nome do microfone e da saída de áudio; vazio usa o padrão do Windows.
    pub microfone: String,
    pub saida: String,
    pub drive_client_id: String,
    pub drive_client_secret: String,
    /// Apagar reuniões com mais de N dias ao abrir o app. 0 = nunca.
    pub apagar_apos_dias: u32,
    /// Transcrever no próprio computador (whisper.cpp) em vez de usar a OpenAI.
    pub transcricao_local: bool,
    pub whisper_local: String,
    /// Resumir e responder perguntas no próprio computador (llama.cpp) em vez de usar a OpenAI.
    pub analise_local: bool,
    pub llm_local: String,
    /// "cpu" ou "vulkan" (placa de vídeo) para o modelo de análise local.
    pub aceleracao: String,
    /// Manda nomes e siglas do material como dica ao transcritor.
    pub dica_termos: bool,
    /// Separa os participantes em "Outros" (só com a transcrição da OpenAI).
    pub identificar_falantes: bool,
    /// Procura versão nova no GitHub ao abrir o app.
    pub verificar_atualizacoes: bool,
    /// Achar conversas pelo sentido (embeddings da OpenAI), além do texto digitado.
    pub busca_significado: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            openai_key: String::new(),
            openai_model: "gpt-4o-mini".into(),
            chatgpt_model: "gpt-5".into(),
            chatgpt_conectado: false,
            transcricao_model: "gpt-4o-transcribe".into(),
            smtp_user: String::new(),
            smtp_pass: String::new(),
            destinatario: String::new(),
            enviar_automatico: true,
            idioma: "pt".into(),
            microfone: String::new(),
            saida: String::new(),
            drive_client_id: String::new(),
            drive_client_secret: String::new(),
            apagar_apos_dias: 0,
            transcricao_local: false,
            whisper_local: "whisper-small".into(),
            analise_local: false,
            llm_local: "qwen3-4b-q4".into(),
            aceleracao: "cpu".into(),
            dica_termos: true,
            identificar_falantes: false,
            verificar_atualizacoes: true,
            busca_significado: true,
        }
    }
}

pub(crate) const SEGREDO_CHATGPT: &str = "chatgptAuth";
const SEGREDO_OPENAI: &str = "openaiKey";
const SEGREDO_SMTP: &str = "smtpPass";
const SEGREDO_DRIVE: &str = "driveClientSecret";

pub struct Configuracoes {
    arquivo: PathBuf,
    segredos: Arc<dyn Segredos>,
    /// ID e segredo do cliente Google embutidos no programa na hora de gerar o instalador.
    drive_embutido: Option<(String, String)>,
}

impl Configuracoes {
    pub fn new(arquivo: PathBuf, segredos: Arc<dyn Segredos>) -> Self {
        Self { arquivo, segredos, drive_embutido: None }
    }

    /// Usa o ID e o segredo do Google embutidos no programa (ignora os dos Ajustes). Vazio não faz nada.
    pub fn com_drive_embutido(mut self, id: &str, segredo: &str) -> Self {
        if !id.trim().is_empty() && !segredo.trim().is_empty() {
            self.drive_embutido = Some((id.trim().to_string(), segredo.trim().to_string()));
        }
        self
    }

    pub fn segredos(&self) -> Arc<dyn Segredos> {
        self.segredos.clone()
    }

    fn ler_arquivo(&self) -> Value {
        std::fs::read_to_string(&self.arquivo).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}))
    }

    /// Valores completos, com segredos (uso interno).
    pub fn obter(&self) -> Config {
        let mut c: Config = serde_json::from_value(self.ler_arquivo()).unwrap_or_default();
        c.openai_key = self.segredos.ler(SEGREDO_OPENAI).unwrap_or_default();
        c.smtp_pass = self.segredos.ler(SEGREDO_SMTP).unwrap_or_default();
        c.chatgpt_conectado = self.segredos.ler(SEGREDO_CHATGPT).is_some_and(|t| !t.is_empty());
        c.drive_client_secret = self.segredos.ler(SEGREDO_DRIVE).unwrap_or_default();
        if let Some((id, segredo)) = &self.drive_embutido {
            c.drive_client_id = id.clone();
            c.drive_client_secret = segredo.clone();
        }
        if c.idioma.trim().is_empty() {
            c.idioma = "pt".into();
        }
        c
    }

    /// Chave da OpenAI para a busca por significado, ou vazio quando ela não vale. Manda o texto das
    /// conversas à OpenAI, então exige a opção ligada, uma chave da API e a análise fora do modo local
    /// (quem escolheu o modo local não quer texto saindo do computador). Lê só a chave, não todos os segredos.
    pub fn chave_significado(&self) -> String {
        let c: Config = serde_json::from_value(self.ler_arquivo()).unwrap_or_default();
        if !c.busca_significado || c.analise_local {
            return String::new();
        }
        self.segredos.ler(SEGREDO_OPENAI).unwrap_or_default()
    }

    /// Versão para a tela: nunca devolve o segredo, só se ele existe.
    pub fn para_tela(&self) -> Value {
        let c = self.obter();
        json!({
            "openaiKey": "", "openaiKeyDefinida": !c.openai_key.is_empty(),
            "openaiModel": c.openai_model,
            "chatgptModel": c.chatgpt_model,
            "chatgptConectado": c.chatgpt_conectado,
            "chatgptConta": self.segredos.ler(SEGREDO_CHATGPT).and_then(|t| crate::chatgpt::conta_de(&t)).unwrap_or_default(),
            "transcricaoModel": c.transcricao_model,
            "smtpUser": c.smtp_user,
            "smtpPass": "", "smtpPassDefinida": !c.smtp_pass.is_empty(),
            "destinatario": c.destinatario,
            "enviarAutomatico": c.enviar_automatico,
            "idioma": c.idioma,
            "microfone": c.microfone,
            "saida": c.saida,
            "driveEmbutido": self.drive_embutido.is_some(),
            // com credenciais embutidas, a tela nem recebe o ID
            "driveClientId": if self.drive_embutido.is_some() { String::new() } else { c.drive_client_id },
            "driveClientSecret": "", "driveClientSecretDefinida": !c.drive_client_secret.is_empty(),
            "apagarAposDias": c.apagar_apos_dias,
            "transcricaoLocal": c.transcricao_local,
            "whisperLocal": c.whisper_local,
            "analiseLocal": c.analise_local,
            "llmLocal": c.llm_local,
            "aceleracao": c.aceleracao,
            "dicaTermos": c.dica_termos,
            "identificarFalantes": c.identificar_falantes,
            "verificarAtualizacoes": c.verificar_atualizacoes,
            "buscaSignificado": c.busca_significado,
        })
    }

    /// Salva os campos recebidos (nomes em camelCase, como a tela envia). Segredo vazio mantém o anterior.
    pub fn salvar(&self, novos: &Value) -> Result<()> {
        let mut c: Config = serde_json::from_value(self.ler_arquivo()).unwrap_or_default();
        let texto = |k: &str| novos[k].as_str().map(str::to_owned);
        if let Some(v) = texto("openaiModel").filter(|v| !v.is_empty()) { c.openai_model = v; }
        if let Some(v) = texto("chatgptModel").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) { c.chatgpt_model = v; }
        if let Some(v) = texto("transcricaoModel").filter(|v| !v.is_empty()) { c.transcricao_model = v; }
        if let Some(v) = texto("smtpUser") { c.smtp_user = v; }
        if let Some(v) = texto("destinatario") { c.destinatario = v; }
        if let Some(v) = novos["enviarAutomatico"].as_bool() { c.enviar_automatico = v; }
        if let Some(v) = texto("idioma") { c.idioma = v.trim().to_lowercase().chars().take(5).collect(); }
        if let Some(v) = texto("microfone") { c.microfone = v; }
        if let Some(v) = texto("saida") { c.saida = v; }
        let embutido = self.drive_embutido.is_some();
        if let Some(v) = texto("driveClientId").filter(|_| !embutido) { c.drive_client_id = v.trim().to_string(); }
        if let Some(v) = novos["apagarAposDias"].as_u64() { c.apagar_apos_dias = v.min(3650) as u32; }
        if let Some(v) = novos["transcricaoLocal"].as_bool() { c.transcricao_local = v; }
        if let Some(v) = novos["analiseLocal"].as_bool() { c.analise_local = v; }
        if let Some(v) = novos["dicaTermos"].as_bool() { c.dica_termos = v; }
        if let Some(v) = novos["identificarFalantes"].as_bool() { c.identificar_falantes = v; }
        if let Some(v) = novos["verificarAtualizacoes"].as_bool() { c.verificar_atualizacoes = v; }
        if let Some(v) = novos["buscaSignificado"].as_bool() { c.busca_significado = v; }
        let id_valido = |id: &str, tipo: Tipo| crate::locais::CATALOGO.iter().any(|m| m.id == id && m.tipo == tipo);
        if let Some(v) = texto("whisperLocal").filter(|v| id_valido(v, Tipo::Whisper)) { c.whisper_local = v; }
        if let Some(v) = texto("llmLocal").filter(|v| id_valido(v, Tipo::Llm)) { c.llm_local = v; }
        if let Some(v) = texto("aceleracao").filter(|v| v == "cpu" || v == "vulkan") { c.aceleracao = v; }
        if let Some(v) = texto(SEGREDO_DRIVE).filter(|v| !v.is_empty() && !embutido) { self.segredos.gravar(SEGREDO_DRIVE, v.trim())?; }
        if let Some(v) = texto(SEGREDO_OPENAI).filter(|v| !v.is_empty()) { self.segredos.gravar(SEGREDO_OPENAI, &v)?; }
        if let Some(v) = texto(SEGREDO_SMTP).filter(|v| !v.is_empty()) { self.segredos.gravar(SEGREDO_SMTP, &v)?; }
        // segredos nunca vão para o arquivo
        c.openai_key.clear();
        c.smtp_pass.clear();
        c.drive_client_secret.clear();
        if let Some(pai) = self.arquivo.parent() { std::fs::create_dir_all(pai)?; }
        std::fs::write(&self.arquivo, serde_json::to_string_pretty(&c)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Memoria(Mutex<HashMap<String, String>>);
    impl Segredos for Memoria {
        fn ler(&self, n: &str) -> Option<String> { self.0.lock().unwrap().get(n).cloned() }
        fn gravar(&self, n: &str, v: &str) -> Result<()> { self.0.lock().unwrap().insert(n.into(), v.into()); Ok(()) }
    }

    #[test]
    fn segredos_ficam_fora_do_arquivo_e_da_tela() {
        let pasta = std::env::temp_dir().join(format!("cfg-{}", uuid::Uuid::new_v4()));
        let arquivo = pasta.join("config.json");
        let s = Configuracoes::new(arquivo.clone(), Arc::new(Memoria::default()));
        s.salvar(&json!({"openaiKey": "sk-1", "smtpUser": "eu@gmail.com", "smtpPass": "senha"})).unwrap();
        let em_disco = std::fs::read_to_string(&arquivo).unwrap();
        assert!(!em_disco.contains("sk-1") && !em_disco.contains("senha"));
        assert_eq!(s.obter().openai_key, "sk-1");
        let tela = s.para_tela();
        assert_eq!(tela["openaiKey"], "");
        assert_eq!(tela["openaiKeyDefinida"], true);
        s.salvar(&json!({"openaiKey": "", "destinatario": "a@b.com"})).unwrap(); // vazio mantém
        assert_eq!(s.obter().openai_key, "sk-1");
        assert_eq!(s.obter().destinatario, "a@b.com");
        assert_eq!(s.obter().openai_model, "gpt-4o-mini");
        assert_eq!(s.obter().transcricao_model, "gpt-4o-transcribe");
        assert_eq!(s.obter().idioma, "pt");
        s.salvar(&json!({"idioma": "EN", "microfone": "Headset", "driveClientId": " id ", "driveClientSecret": "segredo-g", "apagarAposDias": 30})).unwrap();
        let c = s.obter();
        assert_eq!((c.idioma.as_str(), c.microfone.as_str(), c.drive_client_id.as_str(), c.drive_client_secret.as_str(), c.apagar_apos_dias), ("en", "Headset", "id", "segredo-g", 30));
        assert!(!std::fs::read_to_string(&arquivo).unwrap().contains("segredo-g"));
        assert!(!c.transcricao_local && !c.analise_local && c.aceleracao == "cpu");
        s.salvar(&json!({"transcricaoLocal": true, "analiseLocal": true, "whisperLocal": "nao-existe", "llmLocal": "qwen3-4b-q8", "aceleracao": "vulkan"})).unwrap();
        let c = s.obter();
        assert!(c.transcricao_local && c.analise_local && c.aceleracao == "vulkan");
        assert_eq!(c.whisper_local, "whisper-small", "id desconhecido é ignorado");
        assert_eq!(c.llm_local, "qwen3-4b-q8");
        s.salvar(&json!({"whisperLocal": "qwen3-4b-q4", "aceleracao": "cuda"})).unwrap();
        assert_eq!((s.obter().whisper_local.as_str(), s.obter().aceleracao.as_str()), ("whisper-small", "vulkan"), "tipo errado e valor inválido são ignorados");
        assert_eq!(s.para_tela()["driveClientSecret"], "");
        assert_eq!(s.para_tela()["driveClientSecretDefinida"], true);
        // busca por significado: ligada por padrão, mas só vale com chave e sem análise local
        assert!(s.obter().busca_significado && s.chave_significado().is_empty(), "análise local ligada acima");
        s.salvar(&json!({"analiseLocal": false})).unwrap();
        assert_eq!(s.chave_significado(), "sk-1");
        s.salvar(&json!({"buscaSignificado": false})).unwrap();
        assert!(s.chave_significado().is_empty());
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[test]
    fn credenciais_do_google_embutidas_valem_e_nao_vao_para_a_tela_nem_para_o_arquivo() {
        let pasta = std::env::temp_dir().join(format!("cfg-{}", uuid::Uuid::new_v4()));
        let arquivo = pasta.join("config.json");
        let s = Configuracoes::new(arquivo.clone(), Arc::new(Memoria::default())).com_drive_embutido("id-embutido", "segredo-embutido");
        s.salvar(&json!({"driveClientId": "outro", "driveClientSecret": "outro-segredo"})).unwrap();
        let c = s.obter();
        assert_eq!((c.drive_client_id.as_str(), c.drive_client_secret.as_str()), ("id-embutido", "segredo-embutido"));
        let tela = s.para_tela();
        assert_eq!(tela["driveEmbutido"], true);
        assert_eq!(tela["driveClientId"], "");
        let texto = std::fs::read_to_string(&arquivo).unwrap();
        assert!(!texto.contains("embutido") && !texto.contains("outro"), "{texto}");
        // vazio não ativa o modo embutido
        let sem = Configuracoes::new(arquivo, Arc::new(Memoria::default())).com_drive_embutido("", "x");
        assert_eq!(sem.para_tela()["driveEmbutido"], false);
        let _ = std::fs::remove_dir_all(pasta);
    }
}
