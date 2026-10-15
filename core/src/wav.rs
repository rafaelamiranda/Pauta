/// Empacota PCM 16 bits mono em um arquivo WAV.
pub fn pcm16_para_wav(pcm: &[i16], taxa: u32) -> Vec<u8> {
    let bytes = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + pcm.len() * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + bytes).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes()); // tamanho do bloco fmt
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&taxa.to_le_bytes());
    w.extend_from_slice(&(taxa * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&bytes.to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn cabecalho_correto() {
        let wav = pcm16_para_wav(&[0i16; 160], 16000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 320);
        assert_eq!(wav.len(), 44 + 320);
    }
}
