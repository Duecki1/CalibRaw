#!/usr/bin/env python3
"""Export Distill-Any-Depth Small to ONNX.

Usage: python3 packaging/models/distill_any_depth_export.py MODEL.safetensors OUT.onnx CHECK.jpg

- MODEL.safetensors: small/model.safetensors of
  huggingface.co/xingyang1/Distill-Any-Depth at revision
  38095a41cca1e28a28e8bb6372c68df721455a2d (SHA-256
  56a173c0e1b5045bf6296a5c1fb16eace0bbde2eddc24b37532cb1774ac09caa).
- CHECK.jpg: any photo, for example a Demo photo. The export runs it on the
  ONNX Runtime CPU provider at three sizes and compares it with PyTorch.
- Python 3.14 with torch 2.13.0, transformers 5.14.1, safetensors 0.8.0,
  onnx 1.23.2, onnxruntime 1.31.1 and pillow.

The checkpoint is Depth Anything V2 Small in the original repository's
layout (`pretrained.*` encoder, `depth_head.*` DPT head); it is renamed to
transformers' DepthAnythingForDepthEstimation and loaded strictly.

The model takes `pixel_values`, 1 x 3 x H x W RGB normalized with the
ImageNet mean and deviation, H and W multiples of 14, and returns
`predicted_depth`, 1 x H x W relative inverse depth (high is near). Both
sides are dynamic: CalibRaw scales the photo to 1540 pixels on its longer
side, keeping its aspect ratio.

Two changes keep the export usable at that size, without changing its
result (PyTorch agrees with the unchanged model to about 1e-6):

- Attention runs one head at a time, and each head in QUERY_CHUNKS blocks
  of query rows; softmax is per row, so the blocks are independent. At
  1036 x 1540 a head's score matrix is 8141 x 8141 float32 (265 MB); with
  all six at once ONNX Runtime's CPU provider peaks near 6.9 GB, one at a
  time near 1.9 GB. ONNX Runtime's WebGPU provider reuses a released
  buffer within a run only up to 160 MiB, and in a few sizes it keeps
  several of (`BUCKET_DEFAULT_LIMIT_TABLE` in its `buffer_manager.cc`); it
  holds the others until its next flush, every 16 dispatches, so whole
  score matrices piled up to 6.3 GB on the GPU. An eighth of the rows is
  32 MiB at 1036 x 1540, a size it keeps 22 of, and 73 MB at 1540 x 1540.
  With 8 blocks the GPU peaks near 2.0 GiB.
- The blocks' bounds come from the token count, which ONNX's own shape
  inference does not follow, so ONNX Runtime could not prove that buffers
  fit one another and reuse them: the CPU peak rose from about 1.0 to
  1.6 GB. ONNX Runtime's symbolic shape inference names every dimension in
  the saved graph, which brings it to about 0.8 GB and speeds up the GPU,
  for about 0.4 GiB more GPU memory than without it.
- The head's last resize takes its size from the input instead of a Python
  integer, which the TorchScript exporter would freeze at the export size.

The Small model is Apache-2.0 (model card) and its code MIT; publish the
export with both notices. Record the size and SHA-256 of OUT.onnx in
crates/calibraw-ai/src/registry/mod.rs.
"""

import os
import re
import sys
import time

os.environ["TORCHDYNAMO_DISABLE"] = "1"

import numpy as np
import onnx
import onnxruntime
import torch
from onnxruntime.tools.symbolic_shape_infer import SymbolicShapeInference
from PIL import Image
from safetensors.torch import load_file
from torch import nn
from transformers import DepthAnythingConfig, DepthAnythingForDepthEstimation, Dinov2Config
import transformers.models.depth_anything.modeling_depth_anything as depth_anything
import transformers.models.dinov2.modeling_dinov2 as dinov2

