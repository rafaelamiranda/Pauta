# Pauta

App para Windows e Linux que fica numa janela flutuante sobre a tela. Ele faz três coisas:

- Transcreve reuniões em tempo real (microfone e som do computador).
- Ao finalizar, mostra o resumo e as pendências na tela e envia a transcrição por e-mail.
- Responde perguntas em texto usando o material de apoio (texto colado e documentos anexados) e o que foi dito na reunião. A resposta diz de onde veio cada informação.
- Aceita documentos do Google Drive, PDF, Word e TXT como material de apoio.

Todas as conversas ficam salvas e podem ser buscadas por assunto, resumo ou qualquer trecho falado.

O projeto nasceu de um estudo de como apps desse tipo fazem busca, transcrição e resumo. Não usa código, marca nem arquivos dele.

## Como funciona

| Parte | O que usa |
|---|---|
| Janela e atalho | Tauri 2 (Rust). Transparente, sem borda, sempre por cima. `Ctrl+Shift+Espaço` mostra ou esconde. |
| Captura de áudio | Windows: `cpal`, com loopback do WASAPI. Linux: `parec` (PulseAudio ou PipeWire), com o monitor da saída. Microfone ("Eu") e som do computador ("Outros"). |
| Divisão em falas | Detector de voz por volume (RMS). Fecha a fala após 700 ms de silêncio. |
| Transcrição | OpenAI `gpt-4o-transcribe`, ou whisper.cpp no próprio computador (opcional). Nomes e siglas do material vão como dica ao transcritor. |
| Identificação de falantes | Opcional, só com a OpenAI: `gpt-4o-transcribe-diarize` separa as pessoas no som do computador. |
| Resumo, pendências e perguntas | OpenAI `gpt-4o-mini`, ou llama.cpp no próprio computador (opcional). |
| Busca | SQLite FTS5. Ignora acento e maiúscula, aceita início de palavra. |
| Google Drive | API do Drive, somente leitura. Login pelo navegador (OAuth com PKCE). |
| Documentos | Texto de Google Docs, Planilhas e Apresentações (exportados pelo Google), TXT, MD, CSV, JSON, HTML, DOCX e PDF com texto. |
| E-mail | SMTP do Gmail com senha de app. |
| Segredos | Windows: Gerenciador de Credenciais. Linux: chaveiro do sistema, pelo `secret-tool`. |

Os dois modelos, o idioma, o microfone e a saída de áudio podem ser trocados na aba Ajustes.

## O que o app faz hoje

- **Bandeja do Windows:** o app tem um ícone ao lado do relógio. Clique para mostrar ou esconder a janela. Clique com o botão direito e escolha **Sair da Pauta** para fechar. Com reunião em andamento, o app não fecha: finalize antes.
- **Reunião:** transcrição ao vivo com medidor de volume do microfone e do som do computador. `Ctrl+Shift+R` inicia e finaliza a reunião mesmo com a janela escondida.
- **Perguntas:** caixa de pergunta, botão "O que respondo?" (fala pronta em primeira pessoa) e, durante a reunião, "O que eu perdi?" (últimos 5 minutos). A resposta aparece aos poucos, enquanto é escrita. Botão para copiar a resposta.
- **Dica de termos:** o app tira do material de apoio (texto colado e documentos) os nomes próprios, siglas e códigos, os mais repetidos primeiro, e os envia como dica ao transcritor (até 300 caracteres), para errar menos esses nomes. A lista é refeita quando o material muda durante a reunião. Nos Ajustes dá para desligar, caso o modelo comece a repetir a dica no lugar da fala (o app descarta esse tipo de resposta). Funciona na OpenAI e no Whisper local.
- **Identificação de falantes (opcional):** nos Ajustes, "Separar as pessoas em Outros". No som do computador, cada trecho vai para o modelo `gpt-4o-transcribe-diarize`, e as vozes aparecem como "Pessoa 1", "Pessoa 2" (até 4). Para manter o mesmo nome entre um trecho e outro, o app guarda de 2 a 10 segundos da voz de cada pessoa e envia como referência nos trechos seguintes. Isso põe os trechos do som do computador numa fila, então a transcrição pode atrasar mais. Se a pessoa falou menos de 2 segundos, ou já são 4 vozes, o texto fica como "Outros". O resumo e o e-mail usam esses nomes. Não vale na transcrição local, e o microfone continua sendo "Eu".
- **Material de apoio:** texto colado, documentos anexados (arquivo do computador ou Google Drive) e materiais salvos por cliente ou projeto, para reutilizar sem colar de novo.
- **Pendências:** aba com as pendências de todas as reuniões, com marcar como feita.
- **Conversas:** copiar ou salvar em Markdown (pasta Downloads), apagar uma conversa, apagar tudo, apagar sozinho reuniões com mais de N dias, e salvar uma cópia do banco.

