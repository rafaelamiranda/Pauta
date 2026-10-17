//! Busca por significado: cada conversa vira trechos com embeddings (OpenAI), guardados no banco.
//! A busca de texto (FTS) continua valendo; esta só acrescenta as conversas de sentido parecido.

use crate::store::Store;
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const MODELO: &str = "text-embedding-3-small";
/// Tamanho aproximado de cada trecho, em caracteres.
const TAMANHO_TRECHO: usize = 1_200;
/// Trechos enviados à OpenAI por chamada.
const LOTE: usize = 48;
/// Abaixo disso a semelhança é ruído (medida em cosseno).
pub const LIMIAR: f32 = 0.3;

pub type Chave = Arc<dyn Fn() -> String + Send + Sync>;

pub struct Significado {
    url_base: String,
    http: reqwest::Client,
    chave: Chave,
    indexando: AtomicBool,
}

impl Significado {
    pub fn new(chave: Chave) -> Self {
        Self::com_url("https://api.openai.com/v1", chave)
    }

    pub fn com_url(url: impl Into<String>, chave: Chave) -> Self {
        Self { url_base: url.into(), http: reqwest::Client::new(), chave, indexando: AtomicBool::new(false) }
    }

    /// Um vetor para cada texto, na mesma ordem.
    pub async fn vetores(&self, textos: &[String]) -> Result<Vec<Vec<f32>>> {
        let chave = (self.chave)();
        if chave.is_empty() {
            return Err(anyhow!("Chave da OpenAI não configurada."));
        }
        let res = self
            .http
            .post(format!("{}/embeddings", self.url_base))
            .bearer_auth(chave)
            .json(&serde_json::json!({ "model": MODELO, "input": textos }))
            .send()
            .await?;
        let status = res.status();
        let corpo: serde_json::Value = res.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = corpo["error"]["message"].as_str().unwrap_or("sem detalhe");
            return Err(anyhow!("Embeddings da OpenAI falharam ({}): {msg}", status.as_u16()));
        }
        let mut itens: Vec<(usize, Vec<f32>)> = corpo["data"]
            .as_array()
            .ok_or_else(|| anyhow!("Resposta de embeddings sem dados."))?
            .iter()
            .map(|d| {
                let v = d["embedding"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64().map(|x| x as f32)).collect()).unwrap_or_default();
                (d["index"].as_u64().unwrap_or(0) as usize, v)
            })
            .collect();
        itens.sort_by_key(|(i, _)| *i);
        if itens.len() != textos.len() || itens.iter().any(|(_, v)| v.is_empty()) {
            return Err(anyhow!("Resposta de embeddings incompleta."));
        }
        Ok(itens.into_iter().map(|(_, v)| v).collect())
    }

    pub async fn vetor(&self, texto: &str) -> Result<Vec<f32>> {
        Ok(self.vetores(&[texto.to_string()]).await?.remove(0))
    }

    /// Indexa as conversas encerradas que ainda não têm trechos. Se outra indexação já está
    /// rodando, não faz nada. Devolve quantas conversas indexou.
    pub async fn indexar_pendentes(&self, store: &Mutex<Store>, usuario: &str) -> Result<usize> {
        if self.indexando.swap(true, Ordering::AcqRel) {
            return Ok(0);
        }
        let r = self.indexar(store, usuario).await;
        self.indexando.store(false, Ordering::Release);
        r
    }

    async fn indexar(&self, store: &Mutex<Store>, usuario: &str) -> Result<usize> {
        let pendentes = store.lock().unwrap().sessoes_sem_trechos(usuario)?;
        let mut feitas = 0;
        for id in pendentes {
            let trechos = store.lock().unwrap().trechos_da_sessao(&id)?;
            let mut vetores = Vec::with_capacity(trechos.len());
            for lote in trechos.chunks(LOTE) {
                vetores.extend(self.vetores(lote).await?);
            }
            // conversa apagada durante a indexação: o insert falharia pela chave estrangeira
            if store.lock().unwrap().guardar_trechos(&id, &trechos, &vetores).is_ok() {
                feitas += 1;
            }
        }
        Ok(feitas)
    }
}

/// Junta textos curtos em trechos de até `TAMANHO_TRECHO` caracteres (um texto maior é cortado).
pub fn agrupar(textos: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut trechos = Vec::new();
    let mut atual = String::new();
    for t in textos {
        let t: String = t.chars().take(TAMANHO_TRECHO).collect();
        if !atual.is_empty() && atual.chars().count() + t.chars().count() + 1 > TAMANHO_TRECHO {
            trechos.push(std::mem::take(&mut atual));
        }
        if !atual.is_empty() {
            atual.push('\n');
        }
        atual.push_str(&t);
    }
    if !atual.is_empty() {
        trechos.push(atual);
    }
    trechos
}

