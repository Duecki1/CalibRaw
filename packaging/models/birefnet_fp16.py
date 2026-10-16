#!/usr/bin/env python3
"""Convert a GridSample BiRefNet model to float16 for GPU execution providers.

Usage: python3 packaging/models/birefnet_fp16.py INPUT.onnx OUTPUT.onnx

INPUT is the output of birefnet_gridsample.py (birefnet-lowQ-gridsample.onnx or
birefnet-lite-768-gridsample.onnx). Weights and activations become float16,
which halves the GPU memory a subject mask needs and lets RTX tensor cores run
it. Inputs and outputs stay float32, so CalibRaw feeds both precisions the
same tensors.

GridSample stays float32: CUDA has no float16 kernel for it and runs it on the
CPU, which makes the model about ten times slower. The converter's default
block list keeps its other numerically sensitive operators in float32.

On the 1024 model with CUDA, peak process memory falls from 2266 to 1248 MiB
and the mask matches float32 with an IoU of 0.9989 (mean alpha difference
0.09/255). CPU execution providers have few float16 kernels, so CalibRaw uses
this model only for GPU sessions.

Requires: onnx 1.22, onnxconverter-common 1.16.0. Output is deterministic;
record its size and SHA-256 in crates/calibraw-ai/src/ai_masks.rs.
"""

import sys

import onnx
from onnxconverter_common import float16


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    model = onnx.load(sys.argv[1])
    converted = float16.convert_float_to_float16(
        model,
        keep_io_types=True,
        op_block_list=float16.DEFAULT_OP_BLOCK_LIST + ["GridSample"],
    )
    onnx.checker.check_model(converted)
    onnx.save(converted, sys.argv[2])


if __name__ == "__main__":
    main()
