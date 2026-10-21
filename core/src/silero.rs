//! Detector de voz neural (Silero VAD v5, 16 kHz), reescrito em Rust: nenhuma biblioteca nativa nem ONNX
//! Runtime. Os pesos (MIT, Silero Team) vêm de `core/assets/silero_v5_16k.f32`, gerados por
//! `scripts/gerar-silero.py` a partir do modelo oficial.
//!
//! A rede, por janela de 512 amostras (32 ms) mais 64 amostras de contexto da janela anterior:
//! 1. espelha 64 amostras no fim e faz uma STFT (convolução de 256 amostras, passo 128), que dá 4 quadros;
//!    o módulo de cada um (129 frequências) entra na rede;
//! 2. 4 convoluções com ReLU (passos 1, 2, 2, 1) reduzem os 4 quadros a 1 vetor de 128 números;
//! 3. uma célula LSTM (128) guarda o que já ouviu entre janelas;
//! 4. ReLU, convolução 1x1 e sigmoide dão a probabilidade de ser voz.

use std::sync::OnceLock;

const JANELA: usize = 512;
const CONTEXTO: usize = 64;
const OCULTO: usize = 128;
const FREQ: usize = 129;

static BRUTO: &[u8] = include_bytes!("../assets/silero_v5_16k.f32");

struct Conv {
    peso: Vec<f32>, // [saida][entrada][3]
    vies: Vec<f32>,
    entrada: usize,
    saida: usize,
    passo: usize,
}

struct Pesos {
    base_stft: Vec<f32>, // [258][256]
    encoder: [Conv; 4],
    w_ih: Vec<f32>, // [512][128], ordem das portas: entrada, esquecimento, candidato, saída
    w_hh: Vec<f32>,
    b: Vec<f32>, // bias_ih + bias_hh
    dec_peso: Vec<f32>,
    dec_vies: f32,
}

fn pesos() -> &'static Pesos {
    static P: OnceLock<Pesos> = OnceLock::new();
    P.get_or_init(|| {
        let todos: Vec<f32> = BRUTO.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let mut pos = 0;
        let mut pegar = |n: usize| {
            let v = todos[pos..pos + n].to_vec();
            pos += n;
            v
        };
        let base_stft = pegar(258 * 256);
        let mut conv = |entrada: usize, saida: usize, passo: usize| {
            let peso = pegar(saida * entrada * 3);
            let vies = pegar(saida);
            Conv { peso, vies, entrada, saida, passo }
        };
        let encoder = [conv(129, 128, 1), conv(128, 64, 2), conv(64, 64, 2), conv(64, 128, 1)];
        let w_ih = pegar(512 * OCULTO);
        let w_hh = pegar(512 * OCULTO);
        let b_ih = pegar(512);
        let b_hh = pegar(512);
        let b = b_ih.iter().zip(&b_hh).map(|(a, b)| a + b).collect();
        let dec_peso = pegar(OCULTO);
        let dec_vies = pegar(1)[0];
        assert_eq!(pos, todos.len(), "arquivo de pesos do Silero com tamanho inesperado");
        Pesos { base_stft, encoder, w_ih, w_hh, b, dec_peso, dec_vies }
    })
}

