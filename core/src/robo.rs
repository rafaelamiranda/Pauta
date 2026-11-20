//! Robô que entra na chamada do Google Meet só para ler os nomes: quem participa e quem está falando.
//!
//! Abre o Chrome, o Edge ou o Chromium do computador com um perfil só dele (`<dados>/robo-navegador`), sem
//! microfone, sem câmera e sem som (`--mute-audio`, para o som do robô não entrar na gravação). Fala com o
//! navegador pelo protocolo de depuração (DevTools) numa porta local. A cada 250 ms lê os quadros dos
//! participantes e marca no `Quadro` quem o Meet mostra como falando. A reunião usa isso para trocar
//! "Outros" pelo nome de quem falou naquele horário.
//!
//! Limites: a página do Meet não tem marcação estável de "quem fala". O robô usa os seletores que outros
//! robôs de código aberto usam hoje (`data-participant-id`, `span.notranslate` e classes como `Oaajhc`).
//! Quando o Google muda a página, os nomes param de aparecer até o app ser atualizado; a transcrição continua.
//! Só lê os quadros que o Meet desenha na tela (numa chamada grande, quem não aparece não é lido).

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Emitir = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Folga antes e depois da fala ao procurar quem falou: o destaque do Meet chega um pouco atrasado.
const FOLGA_ANTES_MS: i64 = 300;
const FOLGA_DEPOIS_MS: i64 = 700;
/// Leituras de "falando" necessárias para dar o nome (cada leitura vale ~250 ms).
const LEITURAS_MINIMAS: usize = 2;
const INTERVALO: Duration = Duration::from_millis(250);
/// Desiste de entrar se ninguém deixar o robô entrar nesse tempo.
const ESPERA_MAXIMA: Duration = Duration::from_secs(10 * 60);

// ---------------------------------------------------------------- quadro: quem falou quando

#[derive(Default)]
struct Dados {
    /// (hora em ms, nomes destacados como falando). Só leituras com alguém falando, em ordem de hora.
    amostras: Vec<(i64, Vec<String>)>,
    /// Todos os nomes vistos na chamada, na ordem em que apareceram.
    participantes: Vec<String>,
}

/// Linha do tempo de quem o Meet mostrou falando. Compartilhada entre o robô (escreve) e a reunião (lê).
#[derive(Default)]
pub struct Quadro {
    dados: Mutex<Dados>,
}

impl Quadro {
    pub fn registrar(&self, ms: i64, falando: Vec<String>, participantes: &[String]) {
        let mut d = self.dados.lock().unwrap();
        for p in participantes {
            if !d.participantes.contains(p) {
                d.participantes.push(p.clone());
            }
        }
        if !falando.is_empty() {
            d.amostras.push((ms, falando));
        }
    }

    /// Nome de quem mais apareceu falando entre `ini` e `fim` (ms), se for a maioria das leituras.
    pub fn quem_falou(&self, ini: i64, fim: i64) -> Option<String> {
        let d = self.dados.lock().unwrap();
        let de = d.amostras.partition_point(|(t, _)| *t < ini - FOLGA_ANTES_MS);
        let ate = d.amostras.partition_point(|(t, _)| *t <= fim + FOLGA_DEPOIS_MS);
        let trecho = &d.amostras[de..ate];
        let mut contagem: Vec<(&str, usize)> = vec![];
        for (_, nomes) in trecho {
            for n in nomes {
                match contagem.iter_mut().find(|(x, _)| x == n) {
                    Some(c) => c.1 += 1,
                    None => contagem.push((n, 1)),
                }
            }
        }
        // empate fica com quem apareceu primeiro
        let (nome, vezes) = contagem.into_iter().fold(None::<(&str, usize)>, |m, c| match m {
            Some(m) if m.1 >= c.1 => Some(m),
            _ => Some(c),
        })?;
        (vezes >= LEITURAS_MINIMAS && vezes * 2 >= trecho.len()).then(|| nome.to_string())
    }

    pub fn participantes(&self) -> Vec<String> {
        self.dados.lock().unwrap().participantes.clone()
    }

    /// O robô viu alguém falando pelo menos uma vez (se não, o Meet provavelmente mudou a página).
    pub fn viu_alguem_falar(&self) -> bool {
        !self.dados.lock().unwrap().amostras.is_empty()
    }
}

// ---------------------------------------------------------------- navegador

