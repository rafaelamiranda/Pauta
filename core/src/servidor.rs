//! Sobe e derruba um servidor local (whisper-server ou llama-server) como processo filho.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PORTA_WHISPER: u16 = 8178;
pub const PORTA_LLM: u16 = 8179;

pub struct ProcessoServidor {
    nome: &'static str,
    exe: PathBuf,
    args: Vec<String>,
    porta: u16,
    /// Caminho que responde 200 quando o servidor está pronto. Sem ele, basta a porta abrir.
    saude: Option<&'static str>,
    limite: Duration,
    /// Onde guardar a saída de erro, para explicar uma falha ao iniciar.
    log: PathBuf,
    filho: Mutex<Option<Child>>,
}

impl ProcessoServidor {
    pub fn novo(nome: &'static str, exe: PathBuf, args: Vec<String>, porta: u16, saude: Option<&'static str>, limite: Duration, log: PathBuf) -> Self {
        Self { nome, exe, args, porta, saude, limite, log, filho: Mutex::new(None) }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.porta)
    }

    pub fn vivo(&self) -> bool {
        let mut g = self.filho.lock().unwrap();
        match g.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                *g = None;
                false
            }
            None => false,
        }
    }

    fn fim_do_log(&self) -> String {
        let t = std::fs::read_to_string(&self.log).unwrap_or_default();
        let t = t.trim();
        let mut i = t.len().saturating_sub(500);
        while !t.is_char_boundary(i) {
            i += 1;
        }
        t[i..].to_string()
    }

    async fn pronto(&self, http: &reqwest::Client) -> bool {
        match self.saude {
            Some(caminho) => http.get(format!("{}{caminho}", self.url())).send().await.is_ok_and(|r| r.status().is_success()),
            None => tokio::net::TcpStream::connect(("127.0.0.1", self.porta)).await.is_ok(),
        }
    }

    /// Inicia o servidor (se ainda não estiver rodando) e espera ele ficar pronto.
    pub async fn iniciar(&self) -> Result<()> {
        if self.vivo() {
            return Ok(());
        }
        if !self.exe.exists() {
            return Err(anyhow!("O programa do {} não está instalado. Baixe o modelo nos Ajustes.", self.nome));
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", self.porta)).await.is_ok() {
            return Err(anyhow!(
                "A porta {} já está em uso, provavelmente por um {} que ficou aberto. Feche esse processo (Gerenciador de Tarefas no Windows, ou `pkill` no Linux) e tente de novo.",
                self.porta, self.nome
            ));
        }
        if let Some(pai) = self.log.parent() {
            std::fs::create_dir_all(pai)?;
        }
        let log = std::fs::File::create(&self.log)?;
        let mut cmd = Command::new(&self.exe);
        cmd.args(&self.args)
            .current_dir(self.exe.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        #[cfg(unix)]
        if let Some(pasta) = self.exe.parent() {
            // as bibliotecas (.so) do whisper.cpp e do llama.cpp ficam ao lado do programa
            let atual = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
            cmd.env("LD_LIBRARY_PATH", if atual.is_empty() { pasta.display().to_string() } else { format!("{}:{atual}", pasta.display()) });
        }
        *self.filho.lock().unwrap() = Some(cmd.spawn().map_err(|e| anyhow!("Não foi possível abrir o {}: {e}", self.nome))?);

        let http = reqwest::Client::new();
        let limite = Instant::now() + self.limite;
        while Instant::now() < limite {
            if !self.vivo() {
                let log = self.fim_do_log();
                if log.contains("error while loading shared libraries") {
                    return Err(anyhow!(
                        "O {} não abriu porque falta uma biblioteca do sistema. Instale-a (ex.: no Fedora recente, `sudo dnf install openssl3-libs`). Detalhe: {log}",
                        self.nome
                    ));
                }
                return Err(anyhow!("O {} fechou ao iniciar. Pode ter faltado memória. Detalhe: {}", self.nome, log));
            }
            if self.pronto(&http).await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        self.parar();
        Err(anyhow!("O {} demorou demais para iniciar.", self.nome))
    }

    pub fn parar(&self) {
        if let Some(mut c) = self.filho.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for ProcessoServidor {
    fn drop(&mut self) {
        self.parar();
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn processo(exe: &str, porta: u16) -> ProcessoServidor {
        ProcessoServidor::novo("teste-server", PathBuf::from(exe), vec![], porta, None, Duration::from_secs(2), std::env::temp_dir().join("servidor-teste.log"))
    }

    #[tokio::test]
    async fn sem_instalacao_explica_o_que_fazer() {
        let e = processo("/nao/existe.exe", 1).iniciar().await.unwrap_err().to_string();
        assert!(e.contains("não está instalado") && e.contains("Ajustes"), "{e}");
    }

    #[tokio::test]
    async fn porta_ocupada_nao_confunde_com_o_nosso_processo() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let porta = l.local_addr().unwrap().port();
        let exe = std::env::current_exe().unwrap(); // existe, mas nem chega a ser aberto
        let e = processo(exe.to_str().unwrap(), porta).iniciar().await.unwrap_err().to_string();
        assert!(e.contains("já está em uso"), "{e}");
    }
}
