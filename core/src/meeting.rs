//! Uma reunião: recebe áudio das duas fontes, transcreve em tempo real e, ao parar,
//! gera o resumo, grava e envia por e-mail. O que a pessoa precisa ver sai por `emitir`.

use crate::mail::{DadosEmail, Envio, Remetente};
use crate::material::{compor, reduzir, Documento};
use crate::segmenter::{Fala, Segmentador};
use crate::settings::Config;
use crate::store::{NovaSessao, Pendencia, Store};
use crate::summary::{formatar_transcricao, Resumidor, Resumo};
use crate::whisper::Transcritor;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

pub type Emitir = Arc<dyn Fn(&str, Value) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fonte {
    Mic,
    Sistema,
}

impl Fonte {
    fn falante(self) -> &'static str {
        match self {
            Fonte::Mic => "ME",
            Fonte::Sistema => "THEY",
        }
    }
}

#[derive(Clone)]
pub struct Dependencias {
    pub store: Arc<Mutex<Store>>,
    pub usuario: String,
    pub transcritor: Arc<dyn Transcritor>,
    pub resumidor: Arc<dyn Resumidor>,
    pub remetente: Arc<dyn Remetente>,
    pub config: Arc<dyn Fn() -> Config + Send + Sync>,
    pub emitir: Emitir,
    pub agora: Arc<dyn Fn() -> i64 + Send + Sync>,
}

struct Ativa {
    id: String,
    inicio: i64,
    mic: Segmentador,
    sistema: Segmentador,
    tarefas: Vec<JoinHandle<()>>,
    rt: tokio::runtime::Handle,
}

pub struct Reuniao {
    dep: Dependencias,
    ativa: Mutex<Option<Ativa>>,
    /// Material de apoio colado pela pessoa; vale para a reunião atual e a próxima.
    material: Mutex<String>,
    /// Documentos anexados (Google Drive ou arquivo); valem junto com o material colado.
    documentos: Mutex<Vec<Documento>>,
    /// Última vez que o volume de cada fonte foi enviado à tela (mic, sistema).
    ultimo_nivel: Mutex<[i64; 2]>,
}

/// Quanto do material entra numa pergunta (em bytes). Acima disso, só os trechos mais relevantes.
pub const ORCAMENTO_MATERIAL: usize = 150_000;
/// Quanto do material entra no resumo da reunião.
const ORCAMENTO_MATERIAL_RESUMO: usize = 60_000;
/// Com o modelo local (contexto menor), os limites são menores.
const ORCAMENTO_MATERIAL_LOCAL: usize = 25_000;
const ORCAMENTO_MATERIAL_RESUMO_LOCAL: usize = 8_000;
const TRANSCRICAO_PERGUNTA_LOCAL: usize = 40_000;
/// Parte final da transcrição usada para achar os trechos do material que importam agora.
const CAUDA_CONSULTA: usize = 2_000;

fn fim_do_texto(t: &str, max: usize) -> &str {
    if t.len() <= max {
        return t;
    }
    let mut i = t.len() - max;
    while !t.is_char_boundary(i) {
        i += 1;
    }
    &t[i..]
}

fn data_local(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map(|d| d.format("%d-%m-%Y").to_string()).unwrap_or_default()
}

impl Reuniao {
    pub fn new(dep: Dependencias) -> Self {
        Self {
            dep, ativa: Mutex::new(None), material: Mutex::new(String::new()),
            documentos: Mutex::new(vec![]), ultimo_nivel: Mutex::new([0; 2]),
        }
    }

    /// Grava o material atual na reunião em andamento, se houver.
    fn gravar_material_na_reuniao(&self) -> Result<()> {
        if let Some(a) = self.ativa.lock().unwrap().as_ref() {
            self.dep.store.lock().unwrap().definir_contexto(&a.id, &self.material())?;
        }
        Ok(())
    }