### Modelos locais (opcional)

Nos Ajustes, seção **Modelos locais**, dá para trocar a nuvem pelo próprio computador, separadamente para a transcrição e para a análise (resumo, pendências e perguntas). Nesses modos, o áudio e o texto não vão para a OpenAI.

- **Botão Baixar:** baixa o programa (whisper.cpp ou llama.cpp, do GitHub oficial deles) e o modelo (do Hugging Face) para a pasta de dados do app. Mostra a barra de progresso e o tamanho, tem Cancelar (apaga o parcial), continua de onde parou se a conexão cair, confere tamanho e hash do arquivo no final e tem Apagar modelo.
- **Modelos oferecidos:** Whisper base, small, medium e large v3 turbo (os dois últimos compactados) para voz, licença MIT. Qwen3 4B Instruct 2507 em duas compactações (Q4_K_M e Q8_0) para análise, licença Apache 2.0. Tamanhos e licenças aparecem em cada cartão.
- **Processador ou placa de vídeo:** a análise pode rodar no processador ou na placa de vídeo (Vulkan), que baixa um programa a mais.
- **Testar:** liga o servidor local e mostra quanto tempo levou.
- **Plano B:** se o modelo local não estiver baixado, faltar memória ou falhar, o app mostra o erro e oferece usar a OpenAI (na tela de erro e nos Ajustes).
- **Limites menores:** o modelo local tem contexto de 32 mil tokens. Por isso o resumo local aceita até cerca de 60 mil caracteres de transcrição, e nas perguntas o app usa só o final da reunião (últimos 40 mil caracteres) e até 25 mil caracteres de material. Reuniões maiores precisam da OpenAI.
- **Memória:** o modelo fica carregado até você fechar o app ou voltar para a OpenAI nos Ajustes.

### Documentos e resposta certeira

Cada documento entra no material com o nome dele, e o modelo cita `(Documento: nome)`, `(Material)` ou `(Reunião hh:mm:ss)`. Se o material passa de 150 mil caracteres, o app envia só os trechos que mais combinam com a pergunta e com a fala recente. Essa escolha é por palavras (ignora acento e maiúscula), não por significado. Se a pergunta usa palavras diferentes das do documento, o trecho certo pode não ser escolhido.

### Conectar o Google Drive

O Google não aceita login e senha para apps: o acesso é pelo login do próprio Google, no navegador. Você, como dono do app, cria o ID do cliente uma vez, e seus usuários só clicam em **Entrar com Google**.

**Passo a passo (você, uma vez):**

1. Entre em console.cloud.google.com e crie um projeto.
2. Em "APIs e serviços", ative a **Google Drive API**.
3. Em "Tela de permissão OAuth", escolha usuário externo, preencha o nome do app e adicione os Gmails que vão testar em "Usuários de teste".
4. Em "Credenciais", crie um **ID do cliente OAuth** do tipo **App para computador**.
5. Copie `.env.example` para `.env` na raiz do projeto e preencha `GOOGLE_CLIENT_ID` e `GOOGLE_CLIENT_SECRET`.
6. Gere o app (`npm run build` ou `npm run dev`). Os valores são embutidos no programa na hora da compilação. O `.env` não vai dentro do instalador, e a tela de Ajustes do usuário não mostra os campos de ID e segredo.

Sem `.env`, o app mostra os campos de ID e segredo nos Ajustes, para uso pessoal.

**Para o usuário:** Ajustes, "Entrar com Google", escolher a conta e permitir. O app pede só leitura (`drive.readonly`) e não altera nem apaga nada no Drive. Para anexar, abra a aba Reunião, clique em "Anexar do Google Drive" e busque pelo nome, por um trecho do texto ou cole o link do arquivo.

