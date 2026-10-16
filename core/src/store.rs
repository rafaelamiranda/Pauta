//! Conversas e mensagens em SQLite, com busca de texto completo (FTS5).

use crate::material::Documento;
use anyhow::{anyhow, Result};
use rusqlite::{params, params_from_iter, types::Value, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Pendencia {
    pub tarefa: String,
    pub responsavel: Option<String>,
    pub prazo: Option<String>,
    /// Marcada como feita na lista de pendências.
    #[serde(default)]
    pub feita: bool,
}

/// Pendência de uma reunião, com a posição dela na lista da reunião.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PendenciaGeral {
    pub sessao_id: String,
    pub titulo: Option<String>,
    pub data: i64,
    pub indice: usize,
    pub pendencia: Pendencia,
}

/// Material de apoio salvo para reutilizar (cliente ou projeto).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MaterialSalvo {
    pub id: String,
    pub nome: String,
    pub texto: String,
    pub documentos: Vec<Documento>,
    pub atualizado_em: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultadoSessao {
    pub id: String,
    pub subject: Option<String>,
    pub overview: Option<String>,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// Trecho da mensagem que casou, com o termo entre colchetes.
    pub snippet: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Busca {
    pub sessions: Vec<ResultadoSessao>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Sessao {
    pub id: String,
    pub user_id: String,
    pub subject: Option<String>,
    pub overview: Option<String>,
    pub pending: Vec<Pendencia>,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FalaGravada {
    pub speaker: Option<String>,
    pub text: String,
    pub timestamp: i64,
}

#[derive(Default)]
pub struct NovaSessao<'a> {
    pub id: &'a str,
    pub user_id: &'a str,
    pub subject: Option<&'a str>,
    pub overview: Option<&'a str>,
    pub pending: Option<&'a [Pendencia]>,
    pub ativa: bool,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

pub struct OpcoesBusca<'a> {
    pub texto: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
    pub excluir_ativas: bool,
}

impl Default for OpcoesBusca<'_> {
    fn default() -> Self {
        Self { texto: None, limit: 20, offset: 0, excluir_ativas: true }
    }
}

/// Transforma o texto digitado em uma consulta FTS5 segura: cada palavra vira um termo
/// entre aspas com prefixo (`"reun"*`), todos com AND. Evita erro de sintaxe com símbolos.
pub fn para_consulta_fts(texto: &str) -> Option<String> {
    let termos: Vec<String> = texto
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!termos.is_empty()).then(|| termos.join(" "))
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn abrir(caminho: &std::path::Path) -> Result<Self> {
        Self::de_conexao(Connection::open(caminho)?)
    }

    pub fn em_memoria() -> Result<Self> {
        Self::de_conexao(Connection::open_in_memory()?)
    }

    fn de_conexao(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::liberar_falantes(&conn)?;
        conn.execute_batch(include_str!("schema.sql"))?;
        let s = Self { conn };
        s.garantir_coluna("sessions", "context", "TEXT")?; // bancos criados antes do material de apoio
        s.reconstruir_indices()?;
        Ok(s)
    }

    /// Bancos antigos só aceitavam "ME" e "THEY" como falante. Agora também vale "THEY:Pessoa 2".
    /// O SQLite não altera uma regra CHECK, então a tabela é refeita, mantendo os rowids (o índice de busca usa eles).
    fn liberar_falantes(conn: &Connection) -> Result<()> {
        use rusqlite::OptionalExtension;
        let sql: Option<String> = conn.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'messages'", [], |r| r.get(0)).optional()?;
        if !sql.is_some_and(|s| s.contains("speaker IN ('ME', 'THEY')") && !s.contains("LIKE")) {
            return Ok(());
        }
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE messages_nova (
               id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
               content TEXT NOT NULL,
               speaker TEXT CHECK(speaker IS NULL OR speaker IN ('ME', 'THEY') OR speaker LIKE 'THEY:%'),
               timestamp INTEGER NOT NULL,
               FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
             );
             INSERT INTO messages_nova (rowid, id, session_id, role, content, speaker, timestamp)
               SELECT rowid, id, session_id, role, content, speaker, timestamp FROM messages;
             DROP TABLE messages;
             ALTER TABLE messages_nova RENAME TO messages;
             COMMIT;",
        )?;
        Ok(())
    }

    fn garantir_coluna(&self, tabela: &str, coluna: &str, tipo: &str) -> Result<()> {
        let mut st = self.conn.prepare(&format!("PRAGMA table_info({tabela})"))?;
        let existe = st.query_map([], |r| r.get::<_, String>(1))?.any(|n| n.map(|n| n == coluna).unwrap_or(false));
        if !existe {
            self.conn.execute_batch(&format!("ALTER TABLE {tabela} ADD COLUMN {coluna} {tipo}"))?;
        }
        Ok(())
    }

    /// Reconstrói os índices quando há dados mas o índice está vazio.
    fn reconstruir_indices(&self) -> Result<()> {
        let n = |t: &str| -> Result<i64> { Ok(self.conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))?) };
        if n("sessions")? > 0 && n("sessions_fts")? == 0 {
            self.conn.execute_batch("INSERT INTO sessions_fts(rowid, id, subject, overview) SELECT rowid, id, subject, overview FROM sessions")?;
        }
        if n("messages")? > 0 && n("messages_fts")? == 0 {
            self.conn.execute_batch("INSERT INTO messages_fts(rowid, id, session_id, content) SELECT rowid, id, session_id, content FROM messages")?;
        }
        Ok(())
    }

    pub fn adicionar_sessao(&self, s: NovaSessao) -> Result<()> {
        let pend = s.pending.map(serde_json::to_string).transpose()?;
        self.conn.execute(
            "INSERT INTO sessions (id, user_id, subject, overview, pending, status, started_at, ended_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![s.id, s.user_id, s.subject, s.overview, pend, if s.ativa { "active" } else { "ended" }, s.started_at, s.ended_at],
        )?;
        Ok(())
    }

    pub fn adicionar_mensagem(&self, id: &str, sessao_id: &str, texto: &str, falante: Option<&str>, timestamp: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO messages (id, session_id, role, content, speaker, timestamp) VALUES (?, ?, 'user', ?, ?, ?)",
            params![id, sessao_id, texto, falante, timestamp],
        )?;
        Ok(())
    }

    /// Encerra a reunião e grava o resumo e as pendências.
    pub fn encerrar_sessao(&self, id: &str, fim_s: i64, titulo: Option<&str>, resumo: Option<&str>, pendencias: &[Pendencia]) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = 'ended', ended_at = ?, subject = COALESCE(?, subject), overview = ?, pending = ? WHERE id = ?",
            params![fim_s, titulo, resumo, serde_json::to_string(pendencias)?, id],
        )?;
        Ok(())
    }

    /// Material de apoio (texto colado) ligado à conversa.
    pub fn definir_contexto(&self, id: &str, texto: &str) -> Result<()> {
        self.conn.execute("UPDATE sessions SET context = ? WHERE id = ?", params![texto, id])?;
        Ok(())
    }

    pub fn contexto(&self, id: &str) -> Result<Option<String>> {
        let r = self.conn.query_row("SELECT context FROM sessions WHERE id = ?", params![id], |r| r.get::<_, Option<String>>(0));
        match r {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Apaga uma conversa (e as mensagens dela). Usado quando a reunião nem chega a começar.
    pub fn apagar_sessao(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE id = ?", params![id])?;
        Ok(())
    }

    pub fn obter_sessao(&self, id: &str) -> Result<Option<Sessao>> {
        let mut st = self.conn.prepare(
            "SELECT id, user_id, subject, overview, pending, status, started_at, ended_at FROM sessions WHERE id = ?",
        )?;
        let mut linhas = st.query(params![id])?;
        let Some(r) = linhas.next()? else { return Ok(None) };
        let pend: Option<String> = r.get(4)?;
        Ok(Some(Sessao {
            id: r.get(0)?,
            user_id: r.get(1)?,
            subject: r.get(2)?,
            overview: r.get(3)?,
            pending: pend.map(|p| serde_json::from_str(&p)).transpose()?.unwrap_or_default(),
            status: r.get(5)?,
            started_at: r.get(6)?,
            ended_at: r.get(7)?,
        }))
    }

    /// Falas da reunião em ordem de tempo.
    pub fn transcricao(&self, sessao_id: &str) -> Result<Vec<FalaGravada>> {
        let mut st = self.conn.prepare("SELECT speaker, content, timestamp FROM messages WHERE session_id = ? ORDER BY timestamp, rowid")?;
        let linhas = st.query_map(params![sessao_id], |r| {
            Ok(FalaGravada { speaker: r.get(0)?, text: r.get(1)?, timestamp: r.get(2)? })
        })?;
        Ok(linhas.collect::<std::result::Result<_, _>>()?)
    }

    /// Busca conversas do usuário: uma entra se o termo aparece no assunto, no resumo ou em
    /// qualquer mensagem. Sem termo, lista todas (paginadas).
    pub fn buscar(&self, user_id: &str, op: OpcoesBusca) -> Result<Busca> {
        if user_id.is_empty() {
            return Ok(Busca { sessions: vec![], total: 0 });
        }
        let limite = op.limit.clamp(1, 200);
        let pular = op.offset.max(0);

        let mut cond = vec!["s.user_id = ?".to_string()];
        let mut params: Vec<Value> = vec![user_id.to_string().into()];
        if op.excluir_ativas {
            cond.push("s.status != 'active'".into());
        }

        let texto = op.texto.map(str::trim).filter(|t| !t.is_empty());
        let fts = match texto {
            Some(t) => match para_consulta_fts(t) {
                Some(q) => Some(q),
                None => return Ok(Busca { sessions: vec![], total: 0 }), // só símbolos
            },
            None => None,
        };
        if let Some(q) = &fts {
            cond.push(
                "(s.id IN (SELECT id FROM sessions_fts WHERE sessions_fts MATCH ?)
                  OR s.id IN (SELECT session_id FROM messages_fts WHERE messages_fts MATCH ?))"
                    .into(),
            );
            params.push(q.clone().into());
            params.push(q.clone().into());
        }
        let onde = format!("WHERE {}", cond.join(" AND "));

        let sql = format!(
            "SELECT s.id, s.subject, s.overview, s.status, s.started_at, s.ended_at FROM sessions s {onde}
             ORDER BY (s.ended_at IS NULL) DESC, COALESCE(s.ended_at, s.started_at) DESC LIMIT ? OFFSET ?"
        );
        let mut p_pagina = params.clone();
        p_pagina.push(limite.into());
        p_pagina.push(pular.into());
        let mut st = self.conn.prepare(&sql)?;
        let mut sessions: Vec<ResultadoSessao> = st
            .query_map(params_from_iter(p_pagina), |r| {
                Ok(ResultadoSessao {
                    id: r.get(0)?, subject: r.get(1)?, overview: r.get(2)?, status: r.get(3)?,
                    started_at: r.get(4)?, ended_at: r.get(5)?, snippet: None,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;

        let total: i64 = self.conn.query_row(&format!("SELECT COUNT(*) FROM sessions s {onde}"), params_from_iter(params), |r| r.get(0))?;

        if let Some(q) = &fts {
            let mut snip = self.conn.prepare(
                "SELECT snippet(messages_fts, 2, '[', ']', '…', 12) FROM messages_fts
                 WHERE session_id = ? AND messages_fts MATCH ? ORDER BY rank LIMIT 1",
            )?;
            for s in &mut sessions {
                s.snippet = snip.query_row(params![s.id, q], |r| r.get(0)).ok();
            }
        }
        Ok(Busca { sessions, total })
    }

    /// Apaga todas as conversas encerradas do usuário (as mensagens vão junto). Devolve quantas.
    pub fn apagar_todas(&self, user_id: &str) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM sessions WHERE user_id = ? AND status != 'active'", params![user_id])?)
    }

    /// Apaga as conversas encerradas antes de `limite_s` (segundos desde 1970). Devolve quantas.
    pub fn apagar_antes_de(&self, user_id: &str, limite_s: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM sessions WHERE user_id = ? AND status != 'active' AND COALESCE(ended_at, started_at) < ?",
            params![user_id, limite_s],
        )?)
    }

    /// Pendências de todas as reuniões encerradas, das mais recentes para as mais antigas.
    pub fn listar_pendencias(&self, user_id: &str, incluir_feitas: bool) -> Result<Vec<PendenciaGeral>> {
        let mut st = self.conn.prepare(
            "SELECT id, subject, COALESCE(ended_at, started_at), pending FROM sessions
             WHERE user_id = ? AND status != 'active' AND pending IS NOT NULL ORDER BY COALESCE(ended_at, started_at) DESC",
        )?;
        let linhas = st.query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)))?;
        let mut saida = vec![];
        for l in linhas {
            let (id, titulo, data, json) = l?;
            let lista: Vec<Pendencia> = serde_json::from_str(&json).unwrap_or_default();
            for (indice, pendencia) in lista.into_iter().enumerate() {
                if incluir_feitas || !pendencia.feita {
                    saida.push(PendenciaGeral { sessao_id: id.clone(), titulo: titulo.clone(), data, indice, pendencia });
                }
            }
        }
        Ok(saida)
    }

    pub fn marcar_pendencia(&self, sessao_id: &str, indice: usize, feita: bool) -> Result<()> {
        let mut sessao = self.obter_sessao(sessao_id)?.ok_or_else(|| anyhow!("Conversa não encontrada."))?;
        let p = sessao.pending.get_mut(indice).ok_or_else(|| anyhow!("Pendência não encontrada."))?;
        p.feita = feita;
        self.conn.execute("UPDATE sessions SET pending = ? WHERE id = ?", params![serde_json::to_string(&sessao.pending)?, sessao_id])?;
        Ok(())
    }

    /// Cria ou atualiza um material salvo. Sem `id`, cria. Devolve o id.
    pub fn salvar_material(&self, user_id: &str, id: Option<&str>, nome: &str, texto: &str, documentos: &[Documento], agora_s: i64) -> Result<String> {
        let nome = nome.trim();
        if nome.is_empty() {
            return Err(anyhow!("Dê um nome ao material."));
        }
        let id = id.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let docs = serde_json::to_string(documentos)?;
        let repetido: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM materiais WHERE user_id = ? AND lower(nome) = lower(?) AND id != ?", params![user_id, nome, id], |r| r.get(0),
        )?;
        if repetido > 0 {
            return Err(anyhow!("Já existe um material com o nome \"{nome}\"."));
        }
        self.conn.execute(
            "INSERT INTO materiais (id, user_id, nome, texto, documentos, atualizado_em) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET nome = ?3, texto = ?4, documentos = ?5, atualizado_em = ?6 WHERE user_id = ?2",
            params![id, user_id, nome, texto, docs, agora_s],
        )?;
        Ok(id)
    }

    pub fn listar_materiais(&self, user_id: &str) -> Result<Vec<MaterialSalvo>> {
        let mut st = self.conn.prepare("SELECT id, nome, texto, documentos, atualizado_em FROM materiais WHERE user_id = ? ORDER BY lower(nome)")?;
        let linhas = st.query_map(params![user_id], |r| {
            let docs: String = r.get(3)?;
            Ok(MaterialSalvo {
                id: r.get(0)?, nome: r.get(1)?, texto: r.get(2)?,
                documentos: serde_json::from_str(&docs).unwrap_or_default(), atualizado_em: r.get(4)?,
            })
        })?;
        Ok(linhas.collect::<std::result::Result<_, _>>()?)
    }

    pub fn apagar_material(&self, user_id: &str, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM materiais WHERE id = ? AND user_id = ?", params![id, user_id])?;
        Ok(())
    }

    /// Copia o banco inteiro para um arquivo novo (backup). O destino não pode existir.
    pub fn copiar_para(&self, destino: &std::path::Path) -> Result<()> {
        if destino.exists() {
            return Err(anyhow!("O arquivo de destino já existe."));
        }
        self.conn.execute("VACUUM INTO ?", params![destino.to_string_lossy()])?;
        Ok(())
    }

    /// Acesso direto ao SQL, só para os testes.
    #[cfg(test)]
    pub fn sql(&self, q: &str) {
        self.conn.execute_batch(q).unwrap();
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn exemplo() -> Store {
        let s = Store::em_memoria().unwrap();
        let nova = |id, user, assunto, resumo, ativa, ini, fim| NovaSessao {
            id, user_id: user, subject: Some(assunto), overview: resumo, ativa, started_at: ini, ended_at: fim, ..Default::default()
        };
        s.adicionar_sessao(nova("s1", "u1", "Reunião de vendas", Some("Metas do trimestre"), false, 100, Some(200))).unwrap();
        s.adicionar_sessao(nova("s2", "u1", "Orçamento", Some("Custos"), false, 300, Some(400))).unwrap();
        s.adicionar_sessao(nova("s3", "u1", "Ativa", None, true, 500, None)).unwrap();
        s.adicionar_sessao(nova("s4", "u2", "Reunião de outro usuário", None, false, 100, Some(200))).unwrap();
        s.adicionar_mensagem("m1", "s2", "Precisamos revisar o contrato com o fornecedor", None, 310).unwrap();
        s.adicionar_mensagem("m2", "s3", "contrato ativo", None, 510).unwrap();
        s
    }

    fn ids(b: &Busca) -> Vec<&str> {
        b.sessions.iter().map(|s| s.id.as_str()).collect()
    }

    fn busca<'a>(t: &'a str) -> OpcoesBusca<'a> {
        OpcoesBusca { texto: Some(t), ..Default::default() }
    }

    #[test]
    fn acha_pelo_assunto_sem_diferenciar_acento_nem_maiuscula() {
        assert_eq!(ids(&exemplo().buscar("u1", busca("REUNIAO")).unwrap()), ["s1"]);
    }

    #[test]
    fn acha_pelo_conteudo_e_devolve_trecho() {
        let b = exemplo().buscar("u1", busca("contrato")).unwrap();
        assert_eq!(b.total, 1);
        assert_eq!(b.sessions[0].id, "s2");
        assert!(b.sessions[0].snippet.as_deref().unwrap().contains("[contrato]"));
    }

    #[test]
    fn aceita_prefixo() {
        assert_eq!(ids(&exemplo().buscar("u1", busca("fornec")).unwrap()), ["s2"]);
    }

    #[test]
    fn nao_vaza_outro_usuario_e_oculta_ativas() {
        let s = exemplo();
        assert_eq!(s.buscar("u1", busca("outro")).unwrap().total, 0);
        assert_eq!(s.buscar("u1", busca("contrato")).unwrap().sessions.len(), 1);
        let todas = OpcoesBusca { texto: Some("contrato"), excluir_ativas: false, ..Default::default() };
        assert_eq!(s.buscar("u1", todas).unwrap().total, 2);
    }

    #[test]
    fn simbolos_nao_quebram_a_consulta() {
        let s = exemplo();
        for q in ["\"", "a OR (", "-", "NEAR(", "o'brien", "%", "_"] {
            assert!(s.buscar("u1", busca(q)).is_ok(), "falhou com {q:?}");
        }
    }

    #[test]
    fn mensagem_editada_ou_apagada_atualiza_o_indice() {
        let s = exemplo();
        s.sql("UPDATE messages SET content = 'nada relevante' WHERE id = 'm1'");
        assert_eq!(s.buscar("u1", busca("contrato")).unwrap().total, 0);
        s.sql("UPDATE messages SET content = 'prazo final' WHERE id = 'm1'");
        assert_eq!(s.buscar("u1", busca("prazo")).unwrap().total, 1);
        s.sql("DELETE FROM messages WHERE id = 'm1'");
        assert_eq!(s.buscar("u1", busca("prazo")).unwrap().total, 0);
    }

    #[test]
    fn sem_termo_lista_tudo_paginado() {
        let b = exemplo().buscar("u1", OpcoesBusca { limit: 1, offset: 1, ..Default::default() }).unwrap();
        assert_eq!(b.total, 2);
        assert_eq!(b.sessions.len(), 1);
    }

    #[test]
    fn consulta_fts() {
        assert_eq!(para_consulta_fts("reunião vendas").unwrap(), "\"reunião\"* \"vendas\"*");
        assert!(para_consulta_fts("!!!").is_none());
    }

    #[test]
    fn material_de_apoio_e_migracao_de_banco_antigo() {
        let s = exemplo();
        assert_eq!(s.contexto("s1").unwrap(), None);
        s.definir_contexto("s1", "Pauta: orçamento 2027").unwrap();
        assert_eq!(s.contexto("s1").unwrap().as_deref(), Some("Pauta: orçamento 2027"));
        assert_eq!(s.contexto("nao-existe").unwrap(), None);

        // banco criado antes da coluna "context": abrir de novo deve acrescentá-la
        let arq = std::env::temp_dir().join(format!("antigo-{}.db", uuid::Uuid::new_v4()));
        {
            let c = Connection::open(&arq).unwrap();
            c.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, subject TEXT, overview TEXT, pending TEXT,
                status TEXT NOT NULL DEFAULT 'ended', started_at INTEGER NOT NULL, ended_at INTEGER);
                INSERT INTO sessions (id, user_id, started_at) VALUES ('velha', 'u', 1);").unwrap();
        }
        let novo = Store::abrir(&arq).unwrap();
        novo.definir_contexto("velha", "ok").unwrap();
        assert_eq!(novo.contexto("velha").unwrap().as_deref(), Some("ok"));
        drop(novo);
        let _ = std::fs::remove_file(arq);
    }

    #[test]
    fn banco_antigo_passa_a_aceitar_falante_com_nome_sem_perder_a_busca() {
        let arq = std::env::temp_dir().join(format!("antigo-{}.db", uuid::Uuid::new_v4()));
        {
            let c = Connection::open(&arq).unwrap();
            // esquema de antes da identificação de falantes
            let antigo = include_str!("schema.sql").replace("speaker TEXT CHECK(speaker IS NULL OR speaker IN ('ME', 'THEY') OR speaker LIKE 'THEY:%'),", "speaker TEXT CHECK(speaker IN ('ME', 'THEY')),");
            c.execute_batch(&antigo).unwrap();
            c.execute_batch("INSERT INTO sessions (id, user_id, started_at) VALUES ('s', 'u', 1);
                INSERT INTO messages (id, session_id, role, content, speaker, timestamp) VALUES ('m1', 's', 'user', 'orçamento aprovado', 'THEY', 5);").unwrap();
            assert!(c.execute_batch("INSERT INTO messages (id, session_id, role, content, speaker, timestamp) VALUES ('m2', 's', 'user', 'x', 'THEY:Pessoa 1', 6)").is_err());
        }
        let s = Store::abrir(&arq).unwrap();
        s.adicionar_mensagem("m3", "s", "prazo é sexta", Some("THEY:Pessoa 1"), 7).unwrap();
        assert!(s.adicionar_mensagem("m4", "s", "x", Some("QUALQUER"), 8).is_err(), "a regra continua valendo");
        let falas = s.transcricao("s").unwrap();
        assert_eq!(falas.iter().map(|f| f.speaker.as_deref().unwrap()).collect::<Vec<_>>(), ["THEY", "THEY:Pessoa 1"]);
        assert_eq!(s.buscar("u", busca("orcamento")).unwrap().total, 1, "o índice de busca continua ligado às mensagens antigas");
        assert_eq!(s.buscar("u", busca("sexta")).unwrap().total, 1, "e as novas entram no índice");
        drop(s);
        let _ = Store::abrir(&arq).unwrap(); // abrir de novo não refaz nada
        let _ = std::fs::remove_file(arq);
    }

    #[test]
    fn encerrar_e_ler_a_sessao() {
        let s = exemplo();
        let pend = [Pendencia { tarefa: "Enviar".into(), prazo: Some("sexta".into()), ..Default::default() }];
        s.encerrar_sessao("s3", 999, Some("Título"), Some("Resumo"), &pend).unwrap();
        let sessao = s.obter_sessao("s3").unwrap().unwrap();
        assert_eq!(sessao.status, "ended");
        assert_eq!(sessao.pending, pend);
        assert_eq!(s.buscar("u1", busca("titulo")).unwrap().total, 1, "título novo fica pesquisável");
    }

    #[test]
    fn apagar_uma_todas_ou_as_antigas_limpa_mensagens_e_indice() {
        let s = exemplo();
        s.apagar_sessao("s2").unwrap();
        assert_eq!(s.buscar("u1", busca("contrato")).unwrap().total, 0, "some da busca");
        assert!(s.transcricao("s2").unwrap().is_empty());

        s.adicionar_sessao(NovaSessao { id: "s5", user_id: "u1", ended_at: Some(900), started_at: 800, ..Default::default() }).unwrap();
        assert_eq!(s.apagar_antes_de("u1", 250).unwrap(), 1);
        assert!(s.obter_sessao("s1").unwrap().is_none());
        assert!(s.obter_sessao("s4").unwrap().is_some(), "outro usuário fica");
        assert!(s.obter_sessao("s3").unwrap().is_some(), "a reunião ativa nunca é apagada");

        assert_eq!(s.apagar_todas("u1").unwrap(), 1);
        assert!(s.obter_sessao("s5").unwrap().is_none());
        assert!(s.obter_sessao("s3").unwrap().is_some());
        assert!(s.obter_sessao("s4").unwrap().is_some());
    }

    #[test]
    fn lista_geral_de_pendencias_e_marcar_como_feita() {
        let s = exemplo();
        let a = Pendencia { tarefa: "Enviar proposta".into(), ..Default::default() };
        let b = Pendencia { tarefa: "Ligar".into(), responsavel: Some("Eu".into()), ..Default::default() };
        s.encerrar_sessao("s1", 200, None, None, &[a.clone(), b.clone()]).unwrap();
        s.encerrar_sessao("s2", 400, None, None, &[]).unwrap();
        s.encerrar_sessao("s4", 400, None, None, &[a.clone()]).unwrap();
        let todas = s.listar_pendencias("u1", true).unwrap();
        assert_eq!(todas.len(), 2);
        assert_eq!((todas[0].sessao_id.as_str(), todas[0].indice), ("s1", 0));
        s.marcar_pendencia("s1", 0, true).unwrap();
        let abertas = s.listar_pendencias("u1", false).unwrap();
        assert_eq!(abertas.len(), 1);
        assert_eq!(abertas[0].pendencia.tarefa, "Ligar");
        assert_eq!(s.listar_pendencias("u1", true).unwrap().len(), 2);
        assert!(s.marcar_pendencia("s1", 9, true).is_err());
        assert!(s.marcar_pendencia("nao", 0, true).is_err());
        // pendência gravada antes do campo "feita" continua legível
        s.sql("UPDATE sessions SET pending = '[{\"tarefa\":\"Velha\",\"responsavel\":null,\"prazo\":null}]' WHERE id = 's1'");
        assert!(!s.listar_pendencias("u1", true).unwrap()[0].pendencia.feita);
    }

    #[test]
    fn materiais_salvos_por_cliente() {
        let s = exemplo();
        let doc = Documento { id: "drive:1".into(), nome: "Preços".into(), origem: "drive".into(), texto: "Ouro: 100".into() };
        let id = s.salvar_material("u1", None, " Cliente Alfa ", "Pauta Alfa", &[doc.clone()], 10).unwrap();
        s.salvar_material("u1", None, "Projeto Beta", "Pauta Beta", &[], 11).unwrap();
        s.salvar_material("u2", None, "Cliente Alfa", "de outro usuário", &[], 12).unwrap();
        let lista = s.listar_materiais("u1").unwrap();
        assert_eq!(lista.iter().map(|m| m.nome.as_str()).collect::<Vec<_>>(), ["Cliente Alfa", "Projeto Beta"]);
        assert_eq!(lista[0].documentos, vec![doc]);
        assert!(s.salvar_material("u1", None, "cliente alfa", "", &[], 13).unwrap_err().to_string().contains("Já existe"));
        assert!(s.salvar_material("u1", None, "  ", "", &[], 13).is_err());
        s.salvar_material("u1", Some(&id), "Cliente Alfa", "Pauta nova", &[], 14).unwrap();
        let alfa = &s.listar_materiais("u1").unwrap()[0];
        assert_eq!((alfa.texto.as_str(), alfa.documentos.len(), alfa.atualizado_em), ("Pauta nova", 0, 14));
        // outro usuário não consegue sobrescrever nem apagar o material
        s.salvar_material("u2", Some(&id), "Invasor", "x", &[], 15).ok();
        assert_eq!(s.listar_materiais("u1").unwrap()[0].nome, "Cliente Alfa");
        s.apagar_material("u2", &id).unwrap();
        assert_eq!(s.listar_materiais("u1").unwrap().len(), 2);
        s.apagar_material("u1", &id).unwrap();
        assert_eq!(s.listar_materiais("u1").unwrap().len(), 1);
    }

    #[test]
    fn backup_copia_o_banco() {
        let s = exemplo();
        let destino = std::env::temp_dir().join(format!("backup-{}.db", uuid::Uuid::new_v4()));
        s.copiar_para(&destino).unwrap();
        assert!(s.copiar_para(&destino).is_err(), "não sobrescreve");
        let copia = Store::abrir(&destino).unwrap();
        assert_eq!(copia.buscar("u1", busca("contrato")).unwrap().total, 1);
        drop(copia);
        let _ = std::fs::remove_file(destino);
    }
}
