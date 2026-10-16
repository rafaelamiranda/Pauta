//! Captura nativa: microfone e som do computador (loopback do WASAPI), sem passar pela interface.

#[cfg(target_os = "linux")]
pub use busca_core::captura_pulse::{dispositivos, iniciar, Captura};

#[allow(unused_imports)]
use busca_core::meeting::Receptor;
#[allow(unused_imports)]
use std::sync::Arc;

#[cfg(not(target_os = "linux"))]
pub struct Captura {
    #[cfg(windows)]
    parar: std::sync::mpsc::Sender<()>,
    #[cfg(windows)]
    fio: Option<std::thread::JoinHandle<()>>,
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn iniciar(_: Arc<dyn Receptor>, _: String, _: String) -> Result<(Captura, Vec<String>), String> {
    Err("A captura de áudio só funciona no Windows e no Linux.".into())
}

/// Nomes dos microfones e das saídas de áudio disponíveis.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn dispositivos() -> (Vec<String>, Vec<String>) {
    (vec![], vec![])
}

/// Nomes dos microfones e das saídas de áudio disponíveis.
#[cfg(windows)]
pub fn dispositivos() -> (Vec<String>, Vec<String>) {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let nomes = |it: Result<Vec<cpal::Device>, String>| -> Vec<String> {
        let mut v: Vec<String> = it.unwrap_or_default().iter().filter_map(|d| d.name().ok()).collect();
        v.sort();
        v.dedup();
        v
    };
    (
        nomes(host.input_devices().map(|i| i.collect()).map_err(|e| e.to_string())),
        nomes(host.output_devices().map(|i| i.collect()).map_err(|e| e.to_string())),
    )
}

/// Acha o dispositivo pelo nome; se o nome estiver vazio ou sumiu, usa o padrão do Windows (e avisa no segundo caso).
#[cfg(windows)]
fn escolher(nome: &str, saida: bool, avisos: &mut Vec<String>) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if !nome.is_empty() {
        let achado = if saida { host.output_devices().ok() } else { host.input_devices().ok() }
            .and_then(|mut it| it.find(|d| d.name().map(|n| n == nome).unwrap_or(false)));
        if achado.is_some() {
            return achado;
        }
        avisos.push(format!("{} \"{nome}\" não foi encontrado. Usando o padrão do Windows.", if saida { "A saída de áudio" } else { "O microfone" }));
    }
    if saida { host.default_output_device() } else { host.default_input_device() }
}

#[cfg(windows)]
impl Drop for Captura {
    fn drop(&mut self) {
        let _ = self.parar.send(());
        if let Some(f) = self.fio.take() {
            let _ = f.join();
        }
    }
}

#[cfg(windows)]
pub fn iniciar(reuniao: Arc<dyn Receptor>, microfone: String, saida: String) -> Result<(Captura, Vec<String>), String> {
    use std::sync::mpsc;
    let (tx_parar, rx_parar) = mpsc::channel::<()>();
    let (tx_pronto, rx_pronto) = mpsc::channel::<Result<Vec<String>, String>>();

    // Os fluxos de áudio ficam presos à thread que os criou; ela os mantém vivos até receber o sinal.
    let fio = std::thread::spawn(move || {
        use cpal::traits::StreamTrait;
        let mut fluxos = Vec::new();
        let mut avisos = Vec::new();

        match escolher(&microfone, false, &mut avisos) {
            Some(d) => match abrir(&d, Fonte::Mic, reuniao.clone()) {
                Ok(s) => fluxos.push(s),
                Err(e) => avisos.push(format!("Microfone indisponível: {e}")),
            },
            None => avisos.push("Nenhum microfone encontrado.".into()),
        }
        match escolher(&saida, true, &mut avisos) {
            Some(d) => match abrir(&d, Fonte::Sistema, reuniao.clone()) {
                Ok(s) => fluxos.push(s),
                Err(e) => avisos.push(format!("Som do computador indisponível: {e}")),
            },
            None => avisos.push("Nenhuma saída de áudio encontrada.".into()),
        }

        if fluxos.is_empty() {
            let _ = tx_pronto.send(Err(avisos.join(" ")));
            return;
        }
        for f in &fluxos {
            if let Err(e) = f.play() {
                avisos.push(format!("Não foi possível iniciar um dispositivo: {e}"));
            }
        }
        let _ = tx_pronto.send(Ok(avisos));
        let _ = rx_parar.recv(); // espera o sinal de parada (ou o descarte da Captura)
        drop(fluxos);
    });

    match rx_pronto.recv() {
        Ok(Ok(avisos)) => Ok((Captura { parar: tx_parar, fio: Some(fio) }, avisos)),
        Ok(Err(e)) => {
            let _ = fio.join();
            Err(e)
        }
        Err(_) => Err("A captura de áudio falhou ao iniciar.".into()),
    }
}

#[cfg(windows)]
use busca_core::meeting::Fonte;

/// Abre um dispositivo. Para a saída (alto-falante), abrir como *entrada* ativa o loopback do WASAPI.
#[cfg(windows)]
fn abrir(dev: &cpal::Device, fonte: Fonte, reuniao: Arc<dyn Receptor>) -> Result<cpal::Stream, String> {
    use cpal::traits::DeviceTrait;
    use cpal::SampleFormat;
    let cfg = match fonte {
        Fonte::Mic => dev.default_input_config(),
        Fonte::Sistema => dev.default_output_config(),
    }
    .map_err(|e| e.to_string())?;
    let formato = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    match formato {
        SampleFormat::F32 => construir::<f32>(dev, &config, fonte, reuniao),
        SampleFormat::I16 => construir::<i16>(dev, &config, fonte, reuniao),
        SampleFormat::U16 => construir::<u16>(dev, &config, fonte, reuniao),
        outro => Err(format!("Formato de áudio sem suporte: {outro:?}")),
    }
}

#[cfg(windows)]
fn construir<T>(dev: &cpal::Device, config: &cpal::StreamConfig, fonte: Fonte, reuniao: Arc<dyn Receptor>) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    use busca_core::resample::ParaPcm16k;
    use cpal::traits::DeviceTrait;
    let mut conversor = ParaPcm16k::new(config.sample_rate.0, config.channels as usize);
    dev.build_input_stream(
        config,
        move |dados: &[T], _| {
            let f: Vec<f32> = dados.iter().map(|&s| <f32 as cpal::FromSample<T>>::from_sample_(s)).collect();
            if !f.is_empty() {
                let rms = (f.iter().map(|x| x * x).sum::<f32>() / f.len() as f32).sqrt();
                reuniao.informar_nivel(fonte, rms * 5.0);
            }
            for trecho in conversor.feed(&f) {
                reuniao.receber_audio(fonte, &trecho);
            }
        },
        |e| eprintln!("erro de áudio: {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}
