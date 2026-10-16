//! Tradução em tempo real: ouve o áudio, transcreve no idioma de origem e traduz cada fala
//! para o idioma de destino. Nada é gravado; o que a pessoa vê sai por `emitir`.

use crate::mail::{montar_email_traducao, Remetente};
use crate::meeting::{Emitir, Fonte, Receptor};
use crate::segmenter::{Fala, Segmentador};
use crate::settings::Config;
use crate::summary::Resumidor;
use crate::whisper::Transcritor;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// Idiomas oferecidos: código do Whisper e nome em português.
pub const IDIOMAS: [(&str, &str); 20] = [
    ("pt", "português"), ("en", "inglês"), ("es", "espanhol"), ("fr", "francês"), ("de", "alemão"),
    ("it", "italiano"), ("nl", "holandês"), ("pl", "polonês"), ("ru", "russo"), ("uk", "ucraniano"),
    ("tr", "turco"), ("ar", "árabe"), ("he", "hebraico"), ("hi", "hindi"), ("ja", "japonês"),
    ("ko", "coreano"), ("zh", "chinês"), ("vi", "vietnamita"), ("id", "indonésio"), ("sv", "sueco"),
];

pub fn nome_idioma(codigo: &str) -> String {
    if codigo == "auto" {
        return "o idioma que for falado".into();
    }
    IDIOMAS.iter().find(|(c, _)| *c == codigo).map(|(_, n)| n.to_string()).unwrap_or_else(|| codigo.to_string())
}

fn data_local(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map(|d| d.format("%d-%m-%Y").to_string()).unwrap_or_default()
}

/// Qual áudio traduzir.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entrada {
    Mic,
    Sistema,
    Ambos,
}

impl Entrada {
    pub fn de_texto(t: &str) -> Self {
        match t {
            "mic" => Entrada::Mic,
            "ambos" => Entrada::Ambos,
            _ => Entrada::Sistema,
        }
    }

    fn aceita(self, f: Fonte) -> bool {
        matches!((self, f), (Entrada::Ambos, _) | (Entrada::Mic, Fonte::Mic) | (Entrada::Sistema, Fonte::Sistema))
    }
}

#[derive(Clone)]
pub struct DependenciasTraducao {
    pub transcritor: Arc<dyn Transcritor>,
    pub resumidor: Arc<dyn Resumidor>,
    pub config: Arc<dyn Fn() -> Config + Send + Sync>,
    pub emitir: Emitir,
    pub agora: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub remetente: Arc<dyn Remetente>,
}

/// O que foi dito e traduzido na última sessão, para enviar por e-mail.
#[derive(Default)]
struct Registro {
    origem: String,
    destino: String,
    inicio_ms: i64,
    /// id da fala -> (original, tradução; vazia se ainda não chegou)
    linhas: BTreeMap<u64, (String, String)>,
}

struct Ativa {
    origem: String,
    destino: String,
    entrada: Entrada,
    mic: Segmentador,
    sistema: Segmentador,
    seq: u64,
    tarefas: Vec<JoinHandle<()>>,
    rt: tokio::runtime::Handle,
}

pub struct Traducao {
    dep: DependenciasTraducao,
    ativa: Mutex<Option<Ativa>>,
    ultimo_nivel: Mutex<i64>,
    registro: Arc<Mutex<Registro>>,
}

/// Segmentador de resposta rápida: fecha a fala com pouco silêncio e corta falas longas cedo.
fn segmentador() -> Segmentador {
    Segmentador::new().com_silencio_ms(250.0).com_maxima_ms(3500.0)
}

impl Traducao {
    pub fn new(dep: DependenciasTraducao) -> Self {
        Self { dep, ativa: Mutex::new(None), ultimo_nivel: Mutex::new(0), registro: Default::default() }
    }

    pub fn em_andamento(&self) -> bool {
        self.ativa.lock().unwrap().is_some()
    }