**Limites do Google:**

- Com o app em modo de teste, só entram as contas cadastradas como usuários de teste (o limite é de 100 contas, segundo a documentação do Google), e a permissão pode expirar em cerca de 7 dias. O usuário só clica de novo em Entrar com Google.
- Para qualquer pessoa usar, o app precisa passar pela verificação do Google. A permissão de leitura do Drive pode ser classificada como restrita e exigir avaliação de segurança. Confira na documentação atual antes de distribuir.
- O segredo embutido num programa instalado no computador não fica realmente escondido: quem souber analisar o `.exe` consegue achá-lo. Isso é esperado nesse tipo de app, mas não coloque nesse ID nada que você não aceite expor.

### Teste dos modelos locais, num computador só

Medido neste projeto, num PC com 32 GB de memória e placa AMD Radeon RX 6650 XT. Os números mudam muito de um computador para outro. Foi uma única reunião de exemplo curta em português, não uma avaliação de qualidade.

| O que | Resultado |
|---|---|
| Download real (programa e modelo, com hash) | Whisper base e Qwen3 4B Q4_K_M baixados e verificados. Retomada de download interrompido funcionou. A internet do teste fazia cerca de 0,45 MB/s: o Qwen3 levou mais de 1 hora. |
| Transcrição local (Whisper small, processador) | 3,5 s para 16,6 s de áudio de voz sintética em português. O texto saiu correto. Voz real, com ruído e vários falantes, não foi testada. |
| Servidor de análise ligando (primeiro uso) | 6,5 s no processador e 8,6 s na placa de vídeo. |
| Resumo e pendências | 33,2 s no processador e 10,6 s na placa de vídeo. O formato estruturado funcionou e as 3 pendências saíram certas. A redação teve defeitos ("enviada pelo eu"). |
| Perguntas simples (garantia, fala pronta) | Corretas, em 0,7 a 2,7 s. |
| Pergunta sem resposta no material (endereço) | Recusou corretamente: "Não encontrei isso no material nem na reunião." |
| Pergunta com informação em dois lugares (prazo de entrega) | **Errou nas duas vezes**: disse "30 dias, plano Prata" e, no processador, ainda inferiu "1º de março", coisas que não estão no material nem na reunião. O correto era 15 de março, dito na reunião. |

Conclusão honesta: o modelo local funciona de ponta a ponta e é rápido, principalmente na placa de vídeo. Mas errou uma pergunta com respostas em duas fontes, e o aviso que o app dá (qualidade menor que a da OpenAI) vale. Para perguntas de preço, prazo e valor numa reunião real, confira a resposta ou use a OpenAI. Não comparei com o `gpt-4o-mini`, então não sei dizer o quanto é pior. A memória usada pelo servidor não foi medida.

## Estrutura

```
core/        Lógica em Rust, sem interface (banco, busca, reunião, resumo, e-mail). Tem os testes.
src-tauri/   Janela, comandos, captura de áudio e cofre de senhas.
renderer/    Interface (HTML, CSS e JavaScript).
scripts/     Instalação do whisper pelo terminal (legado: o app agora baixa sozinho, pelos Ajustes).
```

## Rodar

Uma vez só:

1. Instale o Rust: `winget install Rustlang.Rustup`
2. Instale o compilador C++: `winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`
3. Reinicie o PowerShell.

Sempre que quiser abrir o app:

```
npm install
npm run dev
```

Na aba Ajustes, preencha:

- A chave da API da OpenAI.
- O seu Gmail e uma senha de app (myaccount.google.com/apppasswords).
- O e-mail que recebe a transcrição.

## Rodar no Linux

Testado em Fedora; os pacotes de Debian/Ubuntu estão ao lado. Uma vez só:

```
# Fedora
sudo dnf install webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel libsoup3-devel \
  openssl-devel dbus-devel libxdo-devel gcc pkgconf-pkg-config pulseaudio-utils libsecret openssl3-libs   # openssl3-libs: o llama.cpp baixado usa libssl.so.3, ausente no Fedora 45
# Debian/Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev \
  libssl-dev libdbus-1-dev libxdo-dev build-essential pkg-config pulseaudio-utils libsecret-tools
```

