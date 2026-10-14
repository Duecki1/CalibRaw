#!/usr/bin/env python3
"""Export BiRefNet General-Lite to ONNX at a fixed square input size.

Usage: python3 packaging/models/birefnet_export.py BIREFNET_REPO WEIGHTS EDGE OUTPUT.onnx

Follows upstream's BiRefNet_pth2onnx notebook. With these inputs a 1024 export
has the same graph as upstream's BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx
(the source of birefnet-lowQ.onnx), with folded constants within 1e-6:

- BIREFNET_REPO: github.com/ZhengPeng7/BiRefNet at d8c386b (2024-08-19), with
  `self.bb` in config.py set to index 3 (swin_v1_t). Later revisions use SDPA
  attention and export a different graph.
- WEIGHTS: BiRefNet-general-bb_swin_v1_tiny-epoch_232.pth from upstream's v1
  release (SHA-256 6a1e050c6ec2697e5ed268455df544782b023acf8643ab771250979094875ab1).
- Python 3.11 with torch 2.0.1 and torchvision 0.15.2 (CPU), numpy<2,
  timm 0.9.16, kornia 0.7.2, onnx 1.14.1, opencv-python-headless and
  deform_conv2d_onnx_exporter.

Then rewrite the deformable convolutions with birefnet_gridsample.py (onnx
1.22) so GPU execution providers can run it, and record the result's size and
SHA-256 in crates/calibraw-ai/src/ai_masks.rs.
"""

import os
import sys

if len(sys.argv) != 5:
    sys.exit(__doc__)
repo, weights, edge, output = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
weights, output = os.path.abspath(weights), os.path.abspath(output)
sys.path.insert(0, repo)
os.chdir(repo)

import deform_conv2d_onnx_exporter
import torch
from torch.onnx import symbolic_helper as sym_help


def get_tensor_dim_size(tensor, dim):
    # The notebook's patch for dimensions the TorchScript tracer leaves unknown.
    size = sym_help._get_tensor_dim_size(tensor, dim)
    if size is None and dim in (2, 3):
        strides = tensor.type().strides()
        size = strides[2] if dim == 3 else strides[1] // strides[2]
    elif size is None and dim == 0:
        size = tensor.type().strides()[3]
    return size


deform_conv2d_onnx_exporter.get_tensor_dim_size = get_tensor_dim_size
deform_conv2d_onnx_exporter.register_deform_conv2d_onnx_op()

from models.birefnet import BiRefNet
from utils import check_state_dict

net = BiRefNet(bb_pretrained=False)
net.load_state_dict(check_state_dict(torch.load(weights, map_location="cpu", weights_only=True)))
net.eval()

torch.manual_seed(0)
with torch.no_grad():
    torch.onnx.export(
        net,
        torch.randn(1, 3, edge, edge),
        output,
        verbose=False,
        opset_version=17,
        input_names=["input_image"],
        output_names=["output_image"],
    )
print(f"exported {output} at {edge}x{edge}")