    /// Precisa ser chamado dentro de um runtime do tokio. `origem` pode ser "auto".
    pub fn iniciar(&self, origem: &str, destino: &str, entrada: Entrada) -> Result<()> {
        if destino.is_empty() || destino == "auto" {
            return Err(anyhow!("Escolha o idioma da tradução."));
        }
        if origem == destino {
            return Err(anyhow!("O idioma do áudio e o da tradução são iguais."));
        }
        let mut g = self.ativa.lock().unwrap();
        if g.is_some() {
            return Err(anyhow!("Já existe uma tradução em andamento."));
        }
        *self.registro.lock().unwrap() = Registro {
            origem: nome_idioma(origem), destino: nome_idioma(destino), inicio_ms: (self.dep.agora)(), linhas: BTreeMap::new(),
        };
        *g = Some(Ativa {
            origem: origem.into(), destino: destino.into(), entrada,
            mic: segmentador(), sistema: segmentador(), seq: 0,
            tarefas: vec![], rt: tokio::runtime::Handle::current(),
        });
        (self.dep.emitir)("traducao:estado", json!({"fase": "traduzindo"}));
        Ok(())
    }

    /// Para de ouvir, traduz o que ainda estava no ar e avisa a tela.
    pub async fn parar(&self) -> Result<()> {
        let a = self.ativa.lock().unwrap().take().ok_or_else(|| anyhow!("Nenhuma tradução em andamento."))?;
        let Ativa { origem, destino, entrada, mut mic, mut sistema, mut seq, mut tarefas, rt } = a;
        for (fonte, seg) in [(Fonte::Mic, &mut mic), (Fonte::Sistema, &mut sistema)] {
            if !entrada.aceita(fonte) {
                continue;
            }
            if let Some(f) = seg.flush() {
                seq += 1;
                tarefas.push(self.traduzir_fala(&rt, seq, &origem, &destino, fonte, f));
            }
        }
        for t in tarefas {
            let _ = t.await;
        }
        (self.dep.emitir)("traducao:estado", json!({"fase": "parada"}));
        Ok(())
    }

    /// Apaga o que foi guardado para o e-mail (o botão Limpar da tela).
    pub fn limpar(&self) {
        self.registro.lock().unwrap().linhas.clear();
    }

    pub fn tem_texto(&self) -> bool {
        !self.registro.lock().unwrap().linhas.is_empty()
    }

    /// Envia por e-mail a última tradução. Avisa a tela pelo canal `traducao:email`.
    pub async fn enviar_por_email(&self) {
        let dep = &self.dep;
        let cfg = (dep.config)();
        (dep.emitir)("traducao:email", json!({"estado": "enviando"}));
        let m = {
            let r = self.registro.lock().unwrap();
            if r.linhas.is_empty() {
                None
            } else {
                let linhas: Vec<(String, String)> = r.linhas.values().cloned().collect();
                Some(montar_email_traducao(&r.origem, &r.destino, &data_local(r.inicio_ms), &linhas))
            }
        };
        let resultado = match m {
            Some(m) => dep.remetente.enviar_montado(&cfg.smtp_user, &cfg.smtp_pass, &cfg.destinatario, m).await,
            None => Err(anyhow!("Ainda não há nada traduzido para enviar.")),
        };
        match resultado {
            Ok(()) => (dep.emitir)("traducao:email", json!({"estado": "enviado", "para": cfg.destinatario})),
            Err(e) => (dep.emitir)("traducao:email", json!({"estado": "erro", "mensagem": e.to_string()})),
        }
    }

    fn traduzir_fala(&self, rt: &tokio::runtime::Handle, id: u64, origem: &str, destino: &str, fonte: Fonte, fala: Fala) -> JoinHandle<()> {
        let dep = self.dep.clone();
        let registro = self.registro.clone();
        let (origem, destino) = (origem.to_string(), destino.to_string());
        rt.spawn(async move {
            let texto = match dep.transcritor.transcrever_em(&fala.pcm, &origem).await {
                Ok(t) if !t.is_empty() => t,
                Ok(_) => return,
                Err(e) => {
                    (dep.emitir)("traducao:aviso", json!(format!("Falha ao transcrever um trecho: {e}")));
                    return;
                }
            };
            registro.lock().unwrap().linhas.insert(id, (texto.clone(), String::new()));
            // o original aparece já; a tradução completa a linha quando chegar
            (dep.emitir)("traducao:fala", json!({
                "id": id, "original": texto, "fonte": if fonte == Fonte::Mic { "mic" } else { "sistema" }, "timestamp": fala.inicio_ms
            }));
            let cfg = (dep.config)();
            let emitir = dep.emitir.clone();
            let parcial = move |t: &str| emitir("traducao:traduzida", json!({"id": id, "traducao": t, "parcial": true}));
            let r = dep.resumidor.traduzir_parcial(&texto, &nome_idioma(&origem), &nome_idioma(&destino), &cfg.openai_key, &cfg.openai_model, &parcial).await;
            let dados: Value = match r {
                Ok(t) => {
                    if let Some(l) = registro.lock().unwrap().linhas.get_mut(&id) {
                        l.1 = t.clone();
                    }
                    json!({"id": id, "traducao": t})
                }
                Err(e) => json!({"id": id, "erro": e.to_string()}),
            };
            (dep.emitir)("traducao:traduzida", dados);
        })
    }
}

