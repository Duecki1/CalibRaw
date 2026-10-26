#!/usr/bin/env python3
"""Export Sapiens2-0.4B body-part segmentation to ONNX.

Usage: python3 packaging/models/sapiens2_seg_export.py MODEL_DIR FP32.onnx FP16_WEIGHTS.onnx CHECK.jpg

- MODEL_DIR: config.json, preprocessor_config.json and model.safetensors of
  huggingface.co/facebook/sapiens2-seg-0.4b at revision
  449b3c5335e6722bb94990abdd1aa6e612432f22 (model.safetensors SHA-256
  b85fdb50b7d6123a967d5ee4a505e222baff8d2f7ad6bbf353578c1a61dfbac9).
- CHECK.jpg: a photo with a person, for example the Demo photo
  Demo2.5/DSC09867 (1).jpg. Both exports run it on the ONNX Runtime CPU
  provider and are compared with PyTorch.
- Python 3.12 with torch 2.14.1 (CPU), transformers 5.21.0, onnx 1.23.2,
  onnxscript, onnxruntime 1.31.1 and pillow.

The model takes `pixel_values`, 1 x 3 x 1024 x 768 (height x width) RGB
normalized with the ImageNet mean and deviation, and returns `logits`,
1 x 29 x 1024 x 768, one channel per class of docs/SEG.md in the Sapiens2
repository (0 is background). Upstream stretches every input to that size
without keeping its aspect ratio, and so does CalibRaw.

Attention is exported as plain MatMul and Softmax (eager attention) at opset
18 through torch.export, which every execution provider CalibRaw uses can
run; the TorchScript exporter cannot export the head's instance norms.

FP32.onnx is the reference. FP16_WEIGHTS.onnx, which CalibRaw downloads,
stores every weight of 4096 values or more as float16, each cast back to
float32 where it is used; ONNX Runtime folds the casts when it creates the
session, so it computes in float32 from half the download. Converting the
activations too (onnxconverter-common) halves GPU memory, but the WebGPU
provider then computes in float16 and its masks reach only an IoU of 0.978
against the CPU's, where the AI parity harness asks for 0.98.

The Sapiens2 License (github.com/facebookresearch/sapiens2/blob/main/LICENSE.md)
applies to both exports; publish them with a copy of it. Record the size and
SHA-256 of FP16_WEIGHTS.onnx in crates/calibraw-ai/src/registry/mod.rs.
"""

import os
import sys
import time

import numpy as np
import onnx
import onnxruntime
import torch
from onnx import TensorProto, helper, numpy_helper
from PIL import Image
from transformers import Sapiens2ForSemanticSegmentation

INPUT_HEIGHT, INPUT_WIDTH = 1024, 768
IMAGENET_MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
IMAGENET_STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)
# Weights smaller than this (biases, norms, scales) stay float32; they are a
# negligible part of the file.
FLOAT16_MIN_VALUES = 4096


def preprocess(path):
    image = Image.open(path).convert("RGB")
    image = image.resize((INPUT_WIDTH, INPUT_HEIGHT), Image.Resampling.BILINEAR)
    pixels = (np.asarray(image, dtype=np.float32) / 255.0 - IMAGENET_MEAN) / IMAGENET_STD
    return np.ascontiguousarray(pixels.transpose(2, 0, 1)[None])


def softmax(logits):
    shifted = np.exp(logits - logits.max(axis=1, keepdims=True))
    return shifted / shifted.sum(axis=1, keepdims=True)


def float16_weights(model):
    """Stores the large float32 weights of `model` as float16, each cast back."""
    graph = model.graph
    graph_inputs = {value.name for value in graph.input}
    initializers, casts = [], []
    for initializer in graph.initializer:
        weights = numpy_helper.to_array(initializer)
        if (
            initializer.data_type != TensorProto.FLOAT
            or weights.size < FLOAT16_MIN_VALUES
            or initializer.name in graph_inputs
            or np.abs(weights).max() >= np.finfo(np.float16).max
        ):
            initializers.append(initializer)
            continue
        stored = numpy_helper.from_array(weights.astype(np.float16), initializer.name + ".float16")
        initializers.append(stored)
        casts.append(
            helper.make_node(
                "Cast", [stored.name], [initializer.name], to=TensorProto.FLOAT, name=stored.name
            )
        )
    del graph.initializer[:]
    graph.initializer.extend(initializers)
    nodes = list(graph.node)
    del graph.node[:]
    graph.node.extend(casts + nodes)
    return model


def compare(name, path, pixels, reference):
    session = onnxruntime.InferenceSession(path, providers=["CPUExecutionProvider"])
    session.run(None, {"pixel_values": pixels})
    started = time.perf_counter()
    (logits,) = session.run(None, {"pixel_values": pixels})
    seconds = time.perf_counter() - started
    probabilities, expected = softmax(logits), softmax(reference)
    agreement = (logits.argmax(axis=1) == reference.argmax(axis=1)).mean()
    difference = np.abs(probabilities - expected)
    print(
        f"{name}: {os.path.getsize(path)} bytes, CPU run {seconds:.2f} s, "
        f"label agreement {agreement:.5f}, probability difference "
        f"mean {difference.mean():.2e} max {difference.max():.3f}"
    )


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    model_dir, fp32_path, fp16_weights_path, check_path = sys.argv[1:]
    model = Sapiens2ForSemanticSegmentation.from_pretrained(
        model_dir, attn_implementation="eager", dtype=torch.float32
    ).eval()

    class Logits(torch.nn.Module):
        def __init__(self, inner):
            super().__init__()
            self.inner = inner

        def forward(self, pixel_values):
            return self.inner(pixel_values=pixel_values).logits

    wrapper = Logits(model).eval()
    pixels = preprocess(check_path)
    with torch.no_grad():
        reference = wrapper(torch.from_numpy(pixels)).numpy()
        torch.onnx.export(
            wrapper,
            (torch.zeros(1, 3, INPUT_HEIGHT, INPUT_WIDTH),),
            fp32_path,
            input_names=["pixel_values"],
            output_names=["logits"],
            opset_version=18,
            dynamo=True,
        )
    # The exporter keeps large weights in a separate .data file; CalibRaw
    # downloads one file, which protobuf's 2 GiB limit still allows.
    exported = onnx.load(fp32_path)
    os.remove(fp32_path + ".data")
    onnx.save(exported, fp32_path)
    onnx.checker.check_model(fp32_path)
    converted = float16_weights(exported)
    onnx.checker.check_model(converted)
    onnx.save(converted, fp16_weights_path)

    compare("float32", fp32_path, pixels, reference)
    compare("float16 weights", fp16_weights_path, pixels, reference)


if __name__ == "__main__":
    main()
