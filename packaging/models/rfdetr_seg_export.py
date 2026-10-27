#!/usr/bin/env python3
"""Export an RF-DETR-Seg checkpoint to ONNX with RF-DETR's own exporter.

Usage: python3 packaging/models/rfdetr_seg_export.py SIZE OUTPUT.onnx CHECK.jpg...

- SIZE: nano, small, medium or large. rfdetr downloads the COCO checkpoint
  and checks its MD5 (rfdetr/assets/model_weights.py). Every RF-DETR-Seg
  checkpoint is Apache-2.0.
- CHECK.jpg: photos with people, for example the Demo photos
  Demo2.5/DSC09867 (1).jpg and Demo2.5/DSC09227.jpg. The export runs each on
  the ONNX Runtime CPU provider and compares the people it finds with
  PyTorch.
- Python 3.12 with rfdetr[onnx] 1.11.2, torch 2.14.1 (CPU), onnx 1.23.2,
  onnxruntime 1.31.1 and pillow.

The model takes `input`, 1 x 3 x R x R RGB normalized with the ImageNet mean
and deviation, R being the size's resolution (nano 312, small 384, medium
432, large 504); RF-DETR stretches every photo to that square. It returns,
for each of its Q queries:
- `dets`, 1 x Q x 4: the box as centre x, centre y, width and height, each a
  fraction of the input;
- `labels`, 1 x Q x 91: class logits in COCO category IDs (1 is person);
- `masks`, 1 x Q x R/4 x R/4: instance mask logits over the whole input.

Record the result's size and SHA-256 in crates/calibraw-ai/src/registry/mod.rs.
"""

import os
import shutil
import sys
import tempfile
import time

import numpy as np
import onnxruntime
import torch
import torch.nn.functional as F
from PIL import Image

import rfdetr

SIZES = {
    "nano": rfdetr.RFDETRSegNano,
    "small": rfdetr.RFDETRSegSmall,
    "medium": rfdetr.RFDETRSegMedium,
    "large": rfdetr.RFDETRSegLarge,
}
PERSON = 1
THRESHOLD = 0.5
IMAGENET_MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
IMAGENET_STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)


def preprocess(image, resolution):
    pixels = torch.from_numpy(np.asarray(image, dtype=np.float32) / 255.0).permute(2, 0, 1)
    pixels = F.interpolate(pixels[None], size=(resolution, resolution), mode="bilinear", antialias=False)
    pixels = (pixels[0].permute(1, 2, 0).numpy() - IMAGENET_MEAN) / IMAGENET_STD
    return np.ascontiguousarray(pixels.transpose(2, 0, 1)[None])


def onnx_people(session, image, resolution):
    pixels = preprocess(image, resolution)
    session.run(None, {"input": pixels})
    started = time.perf_counter()
    dets, labels, masks = session.run(None, {"input": pixels})
    seconds = time.perf_counter() - started
    scores = 1.0 / (1.0 + np.exp(-labels[0, :, PERSON]))
    keep = np.nonzero(scores > THRESHOLD)[0]
    width, height = image.size
    boxes = []
    for query in keep[np.argsort(-scores[keep])]:
        cx, cy, w, h = dets[0, query]
        boxes.append(((cx - w / 2) * width, (cy - h / 2) * height, (cx + w / 2) * width, (cy + h / 2) * height))
    return np.array(boxes).reshape(-1, 4), seconds, masks.shape


def iou(a, b):
    x0, y0 = max(a[0], b[0]), max(a[1], b[1])
    x1, y1 = min(a[2], b[2]), min(a[3], b[3])
    inter = max(0.0, x1 - x0) * max(0.0, y1 - y0)
    union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / union if union > 0 else 0.0


def main():
    if len(sys.argv) < 4 or sys.argv[1] not in SIZES:
        sys.exit(__doc__)
    size, output, checks = sys.argv[1], os.path.abspath(sys.argv[2]), sys.argv[3:]
    model = SIZES[size]()
    resolution = model.model.resolution
    with tempfile.TemporaryDirectory() as directory:
        exported = model.export(output_dir=directory, format="onnx", opset_version=17, verbose=False)
        shutil.move(str(exported), output)
    session = onnxruntime.InferenceSession(output, providers=["CPUExecutionProvider"])
    print(f"{size}: {os.path.getsize(output)} bytes, input {resolution}x{resolution}")
    for path in checks:
        image = Image.open(path).convert("RGB")
        boxes, seconds, mask_shape = onnx_people(session, image, resolution)
        detections = model.predict(image, threshold=THRESHOLD)
        reference = detections.xyxy[detections.class_id == PERSON]
        matched = sum(1 for box in reference if any(iou(box, found) > 0.95 for found in boxes))
        print(
            f"{os.path.basename(path)}: CPU run {seconds:.3f} s, masks {mask_shape}, "
            f"{len(boxes)} people, {matched} of PyTorch's {len(reference)} matched at IoU > 0.95"
        )


if __name__ == "__main__":
    main()