impl Receptor for Traducao {
    fn receber_audio(&self, fonte: Fonte, pcm: &[i16]) {
        let mut g = self.ativa.lock().unwrap();
        let Some(a) = g.as_mut() else { return };
        if !a.entrada.aceita(fonte) {
            return;
        }
        let agora = (self.dep.agora)();
        let seg = if fonte == Fonte::Mic { &mut a.mic } else { &mut a.sistema };
        if let Some(fala) = seg.push(pcm, agora) {
            a.seq += 1;
            a.tarefas.retain(|t| !t.is_finished());
            let t = self.traduzir_fala(&a.rt, a.seq, &a.origem, &a.destino, fonte, fala);
            a.tarefas.push(t);
        }
    }

    /// Volume (0 a 1) das fontes em uso, no máximo 10 vezes por segundo.
    fn informar_nivel(&self, fonte: Fonte, valor: f32) {
        let usada = self.ativa.lock().unwrap().as_ref().is_some_and(|a| a.entrada.aceita(fonte));
        if !usada {
            return;
        }
        let agora = (self.dep.agora)();
        let mut ultimo = self.ultimo_nivel.lock().unwrap();
        if agora - *ultimo >= 100 {
            *ultimo = agora;
            (self.dep.emitir)("traducao:nivel", json!({"valor": valor.clamp(0.0, 1.0)}));
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::summary::Resumo;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct Voz(Mutex<Vec<String>>); // guarda o idioma pedido a cada fala
    #[async_trait]
    impl Transcritor for Voz {
        async fn transcrever(&self, _: &[i16]) -> Result<String> {
            Ok("padrão".into())
        }
        async fn transcrever_em(&self, _: &[i16], idioma: &str) -> Result<String> {
            self.0.lock().unwrap().push(idioma.into());
            Ok("Good morning".into())
        }
    }

    struct Tradutor(bool);
    #[async_trait]
    impl Resumidor for Tradutor {
        async fn resumir(&self, _: &str, _: &str, _: &str, _: &str) -> Result<Resumo> {
            Err(anyhow!("não usado"))
        }
        async fn responder(&self, _: &str, _: &str, _: &str, _: &str, _: &str) -> Result<String> {
            Err(anyhow!("não usado"))
        }
        async fn traduzir(&self, texto: &str, origem: &str, destino: &str, _: &str, _: &str) -> Result<String> {
            if self.0 { Err(anyhow!("sem rede")) } else { Ok(format!("{texto}|{origem}|{destino}")) }
        }
        async fn traduzir_parcial(&self, texto: &str, origem: &str, destino: &str, c: &str, m: &str, parcial: &(dyn for<'a> Fn(&'a str) + Send + Sync)) -> Result<String> {
            let r = self.traduzir(texto, origem, destino, c, m).await?;
            parcial("par");
            Ok(r)
        }
    }

    #[derive(Default)]
    struct Caixa(Mutex<Vec<(String, String)>>);
    #[async_trait]
    impl Remetente for Caixa {
        async fn enviar(&self, _: crate::mail::Envio) -> Result<()> {
            Ok(())
        }
        async fn enviar_montado(&self, _: &str, _: &str, para: &str, m: crate::mail::Montado) -> Result<()> {
            self.0.lock().unwrap().push((para.into(), m.texto));
            Ok(())
        }
    }

    type Eventos = Arc<Mutex<Vec<(String, Value)>>>;

    fn montar(falha: bool) -> (Traducao, Eventos, Arc<Voz>) {
        let eventos: Eventos = Default::default();
        let ev = eventos.clone();
        let relogio = Arc::new(AtomicI64::new(1_000_000));
        let voz = Arc::new(Voz(Default::default()));
        let dep = DependenciasTraducao {
            transcritor: voz.clone(),
            resumidor: Arc::new(Tradutor(falha)),
            config: Arc::new(Config::default),
            emitir: Arc::new(move |c, d| ev.lock().unwrap().push((c.to_string(), d))),
            agora: Arc::new(move || relogio.fetch_add(1000, Ordering::SeqCst)),
            remetente: Arc::new(Caixa::default()),
        };
        (Traducao::new(dep), eventos, voz)
    }

    fn falar(t: &Traducao, fonte: Fonte) {
        for _ in 0..6 {
            t.receber_audio(fonte, &[6000i16; 1600]);
        }
    }

    #[tokio::test]
    async fn transcreve_no_idioma_de_origem_e_traduz_para_o_destino() {
        let (t, eventos, voz) = montar(false);
        t.iniciar("en", "pt", Entrada::Sistema).unwrap();
        falar(&t, Fonte::Sistema);
        t.parar().await.unwrap();
        assert_eq!(voz.0.lock().unwrap().as_slice(), ["en"]);
        let ev = eventos.lock().unwrap();
        let fala = ev.iter().find(|e| e.0 == "traducao:fala").unwrap();
        assert_eq!(fala.1["original"], "Good morning");
        let tr = ev.iter().find(|e| e.0 == "traducao:traduzida" && e.1["parcial"].is_null()).unwrap();
        assert_eq!((tr.1["id"].clone(), tr.1["traducao"].clone()), (fala.1["id"].clone(), json!("Good morning|inglês|português")));
        assert_eq!(ev.last().unwrap().1["fase"], "parada");
    }

    #[tokio::test]
    async fn so_ouve_a_fonte_escolhida() {
        let (t, eventos, voz) = montar(false);
        t.iniciar("en", "pt", Entrada::Mic).unwrap();
        falar(&t, Fonte::Sistema);
        t.informar_nivel(Fonte::Sistema, 0.5);
        t.parar().await.unwrap();
        assert!(voz.0.lock().unwrap().is_empty());
        assert!(!eventos.lock().unwrap().iter().any(|e| e.0 == "traducao:nivel" || e.0 == "traducao:fala"));
    }

    #[tokio::test]
    async fn falha_na_traducao_mantem_o_original_e_avisa() {
        let (t, eventos, _) = montar(true);
        t.iniciar("auto", "pt", Entrada::Ambos).unwrap();
        falar(&t, Fonte::Mic);
        t.parar().await.unwrap();
        let ev = eventos.lock().unwrap();
        assert!(ev.iter().any(|e| e.0 == "traducao:fala"));
        assert_eq!(ev.iter().find(|e| e.0 == "traducao:traduzida").unwrap().1["erro"], "sem rede");
    }

    #[tokio::test]
    async fn valida_os_idiomas_e_a_exclusividade() {
        let (t, _, _) = montar(false);
        assert!(t.iniciar("pt", "pt", Entrada::Sistema).is_err());
        assert!(t.iniciar("en", "auto", Entrada::Sistema).is_err());
        assert!(t.parar().await.is_err());
        t.iniciar("en", "pt", Entrada::Sistema).unwrap();
        assert!(t.iniciar("en", "pt", Entrada::Sistema).is_err());
        assert!(t.em_andamento());
    }

    #[tokio::test]
    async fn envia_por_email_o_que_foi_traduzido() {
        let (t, eventos, _) = montar(false);
        t.enviar_por_email().await;
        assert_eq!(eventos.lock().unwrap().last().unwrap().1["estado"], "erro", "nada para enviar");

        t.iniciar("en", "pt", Entrada::Sistema).unwrap();
        falar(&t, Fonte::Sistema);
        t.parar().await.unwrap();
        assert!(t.tem_texto());
        t.enviar_por_email().await;
        assert_eq!(eventos.lock().unwrap().last().unwrap().1["estado"], "enviado");

        t.limpar();
        assert!(!t.tem_texto());
    }

    #[test]
    fn nomes_de_idioma() {
        assert_eq!(nome_idioma("ja"), "japonês");
        assert_eq!(nome_idioma("xx"), "xx");
        assert_eq!(Entrada::de_texto("ambos"), Entrada::Ambos);
        assert_eq!(Entrada::de_texto("?"), Entrada::Sistema);
    }
}
