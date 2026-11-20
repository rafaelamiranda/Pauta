//! Refazer ou trazer conversas a partir de áudio: retranscrever uma conversa com o áudio guardado,
//! importar um arquivo de áudio como conversa nova e recuperar reuniões interrompidas por uma queda.

use crate::meeting::{transcrever_fala, ContextoFala, Fonte, Reuniao};
use crate::material::dica_de_termos;
use crate::segmenter::{Fala, Segmentador};
use crate::store::{FalaGravada, NovaSessao, Store};
use anyhow::{anyhow, Result};
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};

const TAMANHO_DICA: usize = 300;

impl Reuniao {
    fn progresso(&self, fase: &str, parte: usize, partes: usize) {
        (self.dep.emitir)("reuniao:reprocesso", json!({"fase": fase, "parte": parte, "partes": partes}));
    }

    /// A conversa tem áudio guardado (dá para retranscrever)?
    pub fn tem_audio(&self, id: &str) -> bool {
        self.dep.pasta_audio().is_some_and(|p| crate::audio::existe(&p, id))
    }

    /// Grava em `destino` o áudio da conversa inteira (WAV, microfone e som do computador juntos).
    /// Devolve a duração em ms, ou `None` se não há áudio guardado.
    pub fn salvar_audio_wav(&self, id: &str, destino: &std::path::Path) -> anyhow::Result<Option<i64>> {
        match self.dep.pasta_audio() {
            Some(p) => crate::audio::salvar_reuniao_wav(&p, id, destino),
            None => Ok(None),
        }
    }

    /// Apaga o áudio das conversas que não existem mais. Chamar depois de apagar conversas e ao abrir o app.
    pub fn limpar_audio_orfao(&self) {
        if let Some(p) = self.dep.pasta_audio() {
            let store = self.dep.store.clone();
            crate::audio::limpar_orfaos(&p, |id| store.lock().unwrap().existe_sessao(id));
        }
    }

    /// Transcreve de novo, com a configuração de agora (outro modelo, outro idioma), o áudio guardado da
    /// conversa e refaz o resumo. O texto antigo só é trocado se TODAS as falas forem transcritas.
    pub async fn retranscrever(&self, id: &str) -> Result<()> {
        if self.em_andamento() {
            return Err(anyhow!("Finalize a reunião em andamento antes de retranscrever."));
        }
        let pasta = self.dep.pasta_audio().ok_or_else(|| anyhow!("Esta conversa não tem áudio guardado."))?;
        let sessao = self.dep.store.lock().unwrap().obter_sessao(id)?.ok_or_else(|| anyhow!("Conversa não encontrada."))?;
        if sessao.status == "active" {
            return Err(anyhow!("A conversa ainda está em andamento."));
        }
        let mut falas: Vec<(Fonte, Fala)> = [Fonte::Mic, Fonte::Sistema]
            .into_iter()
            .flat_map(|f| crate::audio::ler(&pasta, id, f).into_iter().map(move |fala| (f, fala)))
            .collect();
        if falas.is_empty() {
            return Err(anyhow!("Esta conversa não tem áudio guardado. Ligue \"Guardar o áudio\" nos Ajustes antes da próxima reunião."));
        }
        falas.sort_by_key(|(_, f)| f.inicio_ms);

        // as falas novas vão para um banco temporário; o da conversa só muda no fim
        let temp = Arc::new(Mutex::new(Store::em_memoria()?));
        temp.lock().unwrap().adicionar_sessao(NovaSessao { id, user_id: &self.dep.usuario, ativa: true, started_at: sessao.started_at, ..Default::default() })?;
        let dep = crate::meeting::Dependencias { store: temp.clone(), ..self.dep.clone() };
        let material = self.dep.store.lock().unwrap().contexto(id).ok().flatten().unwrap_or_default();
        let ctx = ContextoFala::novo(dica_de_termos(&material, TAMANHO_DICA));

        let total = falas.len();
        let mut falhas = 0;
        for (i, (fonte, fala)) in falas.iter().enumerate() {
            self.progresso("retranscrevendo", i + 1, total);
            if !transcrever_fala(&dep, id, *fonte, fala, &ctx, false).await {
                falhas += 1;
            }
        }
        if falhas > 0 {
            return Err(anyhow!("{falhas} de {total} trechos falharam ao transcrever. A transcrição anterior foi mantida."));
        }
        let novas = temp.lock().unwrap().transcricao(id)?;
        if novas.is_empty() {
            return Err(anyhow!("A nova transcrição saiu vazia. A anterior foi mantida."));
        }
        self.dep.store.lock().unwrap().substituir_falas(id, &novas)?;

        let fim = sessao.ended_at.unwrap_or(sessao.started_at) * 1000;
        let saida = self.resumir_e_encerrar(id, sessao.started_at * 1000, fim).await?;
        self.avisar_final(id, &saida);
        Ok(())
    }

