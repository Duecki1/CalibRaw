#!/usr/bin/env python3
"""Rewrite an ONNX model's attention to run in blocks.

Usage:
    python3 packaging/models/attention_blocks.py IN.onnx OUT.onnx rows BLOCKS
    python3 packaging/models/attention_blocks.py IN.onnx OUT.onnx heads BLOCKS HEADS

- Python 3.14 with onnx 1.23.2 and onnxruntime 1.31.1.

Every attention in the graph,

    scores = MatMul(query, keys)        query [B, H, N, d], keys [B, H, d, M]
    (Mul or Div by a scalar, Cast)      optional, before or after Softmax
    weights = Softmax(scores, axis=-1)
    out = MatMul(weights, values)       values [B, H, M, d]

is computed in BLOCKS parts, each through its own copy of those nodes, and
the outputs are concatenated again: with `rows`, slices of `query` along
its rows (axis -2), bounded by N at run time; with `heads`, groups of the
HEADS heads (axis 1) of query, keys and values, with constant bounds.
Softmax is per row and per head, so the parts are independent, and ONNX
Runtime's CPU provider gives the original's output bit for bit.

Why: ONNX Runtime's WebGPU provider reuses a released buffer within a run
only up to 160 MiB, and keeps a few of each size up to that for good
(`BUCKET_DEFAULT_LIMIT_TABLE` in its `buffer_manager.cc`); it holds the
larger ones until its next flush, every 16 dispatches. A [1, 16, N, N]
float32 score tensor is 604 MB at N = 3072 (Sapiens2 at 1024 x 768).

Measure before shipping a rewrite; it does not always help:
- Sapiens2 0.4B, `rows 24` (24 MiB blocks): WebGPU peak 4550 -> 3436 MiB,
  same speed; `rows 12`, `rows 48` and `heads 16 16` came out higher.
- MoGe-2 ViT-L, every layout tried: WebGPU peak 4774 MiB rose to between
  4574 and 5319 and runs got slower, though `heads 16 16` cut its CPU
  peak by 700 MiB. The buffers the provider keeps for good outweigh the
  ones it frees; the authors' export stays as it is.

With `rows`, ONNX's own shape inference does not follow the run-time
bounds, so ONNX Runtime could not prove that buffers fit one another and
reuse them, which costs memory on the CPU and speed on the GPU; the output
is therefore annotated with ONNX Runtime's symbolic shape inference, which
names every dimension, where that tool can follow the graph (not MoGe-2's).

Scalar factors are the only other inputs the nodes between the MatMuls may
take; an attention with a mask or bias over its rows is left as it is.
"""

import sys

import numpy as np
import onnx
from onnx import helper, numpy_helper
from onnxruntime.tools.symbolic_shape_infer import SymbolicShapeInference

# Nodes between the two MatMuls that act on each score on its own.
ELEMENTWISE = {"Mul", "Div", "Cast"}


def scalar_inputs(graph):
    """Names of scalar initializers and Constant outputs."""
    names = {t.name for t in graph.initializer if int(np.prod(t.dims)) == 1}
    for node in graph.node:
        if node.op_type == "Constant" and node.attribute:
            value = helper.get_attribute_value(node.attribute[0])
            if isinstance(value, onnx.TensorProto) and int(np.prod(value.dims)) == 1:
                names.add(node.output[0])
    return names


def softmax_axis_is_last(node, opset):
    axes = [helper.get_attribute_value(a) for a in node.attribute if a.name == "axis"]
    # Softmax before opset 13 flattens from `axis` (default 1) on, which is
    # per row only for the last axis; from 13 on its default is -1.
    if not axes:
        return opset >= 13
    return axes[0] == -1


def find_attentions(graph, opset):
    producer = {output: node for node in graph.node for output in node.output}
    consumers = {}
    for node in graph.node:
        for name in node.input:
            consumers.setdefault(name, []).append(node)
    scalars = scalar_inputs(graph)

    def only_consumer(name):
        found = consumers.get(name, [])
        return found[0] if len(found) == 1 else None

    def elementwise_input(node):
        """The non-scalar input of an elementwise node, if it has one."""
        if node.op_type == "Cast":
            return node.input[0]
        others = [name for name in node.input if name not in scalars]
        if len(others) != 1 or (node.op_type == "Div" and others[0] != node.input[0]):
            return None
        return others[0]

    attentions = []
    outputs = {o.name for o in graph.output}
    for softmax in (n for n in graph.node if n.op_type == "Softmax"):
        if not softmax_axis_is_last(softmax, opset):
            continue
        chain = [softmax]
        name = softmax.input[0]
        while True:
            node = producer.get(name)
            if node is None:
                break
            if node.op_type == "MatMul":
                first = node
                break
            if node.op_type not in ELEMENTWISE or elementwise_input(node) is None:
                node = None
                break
            chain.insert(0, node)
            name = elementwise_input(node)
        else:
            continue
        if node is None or first.output[0] in outputs:
            continue
        chain.insert(0, first)
        name = softmax.output[0]
        last = None
        while True:
            node = only_consumer(name)
            if node is None or name in outputs:
                break
            if node.op_type == "MatMul" and node.input[0] == name:
                last = node
                break
            if node.op_type not in ELEMENTWISE or elementwise_input(node) != name:
                break
            chain.append(node)
            name = node.output[0]
        if last is None:
            continue
        chain.append(last)
        if any(only_consumer(n.output[0]) is None for n in chain[:-1]):
            continue
        attentions.append(chain)
    return attentions