fn sigmoide(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Convolução 1D com preenchimento de 1 em cada ponta, sobre `x[canal][tempo]`, seguida de ReLU.
fn conv_relu(c: &Conv, x: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let t_in = x[0].len();
    let t_out = (t_in + 2 - 3) / c.passo + 1;
    let mut saida = vec![vec![0.0f32; t_out]; c.saida];
    for (o, linha) in saida.iter_mut().enumerate() {
        for (t, v) in linha.iter_mut().enumerate() {
            let mut soma = c.vies[o];
            for (i, canal) in x.iter().enumerate().take(c.entrada) {
                for k in 0..3 {
                    // posição na entrada sem preenchimento (-1 e t_in são os zeros das pontas)
                    let p = (t * c.passo + k) as isize - 1;
                    if p >= 0 && (p as usize) < t_in {
                        soma += c.peso[(o * c.entrada + i) * 3 + k] * canal[p as usize];
                    }
                }
            }
            *v = soma.max(0.0);
        }
    }
    saida
}

pub struct Silero {
    h: [f32; OCULTO],
    c: [f32; OCULTO],
    contexto: [f32; CONTEXTO],
    /// Amostras que ainda não completaram uma janela de 512.
    pendente: Vec<f32>,
    ultima: f32,
}

impl Default for Silero {
    fn default() -> Self {
        Self::new()
    }
}

impl Silero {
    pub fn new() -> Self {
        pesos(); // carrega os pesos agora, não no meio da reunião
        Self { h: [0.0; OCULTO], c: [0.0; OCULTO], contexto: [0.0; CONTEXTO], pendente: Vec::new(), ultima: 0.0 }
    }

    /// Probabilidade (0 a 1) de haver voz numa janela de exatamente 512 amostras (-1 a 1). Atualiza a memória da rede.
    pub fn janela(&mut self, amostras: &[f32]) -> f32 {
        assert_eq!(amostras.len(), JANELA);
        let p = pesos();
        // contexto + janela, mais 64 amostras espelhadas no fim (sem repetir a última)
        let mut x: Vec<f32> = Vec::with_capacity(CONTEXTO + JANELA + 64);
        x.extend_from_slice(&self.contexto);
        x.extend_from_slice(amostras);
        let n = x.len();
        for i in 0..64 {
            x.push(x[n - 2 - i]);
        }
        self.contexto.copy_from_slice(&amostras[JANELA - CONTEXTO..]);

        // STFT: 258 filtros de 256 amostras, passo 128 -> 4 quadros; módulo de (real, imaginário)
        let quadros = (x.len() - 256) / 128 + 1;
        let mut modulo = vec![vec![0.0f32; quadros]; FREQ];
        for q in 0..quadros {
            let trecho = &x[q * 128..q * 128 + 256];
            for f in 0..FREQ {
                let dot = |linha: usize| p.base_stft[linha * 256..(linha + 1) * 256].iter().zip(trecho).map(|(a, b)| a * b).sum::<f32>();
                let (re, im) = (dot(f), dot(FREQ + f));
                modulo[f][q] = (re * re + im * im).sqrt();
            }
        }

        let mut h = modulo;
        for c in &p.encoder {
            h = conv_relu(c, &h);
        }
        let entrada: Vec<f32> = h.iter().map(|canal| canal[0]).collect(); // [128]

        // célula LSTM
        let mut portas = p.b.clone();
        for (g, porta) in portas.iter_mut().enumerate() {
            let ih = &p.w_ih[g * OCULTO..(g + 1) * OCULTO];
            let hh = &p.w_hh[g * OCULTO..(g + 1) * OCULTO];
            *porta += ih.iter().zip(&entrada).map(|(a, b)| a * b).sum::<f32>() + hh.iter().zip(&self.h).map(|(a, b)| a * b).sum::<f32>();
        }
        for j in 0..OCULTO {
            let (i, f, g, o) = (sigmoide(portas[j]), sigmoide(portas[OCULTO + j]), portas[2 * OCULTO + j].tanh(), sigmoide(portas[3 * OCULTO + j]));
            self.c[j] = f * self.c[j] + i * g;
            self.h[j] = o * self.c[j].tanh();
        }

        let saida: f32 = p.dec_peso.iter().zip(&self.h).map(|(w, h)| w * h.max(0.0)).sum::<f32>() + p.dec_vies;
        sigmoide(saida)
    }

    /// Recebe PCM 16 bits de qualquer tamanho e devolve a maior probabilidade entre as janelas completadas.
    /// Se nenhuma janela fechou (trecho curto), repete a última probabilidade.
    pub fn processar(&mut self, pcm: &[i16]) -> f32 {
        self.pendente.extend(pcm.iter().map(|&s| s as f32 / 32768.0));
        let mut maior: Option<f32> = None;
        while self.pendente.len() >= JANELA {
            let janela: Vec<f32> = self.pendente.drain(..JANELA).collect();
            let p = self.janela(&janela);
            maior = Some(maior.map_or(p, |m| m.max(p)));
        }
        if let Some(m) = maior {
            self.ultima = m;
        }
        self.ultima
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    /// Sinal parecido com uma vogal (harmônicos de ~110 Hz com formantes e modulação de amplitude).
    /// A mesma fórmula, em numpy, alimentou o ONNX oficial (onnx.reference.ReferenceEvaluator) para gerar `ESPERADO`.
    fn sinal(n: usize) -> f32 {
        let pi2 = 2.0 * std::f64::consts::PI;
        let t = n as f64 / 16000.0;
        let f0 = 110.0 + 20.0 * (pi2 * 3.0 * t).sin();
        let mut s = 0.0f64;
        for h in 1..30 {
            let f = h as f64 * f0;
            let env = (-((f - 700.0) / 250.0).powi(2)).exp() + 0.6 * (-((f - 1200.0) / 300.0).powi(2)).exp() + 0.3 * (-((f - 2600.0) / 400.0).powi(2)).exp();
            s += env / h as f64 * (pi2 * h as f64 * 110.0 * t + h as f64 * pi2 * 20.0 / (pi2 * 3.0) * (1.0 - (pi2 * 3.0 * t).cos())).sin();
        }
        let am = if n % 16000 < 11000 { 0.5 + 0.5 * (pi2 * 4.0 * t).sin().powi(2) } else { 0.0 };
        (0.25 * s * am) as f32
    }

    const ESPERADO: [f32; 36] = [0.481241, 0.413860, 0.264402, 0.153471, 0.051242, 0.049443, 0.022797, 0.018184, 0.025137, 0.032916, 0.009998, 0.007044, 0.004729, 0.009529, 0.005436, 0.004152, 0.007236, 0.065676, 0.074424, 0.038150, 0.019212, 0.025906, 0.012394, 0.012899, 0.009820, 0.004845, 0.004431, 0.003534, 0.003035, 0.002657, 0.002406, 0.551225, 0.519801, 0.224800, 0.094302, 0.034427];

    #[test]
    fn bate_com_o_modelo_oficial() {
        let mut s = Silero::new();
        for (k, esperado) in ESPERADO.iter().enumerate() {
            let janela: Vec<f32> = (0..512).map(|i| sinal(k * 512 + i)).collect();
            let p = s.janela(&janela);
            assert!((p - esperado).abs() < 2e-3, "janela {k}: {p} esperado {esperado}");
        }
    }

    #[test]
    fn silencio_nao_e_voz_e_processar_junta_trechos_quebrados() {
        let mut s = Silero::new();
        assert!(s.processar(&vec![0i16; 1600]) < 0.2);
        // 1600 amostras = 3 janelas + 64 sobrando; picotar o áudio não muda o resultado
        let pcm: Vec<i16> = (0..3200).map(|n| (sinal(n) * 20000.0) as i16).collect();
        let (mut a, mut b) = (Silero::new(), Silero::new());
        let inteiro = a.processar(&pcm);
        let mut picotado = 0.0f32;
        for pedaco in pcm.chunks(700) {
            picotado = picotado.max(b.processar(pedaco));
        }
        assert!(inteiro.is_finite() && (0.0..=1.0).contains(&inteiro));
        assert!(picotado.is_finite());
        assert_eq!(a.pendente.len(), b.pendente.len());
        assert_eq!(a.h, b.h, "a memória da rede não depende de como o áudio foi picotado");
    }
}
