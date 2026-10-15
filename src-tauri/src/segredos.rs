//! Cofre de segredos: no Windows, o Gerenciador de Credenciais; no Linux, o chaveiro do sistema (secret-tool).

#[cfg(not(target_os = "linux"))]
use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use busca_core::settings::Segredos;

#[cfg(not(target_os = "linux"))]
const SERVICO: &str = "busca-overlay";

#[cfg(windows)]
#[derive(Default)]
pub struct Cofre;

#[cfg(windows)]
impl Segredos for Cofre {
    fn ler(&self, nome: &str) -> Option<String> {
        keyring::Entry::new(SERVICO, nome).ok()?.get_password().ok()
    }

    fn gravar(&self, nome: &str, valor: &str) -> Result<()> {
        keyring::Entry::new(SERVICO, nome)?.set_password(valor)?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use busca_core::segredos_linux::CofreSecretTool as Cofre;

/// Em outros sistemas (só para desenvolvimento), guarda em memória.
#[cfg(not(any(windows, target_os = "linux")))]
#[derive(Default)]
pub struct Cofre(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(not(any(windows, target_os = "linux")))]
impl Segredos for Cofre {
    fn ler(&self, nome: &str) -> Option<String> {
        let _ = SERVICO;
        self.0.lock().unwrap().get(nome).cloned()
    }

    fn gravar(&self, nome: &str, valor: &str) -> Result<()> {
        self.0.lock().unwrap().insert(nome.into(), valor.into());
        Ok(())
    }
}