/// Endereço do Meet, aceitando o link sem "https://". Recusa qualquer outro site.
pub fn link_do_meet(texto: &str) -> Result<String> {
    let t = texto.trim();
    let t = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(t);
    let Some(resto) = t.strip_prefix("meet.google.com/") else {
        bail!("O robô só entra em links do Google Meet (meet.google.com/...).")
    };
    if resto.is_empty() || resto.contains(char::is_whitespace) {
        bail!("Link do Google Meet incompleto.");
    }
    Ok(format!("https://meet.google.com/{resto}"))
}

/// Chrome, Edge, Chromium ou Brave instalado no computador.
pub fn achar_navegador() -> Option<PathBuf> {
    if cfg!(windows) {
        let mut c = vec![];
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                c.push(base.join(r"Google\Chrome\Application\chrome.exe"));
                c.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
                c.push(base.join(r"BraveSoftware\Brave-Browser\Application\brave.exe"));
            }
        }
        c.into_iter().find(|p| p.is_file())
    } else {
        let nomes = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "microsoft-edge-stable", "brave-browser"];
        let caminho = std::env::var_os("PATH")?;
        nomes.iter().flat_map(|n| std::env::split_paths(&caminho).map(move |d| d.join(n))).find(|p| p.is_file())
    }
}

fn sem_navegador() -> anyhow::Error {
    anyhow!("Não achei o Chrome, o Edge ou o Chromium neste computador. O robô precisa de um deles.")
}

/// Abre o navegador do robô na página de login do Google, para a pessoa entrar uma vez na conta do robô.
/// Sem login o robô também funciona: entra como convidado e alguém da reunião precisa aceitá-lo.
pub fn abrir_login(perfil: &Path) -> Result<()> {
    let nav = achar_navegador().ok_or_else(sem_navegador)?;
    std::fs::create_dir_all(perfil)?;
    Command::new(nav)
        .arg(format!("--user-data-dir={}", perfil.display()))
        .args(["--no-first-run", "--no-default-browser-check", "https://accounts.google.com/"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// GET simples na porta de depuração local (127.0.0.1). HTTP/1.1: o Chrome recusa HTTP/1.0 nessa porta.
fn http_get(porta: u16, caminho: &str) -> Result<Value> {
    let mut s = TcpStream::connect(("127.0.0.1", porta))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(s, "GET {caminho} HTTP/1.1\r\nHost: 127.0.0.1:{porta}\r\nConnection: close\r\n\r\n")?;
    // o Chrome mantém a conexão aberta: lê até completar o Content-Length
    let mut r: Vec<u8> = vec![];
    let mut buf = [0u8; 8192];
    loop {
        if let Some(fim) = r.windows(4).position(|w| w == b"\r\n\r\n") {
            let cab = String::from_utf8_lossy(&r[..fim]).to_lowercase();
            let tamanho = cab.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok());
            if tamanho.is_some_and(|t| r.len() >= fim + 4 + t) {
                return Ok(serde_json::from_slice(&r[fim + 4..fim + 4 + tamanho.unwrap_or(0)])?);
            }
        }
        let n = s.read(&mut buf)?;
        if n == 0 {
            let fim = r.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| anyhow!("resposta vazia do navegador"))?;
            return Ok(serde_json::from_slice(&r[fim + 4..])?);
        }
        r.extend_from_slice(&buf[..n]);
    }
}

/// Conexão com uma página ou com o navegador pelo protocolo de depuração.
struct Cdp {
    ws: tungstenite::WebSocket<TcpStream>,
    prox: u64,
}

impl Cdp {
    fn conectar(url: &str) -> Result<Self> {
        let host = url.strip_prefix("ws://").and_then(|r| r.split('/').next()).ok_or_else(|| anyhow!("endereço de depuração estranho: {url}"))?;
        let s = TcpStream::connect(host)?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        let (ws, _) = tungstenite::client(url, s).map_err(|e| anyhow!("{e}"))?;
        Ok(Self { ws, prox: 0 })
    }

    fn chamar(&mut self, metodo: &str, params: Value) -> Result<Value> {
        self.prox += 1;
        let id = self.prox;
        self.ws.send(tungstenite::Message::Text(json!({"id": id, "method": metodo, "params": params}).to_string()))?;
        loop {
            if let tungstenite::Message::Text(t) = self.ws.read()? {
                let v: Value = serde_json::from_str(&t)?;
                if v["id"] == id {
                    if let Some(e) = v.get("error") {
                        bail!("{metodo}: {}", e["message"].as_str().unwrap_or("erro"));
                    }
                    return Ok(v["result"].clone());
                }
            }
        }
    }

    /// Roda o script na página e lê o JSON que ele devolve como texto.
    fn avaliar(&mut self, script: &str) -> Result<Value> {
        let r = self.chamar("Runtime.evaluate", json!({"expression": script, "returnByValue": true}))?;
        if let Some(e) = r.get("exceptionDetails") {
            bail!("script: {}", e["exception"]["description"].as_str().or(e["text"].as_str()).unwrap_or("erro"));
        }
        Ok(r["result"]["value"].as_str().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null))
    }
}