MEAN = np.array([0.485, 0.456, 0.406], np.float32)
STD = np.array([0.229, 0.224, 0.225], np.float32)
# Input sizes the check runs: the landscape and portrait size CalibRaw uses
# for a 3:2 photo and a smaller one.
CHECK_SIZES = [(1036, 1540), (1540, 1036), (728, 1092)]


# Blocks of query rows per head; see the module notes.
QUERY_CHUNKS = 8


def per_head_attention(module, query, key, value, attention_mask, scaling=None, dropout=0.0, **kwargs):
    if scaling is None:
        scaling = query.size(-1) ** -0.5
    tokens = query.size(2)
    outputs = []
    for head in range(query.shape[1]):
        keys = key[:, head : head + 1].transpose(2, 3)
        values = value[:, head : head + 1]
        rows = []
        for chunk in range(QUERY_CHUNKS):
            # Traced from the input's token count, so the bounds follow the
            # dynamic height and width.
            start = tokens * chunk // QUERY_CHUNKS
            end = tokens * (chunk + 1) // QUERY_CHUNKS
            scores = torch.matmul(query[:, head : head + 1, start:end], keys) * scaling
            rows.append(torch.matmul(nn.functional.softmax(scores, dim=-1), values))
        outputs.append(torch.cat(rows, dim=2))
    return torch.cat(outputs, dim=1).transpose(1, 2).contiguous(), None


def head_forward(self, hidden_states, patch_height, patch_width):
    depth = self.conv1(hidden_states[self.head_in_index])
    depth = nn.functional.interpolate(
        depth,
        (patch_height * self.patch_size, patch_width * self.patch_size),
        mode="bilinear",
        align_corners=True,
    )
    depth = self.activation2(self.conv3(self.activation1(self.conv2(depth)))) * self.max_depth
    return depth.squeeze(dim=1)


def renamed(state):
    """Original Depth Anything V2 names to transformers' names."""
    out = {}
    for key, value in state.items():
        block = re.match(r"pretrained\.blocks\.(\d+)\.(.*)", key)
        if block:
            index, rest = block.groups()
            prefix = f"backbone.encoder.layer.{index}."
            if rest.startswith("attn.qkv."):
                kind = rest.split(".")[-1]
                for name, part in zip(["query", "key", "value"], value.chunk(3, dim=0)):
                    out[f"{prefix}attention.attention.{name}.{kind}"] = part
                continue
            rest = (
                rest.replace("attn.proj", "attention.output.dense")
                .replace("ls1.gamma", "layer_scale1.lambda1")
                .replace("ls2.gamma", "layer_scale2.lambda1")
            )
            out[prefix + rest] = value
            continue
        key = (
            key.replace("pretrained.cls_token", "backbone.embeddings.cls_token")
            .replace("pretrained.mask_token", "backbone.embeddings.mask_token")
            .replace("pretrained.pos_embed", "backbone.embeddings.position_embeddings")
            .replace("pretrained.patch_embed.proj", "backbone.embeddings.patch_embeddings.projection")
            .replace("pretrained.norm", "backbone.layernorm")
        )
        key = re.sub(r"depth_head\.projects\.(\d)", r"neck.reassemble_stage.layers.\1.projection", key)
        key = re.sub(r"depth_head\.resize_layers\.(\d)", r"neck.reassemble_stage.layers.\1.resize", key)
        key = re.sub(r"depth_head\.scratch\.layer(\d)_rn", lambda m: f"neck.convs.{int(m.group(1)) - 1}", key)
        fusion = re.match(r"depth_head\.scratch\.refinenet(\d)\.(.*)", key)
        if fusion:
            index, rest = fusion.groups()
            rest = (
                rest.replace("out_conv", "projection")
                .replace("resConfUnit1", "residual_layer1")
                .replace("resConfUnit2", "residual_layer2")
                .replace("conv1", "convolution1")
                .replace("conv2", "convolution2")
            )
            key = f"neck.fusion_stage.layers.{4 - int(index)}.{rest}"
        key = (
            key.replace("depth_head.scratch.output_conv1", "head.conv1")
            .replace("depth_head.scratch.output_conv2.0", "head.conv2")
            .replace("depth_head.scratch.output_conv2.2", "head.conv3")
        )
        out[key] = value
    return out


