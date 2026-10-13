#!/usr/bin/env python3
"""Rewrite BiRefNet's deformable convolutions for GPU execution providers.

Usage: python3 packaging/models/birefnet_gridsample.py INPUT.onnx OUTPUT.onnx

The published BiRefNet export emulates torchvision's modulated deform_conv2d
with GatherND over a k-times upsampled im2col image. In decoder_block1 that is
a 7x7 kernel at 256x256 with 64 channels: several 820 MB tensors alive at once,
which overruns GPU memory or the Windows GPU watchdog under DirectML and takes
the whole device down. This script replaces each emulated block with the same
computation done one kernel row at a time:

    grid   = (base + offset), normalised for align_corners=1
    sample = GridSample(x, grid, bilinear, zeros)        per kernel row
    out   += Conv1x1(sample * mask, weight[:, :, row])
    out   += bias

Bilinear sampling with zero padding matches deform_conv2d's boundary handling
(corners outside the input contribute zero), so outputs agree to float32
rounding. Only ONNX opset 16 operators are used, all of which DirectML, CUDA
and the CPU provider implement.

Requires: onnx, numpy. Output is deterministic; record its size and SHA-256 in
crates/calibraw-ai/src/ai_masks.rs.
"""

import sys

import numpy as np
import onnx
from onnx import helper, numpy_helper, shape_inference


def tensor_shapes(model):
    inferred = shape_inference.infer_shapes(model, data_prop=True)
    shapes = {}
    for info in [*inferred.graph.value_info, *inferred.graph.input, *inferred.graph.output]:
        dims = info.type.tensor_type.shape.dim
        if all(dim.HasField("dim_value") for dim in dims):
            shapes[info.name] = [dim.dim_value for dim in dims]
    return shapes


def attributes(node):
    return {attribute.name: helper.get_attribute_value(attribute) for attribute in node.attribute}


def constant_value(graph, name, initializers):
    if name in initializers:
        return numpy_helper.to_array(initializers[name])
    for node in graph.node:
        if node.op_type == "Constant" and name in node.output:
            return numpy_helper.to_array(attributes(node)["value"])
    raise ValueError(f"{name} is not a constant")


