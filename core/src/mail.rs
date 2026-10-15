//! E-mail da reunião, enviado pelo SMTP do Gmail com senha de app.

use crate::summary::Resumo;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub struct DadosEmail {
    pub titulo: String,
    pub data: String,
    pub resumo: Option<Resumo>,
    pub erro_resumo: Option<String>,
    pub transcricao: String,
}

pub struct Envio {
    pub usuario: String,
    pub senha: String,
    pub destinatario: String,
    pub dados: DadosEmail,
}

#[async_trait]
pub trait Remetente: Send + Sync {
    async fn enviar(&self, envio: Envio) -> Result<()>;
}

pub struct Montado {
    pub assunto: String,
    pub texto: String,
    pub html: String,
    pub anexo_nome: String,
    pub anexo: String,
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Monta assunto, corpo e anexo. `resumo` pode ser None se a IA falhou.
pub fn montar_email(d: &DadosEmail) -> Montado {
    let mut texto: Vec<String> = Vec::new();
    let mut html = String::new();

    if let Some(r) = &d.resumo {
        texto.extend(["RESUMO".into(), r.resumo.clone(), String::new()]);
        html += &format!("<h3>Resumo</h3><p>{}</p>", esc(&r.resumo));
        let mut bloco = |nome: &str, itens: Vec<String>| {
            texto.push(nome.to_uppercase());
            if itens.is_empty() { texto.push("- Nenhum".into()); } else { texto.extend(itens.iter().map(|i| format!("- {i}"))); }
            texto.push(String::new());
            html += &format!("<h3>{nome}</h3>");
            if itens.is_empty() { html += "<p>Nenhum</p>"; } else {
                html += &format!("<ul>{}</ul>", itens.iter().map(|i| format!("<li>{}</li>", esc(i))).collect::<String>());
            }
        };
        bloco("Decisões", r.decisoes.clone());
        bloco("Pendências", r.pendencias.iter().map(|p| {
            format!("{}{}{}", p.tarefa,
                p.responsavel.as_ref().map(|x| format!(" (responsável: {x})")).unwrap_or_default(),
                p.prazo.as_ref().map(|x| format!(" (prazo: {x})")).unwrap_or_default())
        }).collect());
        bloco("Dúvidas em aberto", r.duvidas.clone());
    } else {
        let aviso = format!("Não foi possível gerar o resumo{}", d.erro_resumo.as_ref().map(|e| format!(": {e}")).unwrap_or_else(|| ".".into()));
        html += &format!("<p>{}</p>", esc(&aviso));
        texto.extend([aviso, String::new()]);
    }
    texto.push("A transcrição completa está no arquivo anexo.".into());
    html += "<p>A transcrição completa está no arquivo anexo.</p>";

    Montado {
        assunto: format!("Reunião: {} ({})", d.titulo, d.data),
        texto: texto.join("\n"),
        html,
        anexo_nome: format!("transcricao-{}.txt", d.data),
        anexo: d.transcricao.clone(),
    }
}

/// Gmail por SMTP com TLS (porta 465).
pub struct SmtpGmail;

#[async_trait]
impl Remetente for SmtpGmail {
    async fn enviar(&self, e: Envio) -> Result<()> {
        if e.usuario.is_empty() || e.senha.is_empty() || e.destinatario.is_empty() {
            return Err(anyhow!("E-mail não configurado (Gmail, senha de app e destinatário)."));
        }
        let m = montar_email(&e.dados);
        let corpo = MultiPart::alternative()
            .singlepart(SinglePart::plain(m.texto))
            .singlepart(SinglePart::html(m.html));
        let anexo = Attachment::new(m.anexo_nome).body(m.anexo, ContentType::parse("text/plain; charset=utf-8")?);
        let msg = Message::builder()
            .from(e.usuario.parse().map_err(|_| anyhow!("Endereço do Gmail inválido."))?)
            .to(e.destinatario.parse().map_err(|_| anyhow!("Endereço do destinatário inválido."))?)
            .subject(m.assunto)
            .multipart(MultiPart::mixed().multipart(corpo).singlepart(anexo))?;
        let envio = AsyncSmtpTransport::<Tokio1Executor>::relay("smtp.gmail.com")?
            .credentials(Credentials::new(e.usuario, e.senha))
            .build();
        envio.send(msg).await.map_err(|er| anyhow!("Falha no envio: {er}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::store::Pendencia;

    fn dados(resumo: Option<Resumo>, erro: Option<&str>) -> DadosEmail {
        DadosEmail { titulo: "Vendas <b>".into(), data: "13-10-2026".into(), resumo, erro_resumo: erro.map(Into::into), transcricao: "linha".into() }
    }

    #[test]
    fn escapa_html_e_formata_pendencias() {
        let r = Resumo {
            titulo: "T".into(), resumo: "ok <script>".into(), decisoes: vec!["d".into()],
            pendencias: vec![Pendencia { tarefa: "enviar".into(), responsavel: Some("Ana".into()), prazo: Some("sexta".into()), feita: false }],
            duvidas: vec![],
        };
        let m = montar_email(&dados(Some(r), None));
        assert!(m.html.contains("enviar (responsável: Ana) (prazo: sexta)"));
        assert!(!m.html.contains("<script>"));
        assert_eq!(m.anexo, "linha");
        assert_eq!(m.assunto, "Reunião: Vendas <b> (13-10-2026)");
    }

    #[test]
    fn sem_resumo_explica_o_motivo() {
        let m = montar_email(&dados(None, Some("sem chave")));
        assert!(m.texto.contains("Não foi possível gerar o resumo: sem chave"));
    }

    #[tokio::test]
    async fn exige_configuracao() {
        let e = SmtpGmail.enviar(Envio { usuario: "".into(), senha: "".into(), destinatario: "".into(), dados: dados(None, None) }).await;
        assert!(e.unwrap_err().to_string().contains("não configurado"));
    }
}