    /// Importa um arquivo de áudio como conversa nova: transcreve, resume e mostra o resultado.
    /// Devolve o id da conversa.
    pub async fn importar_audio(&self, caminho: &Path) -> Result<String> {
        if self.em_andamento() {
            return Err(anyhow!("Finalize a reunião em andamento antes de importar um áudio."));
        }
        // 1. decodifica e separa as falas (trabalho de CPU, fora do runtime)
        self.progresso("lendo", 0, 0);
        let origem = caminho.to_path_buf();
        let cfg = (self.dep.config)();
        let (falas, duracao_s) = tokio::task::spawn_blocking(move || -> Result<(Vec<Fala>, f64)> {
            let mut seg = Segmentador::da_config(&cfg);
            let (mut falas, mut pos_ms) = (vec![], 0i64);
            let dur = crate::decodificar::decodificar(&origem, |trecho| {
                if let Some(f) = seg.push(&trecho, pos_ms) { falas.push(f); }
                pos_ms += 100;
            })?;
            falas.extend(seg.flush());
            Ok((falas, dur))
        })
        .await
        .map_err(|e| anyhow!("Falha ao ler o áudio: {e}"))??;
        if falas.is_empty() {
            return Err(anyhow!("Não encontrei fala neste áudio."));
        }

        // 2. cria a conversa; o horário é o do arquivo (quando foi gravado, se o sistema sabe)
        let inicio_ms = std::fs::metadata(caminho).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or((self.dep.agora)(), |d| d.as_millis() as i64 - (duracao_s * 1000.0) as i64);
        let id = uuid::Uuid::new_v4().to_string();
        self.dep.store.lock().unwrap().adicionar_sessao(NovaSessao { id: &id, user_id: &self.dep.usuario, ativa: true, started_at: inicio_ms / 1000, ..Default::default() })?;
        let material = self.material();
        self.dep.store.lock().unwrap().definir_contexto(&id, &material)?;
        let ctx = ContextoFala::novo(dica_de_termos(&material, TAMANHO_DICA));
        let falas: Vec<Fala> = falas.into_iter().map(|f| Fala { inicio_ms: inicio_ms + f.inicio_ms, ..f }).collect();

        // 3. guarda o áudio (se a opção está ligada) e transcreve
        let mut gravador = match ((self.dep.config)().guardar_audio, self.dep.pasta_audio()) {
            (true, Some(p)) => crate::audio::Gravador::abrir(&p, &id, Fonte::Sistema).ok(),
            _ => None,
        };
        let total = falas.len();
        let mut falhas = 0;
        for (i, fala) in falas.iter().enumerate() {
            self.progresso("importando", i + 1, total);
            if let Some(g) = gravador.as_mut() { let _ = g.gravar(fala); }
            if !transcrever_fala(&self.dep, &id, Fonte::Sistema, fala, &ctx, false).await {
                falhas += 1;
            }
        }
        if falhas == total {
            let _ = self.dep.store.lock().unwrap().apagar_sessao(&id);
            self.limpar_audio_orfao();
            return Err(anyhow!("Nenhum trecho do áudio pôde ser transcrito. Veja os avisos acima."));
        }

        // 4. resumo e resultado
        let fim = inicio_ms + (duracao_s * 1000.0) as i64;
        let saida = self.resumir_e_encerrar(&id, inicio_ms, fim).await?;
        self.avisar_final(&id, &saida);
        Ok(id)
    }

