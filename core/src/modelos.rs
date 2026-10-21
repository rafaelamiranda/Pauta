//! Modelos de resumo: o tipo de reunião muda o foco do resumo, mas o formato da resposta
//! (título, resumo, decisões, pendências, dúvidas) é sempre o mesmo.
//! Além dos modelos do app, a pessoa pode criar os seus num arquivo JSON.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModeloResumo {
    pub id: String,
    pub nome: String,
    /// O que o resumo deve priorizar; vai para o modelo de linguagem.
    pub foco: String,
}

pub const PADRAO: &str = "padrao";

fn m(id: &str, nome: &str, foco: &str) -> ModeloResumo {
    ModeloResumo { id: id.into(), nome: nome.into(), foco: foco.into() }
}

/// Modelos que já vêm com o app. O primeiro (`padrao`) não acrescenta nada.
pub fn embutidos() -> Vec<ModeloResumo> {
    vec![
        m(PADRAO, "Reunião comum", ""),
        m("daily", "Reunião diária (daily)", "Reunião diária de equipe. No resumo, separe por pessoa: o que fez, o que vai fazer e o que a impede de avançar. Impedimentos entram em dúvidas ou pendências."),
        m("retrospectiva", "Retrospectiva", "Retrospectiva de equipe. No resumo, destaque o que funcionou, o que não funcionou e as melhorias propostas. Melhorias com dono ou prazo entram em pendências."),
        m("comercial", "Reunião comercial", "Conversa com cliente ou prospecto. No resumo, destaque necessidades e dores do cliente, objeções, preços e condições citados, próximos passos e prazos. Compromissos de envio de proposta ou retorno entram em pendências."),
        m("projeto", "Acompanhamento de projeto", "Acompanhamento de projeto. No resumo, destaque o andamento por frente de trabalho, riscos, bloqueios, mudanças de prazo ou escopo e decisões."),
    ]
}

/// Modelos do app mais os do arquivo da pessoa. Um modelo próprio com o mesmo id de um do app o substitui.
/// Arquivo ausente ou inválido é ignorado: os modelos do app continuam valendo.
pub fn todos(arquivo: Option<&std::path::Path>) -> Vec<ModeloResumo> {
    let mut lista = embutidos();
    let proprios: Vec<ModeloResumo> = arquivo
        .and_then(|a| std::fs::read_to_string(a).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    for p in proprios.into_iter().filter(|p| !p.id.trim().is_empty() && !p.nome.trim().is_empty()) {
        match lista.iter_mut().find(|e| e.id == p.id) {
            Some(e) => *e = p,
            None => lista.push(p),
        }
    }
    lista
}

/// Foco do modelo `id`; vazio se não existir (cai no resumo comum).
pub fn foco(id: &str, arquivo: Option<&std::path::Path>) -> String {
    todos(arquivo).into_iter().find(|m| m.id == id).map(|m| m.foco).unwrap_or_default()
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn padrao_nao_acrescenta_foco_e_id_desconhecido_cai_nele() {
        assert_eq!(foco(PADRAO, None), "");
        assert_eq!(foco("nao-existe", None), "");
        assert!(foco("daily", None).contains("impede"));
    }

    #[test]
    fn modelos_proprios_somam_ou_substituem_e_arquivo_ruim_e_ignorado() {
        let dir = std::env::temp_dir().join(format!("mod-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("modelos.json");
        std::fs::write(&a, r#"[{"id":"juridico","nome":"Jurídico","foco":"Cláusulas e prazos."},{"id":"daily","nome":"Daily nossa","foco":"Só bloqueios."},{"id":"","nome":"x","foco":"y"}]"#).unwrap();
        let t = todos(Some(&a));
        assert_eq!(t.len(), embutidos().len() + 1);
        assert_eq!(foco("juridico", Some(&a)), "Cláusulas e prazos.");
        assert_eq!(foco("daily", Some(&a)), "Só bloqueios.");
        std::fs::write(&a, "isto não é json").unwrap();
        assert_eq!(todos(Some(&a)), embutidos());
        assert_eq!(todos(Some(&dir.join("nada.json"))), embutidos());
        let _ = std::fs::remove_dir_all(dir);
    }
}