def rewrite(model, mode, blocks, heads=None):
    """Splits every attention into `blocks`: of query rows (`mode` "rows"),
    or groups of its `heads` heads (`mode` "heads")."""
    graph = model.graph
    opset = next(o.version for o in model.opset_import if o.domain in ("", "ai.onnx"))
    attentions = find_attentions(graph, opset)
    replaced = {id(node) for chain in attentions for node in chain}
    new_nodes = {}
    constant = lambda name, values: numpy_helper.from_array(np.array(values, np.int64), name)
    axis = -2 if mode == "rows" else 1
    graph.initializer.extend(
        [constant("attention_blocks/axis", [axis]), constant("attention_blocks/zero", [0])]
    )
    if mode == "rows":
        graph.initializer.extend(
            [constant("attention_blocks/count", blocks)]
            + [constant(f"attention_blocks/index{b}", b) for b in range(1, blocks)]
        )
    else:
        graph.initializer.extend(
            constant(f"attention_blocks/bound{b}", [heads * b // blocks])
            for b in range(1, blocks + 1)
        )
    for index, chain in enumerate(attentions):
        prefix = f"attention_blocks/{index}"
        first, last = chain[0], chain[-1]
        query = first.input[0]
        nodes = []
        if mode == "rows":
            # Bounds from the row count at run time.
            nodes += [
                helper.make_node("Shape", [query], [f"{prefix}/shape"]),
                helper.make_node(
                    "Gather",
                    [f"{prefix}/shape", "attention_blocks/axis"],
                    [f"{prefix}/rows"],
                    axis=0,
                ),
            ]
            bounds = ["attention_blocks/zero"]
            for b in range(1, blocks):
                nodes += [
                    helper.make_node(
                        "Mul",
                        [f"{prefix}/rows", f"attention_blocks/index{b}"],
                        [f"{prefix}/scaled{b}"],
                    ),
                    helper.make_node(
                        "Div",
                        [f"{prefix}/scaled{b}", "attention_blocks/count"],
                        [f"{prefix}/bound{b}"],
                    ),
                ]
                bounds.append(f"{prefix}/bound{b}")
            bounds.append(f"{prefix}/rows")
            # Only the query is cut; every block sees all keys and values.
            sliced = [query]
        else:
            # Constant bounds, whole heads each, so shape inference keeps every
            # dimension it knew. Keys and values are cut by head as well.
            bounds = ["attention_blocks/zero"] + [
                f"attention_blocks/bound{b}" for b in range(1, blocks + 1)
            ]
            sliced = [query, first.input[1], last.input[1]]
        block_outputs = []
        for b in range(blocks):
            rename = {}
            for name in sliced:
                rename[name] = f"{prefix}/{name}/slice{b}"
                nodes.append(
                    helper.make_node(
                        "Slice",
                        [name, bounds[b], bounds[b + 1], "attention_blocks/axis"],
                        [rename[name]],
                    )
                )
            for node in chain:
                copy = onnx.NodeProto()
                copy.CopyFrom(node)
                copy.name = f"{prefix}/{node.name or node.op_type}/{b}"
                for position, name in enumerate(copy.input):
                    copy.input[position] = rename.get(name, name)
                for position, name in enumerate(copy.output):
                    rename[name] = f"{prefix}/{name}/{b}"
                    copy.output[position] = rename[name]
                nodes.append(copy)
            block_outputs.append(rename[last.output[0]])
        nodes.append(helper.make_node("Concat", block_outputs, [last.output[0]], axis=axis))
        new_nodes[id(last)] = nodes
    ordered = []
    for node in graph.node:
        if id(node) in new_nodes:
            ordered.extend(new_nodes[id(node)])
        elif id(node) not in replaced:
            ordered.append(node)
    del graph.node[:]
    graph.node.extend(ordered)
    return len(attentions)


def main():
    source, output, mode, blocks = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
    assert mode in ("rows", "heads"), "MODE is rows or heads"
    heads = int(sys.argv[5]) if mode == "heads" else None
    model = onnx.load(source)
    count = rewrite(model, mode, blocks, heads)
    assert count, "no attention found"
    print(f"{count} attentions in {blocks} blocks of {mode} each")
    try:
        model = SymbolicShapeInference.infer_shapes(model, auto_merge=True)
    except Exception as error:  # noqa: BLE001 - the tool raises bare assertions
        # MoGe-2's exports reshape to shapes of data-dependent rank, which the
        # tool cannot follow; their attention shapes were unknown before too.
        print(f"symbolic shape inference failed, output not annotated: {error!r}")
    onnx.save(model, output, save_as_external_data=model.ByteSize() > 2_000_000_000)
    onnx.checker.check_model(output)


if __name__ == "__main__":
    main()