pub fn cosseno(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 { 0.0 } else { ab / (aa.sqrt() * bb.sqrt()) }
}

pub fn para_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn de_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::whisper::testes::servidor_falso;

    #[test]
    fn agrupa_em_trechos_sem_passar_do_tamanho() {
        let falas: Vec<String> = (0..10).map(|i| format!("{i}{}", "x".repeat(399))).collect();
        let t = agrupar(falas);
        assert_eq!(t.len(), 5); // três falas dariam 1202 caracteres: cabem duas por trecho
        assert!(t.iter().all(|x| x.chars().count() <= TAMANHO_TRECHO));
        assert_eq!(agrupar(vec!["y".repeat(5_000)])[0].chars().count(), TAMANHO_TRECHO);
        assert!(agrupar(Vec::<String>::new()).is_empty());
    }

    #[test]
    fn cosseno_e_bytes() {
        assert!((cosseno(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosseno(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosseno(&[1.0], &[1.0, 2.0]), 0.0, "tamanhos diferentes não casam");
        assert_eq!(cosseno(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(de_bytes(&para_bytes(&[0.5, -2.0, 3.25])), vec![0.5, -2.0, 3.25]);
    }

    #[tokio::test]
    async fn pede_embeddings_e_reordena_pelo_indice() {
        let (url, pedidos) = servidor_falso(200, r#"{"data":[{"index":1,"embedding":[0,1]},{"index":0,"embedding":[1,0]}]}"#).await;
        let s = Significado::com_url(url, Arc::new(|| "sk-x".into()));
        let v = s.vetores(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(v, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        let p = pedidos.lock().unwrap()[0].clone();
        assert!(p.starts_with("POST /embeddings") && p.contains(MODELO) && p.contains("\"input\":[\"a\",\"b\"]"), "{p}");
        assert!(p.to_lowercase().contains("authorization: bearer sk-x"));
    }

    #[tokio::test]
    async fn erros_viram_mensagem() {
        let (url, _) = servidor_falso(401, r#"{"error":{"message":"chave inválida"}}"#).await;
        let e = Significado::com_url(url.clone(), Arc::new(|| "k".into())).vetor("a").await.unwrap_err().to_string();
        assert!(e.contains("401") && e.contains("chave inválida"), "{e}");
        let e = Significado::com_url(url, Arc::new(String::new)).vetor("a").await.unwrap_err().to_string();
        assert!(e.contains("Chave da OpenAI"), "{e}");
        let (url, _) = servidor_falso(200, r#"{"data":[]}"#).await;
        assert!(Significado::com_url(url, Arc::new(|| "k".into())).vetor("a").await.is_err());
    }

    #[tokio::test]
    async fn indexa_so_o_que_falta_e_acha_pelo_sentido() {
        use crate::store::{NovaSessao, OpcoesBusca};
        let (url, pedidos) = servidor_falso(200, r#"{"data":[{"index":0,"embedding":[1,0]}]}"#).await;
        let store = Mutex::new(Store::em_memoria().unwrap());
        {
            let s = store.lock().unwrap();
            s.adicionar_sessao(NovaSessao { id: "s1", user_id: "u", subject: Some("Orçamento"), overview: Some("Custos"), started_at: 1, ended_at: Some(2), ..Default::default() }).unwrap();
            s.adicionar_sessao(NovaSessao { id: "ativa", user_id: "u", ativa: true, started_at: 1, ..Default::default() }).unwrap();
        }
        let sig = Significado::com_url(url, Arc::new(|| "k".into()));
        assert_eq!(sig.indexar_pendentes(&store, "u").await.unwrap(), 1, "a ativa fica de fora");
        assert_eq!(sig.indexar_pendentes(&store, "u").await.unwrap(), 0, "já indexada");
        assert_eq!(pedidos.lock().unwrap().len(), 1);
        let s = store.lock().unwrap();
        let b = s.buscar("u", OpcoesBusca { texto: Some("dinheiro"), significado: Some(&[1.0, 0.0]), ..Default::default() }).unwrap();
        assert_eq!(b.sessions.len(), 1);
        assert!(b.sessions[0].por_significado);
    }
}