    /// Define o texto colado do material de apoio. Se há reunião em andamento, grava nela também.
    pub fn definir_material(&self, texto: &str) -> Result<()> {
        *self.material.lock().unwrap() = texto.to_string();
        self.gravar_material_na_reuniao()
    }

    /// Define os documentos anexados. Se há reunião em andamento, grava nela também.
    pub fn definir_documentos(&self, docs: Vec<Documento>) -> Result<()> {
        *self.documentos.lock().unwrap() = docs;
        self.gravar_material_na_reuniao()
    }

    /// Acrescenta um documento (troca o de mesmo id, se já estava anexado).
    pub fn anexar_documento(&self, doc: Documento) -> Result<()> {
        let mut docs = self.documentos();
        docs.retain(|d| d.id != doc.id);
        docs.push(doc);
        self.definir_documentos(docs)
    }

    pub fn remover_documento(&self, id: &str) -> Result<()> {
        let mut docs = self.documentos();
        docs.retain(|d| d.id != id);
        self.definir_documentos(docs)
    }

    pub fn documentos(&self) -> Vec<Documento> {
        self.documentos.lock().unwrap().clone()
    }

    /// Só o texto colado (o que aparece na caixa de texto).
    pub fn texto_material(&self) -> String {
        self.material.lock().unwrap().clone()
    }

    /// Texto colado e documentos juntos, como vai para as perguntas.
    pub fn material(&self) -> String {
        compor(&self.texto_material(), &self.documentos())
    }

    /// Informa o volume (0 a 1) de uma fonte. Limita o envio a cerca de 10 por segundo.
    pub fn informar_nivel(&self, fonte: Fonte, valor: f32) {
        let agora = (self.dep.agora)();
        let i = if fonte == Fonte::Mic { 0 } else { 1 };
        let mut g = self.ultimo_nivel.lock().unwrap();
        if agora - g[i] >= 100 {
            g[i] = agora;
            (self.dep.emitir)("reuniao:nivel", json!({"fonte": if i == 0 { "mic" } else { "sistema" }, "valor": valor.clamp(0.0, 1.0)}));
        }
    }

    /// Responde uma pergunta com o material e a transcrição. Sem `sessao`, usa a reunião em
    /// andamento (ou só o material, se não houver). Com `sessao`, usa o material gravado nela.
    pub async fn perguntar(&self, pergunta: &str, sessao: Option<&str>) -> Result<String> {
        let ativa_id = self.ativa.lock().unwrap().as_ref().map(|a| a.id.clone());
        let id = sessao.map(str::to_string).or(ativa_id.clone());
        let (material, transcricao) = match &id {
            Some(id) => {
                let store = self.dep.store.lock().unwrap();
                let gravado = if Some(id) == ativa_id.as_ref() { None } else { store.contexto(id)? };
                let material = gravado.unwrap_or_else(|| self.material());
                (material, formatar_transcricao(&store.transcricao(id)?))
            }
            None => (self.material(), String::new()),
        };
        let cfg = (self.dep.config)();
        let consulta = format!("{pergunta}\n{}", fim_do_texto(&transcricao, CAUDA_CONSULTA));
        let material = reduzir(&material, &consulta, if cfg.analise_local { ORCAMENTO_MATERIAL_LOCAL } else { ORCAMENTO_MATERIAL });
        // o modelo local tem menos contexto: fica com o final da reunião
        let transcricao = if cfg.analise_local && transcricao.len() > TRANSCRICAO_PERGUNTA_LOCAL {
            format!("[início da reunião omitido]\n{}", fim_do_texto(&transcricao, TRANSCRICAO_PERGUNTA_LOCAL))
        } else {
            transcricao
        };
        self.dep.resumidor.responder(pergunta, &material, &transcricao, &cfg.openai_key, &cfg.openai_model).await
    }

    pub fn em_andamento(&self) -> bool {
        self.ativa.lock().unwrap().is_some()
    }

