//! Prepara a fala para o transcritor: tira o ronco grave (filtro passa-alta) e levanta o volume de
//! quem fala baixo. Não muda o tamanho do áudio, e o áudio guardado no disco continua sem mexer.

use crate::resample::TAXA;

/// Corte do filtro passa-alta: abaixo disso é ronco de ar-condicionado, mesa e microfone, não voz.
const CORTE_HZ: f32 = 80.0;
/// Volume (RMS, 0 a 1) para onde a fala é levada quando está mais baixa.
const ALVO_RMS: f32 = 0.08;
/// Ganho máximo: levantar demais só amplifica o chiado.
const GANHO_MAXIMO: f32 = 8.0;
/// Pico máximo depois do ganho (evita estourar).
const PICO_MAXIMO: f32 = 0.95;

/// Tira ruído de fundo constante (ventilador, ar-condicionado, chiado) com o RNNoise, uma rede neural pequena
/// que trabalha em 48 kHz: o áudio sobe de 16 para 48 kHz, passa pela rede em quadros de 10 ms e desce de volta.
/// Mantém o tamanho do áudio. Pode comer o fim de palavras fracas; por isso é opcional.
pub fn reduzir_ruido(pcm: &[i16]) -> Vec<i16> {
    use nnnoiseless::{DenoiseState, FRAME_SIZE};
    if pcm.is_empty() {
        return vec![];
    }
    // sobe 3x com interpolação linear
    let mut alto: Vec<f32> = Vec::with_capacity(pcm.len() * 3 + 2 * FRAME_SIZE);
    // dois quadros de silêncio antes: a rede leva um instante para "esquentar" e a saída desses quadros é descartada
    alto.extend(std::iter::repeat(0.0).take(2 * FRAME_SIZE));
    for (i, &s) in pcm.iter().enumerate() {
        let (a, b) = (s as f32, *pcm.get(i + 1).unwrap_or(&s) as f32);
        alto.extend([a, a + (b - a) / 3.0, a + 2.0 * (b - a) / 3.0]);
    }
    let util = alto.len();
    alto.resize(util.div_ceil(FRAME_SIZE) * FRAME_SIZE, 0.0);

    let mut estado = DenoiseState::new();
    let mut limpo = vec![0.0f32; alto.len()];
    for (entrada, saida) in alto.chunks_exact(FRAME_SIZE).zip(limpo.chunks_exact_mut(FRAME_SIZE)) {
        estado.process_frame(saida, entrada);
    }
    // desce 3x fazendo a média de cada trio (um filtro simples contra serrilhado)
    limpo[2 * FRAME_SIZE..util].chunks_exact(3).map(|t| ((t[0] + t[1] + t[2]) / 3.0).clamp(-32768.0, 32767.0) as i16).collect()
}

pub fn preparar(pcm: &[i16]) -> Vec<i16> {
    if pcm.is_empty() {
        return vec![];
    }
    // filtro de um polo: y[n] = a * (y[n-1] + x[n] - x[n-1])
    let rc = 1.0 / (2.0 * std::f32::consts::PI * CORTE_HZ);
    let dt = 1.0 / TAXA as f32;
    let a = rc / (rc + dt);
    let mut filtrado = Vec::with_capacity(pcm.len());
    let (mut x_ant, mut y_ant) = (0.0f32, 0.0f32);
    for &s in pcm {
        let x = s as f32 / 32768.0;
        let y = a * (y_ant + x - x_ant);
        filtrado.push(y);
        x_ant = x;
        y_ant = y;
    }

    let rms = (filtrado.iter().map(|v| v * v).sum::<f32>() / filtrado.len() as f32).sqrt();
    let pico = filtrado.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    // só sobe o volume (nunca abaixa), dentro do limite de ganho e sem estourar
    let ganho = if rms > 1e-5 { (ALVO_RMS / rms).clamp(1.0, GANHO_MAXIMO).min(PICO_MAXIMO / pico.max(1e-5)).max(1.0) } else { 1.0 };
    filtrado.iter().map(|v| (v * ganho * 32767.0).clamp(-32768.0, 32767.0) as i16).collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    fn senoide(hz: f32, amp: f32, n: usize) -> Vec<i16> {
        (0..n).map(|i| ((i as f32 / TAXA as f32 * hz * 2.0 * std::f32::consts::PI).sin() * amp * 32767.0) as i16).collect()
    }

    fn rms(p: &[i16]) -> f32 {
        (p.iter().map(|&s| (s as f32 / 32768.0).powi(2)).sum::<f32>() / p.len() as f32).sqrt()
    }

    #[test]
    fn mantem_o_tamanho_e_tira_o_ronco_grave() {
        let ronco = senoide(20.0, 0.3, 16000);
        let p = preparar(&ronco);
        assert_eq!(p.len(), ronco.len());
        // o ronco de 20 Hz perde quase toda a energia (o ganho não sobe sinal que ficou quase nulo)
        assert!(rms(&p[4000..]) < rms(&ronco) * 0.5, "{} {}", rms(&p[4000..]), rms(&ronco));
        assert!(preparar(&[]).is_empty());
    }

    /// Chiado pseudoaleatório determinístico (gerador congruencial), amplitude aproximada `amp`.
    fn chiado(n: usize, amp: f32, semente: u32) -> Vec<i16> {
        let mut x = semente;
        (0..n).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (((x >> 16) as f32 / 32768.0 - 1.0) * amp * 32767.0) as i16 }).collect()
    }

    #[test]
    fn rnnoise_mantem_o_tamanho_e_derruba_o_chiado() {
        let ruido = chiado(32000, 0.05, 7);
        let limpo = reduzir_ruido(&ruido);
        assert_eq!(limpo.len(), ruido.len());
        assert!(rms(&limpo[8000..]) < rms(&ruido) * 0.25, "{} {}", rms(&limpo[8000..]), rms(&ruido));
        assert!(reduzir_ruido(&[]).is_empty());
        assert_eq!(reduzir_ruido(&[100; 7]).len(), 7, "áudio mais curto que um quadro também funciona");
    }

    #[test]
    fn fala_baixa_sobe_e_fala_alta_nao_muda_nem_estoura() {
        let baixa = senoide(300.0, 0.01, 16000);
        let p = preparar(&baixa);
        assert!(rms(&p) > rms(&baixa) * 3.0, "{} {}", rms(&p), rms(&baixa));
        assert!(rms(&p) <= ALVO_RMS * 1.2);
        let alta = senoide(300.0, 0.6, 16000);
        let p = preparar(&alta);
        assert!((rms(&p) - rms(&alta)).abs() < rms(&alta) * 0.05, "áudio alto não é mexido");
        let media = senoide(300.0, 0.12, 16000);
        assert!(preparar(&media).iter().all(|&s| (s as f32).abs() <= 0.96 * 32768.0));
        assert!(preparar(&[0i16; 1600]).iter().all(|&s| s == 0), "silêncio continua silêncio");
    }
}
