#!/usr/bin/env python3
"""Extrai os pesos do Silero VAD v5 (16 kHz) do arquivo ONNX oficial para core/assets/silero_v5_16k.f32.

O ONNX do Silero tem ramos "If" (8 kHz e 16 kHz) que só são úteis com o onnxruntime; o Pauta reimplementa a
rede em Rust (core/src/silero.rs) e só precisa dos pesos do ramo de 16 kHz, gravados em ordem fixa como f32
little-endian. Para refazer:

    pip install onnx numpy
    curl -LO https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx
    python3 scripts/gerar-silero.py silero_vad.onnx core/assets/silero_v5_16k.f32

Silero VAD: MIT, Copyright (c) 2020-present Silero Team (veja core/assets/SILERO-LICENSE.txt).
"""
import sys

import numpy as np
import onnx
from onnx import numpy_helper

ORDEM = [
    "stft.forward_basis_buffer",
    "encoder.0.reparam_conv.weight", "encoder.0.reparam_conv.bias",
    "encoder.1.reparam_conv.weight", "encoder.1.reparam_conv.bias",
    "encoder.2.reparam_conv.weight", "encoder.2.reparam_conv.bias",
    "encoder.3.reparam_conv.weight", "encoder.3.reparam_conv.bias",
    "decoder.rnn.weight_ih", "decoder.rnn.weight_hh", "decoder.rnn.bias_ih", "decoder.rnn.bias_hh",
    "decoder.decoder.2.weight", "decoder.decoder.2.bias",
]


def pesos(grafo, achados):
    for no in grafo.node:
        if no.op_type == "Constant" and no.attribute[0].name == "value":
            nome = no.output[0].split("Inline_0__")[-1]
            if nome in ORDEM:
                achados[nome] = numpy_helper.to_array(no.attribute[0].t).astype(np.float32)
        for a in no.attribute:
            if a.type == onnx.AttributeProto.GRAPH:
                pesos(a.g, achados)


def main(origem, destino):
    modelo = onnx.load(origem)
    se = next(n for n in modelo.graph.node if n.op_type == "If")
    ramo_16k = next(a.g for a in se.attribute if a.name == "then_branch")  # sr == 16000
    achados = {}
    pesos(ramo_16k, achados)
    faltam = [n for n in ORDEM if n not in achados]
    if faltam:
        sys.exit(f"Pesos não encontrados: {faltam}")
    with open(destino, "wb") as f:
        for nome in ORDEM:
            f.write(achados[nome].astype("<f4").tobytes())
            print(nome, achados[nome].shape)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
