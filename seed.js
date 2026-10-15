'use strict';

/** Conversas de exemplo (fictícias) para a tela não abrir em branco. */
function seedIfEmpty(store, userId) {
  if (store.searchSessions(userId, { limit: 1 }).total > 0) return;
  const day = 86400;
  const now = Math.floor(Date.now() / 1000);
  const exemplos = [
    ['Reunião de vendas', 'Metas do trimestre e plano de ação', 'Vamos fechar a meta de vendas até o fim do mês e revisar o contrato com o fornecedor.'],
    ['Orçamento do projeto', 'Custos e prazos', 'O orçamento precisa de aprovação antes da próxima etapa. O prazo final é sexta.'],
    ['Entrevista de candidato', 'Perfil e próximos passos', 'O candidato tem experiência com atendimento e boa comunicação.'],
    ['Planejamento da semana', 'Prioridades e tarefas', 'Prioridade um: enviar a proposta. Prioridade dois: atualizar a planilha de custos.'],
  ];
  exemplos.forEach(([subject, overview, texto], i) => {
    const id = `exemplo-${i}`;
    const startedAt = now - (i + 1) * day;
    store.addSession({ id, userId, subject, overview, startedAt, endedAt: startedAt + 1800 });
    store.addMessage({ id: `${id}-m`, sessionId: id, role: 'user', content: texto, timestamp: startedAt + 60 });
  });
}

module.exports = { seedIfEmpty };