Depois, `npm install` e `npm run dev`, como no Windows. Instale também o Rust (`rustup`).

- **Áudio:** precisa do `parec` e do `pactl` (pacote `pulseaudio-utils`, que funciona também com PipeWire). Microfone = fonte escolhida; "Outros" = monitor da saída escolhida (ou da padrão).
- **Segredos:** o `secret-tool` (pacote `libsecret`/`libsecret-tools`) e um chaveiro ativo (GNOME Keyring ou KWallet). Sem eles, o app não consegue guardar a chave da OpenAI, a senha do Gmail nem o token do Drive.
- **Modelos locais:** o botão Baixar pega os programas `.tar.gz` do whisper.cpp e do llama.cpp para Ubuntu (x64 ou arm64). Funcionam em outras distribuições se a `libstdc++` e a `glibc` forem recentes. A opção de placa de vídeo usa o Vulkan: precisa do driver Vulkan instalado.
- **Atalhos globais** (`Ctrl+Shift+Espaço`, `Ctrl+Shift+R`): funcionam no X11. No Wayland, o sistema bloqueia. Alternativa: crie atalhos nas configurações do sistema (Teclado, Atalhos personalizados) com os comandos `pkill -USR1 -x pauta` (mostrar/esconder) e `pkill -USR2 -x pauta` (iniciar/finalizar reunião). Também dá para usar o ícone da bandeja. A janela "sempre por cima" e a transparência também dependem do gerenciador de janelas.
- **Ícone na bandeja:** no GNOME, precisa da extensão AppIndicator.
- **Instalar de forma fixa (sem sudo):** `scripts/instalar-linux.sh` gera o app e instala em `~/.local/bin/pauta`, com ícone e entrada no menu de aplicativos. `--autostart` abre ao entrar na sessão; `--remover` desinstala. Os atalhos do Wayland (`pkill -USR1 -x pauta`) continuam valendo.
- **Gerar pacotes:** `npm run build` cria `.deb`, `.rpm` e `.AppImage` em `target/release/bundle/`.

## Gerar o .exe

```
npm run build
```

