# Source Model Reproduction

This directory is the public reproduction package for the source-model bundle shipped with PV Estimator v0.1.0.

It is intentionally not an experiment archive. The old exploratory scripts, architecture sweeps, geography probes, NSRDB scaffolding, and intermediate result notes were removed. The retained files are only for rebuilding, exporting, or validating the shipped tight-v1 source models.

Normal users do not need this directory. The CLI and TUI use the embedded INT8 ONNX bundle by default, and the same bundle is published at:

```text
https://huggingface.co/lelloman/pv-estimator-tight-v1-int8
```

Start with `RUNBOOK.md` for the full reproduction path.

## Layout

```text
config/     small location lists, source registry, and SARAH3 mask
scripts/    data download, normalization, training, validation, and ONNX export
results/    retained reports for the shipped bundle
runs/       ignored generated outputs
```

## What Is Not Committed

Generated hourly source data, climate-normal `.npy` arrays, PyTorch checkpoints, exported ONNX artifacts, and validation outputs belong under `runs/` or in external artifact storage.