class Block:
    """One exported deformable convolution, identified by its name prefix."""

    def __init__(self, graph, prefix, initializers, shapes):
        self.prefix = prefix
        inside = [node for node in graph.node if node.name.startswith(prefix + "/")]

        def conv_using(weight_suffix):
            matches = [
                node
                for node in inside
                if node.op_type == "Conv" and node.input[1].endswith(weight_suffix)
            ]
            if len(matches) != 1:
                raise ValueError(f"{prefix}: expected one Conv using *{weight_suffix}")
            return matches[0]

        self.offset_conv = conv_using("offset_conv.weight")
        self.modulator_conv = conv_using("modulator_conv.weight")
        regular_conv = conv_using("regular_conv.weight")
        sigmoid = next(
            node
            for node in inside
            if node.op_type == "Sigmoid" and node.input[0] == self.modulator_conv.output[0]
        )
        # mask = 2 * sigmoid(modulator_conv(x)), as in BiRefNet's DeformableConv2d.
        self.mask_mul = next(
            node for node in inside if node.op_type == "Mul" and sigmoid.output[0] in node.input
        )
        kept_nodes = [self.offset_conv, self.modulator_conv, sigmoid, self.mask_mul]
        kept_inputs = {name for node in kept_nodes for name in node.input}
        kept_nodes += [
            node
            for node in inside
            if node.op_type == "Constant" and node.output[0] in kept_inputs
        ]
        self.kept = {id(node) for node in kept_nodes}
        self.removed = [node for node in inside if id(node) not in self.kept]

        produced = {output for node in inside for output in node.output}
        consumers = {}
        for node in graph.node:
            for name in node.input:
                consumers.setdefault(name, []).append(node)
        outputs = [
            name
            for name in produced
            if any(not node.name.startswith(prefix + "/") for node in consumers.get(name, []))
        ]
        if len(outputs) != 1:
            raise ValueError(f"{prefix}: expected one block output, found {outputs}")
        self.output = outputs[0]
        bias_add = next(node for node in inside if self.output in node.output)
        if bias_add.op_type != "Add" or regular_conv.output[0] not in bias_add.input:
            raise ValueError(f"{prefix}: block output is not regular_conv + bias")
        bias_input = next(name for name in bias_add.input if name != regular_conv.output[0])
        self.bias = constant_value(graph, bias_input, initializers).astype(np.float32)

        self.input = self.offset_conv.input[0]
        self.weight = numpy_helper.to_array(initializers[regular_conv.input[1]])
        conv = attributes(self.offset_conv)
        kernel_h, kernel_w = self.weight.shape[2:]
        self.stride = conv.get("strides", [1, 1])
        self.pads = conv.get("pads", [0, 0, 0, 0])
        if conv.get("dilations", [1, 1]) != [1, 1] or conv.get("group", 1) != 1:
            raise ValueError(f"{prefix}: dilated or grouped offset convolutions are unsupported")
        if list(conv.get("kernel_shape", [kernel_h, kernel_w])) != [kernel_h, kernel_w]:
            raise ValueError(f"{prefix}: offset and regular kernels differ")
        if attributes(regular_conv).get("group", 1) != 1:
            raise ValueError(f"{prefix}: grouped regular convolution is unsupported")
        _, self.channels, self.height, self.width = shapes[self.input]
        _, offset_channels, self.out_h, self.out_w = shapes[self.offset_conv.output[0]]
        if offset_channels != 2 * kernel_h * kernel_w:
            raise ValueError(f"{prefix}: offset tensor has {offset_channels} channels")
        if self.height < 2 or self.width < 2:
            raise ValueError(f"{prefix}: align_corners normalisation needs at least 2x2 input")
        self.kernel = (kernel_h, kernel_w)

    def replacement(self):
        """Nodes and initializers computing the block output by kernel rows."""
        kernel_h, kernel_w = self.kernel
        taps = kernel_h * kernel_w
        out_h, out_w = self.out_h, self.out_w
        prefix = self.prefix + "/rows"
        nodes, initializers = [], []

        def const(name, array):
            full = f"{prefix}/{name}"
            initializers.append(numpy_helper.from_array(np.asarray(array), full))
            return full

        def node(op, inputs, name, **attrs):
            output = f"{prefix}/{name}_output_0"
            nodes.append(helper.make_node(op, inputs, [output], name=f"{prefix}/{name}", **attrs))
            return output

        # Sampling position of tap (i, j) at output (y, x), before the learned
        # offset: y * stride - pad + i (dilation is 1). Channel 0 is x, 1 is y.
        rows = np.arange(out_h, dtype=np.float32) * self.stride[0] - self.pads[0]
        cols = np.arange(out_w, dtype=np.float32) * self.stride[1] - self.pads[1]
        base = np.zeros((taps, 2, out_h, out_w), np.float32)
        for i in range(kernel_h):
            for j in range(kernel_w):
                base[i * kernel_w + j, 0] = (cols + j)[None, :]
                base[i * kernel_w + j, 1] = (rows + i)[:, None]
        scale = np.array([2.0 / (self.width - 1), 2.0 / (self.height - 1)], np.float32)

        # torchvision orders each tap's offset pair as (dy, dx); swap to (x, y).
        offsets = node(
            "Reshape",
            [self.offset_conv.output[0], const("offset_shape", np.array([taps, 2, out_h, out_w], np.int64))],
            "offset_reshape",
        )
        offsets_xy = node("Gather", [offsets, const("xy", np.array([1, 0], np.int64))], "offset_xy", axis=1)
        positions = node("Add", [offsets_xy, const("base", base)], "positions")
        scaled = node("Mul", [positions, const("scale", scale.reshape(1, 2, 1, 1))], "scale_mul")
        normalised = node("Sub", [scaled, const("one", np.array(1.0, np.float32))], "normalise")
        grid = node("Transpose", [normalised], "grid", perm=[0, 2, 3, 1])
        mask = node(
            "Reshape",
            [self.mask_mul.output[0], const("mask_shape", np.array([1, 1, taps, out_h, out_w], np.int64))],
            "mask_reshape",
        )

        axis0 = const("axis0", np.array([0], np.int64))
        axis2 = const("axis2", np.array([2], np.int64))
        grid_shape = const("grid_shape", np.array([1, kernel_w * out_h, out_w, 2], np.int64))
        sample_shape = const(
            "sample_shape", np.array([1, self.channels, kernel_w, out_h, out_w], np.int64)
        )
        merged_shape = const(
            "merged_shape", np.array([1, self.channels * kernel_w, out_h, out_w], np.int64)
        )
        total = None
        for row in range(kernel_h):
            start = const(f"row{row}_start", np.array([row * kernel_w], np.int64))
            end = const(f"row{row}_end", np.array([(row + 1) * kernel_w], np.int64))
            row_grid = node("Slice", [grid, start, end, axis0], f"row{row}_grid")
            row_grid = node("Reshape", [row_grid, grid_shape], f"row{row}_grid_reshape")
            sampled = node(
                "GridSample",
                [self.input, row_grid],
                f"row{row}_sample",
                mode="bilinear",
                padding_mode="zeros",
                align_corners=1,
            )
            sampled = node("Reshape", [sampled, sample_shape], f"row{row}_sample_reshape")
            row_mask = node("Slice", [mask, start, end, axis2], f"row{row}_mask")
            modulated = node("Mul", [sampled, row_mask], f"row{row}_modulate")
            modulated = node("Reshape", [modulated, merged_shape], f"row{row}_merge")
            # Input channel c * kernel_w + j holds channel c sampled at tap (row, j).
            row_weight = self.weight[:, :, row, :].reshape(self.weight.shape[0], -1, 1, 1)
            contribution = node(
                "Conv",
                [modulated, const(f"row{row}_weight", np.ascontiguousarray(row_weight))],
                f"row{row}_conv",
            )
            total = contribution if total is None else node("Add", [total, contribution], f"row{row}_accumulate")

        nodes.append(
            helper.make_node("Add", [total, const("bias", self.bias)], [self.output], name=f"{prefix}/bias")
        )
        return nodes, initializers


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    model = onnx.load(sys.argv[1])
    graph = model.graph
    shapes = tensor_shapes(model)
    initializers = {initializer.name: initializer for initializer in graph.initializer}
    prefixes = sorted(
        {node.name.rsplit("/", 1)[0] for node in graph.node if node.op_type == "GatherND"}
    )
    blocks = [Block(graph, prefix, initializers, shapes) for prefix in prefixes]

    removed = {id(node) for block in blocks for node in block.removed}
    replacements = {block.output: block.replacement() for block in blocks}
    nodes = []
    for node in graph.node:
        if id(node) in removed:
            # Emit each replacement where its block's output used to be produced.
            for output in node.output:
                if output in replacements:
                    new_nodes, new_initializers = replacements.pop(output)
                    nodes.extend(new_nodes)
                    graph.initializer.extend(new_initializers)
            continue
        nodes.append(node)
    if replacements:
        raise ValueError(f"replacement outputs were never placed: {sorted(replacements)}")
    del graph.node[:]
    graph.node.extend(nodes)

    # Drop initializers only the removed emulation used (regular_conv weights).
    used = {name for node in graph.node for name in node.input}
    kept = [initializer for initializer in graph.initializer if initializer.name in used]
    del graph.initializer[:]
    graph.initializer.extend(kept)

    onnx.checker.check_model(model)
    onnx.save(model, sys.argv[2])
    remaining = sum(node.op_type in ("GatherND", "ScatterND") for node in graph.node if "atrous_conv" in node.name)
    print(f"rewrote {len(blocks)} deformable convolutions; {remaining} GatherND/ScatterND left in them")


if __name__ == "__main__":
    main()