/// Lido a cada 250 ms na página do Meet. Antes de entrar: preenche o nome de convidado, fecha avisos e clica em
/// participar. Na chamada: devolve os nomes dos quadros e quem está destacado como falando, sem o próprio robô.
const SCRIPT: &str = r#"(() => {
  const NOME = __NOME__;
  const norm = (t) => (t || '').replace(/\s+/g, ' ').trim();
  const visivel = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const botoes = [...document.querySelectorAll('button, [role="button"]')].filter(visivel);
  const textoDe = (b) => norm(b.innerText || b.getAttribute('aria-label')).toLowerCase();
  // "x*" aceita começo; o resto, texto exato. Tenta na ordem da lista.
  const clicar = (lista) => {
    for (const x of lista) {
      const prefixo = x.endsWith('*');
      const alvo = prefixo ? x.slice(0, -1) : x;
      const b = botoes.find((b) => {
        const t = textoDe(b);
        return (prefixo ? t.startsWith(alvo) : t === alvo) && !b.disabled && b.getAttribute('aria-disabled') !== 'true';
      });
      if (b) { b.click(); return alvo; }
    }
    return null;
  };
  const icone = (nome) => [...document.querySelectorAll('i, span')].some((e) => e.childElementCount === 0 && e.textContent === nome && visivel(e));
  const naChamada = icone('call_end') || !!document.querySelector('[aria-label*="Sair da chamada" i], [aria-label*="Leave call" i]');
  const avisos = ['continuar sem microfone*', 'continue without microphone*', 'entendi', 'got it', 'dispensar', 'dismiss'];
  if (!naChamada) {
    const campo = [...document.querySelectorAll('input[type="text"]')].find((i) => visivel(i) && !i.value);
    if (campo && NOME) {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(campo, NOME);
      campo.dispatchEvent(new Event('input', { bubbles: true }));
    }
    const acao = clicar(avisos) || clicar(['participar agora', 'join now', 'pedir para participar', 'ask to join', 'participar', 'join']);
    const corpo = norm(document.body?.innerText).toLowerCase();
    let fase = 'entrando';
    if (/aguardando|deixar você participar|deixe você participar|let you in|asking to join|waiting for/.test(corpo)) fase = 'aguardando';
    if (/não é possível participar|você não pode participar|can.t join this|solicitação .*recusada|request to join was denied|ninguém respondeu|no one responded|você foi removid|you.ve been removed|você saiu da reunião|you left the meeting|a reunião terminou|the call has ended|meeting has ended/.test(corpo)) fase = 'fora';
    return JSON.stringify({ fase, acao, participantes: [], falando: [] });
  }
  clicar(avisos);
  const CLASSES = ['Oaajhc', 'HX2H7', 'wEsLMd', 'OgVli'];
  const LIXO = /^Google Participant \(|spaces\/|devices\//;
  const nomeDo = (t) => {
    let n = norm(t.querySelector('span.notranslate')?.textContent);
    if (!n) n = norm(t.querySelector('[data-self-name]')?.getAttribute('data-self-name'));
    return !n || n.length < 2 || n.length > 60 || LIXO.test(n) ? null : n;
  };
  const vistos = new Map();
  for (const t of document.querySelectorAll('[data-participant-id]')) {
    const id = t.getAttribute('data-participant-id');
    const nome = nomeDo(t);
    if (!id || !nome) continue;
    const r = vistos.get(id) || { nome, eu: false, falando: false };
    r.eu = r.eu || t.hasAttribute('data-self-name') || !!t.querySelector('[data-self-name]');
    r.falando = r.falando || CLASSES.some((c) => t.classList.contains(c) || !!t.querySelector('.' + c));
    vistos.set(id, r);
  }
  const outros = [...vistos.values()].filter((r) => !r.eu && r.nome !== NOME);
  return JSON.stringify({
    fase: 'na_chamada',
    participantes: [...new Set(outros.map((r) => r.nome))],
    falando: [...new Set(outros.filter((r) => r.falando).map((r) => r.nome))],
  });
})()"#;