    /// Fecha as conversas que ficaram "em andamento" porque o app caiu: resume o que já estava transcrito
    /// (sem enviar e-mail) ou apaga as que ficaram vazias. Devolve os ids recuperados.
    pub async fn recuperar_orfas(&self) -> Vec<String> {
        if self.em_andamento() {
            return vec![];
        }
        let orfas = self.dep.store.lock().unwrap().sessoes_ativas(&self.dep.usuario).unwrap_or_default();
        let mut recuperadas = vec![];
        for s in orfas {
            let falas = self.dep.store.lock().unwrap().transcricao(&s.id).unwrap_or_default();
            let Some(ultima) = falas.iter().map(|f: &FalaGravada| f.timestamp).max() else {
                let _ = self.dep.store.lock().unwrap().apagar_sessao(&s.id);
                continue;
            };
            match self.resumir_e_encerrar(&s.id, s.started_at * 1000, ultima).await {
                Ok(_) => recuperadas.push(s.id),
                Err(e) => (self.dep.emitir)("reuniao:aviso", json!(format!("Não foi possível recuperar uma reunião interrompida: {e}"))),
            }
        }
        self.limpar_audio_orfao();
        if !recuperadas.is_empty() {
            (self.dep.emitir)("reuniao:recuperada", json!({"ids": recuperadas}));
        }
        recuperadas
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::meeting::testes::montar_pasta;
    use crate::settings::Config;

    fn falar_e_calar(r: &Reuniao, fonte: Fonte) {
        for _ in 0..6 { r.receber_audio(fonte, &[6000i16; 1600]); }
        for _ in 0..14 { r.receber_audio(fonte, &[0i16; 1600]); }
    }

    fn texto(r: &Reuniao, id: &str) -> Vec<String> {
        r.dep.store.lock().unwrap().transcricao(id).unwrap().into_iter().map(|f| f.text).collect()
    }

    #[tokio::test]
    async fn guarda_o_audio_e_retranscreve_trocando_o_texto_so_se_tudo_der_certo() {
        let (r, eventos, fila, pasta) = montar_pasta(Config { guardar_audio: true, ..Default::default() }, vec!["primeira"]);
        let id = r.iniciar().unwrap();
        falar_e_calar(&r, Fonte::Mic);
        r.parar().await.unwrap();
        assert!(r.tem_audio(&id));
        assert_eq!(texto(&r, &id), vec!["primeira"]);

        fila.0.lock().unwrap().push("segunda");
        r.retranscrever(&id).await.unwrap();
        assert_eq!(texto(&r, &id), vec!["segunda"]);
        let ev = eventos.lock().unwrap();
        assert!(ev.iter().any(|(c, d)| c == "reuniao:reprocesso" && d["fase"] == "retranscrevendo"));
        assert!(ev.iter().filter(|(c, _)| c == "reuniao:final").count() >= 2, "mostra o resultado de novo");
        drop(ev);

        // transcritor devolvendo vazio: a transcrição anterior fica
        let e = r.retranscrever(&id).await.unwrap_err().to_string();
        assert!(e.contains("vazia") && e.contains("mantida"), "{e}");
        assert_eq!(texto(&r, &id), vec!["segunda"]);

        // apagar a conversa leva o áudio junto
        r.dep.store.lock().unwrap().apagar_sessao(&id).unwrap();
        r.limpar_audio_orfao();
        assert!(!r.tem_audio(&id));
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn sem_a_opcao_ligada_nao_guarda_audio_e_nao_da_para_retranscrever() {
        let (r, _, _, pasta) = montar_pasta(Config { guardar_audio: false, ..Default::default() }, vec!["oi"]);
        let id = r.iniciar().unwrap();
        falar_e_calar(&r, Fonte::Mic);
        r.parar().await.unwrap();
        assert!(!r.tem_audio(&id));
        let e = r.retranscrever(&id).await.unwrap_err().to_string();
        assert!(e.contains("não tem áudio"), "{e}");
        assert!(r.retranscrever("nao-existe").await.is_err());
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn recupera_reuniao_interrompida_e_descarta_a_vazia() {
        let (r, eventos, _, pasta) = montar_pasta(Config::default(), vec!["ficou salvo"]);
        let id = r.iniciar().unwrap();
        falar_e_calar(&r, Fonte::Mic);
        while r.ativa.lock().unwrap().as_ref().is_some_and(|a| !a.tarefas.iter().all(|t| t.is_finished())) {
            tokio::task::yield_now().await;
        }
        *r.ativa.lock().unwrap() = None; // o app caiu aqui: a conversa ficou "em andamento"
        let vazia = r.iniciar().unwrap();
        *r.ativa.lock().unwrap() = None;

        let rec = r.recuperar_orfas().await;
        assert_eq!(rec, vec![id.clone()]);
        let s = r.dep.store.lock().unwrap().obter_sessao(&id).unwrap().unwrap();
        assert_eq!(s.status, "ended");
        assert!(r.dep.store.lock().unwrap().obter_sessao(&vazia).unwrap().is_none(), "sem falas, a conversa some");
        assert!(eventos.lock().unwrap().iter().any(|(c, _)| c == "reuniao:recuperada"));
        assert!(!eventos.lock().unwrap().iter().any(|(c, _)| c == "reuniao:final"), "recuperar não abre a tela de resultado");
        assert!(r.recuperar_orfas().await.is_empty());
        let _ = std::fs::remove_dir_all(pasta);
    }

    fn wav_com_fala() -> Vec<u8> {
        let (silencio, tom) = (16000usize, 19200usize);
        let amostras: Vec<i16> = (0..silencio * 2 + tom).map(|i| if (silencio..silencio + tom).contains(&i) { ((i as f32 * 0.1).sin() * 9000.0) as i16 } else { 0 }).collect();
        crate::wav::pcm16_para_wav(&amostras, 16000)
    }

    #[tokio::test]
    async fn importa_um_audio_como_conversa_nova() {
        let (r, eventos, _, pasta) = montar_pasta(Config { guardar_audio: true, ..Default::default() }, vec!["texto do arquivo"]);
        std::fs::create_dir_all(&pasta).unwrap();
        let arq = pasta.join("reuniao.wav");
        std::fs::write(&arq, wav_com_fala()).unwrap();

        let id = r.importar_audio(&arq).await.unwrap();
        let falas = r.dep.store.lock().unwrap().transcricao(&id).unwrap();
        assert_eq!(falas.len(), 1);
        assert_eq!((falas[0].text.as_str(), falas[0].speaker.as_deref()), ("texto do arquivo", Some("THEY")));
        assert_eq!(r.dep.store.lock().unwrap().obter_sessao(&id).unwrap().unwrap().status, "ended");
        assert!(r.tem_audio(&id), "com a opção ligada, o áudio importado também fica guardado");
        let ev = eventos.lock().unwrap();
        assert!(ev.iter().any(|(c, d)| c == "reuniao:reprocesso" && d["fase"] == "importando"));
        assert!(ev.iter().any(|(c, _)| c == "reuniao:final"));
        drop(ev);

        // arquivo sem fala, e arquivo que não é áudio
        let mudo = pasta.join("mudo.wav");
        std::fs::write(&mudo, crate::wav::pcm16_para_wav(&vec![0i16; 32000], 16000)).unwrap();
        assert!(r.importar_audio(&mudo).await.unwrap_err().to_string().contains("fala"));
        let lixo = pasta.join("lixo.mp3");
        std::fs::write(&lixo, b"nao sou audio".repeat(100)).unwrap();
        assert!(r.importar_audio(&lixo).await.is_err());
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn nao_importa_nem_retranscreve_com_reuniao_em_andamento() {
        let (r, _, _, pasta) = montar_pasta(Config::default(), vec![]);
        r.iniciar().unwrap();
        assert!(r.importar_audio(Path::new("x.wav")).await.unwrap_err().to_string().contains("em andamento"));
        assert!(r.retranscrever("x").await.unwrap_err().to_string().contains("em andamento"));
        assert!(r.recuperar_orfas().await.is_empty(), "com reunião rodando, nada é recuperado");
        r.descartar();
        let _ = std::fs::remove_dir_all(pasta);
    }
}