O instalador fica em `target\release\bundle\nsis\`. O executável solto fica em `target\release\pauta.exe`. Sem assinatura digital, o Windows mostra um aviso do SmartScreen na primeira execução.

## Testes

```
npm test
```

Roda os testes do `core` (busca, falas, resumo, e-mail, reunião de ponta a ponta com serviços simulados, documentos, trechos relevantes, Drive com servidor simulado, pendências, materiais salvos, apagar e exportar). A janela, a captura de áudio, os atalhos, o login real no Google e as chamadas reais à OpenAI, ao Drive e ao Gmail não têm teste automático. A leitura de PDF só é testada com arquivo inválido.

## Privacidade

- O áudio das falas e o texto das conversas são enviados à OpenAI, a não ser que você ligue a transcrição e a análise locais nos Ajustes. Mesmo assim, o Drive (se usado) e os downloads dos modelos usam a internet.
- O material colado e os documentos anexados (inteiros ou só os trechos escolhidos) também são enviados à OpenAI a cada pergunta.
- O texto dos documentos fica guardado no seu computador, em `documentos.json` e dentro de `conversas.db`. Para apagar, remova o documento na aba Reunião e apague a conversa.
- O Drive é lido só enquanto você anexa. O token de acesso fica no Gerenciador de Credenciais do Windows (no Linux, no chaveiro do sistema).
- Avise os participantes de que a reunião está sendo gravada e transcrita.
- Chaves e senhas não ficam em arquivo. O banco `conversas.db` fica na pasta de dados do app, no seu usuário.
- O app não guarda o áudio das reuniões. Guarda só o texto transcrito. Para apagar, abra a conversa e use "Apagar esta conversa", ou use os botões de apagar nos Ajustes. O e-mail já enviado não pode ser desfeito.

## Ideias de novas funcionalidades

As ideias abaixo não estão implementadas. O esforço é uma estimativa minha, não medida.

### Material de apoio

| Ideia | Para que serve | Esforço |
|---|---|---|
| Busca por significado | Achar conversas pelo sentido, não só pela palavra, com embeddings. Complementa o FTS. | Alto |
| Material muito grande, por significado | Hoje o app escolhe trechos por palavras. Embeddings achariam trechos pelo sentido. O limite de pergunta continua em cerca de 400 mil caracteres no total. | Alto |
| Documentos escaneados | PDF só com imagem precisa de OCR, que o app não faz. | Alto |
| Mais de uma aba de planilha | Google Planilhas exporta só a primeira aba em CSV. | Médio |

### Uso no dia a dia

| Ideia | Para que serve | Esforço |
|---|---|---|
| Estimativa de custo | Mostrar quanto a reunião gastou na OpenAI, a partir da duração e do tamanho do texto. | Médio |
| Exportar em PDF | Hoje exporta em Markdown. | Baixo |
| Importar backup do banco | Hoje só exporta. Importar exige trocar o banco com o app fechado. | Médio |

### Integrações

| Ideia | Para que serve | Esforço |
|---|---|---|
| Vários destinatários e outros provedores de e-mail | Hoje é um destinatário e Gmail. | Médio |
| Salvar no Notion, Google Drive ou Obsidian | O app estudado como referência tem módulos com os nomes `obsidian` e `mcp-oauth`. Só vi os nomes dos arquivos, não analisei o que fazem. | Alto |
| Detectar reunião sozinho | Sugerir iniciar quando o Meet, Teams ou Zoom abrir. | Alto |
| Criar eventos de acompanhamento | Transformar pendências com prazo em eventos na agenda. | Alto |

### Privacidade e distribuição

| Ideia | Para que serve | Esforço |
|---|---|---|
| Assinatura digital do .exe | Tira o aviso do SmartScreen. Exige comprar um certificado. | Baixo (custo em dinheiro) |
| Atualização automática | Ler uma nova versão de um endereço e instalar. | Médio |

### Ideias para os modelos locais

| Ideia | Para que serve | Esforço |
|---|---|---|
| Mais modelos de análise | Hoje só o Qwen3 4B. Outros (maiores ou de outras famílias) exigem teste real em português e conferência da licença. | Médio |
| Aceleração NVIDIA (CUDA) | Hoje é processador ou Vulkan. | Médio |
| Descarregar o modelo parado | Liberar a memória depois de alguns minutos sem uso. | Baixo |
| Whisper na placa de vídeo | Hoje a transcrição local usa só o processador. | Médio |

### Por onde começar

Já feitos: apagar conversas, exportar e copiar, escolher o microfone com medidor de volume, materiais salvos por cliente ou projeto, lista geral de pendências, anexar documentos (arquivo e Google Drive), transcrição local e análise local com botão de download, dica de termos na transcrição, resposta aparecendo aos poucos e identificação de falantes.

O que falta, nesta ordem:

1. Busca por significado.

## Limitações conhecidas

- Sem fone de ouvido, o microfone pode captar o som dos alto-falantes e repetir falas de "Outros" como "Eu".
- Identificação de falantes: foi testada só com serviços simulados, não com a API real da OpenAI nem com uma reunião de verdade. Não funciona na transcrição local, o limite é de 4 vozes e duas pessoas com voz parecida podem ser confundidas. A dica de termos também não foi comparada com e sem, em áudio real.
- Windows e Linux. O macOS não é suportado.
- No Linux, os atalhos globais e a janela por cima de tudo dependem do X11/gerenciador de janelas (veja "Rodar no Linux").
- A transcrição por fala depende de internet e da cota da conta OpenAI.
- Reuniões muito longas podem passar do limite de texto do resumo (cerca de 300 mil caracteres).
- Não lê PDF escaneado (só imagem) nem Word antigo (`.doc`). Salve como `.docx`. Arquivos acima de 25 MB são recusados.
- Modelos locais: qualidade menor que a da OpenAI, principalmente em resumo e pendências. Medições deste projeto estão na seção abaixo, num único computador.
- Se um `whisper-server` ou `llama-server` ficar aberto depois de uma falha, a porta 8178 ou 8179 fica ocupada e o app avisa. Feche o processo (Gerenciador de Tarefas no Windows, `pkill whisper-server` ou `pkill llama-server` no Linux).
- Microfone e saída escolhidos que sumirem (fone desligado) caem no padrão do sistema, com aviso.
- Os arquivos antigos da versão em Electron (`main.js`, `preload.js`, `lib/`, `search.js` e outros na raiz) não são mais usados.