    /// Precisa ser chamado dentro de um runtime do tokio.
    pub fn iniciar(&self) -> Result<String> {
        let mut g = self.ativa.lock().unwrap();
        if g.is_some() {
            return Err(anyhow!("Já existe uma reunião em andamento."));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let inicio = (self.dep.agora)();
        self.dep.store.lock().unwrap().adicionar_sessao(NovaSessao {
            id: &id, user_id: &self.dep.usuario, ativa: true, started_at: inicio / 1000, ..Default::default()
        })?;
        self.dep.store.lock().unwrap().definir_contexto(&id, &self.material())?;
        *g = Some(Ativa {
            id: id.clone(), inicio, mic: Segmentador::new(), sistema: Segmentador::new(),
            tarefas: vec![], rt: tokio::runtime::Handle::current(),
        });
        (self.dep.emitir)("reuniao:estado", json!({"fase": "gravando", "id": id}));
        Ok(id)
    }

    /// Cancela a reunião em andamento sem resumo nem e-mail (ex.: a captura de áudio não abriu).
    pub fn descartar(&self) {
        if let Some(a) = self.ativa.lock().unwrap().take() {
            for t in a.tarefas { t.abort(); }
            let _ = self.dep.store.lock().unwrap().apagar_sessao(&a.id);
        }
    }

    /// Chamado pelas threads de captura, com PCM 16 bits, 16 kHz mono.
    pub fn receber_audio(&self, fonte: Fonte, pcm: &[i16]) {
        let mut g = self.ativa.lock().unwrap();
        let Some(a) = g.as_mut() else { return };
        let agora = (self.dep.agora)();
        let seg = match fonte {
            Fonte::Mic => &mut a.mic,
            Fonte::Sistema => &mut a.sistema,
        };
        if let Some(fala) = seg.push(pcm, agora) {
            let t = self.transcrever(&a.rt, &a.id, fonte, fala);
            a.tarefas.push(t);
        }
    }

    fn transcrever(&self, rt: &tokio::runtime::Handle, id: &str, fonte: Fonte, fala: Fala) -> JoinHandle<()> {
        let dep = self.dep.clone();
        let id = id.to_string();
        rt.spawn(async move {
            match dep.transcritor.transcrever(&fala.pcm).await {
                Ok(texto) if !texto.is_empty() => {
                    let r = dep.store.lock().unwrap().adicionar_mensagem(
                        &uuid::Uuid::new_v4().to_string(), &id, &texto, Some(fonte.falante()), fala.inicio_ms,
                    );
                    if let Err(e) = r {
                        (dep.emitir)("reuniao:aviso", json!(format!("Não foi possível gravar uma fala: {e}")));
                    }
                    (dep.emitir)("reuniao:fala", json!({"speaker": fonte.falante(), "text": texto, "timestamp": fala.inicio_ms}));
                }
                Ok(_) => {}
                Err(e) => (dep.emitir)("reuniao:aviso", json!(format!("Falha ao transcrever um trecho: {e}"))),
            }
        })
    }

    pub async fn parar(&self) -> Result<()> {
        let a = self.ativa.lock().unwrap().take().ok_or_else(|| anyhow!("Nenhuma reunião em andamento."))?;
        let Ativa { id, inicio, mut mic, mut sistema, mut tarefas, rt } = a;
        let dep = &self.dep;
        (dep.emitir)("reuniao:estado", json!({"fase": "finalizando"}));

        if let Some(f) = mic.flush() { tarefas.push(self.transcrever(&rt, &id, Fonte::Mic, f)); }
        if let Some(f) = sistema.flush() { tarefas.push(self.transcrever(&rt, &id, Fonte::Sistema, f)); }
        for t in tarefas {
            let _ = t.await;
        }

        let falas = dep.store.lock().unwrap().transcricao(&id)?;
        let transcricao = formatar_transcricao(&falas);
        let fim = (dep.agora)();
        let cfg = (dep.config)();
        let data = data_local(inicio);

        if falas.is_empty() {
            dep.store.lock().unwrap().encerrar_sessao(&id, fim / 1000, Some("Reunião sem fala captada"), None, &[])?;
            (dep.emitir)("reuniao:final", json!({"id": id, "vazia": true}));
            return Ok(());
        }

        (dep.emitir)("reuniao:estado", json!({"fase": "resumindo"}));
        let (resumo, erro_resumo) = match dep.resumidor.resumir(&transcricao, &reduzir(&self.material(), fim_do_texto(&transcricao, CAUDA_CONSULTA), if cfg.analise_local { ORCAMENTO_MATERIAL_RESUMO_LOCAL } else { ORCAMENTO_MATERIAL_RESUMO }), &cfg.openai_key, &cfg.openai_model).await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        };

        let titulo = resumo.as_ref().map(|r| r.titulo.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Reunião de {data}"));
        let pendencias: Vec<Pendencia> = resumo.as_ref().map(|r| r.pendencias.clone()).unwrap_or_default();
        dep.store.lock().unwrap().encerrar_sessao(&id, fim / 1000, Some(&titulo), resumo.as_ref().map(|r| r.resumo.as_str()), &pendencias)?;
        (dep.emitir)("reuniao:final", json!({
            "id": id, "titulo": titulo, "resumo": resumo, "erroResumo": erro_resumo, "transcricao": transcricao
        }));

        if cfg.enviar_automatico {
            self.enviar_por_email(&id, Some((resumo, erro_resumo))).await;
        }
        Ok(())
    }

    /// Envia (ou reenvia) a transcrição de uma reunião já encerrada.
    /// `completo` traz o resumo inteiro recém-gerado; sem ele, usa o que está gravado no banco.
    pub async fn enviar_por_email(&self, id: &str, completo: Option<(Option<Resumo>, Option<String>)>) {
        let dep = &self.dep;
        let cfg = (dep.config)();
        (dep.emitir)("reuniao:email", json!({"estado": "enviando"}));

        let preparar = || -> Result<DadosEmail> {
            let store = dep.store.lock().unwrap();
            let sessao = store.obter_sessao(id)?.ok_or_else(|| anyhow!("Conversa não encontrada."))?;
            let transcricao = formatar_transcricao(&store.transcricao(id)?);
            let (resumo, erro_resumo) = match completo {
                Some(c) => c,
                None => (
                    sessao.overview.clone().map(|o| Resumo {
                        titulo: sessao.subject.clone().unwrap_or_default(), resumo: o, decisoes: vec![],
                        pendencias: sessao.pending.clone(), duvidas: vec![],
                    }),
                    None,
                ),
            };
            Ok(DadosEmail {
                titulo: sessao.subject.unwrap_or_else(|| "Reunião".into()),
                data: data_local(sessao.started_at * 1000),
                resumo, erro_resumo, transcricao,
            })
        };

        let resultado = match preparar() {
            Ok(dados) => dep.remetente.enviar(Envio {
                usuario: cfg.smtp_user.clone(), senha: cfg.smtp_pass.clone(), destinatario: cfg.destinatario.clone(), dados,
            }).await,
            Err(e) => Err(e),
        };
        match resultado {
            Ok(()) => (dep.emitir)("reuniao:email", json!({"estado": "enviado", "para": cfg.destinatario})),
            Err(e) => (dep.emitir)("reuniao:email", json!({"estado": "erro", "mensagem": e.to_string()})),
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct Fila(Mutex<Vec<&'static str>>);
    #[async_trait]
    impl Transcritor for Fila {
        async fn transcrever(&self, _: &[i16]) -> Result<String> {
            Ok(self.0.lock().unwrap().pop().unwrap_or("").to_string())
        }
    }

    struct ResumoFixo(Option<Resumo>, Mutex<Vec<String>>);
    #[async_trait]
    impl Resumidor for ResumoFixo {
        async fn resumir(&self, _: &str, material: &str, _: &str, _: &str) -> Result<Resumo> {
            self.1.lock().unwrap().push(format!("resumo|{material}"));
            self.0.clone().ok_or_else(|| anyhow!("sem rede"))
        }
        async fn responder(&self, pergunta: &str, material: &str, transcricao: &str, _: &str, _: &str) -> Result<String> {
            Ok(format!("{pergunta}|{material}|{transcricao}"))
        }
    }

    #[derive(Default)]
    struct Caixa(Mutex<Vec<Envio>>);
    #[async_trait]
    impl Remetente for Arc<Caixa> {
        async fn enviar(&self, e: Envio) -> Result<()> {
            self.0.lock().unwrap().push(e);
            Ok(())
        }
    }

    type Eventos = Arc<Mutex<Vec<(String, Value)>>>;

    fn montar(falas: Vec<&'static str>, resumo: Option<Resumo>) -> (Reuniao, Eventos, Arc<Caixa>) {
        let eventos: Eventos = Default::default();
        let ev = eventos.clone();
        let caixa = Arc::new(Caixa::default());
        let relogio = Arc::new(AtomicI64::new(1_000_000));
        let dep = Dependencias {
            store: Arc::new(Mutex::new(Store::em_memoria().unwrap())),
            usuario: "u".into(),
            transcritor: Arc::new(Fila(Mutex::new(falas))),
            resumidor: Arc::new(ResumoFixo(resumo, Default::default())),
            remetente: Arc::new(caixa.clone()),
            config: Arc::new(|| Config { openai_key: "k".into(), smtp_user: "a".into(), smtp_pass: "b".into(), destinatario: "c".into(), ..Default::default() }),
            emitir: Arc::new(move |c, d| ev.lock().unwrap().push((c.to_string(), d))),
            agora: Arc::new(move || relogio.fetch_add(1000, Ordering::SeqCst)),
        };
        (Reuniao::new(dep), eventos, caixa)
    }

    fn falar(r: &Reuniao, fonte: Fonte) {
        for _ in 0..6 { r.receber_audio(fonte, &[6000i16; 1600]); }
    }

    #[tokio::test]
    async fn ponta_a_ponta_transcreve_resume_grava_e_envia() {
        let resumo = Resumo {
            titulo: "Contrato".into(), resumo: "Fechar contrato".into(), decisoes: vec![], duvidas: vec![],
            pendencias: vec![Pendencia { tarefa: "Enviar proposta".into(), responsavel: Some("Eu".into()), prazo: Some("sexta".into()), feita: false }],
        };
        // a fila é lida do fim para o começo: 1ª fala = "Vamos fechar...", 2ª = "Eu envio..."
        let (r, eventos, caixa) = montar(vec!["Eu envio a proposta até sexta", "Vamos fechar o contrato"], Some(resumo));
        let id = r.iniciar().unwrap();
        assert!(r.iniciar().is_err());
        falar(&r, Fonte::Mic);
        falar(&r, Fonte::Sistema);
        r.parar().await.unwrap();

        let store = r.dep.store.lock().unwrap();
        let s = store.obter_sessao(&id).unwrap().unwrap();
        assert_eq!(s.status, "ended");
        assert_eq!(s.subject.as_deref(), Some("Contrato"));
        assert_eq!(s.pending[0].tarefa, "Enviar proposta");
        assert_eq!(store.transcricao(&id).unwrap().len(), 2);
        let b = store.buscar("u", crate::store::OpcoesBusca { texto: Some("proposta"), ..Default::default() }).unwrap();
        assert_eq!(b.total, 1, "a transcrição fica pesquisável");

        let enviados = caixa.0.lock().unwrap();
        assert_eq!(enviados[0].destinatario, "c");
        assert!(enviados[0].dados.transcricao.contains("Eu: ") && enviados[0].dados.transcricao.contains("Outros: "));
        let ev = eventos.lock().unwrap();
        assert!(ev.iter().any(|e| e.0 == "reuniao:fala") && ev.iter().any(|e| e.0 == "reuniao:final"));
        assert_eq!(ev.iter().rev().find(|e| e.0 == "reuniao:email").unwrap().1["estado"], "enviado");
    }

    #[tokio::test]
    async fn falha_no_resumo_ainda_grava_e_envia_a_transcricao() {
        let (r, eventos, caixa) = montar(vec!["só uma fala"], None);
        let id = r.iniciar().unwrap();
        falar(&r, Fonte::Mic);
        r.parar().await.unwrap();
        let ev = eventos.lock().unwrap();
        let fim = &ev.iter().find(|e| e.0 == "reuniao:final").unwrap().1;
        assert_eq!(fim["erroResumo"], "sem rede");
        assert_eq!(caixa.0.lock().unwrap()[0].dados.erro_resumo.as_deref(), Some("sem rede"));
        assert_eq!(r.dep.store.lock().unwrap().transcricao(&id).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reuniao_sem_fala_nao_chama_resumo_nem_email() {
        let (r, eventos, caixa) = montar(vec![], None);
        r.iniciar().unwrap();
        r.parar().await.unwrap();
        assert!(caixa.0.lock().unwrap().is_empty());
        assert_eq!(eventos.lock().unwrap().iter().find(|e| e.0 == "reuniao:final").unwrap().1["vazia"], true);
        assert!(r.parar().await.is_err(), "não há reunião para parar");
    }

    #[tokio::test]
    async fn pergunta_usa_material_e_transcricao_da_reuniao() {
        let (r, _, _) = montar(vec!["O prazo é sexta"], None);
        // antes da reunião: só o material
        r.definir_material("Pauta: contrato Alfa").unwrap();
        assert_eq!(r.perguntar("Qual o contrato?", None).await.unwrap(), "Qual o contrato?|Pauta: contrato Alfa|");

        let id = r.iniciar().unwrap();
        falar(&r, Fonte::Sistema);
        r.parar().await.unwrap_or(());
        // reunião encerrada: o material ficou gravado nela, mesmo que o atual mude depois
        r.definir_material("outro assunto").unwrap();
        let resp = r.perguntar("Qual o prazo?", Some(&id)).await.unwrap();
        assert!(resp.starts_with("Qual o prazo?|Pauta: contrato Alfa|"), "{resp}");
        assert!(resp.contains("Outros: O prazo é sexta"), "{resp}");
        assert!(r.dep.store.lock().unwrap().contexto(&id).unwrap().unwrap().contains("Alfa"));
    }

    #[tokio::test]
    async fn material_trocado_durante_a_reuniao_vale_na_hora() {
        let (r, _, _) = montar(vec!["ok"], None);
        let id = r.iniciar().unwrap();
        r.definir_material("novo material").unwrap();
        assert_eq!(r.dep.store.lock().unwrap().contexto(&id).unwrap().as_deref(), Some("novo material"));
        assert!(r.perguntar("?", None).await.unwrap().contains("novo material"));
        r.descartar();
    }

    #[tokio::test]
    async fn documentos_anexados_entram_na_pergunta_e_na_reuniao() {
        let (r, _, _) = montar(vec!["ok"], None);
        let doc = |id: &str, nome: &str, texto: &str| Documento { id: id.into(), nome: nome.into(), origem: "drive".into(), texto: texto.into() };
        r.definir_material("Pauta Alfa").unwrap();
        r.anexar_documento(doc("drive:1", "Preços", "Plano Ouro custa R$ 100")).unwrap();
        r.anexar_documento(doc("drive:2", "Contrato", "Prazo de 12 meses")).unwrap();
        r.anexar_documento(doc("drive:1", "Preços", "Plano Ouro custa R$ 120")).unwrap(); // mesmo id: troca
        assert_eq!(r.documentos().len(), 2);
        assert_eq!(r.texto_material(), "Pauta Alfa");

        let id = r.iniciar().unwrap();
        let gravado = r.dep.store.lock().unwrap().contexto(&id).unwrap().unwrap();
        assert!(gravado.starts_with("Pauta Alfa") && gravado.contains("=== DOCUMENTO: Preços ===") && gravado.contains("R$ 120"), "{gravado}");

        // anexar durante a reunião grava na reunião; a pergunta enxerga o documento
        r.remover_documento("drive:2").unwrap();
        r.anexar_documento(doc("arquivo:3", "Notas", "Reunir toda sexta")).unwrap();
        let resp = r.perguntar("Quando reunir?", None).await.unwrap();
        assert!(resp.contains("Reunir toda sexta") && !resp.contains("12 meses"), "{resp}");
        assert!(r.dep.store.lock().unwrap().contexto(&id).unwrap().unwrap().contains("Notas"));
        r.descartar();
    }

    #[tokio::test]
    async fn material_enorme_vai_reduzido_para_a_pergunta() {
        let (r, _, _) = montar(vec![], None);
        let enchimento = "Assunto sem relação nenhuma com o resto. ".repeat(40);
        let mut grande = String::new();
        for i in 0..400 {
            grande += &format!("{enchimento}\n");
            if i == 300 {
                grande += "A garantia do produto Zeta dura 24 meses.\n";
            }
        }
        assert!(grande.len() > ORCAMENTO_MATERIAL);
        r.anexar_documento(Documento { id: "a".into(), nome: "Manual".into(), origem: "arquivo".into(), texto: grande }).unwrap();
        let resp = r.perguntar("Quanto dura a garantia do produto Zeta?", None).await.unwrap();
        assert!(resp.contains("24 meses"), "o trecho certo precisa ir para o modelo");
        assert!(resp.len() < ORCAMENTO_MATERIAL + 5_000, "tamanho {}", resp.len());
    }

    #[test]
    fn volume_e_enviado_no_maximo_dez_vezes_por_segundo() {
        let (r, eventos, _) = montar(vec![], None);
        for _ in 0..5 {
            r.informar_nivel(Fonte::Mic, 0.5); // o relógio de teste avança 1000 ms a cada leitura
        }
        r.informar_nivel(Fonte::Sistema, 7.0);
        let ev = eventos.lock().unwrap();
        let niveis: Vec<_> = ev.iter().filter(|e| e.0 == "reuniao:nivel").collect();
        assert_eq!(niveis.len(), 6);
        assert_eq!(niveis[5].1["fonte"], "sistema");
        assert_eq!(niveis[5].1["valor"], 1.0, "valor é limitado a 1");
    }

    #[tokio::test]
    async fn modo_local_corta_a_transcricao_e_o_material_para_caber() {
        let eventos: Eventos = Default::default();
        let relogio = Arc::new(AtomicI64::new(1_000_000));
        let dep = Dependencias {
            store: Arc::new(Mutex::new(Store::em_memoria().unwrap())),
            usuario: "u".into(),
            transcritor: Arc::new(Fila(Mutex::new(vec![]))),
            resumidor: Arc::new(ResumoFixo(None, Default::default())),
            remetente: Arc::new(Arc::new(Caixa::default())),
            config: Arc::new(|| Config { analise_local: true, ..Default::default() }),
            emitir: Arc::new(move |c, d| eventos.lock().unwrap().push((c.to_string(), d))),
            agora: Arc::new(move || relogio.fetch_add(1000, Ordering::SeqCst)),
        };
        let r = Reuniao::new(dep);
        r.anexar_documento(Documento { id: "a".into(), nome: "Grande".into(), origem: "arquivo".into(), texto: "palavra solta. ".repeat(10_000) }).unwrap();
        let id = r.iniciar().unwrap();
        for i in 0..3000 {
            r.dep.store.lock().unwrap().adicionar_mensagem(&format!("m{i}"), &id, &format!("fala número {i} com algum texto de enchimento"), Some("THEY"), i).unwrap();
        }
        let resp = r.perguntar("o que foi dito?", None).await.unwrap();
        assert!(resp.len() < crate::summary::LIMITE_PERGUNTA_LOCAL, "tamanho {}", resp.len());
        assert!(resp.contains("[início da reunião omitido]") && resp.contains("fala número 2999"));
        r.descartar();
    }
}
