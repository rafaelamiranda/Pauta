'use strict';

const nodemailer = require('nodemailer');

const esc = (t) => String(t).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/** Monta assunto, corpo e anexo do e-mail. `resumo` pode ser null se a IA falhou. */
function montarEmail({ titulo, data, resumo, transcricao, erroResumo }) {
  const assunto = `Reunião: ${titulo} (${data})`;
  const linhas = [];
  const html = [];

  if (resumo) {
    linhas.push('RESUMO', resumo.resumo, '');
    html.push(`<h3>Resumo</h3><p>${esc(resumo.resumo)}</p>`);
    const bloco = (nome, itens, fmt) => {
      linhas.push(nome.toUpperCase());
      linhas.push(...(itens.length ? itens.map((i) => `- ${fmt(i)}`) : ['- Nenhum']), '');
      html.push(`<h3>${nome}</h3>${itens.length ? `<ul>${itens.map((i) => `<li>${esc(fmt(i))}</li>`).join('')}</ul>` : '<p>Nenhum</p>'}`);
    };
    const pend = (p) => `${p.tarefa}${p.responsavel ? ` (responsável: ${p.responsavel})` : ''}${p.prazo ? ` (prazo: ${p.prazo})` : ''}`;
    bloco('Decisões', resumo.decisoes, (d) => d);
    bloco('Pendências', resumo.pendencias, pend);
    bloco('Dúvidas em aberto', resumo.duvidas, (d) => d);
  } else {
    const aviso = `Não foi possível gerar o resumo${erroResumo ? `: ${erroResumo}` : '.'}`;
    linhas.push(aviso, '');
    html.push(`<p>${esc(aviso)}</p>`);
  }
  linhas.push('A transcrição completa está no arquivo anexo.');
  html.push('<p>A transcrição completa está no arquivo anexo.</p>');

  return {
    subject: assunto,
    text: linhas.join('\n'),
    html: html.join(''),
    attachments: [{ filename: `transcricao-${data}.txt`, content: transcricao, contentType: 'text/plain; charset=utf-8' }],
  };
}

/** Envia pelo SMTP do Gmail (senha de app). */
async function enviar({ usuario, senha, destinatario, email, criarTransporte = nodemailer.createTransport }) {
  if (!usuario || !senha || !destinatario) throw new Error('E-mail não configurado (Gmail, senha de app e destinatário).');
  const transporte = criarTransporte({ host: 'smtp.gmail.com', port: 465, secure: true, auth: { user: usuario, pass: senha } });
  await transporte.sendMail({ from: usuario, to: destinatario, ...email });
}

module.exports = { montarEmail, enviar };