const SCRIPT_SAIR: &str = r#"(() => {
  const b = [...document.querySelectorAll('button, [role="button"]')].find((b) =>
    /sair da chamada|leave call/i.test(b.getAttribute('aria-label') || '') || [...b.querySelectorAll('i, span')].some((i) => i.textContent === 'call_end'));
  if (b) b.click();
  return JSON.stringify({ saiu: !!b });
})()"#;

pub struct OpcoesRobo {
    pub link: String,
    pub perfil: PathBuf,
    /// Nome usado quando o robô entra como convidado (sem conta).
    pub nome: String,
}

/// O robô em andamento. Ao encerrar (ou ao ser descartado), sai da chamada e fecha o navegador.
pub struct Robo {
    parar: Arc<AtomicBool>,
    fio: Option<std::thread::JoinHandle<()>>,
}

impl Robo {
    /// Abre o navegador e começa a entrar na chamada. Volta logo; o andamento chega por `robo:estado`.
    pub fn iniciar(op: OpcoesRobo, quadro: Arc<Quadro>, emitir: Emitir) -> Result<Self> {
        let link = link_do_meet(&op.link)?;
        let nav = achar_navegador().ok_or_else(sem_navegador)?;
        std::fs::create_dir_all(&op.perfil)?;
        let parar = Arc::new(AtomicBool::new(false));
        let p = parar.clone();
        let fio = std::thread::Builder::new().name("robo-meet".into()).spawn(move || {
            let emitir2 = emitir.clone();
            if let Err(e) = rodar(&nav, &op.perfil, &link, &op.nome, &quadro, &emitir, &p) {
                emitir2("robo:estado", json!({"fase": "erro", "texto": e.to_string()}));
            }
        })?;
        Ok(Self { parar, fio: Some(fio) })
    }

    /// Sai da chamada e fecha o navegador do robô. Espera até uns 5 segundos.
    pub fn encerrar(mut self) {
        self.parar.store(true, Ordering::SeqCst);
        if let Some(f) = self.fio.take() {
            let _ = f.join();
        }
    }
}

impl Drop for Robo {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::SeqCst);
    }
}

