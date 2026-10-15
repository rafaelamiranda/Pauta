//! Divide o áudio contínuo em falas, como um detector de voz (VAD):
//! mede o volume (RMS) de cada trecho e fecha a fala depois de um tempo de silêncio.
//! Uma instância por fonte de áudio (microfone ou som do computador).

use crate::resample::TAXA;

pub struct Fala {
    pub pcm: Vec<i16>,
    pub inicio_ms: i64,
}

pub struct Segmentador {
    limiar_rms: f32, // volume mínimo para contar como fala (0 a 1)
    silencio_ms: f64, // silêncio que encerra uma fala
    fala_minima_ms: f64, // falas mais curtas que isso são descartadas (ruído)
    maxima_ms: f64, // corte forçado para falas muito longas
    pre_roll_ms: f64, // áudio guardado antes da fala, para não cortar o começo

    pre_roll: Vec<Vec<i16>>,
    pre_roll_amostras: usize,
    trechos: Vec<Vec<i16>>,
    amostras: usize,
    amostras_fala: usize,
    amostras_silencio: usize,
    inicio_ms: i64,
    falando: bool,
}

fn ms(n: usize) -> f64 {
    n as f64 / TAXA as f64 * 1000.0
}

impl Default for Segmentador {
    fn default() -> Self {
        Self::new()
    }
}

impl Segmentador {
    pub fn new() -> Self {
        Self {
            limiar_rms: 0.012,
            silencio_ms: 700.0,
            fala_minima_ms: 250.0,
            maxima_ms: 25000.0,
            pre_roll_ms: 300.0,
            pre_roll: Vec::new(),
            pre_roll_amostras: 0,
            trechos: Vec::new(),
            amostras: 0,
            amostras_fala: 0,
            amostras_silencio: 0,
            inicio_ms: 0,
            falando: false,
        }
    }

    pub fn com_maxima_ms(mut self, v: f64) -> Self {
        self.maxima_ms = v;
        self
    }

    fn zerar(&mut self) {
        self.trechos.clear();
        self.amostras = 0;
        self.amostras_fala = 0;
        self.amostras_silencio = 0;
        self.falando = false;
    }

    /// Recebe um trecho de PCM 16 bits, 16 kHz mono. Devolve a fala quando ela termina.
    pub fn push(&mut self, pcm: &[i16], agora_ms: i64) -> Option<Fala> {
        let soma: f64 = pcm.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
        let rms = (soma / pcm.len().max(1) as f64).sqrt() as f32;
        let voz = rms > self.limiar_rms;

        if !self.falando {
            if voz {
                self.falando = true;
                self.inicio_ms = agora_ms - ms(self.pre_roll_amostras) as i64;
                self.trechos = std::mem::take(&mut self.pre_roll);
                self.amostras = self.pre_roll_amostras;
                self.pre_roll_amostras = 0;
            } else {
                self.pre_roll.push(pcm.to_vec());
                self.pre_roll_amostras += pcm.len();
                while self.pre_roll.len() > 1 && ms(self.pre_roll_amostras) > self.pre_roll_ms {
                    self.pre_roll_amostras -= self.pre_roll.remove(0).len();
                }
                return None;
            }
        }

        self.trechos.push(pcm.to_vec());
        self.amostras += pcm.len();
        if voz {
            self.amostras_fala += pcm.len();
            self.amostras_silencio = 0;
        } else {
            self.amostras_silencio += pcm.len();
        }

        if ms(self.amostras_silencio) >= self.silencio_ms || ms(self.amostras) >= self.maxima_ms {
            return self.fechar();
        }
        None
    }

    /// Encerra a fala em andamento (ao parar a reunião).
    pub fn flush(&mut self) -> Option<Fala> {
        if self.falando {
            self.fechar()
        } else {
            None
        }
    }

    fn fechar(&mut self) -> Option<Fala> {
        let falou_ms = ms(self.amostras_fala);
        let pcm: Vec<i16> = self.trechos.iter().flatten().copied().collect();
        let inicio_ms = self.inicio_ms;
        self.zerar();
        (falou_ms >= self.fala_minima_ms).then_some(Fala { pcm, inicio_ms })
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    const FALA: i16 = 6000;
    fn bloco(v: i16) -> Vec<i16> {
        vec![v; 1600]
    }

    #[test]
    fn fecha_a_fala_apos_o_silencio_e_descarta_ruido_curto() {
        let mut seg = Segmentador::new();
        let mut falas = 0;
        for _ in 0..5 { assert!(seg.push(&bloco(0), 0).is_none()); }
        for _ in 0..6 { assert!(seg.push(&bloco(FALA), 0).is_none()); }
        for _ in 0..8 { if let Some(f) = seg.push(&bloco(0), 0) { falas += 1; assert!(f.pcm.len() >= 6 * 1600); } }
        assert_eq!(falas, 1);

        seg.push(&bloco(FALA), 0); // 100 ms: curto demais
        for _ in 0..8 { assert!(seg.push(&bloco(0), 0).is_none()); }
    }

    #[test]
    fn flush_fecha_e_corte_forcado_em_falas_longas() {
        let mut seg = Segmentador::new().com_maxima_ms(2000.0);
        for _ in 0..5 { seg.push(&bloco(FALA), 0); }
        assert!(seg.flush().is_some());
        let mut cortes = 0;
        for _ in 0..25 { if seg.push(&bloco(FALA), 0).is_some() { cortes += 1; } }
        assert_eq!(cortes, 1, "corte forçado em 2 s");
    }
}