def build(checkpoint, export):
    """Depth Anything V2 Small; with `export`, in the form exported."""
    config = DepthAnythingConfig(
        backbone_config=Dinov2Config(
            hidden_size=384,
            num_hidden_layers=12,
            num_attention_heads=6,
            image_size=518,
            patch_size=14,
            layerscale_value=1.0,
            out_indices=[3, 6, 9, 12],
            apply_layernorm=True,
            reshape_hidden_states=False,
        ),
        reassemble_hidden_size=384,
        fusion_hidden_size=64,
        neck_hidden_sizes=[48, 96, 192, 384],
        depth_estimation_type="relative",
    )
    config._attn_implementation = "eager"
    config.backbone_config._attn_implementation = "eager"
    model = DepthAnythingForDepthEstimation(config)
    model.load_state_dict(renamed(load_file(checkpoint)), strict=True)

    class Depth(nn.Module):
        def __init__(self):
            super().__init__()
            self.model = model

        def forward(self, pixel_values):
            return self.model(pixel_values=pixel_values).predicted_depth

    return Depth().eval()


def main():
    checkpoint, output, check = sys.argv[1:4]
    torch.set_grad_enabled(False)
    photo = Image.open(check).convert("RGB")

    def pixels(height, width):
        resized = np.asarray(photo.resize((width, height), Image.LANCZOS), np.float32) / 255
        return ((resized - MEAN) / STD).transpose(2, 0, 1)[None].copy()

    reference = build(checkpoint, export=False)
    expected = {size: reference(torch.from_numpy(pixels(*size))).numpy() for size in CHECK_SIZES}

    original_attention = dinov2.eager_attention_forward
    original_head = depth_anything.DepthAnythingDepthEstimationHead.forward
    dinov2.eager_attention_forward = per_head_attention
    depth_anything.DepthAnythingDepthEstimationHead.forward = head_forward
    try:
        model = build(checkpoint, export=True)
        for size in CHECK_SIZES:
            changed = model(torch.from_numpy(pixels(*size))).numpy()
            relative = np.abs(changed - expected[size]).max() / np.abs(expected[size]).max()
            print(f"export form {size}: max relative difference {relative:.2e}")
            assert relative < 1e-5, "the export changes must not change the result"
        torch.onnx.export(
            model,
            (torch.from_numpy(pixels(*CHECK_SIZES[0])),),
            output,
            input_names=["pixel_values"],
            output_names=["predicted_depth"],
            dynamic_axes={
                "pixel_values": {0: "batch_size", 2: "height", 3: "width"},
                "predicted_depth": {0: "batch_size", 1: "height", 2: "width"},
            },
            opset_version=17,
            dynamo=False,
            do_constant_folding=True,
        )
    finally:
        dinov2.eager_attention_forward = original_attention
        depth_anything.DepthAnythingDepthEstimationHead.forward = original_head
    onnx.save(SymbolicShapeInference.infer_shapes(onnx.load(output), auto_merge=True), output)

    session = onnxruntime.InferenceSession(output, providers=["CPUExecutionProvider"])
    for size in CHECK_SIZES:
        started = time.time()
        actual = session.run(None, {"pixel_values": pixels(*size)})[0]
        assert actual.shape == expected[size].shape, (actual.shape, expected[size].shape)
        relative = np.abs(actual - expected[size]).max() / np.abs(expected[size]).max()
        print(f"ONNX {size}: max relative difference {relative:.2e} in {time.time() - started:.1f} s")
        assert relative < 1e-3
    print(f"wrote {output} ({os.path.getsize(output)} bytes)")


if __name__ == "__main__":
    main()
