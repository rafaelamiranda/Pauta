//! Converte o áudio do dispositivo (qualquer taxa e nº de canais) para PCM 16 bits, 16 kHz mono,
//! em trechos de 100 ms.

pub const TAXA: u32 = 16000;
pub const TRECHO: usize = 1600; // 100 ms

pub struct ParaPcm16k {
    canais: usize,
    razao: f64, // amostras de entrada por amostra de saída
    fase: f64,
    soma: f32,
    n: u32,
    saida: Vec<i16>,
}

impl ParaPcm16k {
    pub fn new(taxa_entrada: u32, canais: usize) -> Self {
        Self {
            canais: canais.max(1),
            razao: taxa_entrada as f64 / TAXA as f64,
            fase: 0.0,
            soma: 0.0,
            n: 0,
            saida: Vec::with_capacity(TRECHO),
        }
    }

    /// Recebe amostras intercaladas (f32, -1..1). Devolve os trechos de 100 ms já completos.
    /// A redução de taxa faz a média das amostras (filtro simples contra chiado).
    pub fn feed(&mut self, entrada: &[f32]) -> Vec<Vec<i16>> {
        let mut prontos = Vec::new();
        for quadro in entrada.chunks_exact(self.canais) {
            let mono = quadro.iter().sum::<f32>() / self.canais as f32;
            self.soma += mono;
            self.n += 1;
            self.fase += 1.0;
            if self.fase >= self.razao {
                self.fase -= self.razao;
                let media = self.soma / self.n as f32;
                self.soma = 0.0;
                self.n = 0;
                self.saida.push((media.clamp(-1.0, 1.0) * 32767.0) as i16);
                if self.saida.len() == TRECHO {
                    prontos.push(std::mem::replace(&mut self.saida, Vec::with_capacity(TRECHO)));
                }
            }
        }
        prontos
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn reduz_48k_estereo_para_16k_mono() {
        let mut r = ParaPcm16k::new(48000, 2);
        // 1 s de áudio estéreo a 48 kHz = 96000 amostras -> 16000 amostras -> 10 trechos
        let entrada = vec![0.5f32; 96000];
        let trechos = r.feed(&entrada);
        assert_eq!(trechos.len(), 10);
        assert!(trechos.iter().all(|t| t.len() == TRECHO));
        assert!((trechos[0][100] as i32 - 16383).abs() < 2);
    }

    #[test]
    fn taxa_fracionada_mantem_a_duracao() {
        let mut r = ParaPcm16k::new(44100, 1);
        let trechos = r.feed(&vec![0.1f32; 44100]);
        assert!((9..=10).contains(&trechos.len()));
    }

    #[test]
    fn mistura_canais_em_mono() {
        let mut r = ParaPcm16k::new(16000, 2);
        let mut e = Vec::new();
        for _ in 0..TRECHO {
            e.extend_from_slice(&[1.0, -1.0]); // canais opostos se anulam
        }
        let t = r.feed(&e);
        assert_eq!(t[0][10], 0);
    }
}
