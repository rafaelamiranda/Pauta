//! Cofre de segredos no Linux: o chaveiro do sistema (GNOME Keyring, KWallet...) pelo `secret-tool`.

use crate::settings::Segredos;
use anyhow::{anyhow, Result};
use std::io::Write;
use std::process::{Command, Stdio};

const SERVICO: &str = "busca-overlay";

#[derive(Default)]
pub struct CofreSecretTool;

impl Segredos for CofreSecretTool {
    fn ler(&self, nome: &str) -> Option<String> {
        let o = Command::new("secret-tool").args(["lookup", "service", SERVICO, "account", nome]).stderr(Stdio::null()).output().ok()?;
        let valor = String::from_utf8(o.stdout).ok()?;
        (o.status.success() && !valor.is_empty()).then_some(valor)
    }

    fn gravar(&self, nome: &str, valor: &str) -> Result<()> {
        let ajuda = "Instale o libsecret (comando secret-tool) e use um chaveiro, como o GNOME Keyring ou o KWallet.";
        let mut filho = Command::new("secret-tool")
            .args(["store", &format!("--label=Pauta ({nome})"), "service", SERVICO, "account", nome])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("Não foi possível abrir o secret-tool ({e}). {ajuda}"))?;
        filho.stdin.take().ok_or_else(|| anyhow!("secret-tool sem entrada"))?.write_all(valor.as_bytes())?;
        let o = filho.wait_with_output()?;
        if !o.status.success() {
            return Err(anyhow!("O chaveiro recusou guardar o segredo: {}. {ajuda}", String::from_utf8_lossy(&o.stderr).trim()));
        }
        Ok(())
    }
}