/// Fecha o navegador do robô quando a função de rodar termina, por qualquer caminho.
struct Navegador(Child);
impl Drop for Navegador {
    fn drop(&mut self) {
        let fim = Instant::now() + Duration::from_secs(3);
        while Instant::now() < fim {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn abrir_navegador(nav: &Path, perfil: &Path) -> Result<(Navegador, u16)> {
    let arquivo_porta = perfil.join("DevToolsActivePort");
    let _ = std::fs::remove_file(&arquivo_porta);
    let filho = Command::new(nav)
        .arg(format!("--user-data-dir={}", perfil.display()))
        .args([
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--mute-audio",
            "--disable-background-timer-throttling",
            "--disable-backgrounding-occluded-windows",
            "--disable-renderer-backgrounding",
            "--window-size=900,700",
            "about:blank",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("não foi possível abrir o navegador do robô")?;
    let mut nav = Navegador(filho);
    let fim = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(porta) = std::fs::read_to_string(&arquivo_porta).ok().and_then(|t| t.lines().next()?.trim().parse::<u16>().ok()) {
            return Ok((nav, porta));
        }
        if matches!(nav.0.try_wait(), Ok(Some(_))) {
            bail!("O navegador do robô fechou logo ao abrir. Se a janela de login do robô está aberta, feche-a e tente de novo.");
        }
        if Instant::now() > fim {
            bail!("O navegador do robô não respondeu em 30 segundos.");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn rodar(nav: &Path, perfil: &Path, link: &str, nome: &str, quadro: &Quadro, emitir: &Emitir, parar: &AtomicBool) -> Result<()> {
    let estado = |fase: &str, texto: &str, participantes: &[String]| emitir("robo:estado", json!({"fase": fase, "texto": texto, "participantes": participantes}));
    estado("abrindo", "Abrindo o navegador do robô...", &[]);
    let (_navegador, porta) = abrir_navegador(nav, perfil)?;

    // sem microfone nem câmera: o robô só olha
    if let Some(url) = http_get(porta, "/json/version")?["webSocketDebuggerUrl"].as_str() {
        if let Ok(mut b) = Cdp::conectar(url) {
            for p in ["microphone", "camera"] {
                let _ = b.chamar("Browser.setPermission", json!({"permission": {"name": p}, "setting": "denied", "origin": "https://meet.google.com"}));
            }
        }
    }
    let alvos = http_get(porta, "/json/list")?;
    let pagina = alvos.as_array().and_then(|a| a.iter().find(|t| t["type"] == "page")).and_then(|t| t["webSocketDebuggerUrl"].as_str()).ok_or_else(|| anyhow!("O navegador do robô abriu sem página."))?;
    let mut cdp = Cdp::conectar(pagina)?;
    cdp.chamar("Page.navigate", json!({"url": link}))?;
    estado("entrando", "O robô está entrando na chamada...", &[]);

    let script = SCRIPT.replace("__NOME__", &serde_json::to_string(nome)?);
    let comeco = Instant::now();
    let mut fase_antes = String::new();
    let mut nomes_antes: Vec<String> = vec![];
    let mut falhas = 0;
    let mut fora = 0;
    let mut esteve_na_chamada = false;
    while !parar.load(Ordering::SeqCst) {
        std::thread::sleep(INTERVALO);
        let v = match cdp.avaliar(&script) {
            Ok(v) => {
                falhas = 0;
                v
            }
            // durante a troca de página o script falha; só desiste se continuar falhando
            Err(e) => {
                falhas += 1;
                if falhas > 40 {
                    bail!("O robô perdeu o contato com o navegador ({e}). A janela foi fechada?");
                }
                continue;
            }
        };
        let fase = v["fase"].as_str().unwrap_or("").to_string();
        let lista = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default();
        let participantes = lista("participantes");
        if fase == "na_chamada" {
            esteve_na_chamada = true;
            fora = 0;
            quadro.registrar(crate::agora_ms(), lista("falando"), &participantes);
        } else if fase == "fora" {
            fora += 1;
            if fora >= 8 {
                bail!(if esteve_na_chamada { "O robô saiu da chamada (a reunião acabou ou ele foi removido)." } else { "O robô não conseguiu entrar na chamada (pedido recusado ou link inválido)." });
            }
        } else if !esteve_na_chamada && comeco.elapsed() > ESPERA_MAXIMA {
            bail!("Ninguém deixou o robô entrar em 10 minutos. Ele desistiu.");
        }
        if fase != fase_antes || (fase == "na_chamada" && participantes != nomes_antes) {
            let texto = match fase.as_str() {
                "na_chamada" if participantes.is_empty() => "Robô na chamada. Ainda não vi ninguém além de mim.".to_string(),
                "na_chamada" => format!("Robô na chamada. Participantes: {}.", participantes.join(", ")),
                "aguardando" => "O robô pediu para entrar. Aceite o pedido no Meet (Pauta, ou o nome da conta do robô).".to_string(),
                _ => "O robô está entrando na chamada...".to_string(),
            };
            estado(if fase.is_empty() { "entrando" } else { &fase }, &texto, &participantes);
            fase_antes = fase;
            nomes_antes = participantes;
        }
    }
    // saída educada: clica em sair e fecha o navegador
    let _ = cdp.avaliar(SCRIPT_SAIR);
    std::thread::sleep(Duration::from_millis(600));
    if let Ok(v) = http_get(porta, "/json/version") {
        if let Some(mut b) = v["webSocketDebuggerUrl"].as_str().and_then(|u| Cdp::conectar(u).ok()) {
            let _ = b.chamar("Browser.close", json!({}));
        }
    }
    estado("encerrado", "O robô saiu da chamada.", &[]);
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn nomes(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quem_falou_e_a_maioria_das_leituras_no_horario_da_fala() {
        let q = Quadro::default();
        let todos = nomes(&["Ana", "Bruno"]);
        for t in (0..2000).step_by(250) { q.registrar(t, nomes(&["Ana"]), &todos); }
        for t in (2000..2500).step_by(250) { q.registrar(t, vec![], &todos); }
        for t in (2500..5000).step_by(250) { q.registrar(t, nomes(&["Bruno"]), &todos); }
        assert_eq!(q.quem_falou(0, 1800).as_deref(), Some("Ana"));
        assert_eq!(q.quem_falou(2600, 4900).as_deref(), Some("Bruno"));
        assert_eq!(q.quem_falou(10_000, 12_000), None, "ninguém destacado nesse horário");
        assert_eq!(q.participantes(), todos);
        assert!(q.viu_alguem_falar());
    }

    #[test]
    fn sem_maioria_fica_sem_nome() {
        let q = Quadro::default();
        for t in (0..3000).step_by(250) { q.registrar(t, nomes(&[if t % 500 == 0 { "Ana" } else { "Bruno" }]), &[]); }
        // metade Ana, metade Bruno: Ana tem exatamente a metade e vence o empate por ter vindo antes
        assert_eq!(q.quem_falou(0, 3000).as_deref(), Some("Ana"));
        let q = Quadro::default();
        for (i, t) in (0..3000).step_by(250).enumerate() { q.registrar(t, nomes(&[["Ana", "Bruno", "Carla"][i % 3]]), &[]); }
        assert_eq!(q.quem_falou(0, 3000), None, "três pessoas revezando: nenhuma tem a maioria");
        assert!(!Quadro::default().viu_alguem_falar());
        let q = Quadro::default();
        q.registrar(1000, nomes(&["Ana"]), &[]);
        assert_eq!(q.quem_falou(900, 1100), None, "uma leitura só (250 ms) não basta");
    }

    /// Abre o navegador de verdade numa página que imita a do Meet (quadros, ícone de sair, destaque de fala).
    /// Ignorado por padrão: precisa de Chrome, Edge ou Chromium e de uma tela. Rode com `--ignored`.
    #[test]
    #[ignore]
    fn navegador_de_verdade_le_nomes_e_quem_fala() {
        let Some(nav) = achar_navegador() else { panic!("sem navegador") };
        // começa na tela de entrada: o botão só funciona depois do nome; ao clicar, aparece a chamada
        let pagina = r#"<html><head><meta charset="utf-8"></head><body>
            <input type="text" aria-label="Seu nome" oninput="b.disabled = !this.value">
            <button id="b" disabled onclick="if (document.querySelector('input').value === 'Robô') { document.body.innerHTML = document.getElementById('chamada').innerHTML }">Pedir para participar</button>
            <template id="chamada">
            <button aria-label="Sair da chamada"><i>call_end</i></button>
            <div data-participant-id="spaces/a/devices/1"><span class="notranslate">Ana Souza</span><div class="Oaajhc"></div></div>
            <div data-participant-id="spaces/a/devices/2"><span class="notranslate">Bruno</span></div>
            <div data-participant-id="spaces/a/devices/3" data-self-name="Robô"><span class="notranslate">Robô</span><div class="Oaajhc"></div></div>
            </template>
            </body></html>"#;
        let link = format!("data:text/html;base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pagina));
        let perfil = std::env::temp_dir().join(format!("robo-{}", uuid::Uuid::new_v4()));
        let quadro = Arc::new(Quadro::default());
        let eventos: Arc<Mutex<Vec<Value>>> = Default::default();
        let ev = eventos.clone();
        let emitir: Emitir = Arc::new(move |_, v| ev.lock().unwrap().push(v));
        let parar = Arc::new(AtomicBool::new(false));
        let (q, p) = (quadro.clone(), parar.clone());
        let pasta = perfil.clone();
        let fio = std::thread::spawn(move || rodar(&nav, &perfil, &link, "Robô", &q, &emitir, &p));
        let fim = Instant::now() + Duration::from_secs(40);
        while !quadro.viu_alguem_falar() && Instant::now() < fim {
            std::thread::sleep(Duration::from_millis(200));
        }
        std::thread::sleep(Duration::from_secs(1));
        parar.store(true, Ordering::SeqCst);
        let r = fio.join().unwrap();
        let _ = std::fs::remove_dir_all(pasta);
        let ev = eventos.lock().unwrap().clone();
        assert!(r.is_ok(), "{r:?} {ev:?}");
        assert_eq!(quadro.participantes(), ["Ana Souza", "Bruno"], "o próprio robô fica de fora; {ev:?}");
        let agora = crate::agora_ms();
        assert_eq!(quadro.quem_falou(agora - 2000, agora).as_deref(), Some("Ana Souza"));
        assert!(ev.iter().any(|e| e["fase"] == "na_chamada") && ev.last().unwrap()["fase"] == "encerrado", "{ev:?}");
    }

    #[test]
    fn so_aceita_link_do_meet() {
        assert_eq!(link_do_meet(" meet.google.com/abc-defg-hij ").unwrap(), "https://meet.google.com/abc-defg-hij");
        assert_eq!(link_do_meet("https://meet.google.com/abc-defg-hij?authuser=1").unwrap(), "https://meet.google.com/abc-defg-hij?authuser=1");
        assert!(link_do_meet("https://evil.com/meet.google.com/x").is_err());
        assert!(link_do_meet("https://meet.google.com.evil.com/x").is_err());
        assert!(link_do_meet("meet.google.com/").is_err());
    }
}
