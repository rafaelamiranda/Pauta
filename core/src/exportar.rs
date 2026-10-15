//! Exporta uma conversa em Markdown (resumo, pendências e transcrição).

use crate::store::{FalaGravada, Sessao};
use crate::summary::formatar_transcricao;

fn data(seg: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(seg, 0).single().map(|d| d.format("%d/%m/%Y %H:%M").to_string()).unwrap_or_default()
}

pub fn markdown(s: &Sessao, falas: &[FalaGravada]) -> String {
    let mut m = format!("# {}\n\n{}\n", s.subject.as_deref().filter(|t| !t.is_empty()).unwrap_or("Reunião"), data(s.started_at));
    if let Some(r) = s.overview.as_deref().filter(|r| !r.is_empty()) {
        m += &format!("\n## Resumo\n\n{r}\n");
    }
    if !s.pending.is_empty() {
        m += "\n## Pendências\n\n";
        for p in &s.pending {
            m += &format!("- [{}] {}", if p.feita { "x" } else { " " }, p.tarefa);
            if let Some(r) = p.responsavel.as_deref().filter(|r| !r.is_empty()) {
                m += &format!(" (responsável: {r})");
            }
            if let Some(r) = p.prazo.as_deref().filter(|r| !r.is_empty()) {
                m += &format!(" (prazo: {r})");
            }
            m.push('\n');
        }
    }
    if !falas.is_empty() {
        m += "\n## Transcrição\n\n";
        for linha in formatar_transcricao(falas).lines() {
            m += &format!("{linha}\n\n");
        }
    }
    m
}

/// Nome de arquivo seguro para o Windows, a partir do título.
pub fn nome_de_arquivo(titulo: &str, extensao: &str) -> String {
    let limpo: String = titulo
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let limpo: String = limpo.chars().take(60).collect();
    format!("{}.{extensao}", if limpo.is_empty() { "Reuniao".to_string() } else { limpo })
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::store::Pendencia;

    #[test]
    fn markdown_tem_resumo_pendencias_e_transcricao() {
        let s = Sessao {
            id: "1".into(), user_id: "u".into(), subject: Some("Contrato Alfa".into()), overview: Some("Fechar contrato".into()),
            pending: vec![
                Pendencia { tarefa: "Enviar proposta".into(), responsavel: Some("Eu".into()), prazo: Some("sexta".into()), feita: false },
                Pendencia { tarefa: "Ligar".into(), feita: true, ..Default::default() },
            ],
            status: "ended".into(), started_at: 0, ended_at: Some(10),
        };
        let falas = [FalaGravada { speaker: Some("ME".into()), text: "Bom dia".into(), timestamp: 5 }];
        let m = markdown(&s, &falas);
        assert!(m.starts_with("# Contrato Alfa\n"));
        assert!(m.contains("## Resumo\n\nFechar contrato"));
        assert!(m.contains("- [ ] Enviar proposta (responsável: Eu) (prazo: sexta)"));
        assert!(m.contains("- [x] Ligar\n"));
        assert!(m.contains("[00:00:00] Eu: Bom dia"));
    }

    #[test]
    fn conversa_sem_nada_ainda_gera_titulo() {
        let s = Sessao { id: "1".into(), user_id: "u".into(), subject: None, overview: None, pending: vec![], status: "ended".into(), started_at: 0, ended_at: None };
        let m = markdown(&s, &[]);
        assert!(m.starts_with("# Reunião\n") && !m.contains("## "));
    }

    #[test]
    fn nome_de_arquivo_sem_caracteres_proibidos() {
        assert_eq!(nome_de_arquivo("Contrato: Alfa/Beta?", "md"), "Contrato Alfa Beta.md");
        assert_eq!(nome_de_arquivo("???", "md"), "Reuniao.md");
        assert!(nome_de_arquivo(&"a".repeat(200), "md").len() <= 63);
    }
}
