#!/usr/bin/env python3
"""Check promotion gates for geography-aware source-model candidates."""

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path

REFERENCE_PREFIXES = ("pvgis_era5", "pvgis_sarah3")
EUROPE_MEDITERRANEAN_REGIONS = {
    "iberia",
    "france",
    "central europe",
    "balkans greece turkey",
    "north africa middle east",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--regional-summary", type=Path, required=True)
    parser.add_argument("--regional-csv", type=Path, required=True)
    parser.add_argument("--baseline-regional-csv", type=Path, default=None)
    parser.add_argument("--max-era5-mae-pct", type=float, default=3.19)
    parser.add_argument("--max-sarah3-mae-pct", type=float, default=3.93)
    parser.add_argument("--max-region-regression-pct", type=float, default=0.50)
    parser.add_argument("--min-eu-med-improvement-pct", type=float, default=0.25)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    failures: list[str] = []
    summary = json.loads(args.regional_summary.read_text(encoding="utf-8"))
    overall_limits = {
        "pvgis_era5": args.max_era5_mae_pct,
        "pvgis_sarah3": args.max_sarah3_mae_pct,
    }
    for prefix, limit in overall_limits.items():
        key = f"{prefix}_reference"
        actual = summary.get(key, {}).get("energy_mae_pct")
        if actual is None:
            failures.append(f"missing {key}.energy_mae_pct")
        elif float(actual) > limit:
            failures.append(f"{key}.energy_mae_pct {actual:.3f}% exceeds {limit:.3f}%")

    candidate_regions = region_mae(args.regional_csv)
    baseline_regions = region_mae(args.baseline_regional_csv) if args.baseline_regional_csv else {}
    if baseline_regions:
        failures.extend(check_region_regressions(args, candidate_regions, baseline_regions))
        failures.extend(check_eu_med_improvement(args, candidate_regions, baseline_regions))

    print_report(summary, candidate_regions, baseline_regions)
    if failures:
        print("gate=fail")
        for failure in failures:
            print(f"failure: {failure}")
        return 1
    print("gate=pass")
    return 0


def region_mae(path: Path | None) -> dict[str, dict[str, float]]:
    if path is None:
        return {}
    rows_by_region: dict[str, dict[str, list[float]]] = {}
    with path.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            region = normalize_region(row["region"])
            bucket = rows_by_region.setdefault(region, {prefix: [] for prefix in REFERENCE_PREFIXES})
            for prefix in REFERENCE_PREFIXES:
                raw = row.get(f"{prefix}_reference_energy_error_pct", "")
                if raw not in ("", None):
                    bucket[prefix].append(abs(float(raw)))
    return {
        region: {
            prefix: sum(values) / len(values)
            for prefix, values in refs.items()
            if values
        }
        for region, refs in rows_by_region.items()
    }


def normalize_region(region: str) -> str:
    return " ".join(region.strip().lower().replace("_", " ").split())


def check_region_regressions(
    args: argparse.Namespace,
    candidate: dict[str, dict[str, float]],
    baseline: dict[str, dict[str, float]],
) -> list[str]:
    failures = []
    for region, baseline_refs in baseline.items():
        for prefix, baseline_mae in baseline_refs.items():
            candidate_mae = candidate.get(region, {}).get(prefix)
            if candidate_mae is None:
                continue
            regression = candidate_mae - baseline_mae
            if regression > args.max_region_regression_pct:
                failures.append(
                    f"{region} {prefix} MAE regressed by {regression:.3f} pp "
                    f"({baseline_mae:.3f}% -> {candidate_mae:.3f}%)"
                )
    return failures


def check_eu_med_improvement(
    args: argparse.Namespace,
    candidate: dict[str, dict[str, float]],
    baseline: dict[str, dict[str, float]],
) -> list[str]:
    best_improvement = 0.0
    best_label = ""
    for region in EUROPE_MEDITERRANEAN_REGIONS:
        for prefix in REFERENCE_PREFIXES:
            baseline_mae = baseline.get(region, {}).get(prefix)
            candidate_mae = candidate.get(region, {}).get(prefix)
            if baseline_mae is None or candidate_mae is None:
                continue
            improvement = baseline_mae - candidate_mae
            if improvement > best_improvement or not best_label:
                best_improvement = improvement
                best_label = f"{region} {prefix}"
    if best_improvement + 1e-12 < args.min_eu_med_improvement_pct:
        label = best_label or "no comparable Europe/Mediterranean region"
        return [
            f"best Europe/Mediterranean improvement is {best_improvement:.3f} pp "
            f"at {label}; required {args.min_eu_med_improvement_pct:.3f} pp"
        ]
    return []


def print_report(
    summary: dict,
    candidate_regions: dict[str, dict[str, float]],
    baseline_regions: dict[str, dict[str, float]],
) -> None:
    for prefix in REFERENCE_PREFIXES:
        key = f"{prefix}_reference"
        value = summary.get(key)
        if value:
            print(f"{key}: n={value['count']} mae={value['energy_mae_pct']:.3f}%")
    for region, refs in sorted(candidate_regions.items()):
        parts = []
        for prefix in REFERENCE_PREFIXES:
            if prefix not in refs:
                continue
            part = f"{prefix}={refs[prefix]:.3f}%"
            baseline = baseline_regions.get(region, {}).get(prefix)
            if baseline is not None:
                part += f" ({refs[prefix] - baseline:+.3f} pp)"
            parts.append(part)
        if parts:
            print(f"region {region}: {', '.join(parts)}")


if __name__ == "__main__":
    raise SystemExit(main())
