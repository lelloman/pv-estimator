# Tight v1 INT8 Source Model Comparison

Date: 2026-06-05

This compares the previous default INT8 ONNX source-model bundle against the tight-v1 INT8 ONNX bundle using the same `pv-cli estimate` runtime path on the 120-location regional benchmark.

## Bundles

| Bundle | Artifact directory |
| --- | --- |
| Old INT8 | `artifacts/source-models-768x8-int8` before replacement |
| Tight v1 INT8 | `artifacts/source-models-768x8-tight-v1-int8` |

After this comparison, `artifacts/source-models-768x8-int8` was replaced with the tight-v1 files so the embedded CLI/TUI default path uses the improved bundle.

## Accuracy

| Reference | Count | Old INT8 MBE | Old INT8 MAE | Old INT8 RMSE | Tight v1 INT8 MBE | Tight v1 INT8 MAE | Tight v1 INT8 RMSE | MAE change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| PVGIS-ERA5 | 120 | -0.628% | 2.942% | 4.033% | -0.442% | 2.470% | 3.201% | -0.472 pp |
| PVGIS-SARAH3 | 100 | 2.109% | 3.664% | 4.931% | 2.374% | 3.218% | 4.418% | -0.447 pp |

## Distribution

| Reference | Old p90 abs error | Tight v1 p90 abs error | Old max abs error | Tight v1 max abs error |
| --- | ---: | ---: | ---: | ---: |
| PVGIS-ERA5 | 6.731% | 4.892% | 14.920% | 11.623% |
| PVGIS-SARAH3 | 8.018% | 6.469% | 22.215% | 19.456% |

Mean source disagreement spread improved from `6.952%` to `4.785%`.

## Artifact Hashes

| Artifact | SHA-256 |
| --- | --- |
| `source-model-artifacts.json` | `272d1ea72a6f4714b58eaca94abb1d4bd4c071220fca6f60e7153a071cbcaad8` |
| `nasa_power.onnx` | `870c7e621c9ec7d589eb242eeb9b17bbc28a9861216f8261346404301a854403` |
| `pvgis_era5.onnx` | `29eeeed5639dceb39103459f7c58358acf69959504cd13c6bb32409dcccf2afa` |
| `pvgis_sarah3.onnx` | `c75ae7aa4141f1449ac23b8f792a7ab8ad3435db66bd4024bd021f45f1c3c00f` |
| `coverage/pvgis_sarah3_empirical_grid_mask.json` | `05fa9fbeb471b649f9a1443c36d8ab04c27896722519b04e0fb11cfb80ccaa03` |

## Verdict

Tight v1 is the better default bundle. It improves MAE and RMSE against both PVGIS references, reduces p90 and max absolute error against both references, and reduces source disagreement.
