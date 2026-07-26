#!/usr/bin/env python3
"""Deterministic contract tests for performance evidence v1."""

from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).with_name("validate_performance_evidence.py")
SPEC = importlib.util.spec_from_file_location("performance_validator", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


def sample(sequence: int, outcome: str, duration_ns: int) -> dict:
    value = {
        "sequence": sequence,
        "outcome": outcome,
        "duration_ns": duration_ns,
    }
    if outcome == "refused":
        value["refusal"] = {"domain": "runtime.queue", "code": "capacity"}
    elif outcome == "failed":
        value["failure"] = {"code": "provider_error", "detail": "fixture failure"}
    return value


def identity(state: str = "warm") -> dict:
    return {
        "benchmark_id": "runtime.session.launch",
        "state": state,
        "reset_scopes": ["runtime_store", "artifact_cache"],
        "fixture": {
            "id": "launch-v1",
            "sha256": "1" * 64,
            "cardinality": 4,
        },
        "protocol": {
            "warmup_count": 1,
            "sample_count": 4,
            "per_sample_deadline_ns": 5_000_000_000,
            "run_deadline_ns": 60_000_000_000,
            "outlier_policy": "tukey_upper_3_iqr_v1",
        },
        "build_mode": "release",
        "toolchain": "rustc 1.88.0",
        "environment_class": "macos-arm64-m4",
        "os": "macOS 26.0",
        "hardware": "MacBookPro M4 16GB",
        "power_state": "ac",
        "thermal_state": "nominal",
        "measurement_availability": {
            "cpu_time_ns": "available",
            "peak_rss_bytes": "available",
        },
    }


def result_artifact(state: str = "warm") -> dict:
    samples = [
        sample(0, "success", 10),
        sample(1, "success", 20),
        sample(2, "refused", 30),
        sample(3, "deadline_exceeded", 5_000_000_000),
    ]
    for item in samples:
        item["cpu_time_ns"] = max(1, item["duration_ns"] // 2)
        item["peak_rss_bytes"] = 4096 + item["sequence"]
    value = {
        "schema_id": VALIDATOR.RESULT_SCHEMA_ID,
        "run_id": "run-2026-07-26-001",
        "identity": identity(state),
        "build": {
            "source_revision": "2" * 40,
            "artifact_locator": "file:artifacts/result.json",
            "source_provenance": "git:https://github.com/pablof7z/nampplets",
        },
        "samples": samples,
    }
    value["producer_summary"] = VALIDATOR.summarize(samples)
    value["comparison_key"] = VALIDATOR.compute_comparison_key(value["identity"])
    value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
    return value


def reference(value: dict) -> dict:
    return {
        "artifact_locator": value["build"]["artifact_locator"],
        "checksum_sha256": value["checksum_sha256"],
        "comparison_key": value["comparison_key"],
        "identity": copy.deepcopy(value["identity"]),
    }


def comparison_artifact(baseline: dict, candidate: dict) -> dict:
    baseline_reference = reference(baseline)
    candidate_reference = reference(candidate)
    summary = VALIDATOR.summarize_comparison(
        baseline_reference["identity"], candidate_reference["identity"]
    )
    value = {
        "schema_id": VALIDATOR.COMPARISON_SCHEMA_ID,
        "comparison_id": "comparison-2026-07-26-001",
        "baseline": baseline_reference,
        "candidate": candidate_reference,
        "producer_summary": summary,
        "confidence": {
            "disposition": "not_evaluated",
            "reason": {
                "code": (
                    "incomparable_inputs"
                    if summary["disposition"] == "incomparable"
                    else "no_ratified_method"
                ),
                "detail": "No confidence method is ratified for this evidence.",
            },
        },
    }
    value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
    return value


def encoded(value: dict) -> bytes:
    return VALIDATOR.canonical_json(value)


class PerformanceEvidenceTests(unittest.TestCase):
    def assert_error(self, value: bytes | dict, code: str) -> None:
        raw = value if isinstance(value, bytes) else encoded(value)
        report = VALIDATOR.validate_bytes(raw)
        self.assertFalse(report["ok"], report)
        self.assertIn(code, [error["code"] for error in report["errors"]])

    def test_valid_result_fixture_is_accepted(self) -> None:
        report = VALIDATOR.validate_bytes(encoded(result_artifact()))
        self.assertTrue(report["ok"], report)
        self.assertEqual(report["kind"], "result")

    def test_percentiles_and_exact_population_variance_are_integer_only(self) -> None:
        summary = VALIDATOR.summarize(
            [sample(index, "success", duration) for index, duration in enumerate([1, 2, 3])]
        )
        distribution = summary["distributions"][0]
        self.assertEqual(
            (distribution["p50_ns"], distribution["p95_ns"], distribution["p99_ns"]),
            (2, 3, 3),
        )
        self.assertEqual(
            distribution["population_variance_ns2"],
            {"numerator": "6", "denominator": "9"},
        )
        singleton = VALIDATOR.summarize([sample(0, "success", 7)])
        self.assertEqual(
            singleton["distributions"][0]["population_variance_ns2"],
            {"numerator": "0", "denominator": "1"},
        )

    def test_producer_cannot_forge_summary(self) -> None:
        value = result_artifact()
        value["producer_summary"]["distributions"][0]["p95_ns"] = 19
        value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
        self.assert_error(value, "summary_mismatch")

    def test_checksum_and_comparison_key_are_recomputed(self) -> None:
        value = result_artifact()
        value["checksum_sha256"] = "0" * 64
        self.assert_error(value, "checksum_mismatch")
        value = result_artifact()
        value["comparison_key"] = "0" * 64
        value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
        self.assert_error(value, "comparison_key_mismatch")

    def test_canonical_json_and_duplicate_members_are_enforced(self) -> None:
        value = result_artifact()
        pretty = json.dumps(value, indent=2).encode()
        self.assert_error(pretty, "non_canonical_json")
        duplicate = b'{"schema_id":"x","schema_id":"y"}'
        self.assert_error(duplicate, "duplicate_member")

    def test_parser_and_runner_ceilings_have_typed_codes(self) -> None:
        self.assert_error(b" " * (VALIDATOR.MAX_INPUT_BYTES + 1), "input_too_large")
        cases = [
            ("warmup_count", VALIDATOR.MAX_WARMUPS + 1, "warmup_limit_exceeded"),
            ("sample_count", VALIDATOR.MAX_SAMPLES + 1, "sample_limit_exceeded"),
            (
                "per_sample_deadline_ns",
                VALIDATOR.MAX_SAMPLE_DEADLINE_NS + 1,
                "sample_deadline_limit_exceeded",
            ),
            (
                "run_deadline_ns",
                VALIDATOR.MAX_RUN_DEADLINE_NS + 1,
                "run_deadline_limit_exceeded",
            ),
        ]
        for field, limit, code in cases:
            with self.subTest(field=field):
                value = result_artifact()
                value["identity"]["protocol"][field] = limit
                value["comparison_key"] = VALIDATOR.compute_comparison_key(
                    value["identity"]
                )
                value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
                self.assert_error(value, code)

    def test_timeout_cannot_be_a_runtime_refusal(self) -> None:
        value = result_artifact()
        value["samples"][2]["refusal"]["code"] = "timeout"
        value["producer_summary"] = VALIDATOR.summarize(value["samples"])
        value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
        self.assert_error(value, "timeout_is_not_refusal")

    def test_outliers_are_diagnostic_and_never_removed(self) -> None:
        samples = [
            sample(0, "success", 1),
            sample(1, "success", 1),
            sample(2, "success", 1),
            sample(3, "success", 100),
        ]
        summary = VALIDATOR.summarize(samples)
        self.assertEqual(summary["diagnostic_outlier_sequences"], [3])
        self.assertEqual(summary["disposition"], "diagnostic")
        self.assertEqual(summary["outcome_counts"]["success"], 4)

    def test_cold_and_warm_results_are_incomparable(self) -> None:
        warm = result_artifact("warm")
        cold = result_artifact("cold")
        self.assertNotEqual(warm["comparison_key"], cold["comparison_key"])
        comparison = comparison_artifact(warm, cold)
        report = VALIDATOR.validate_bytes(encoded(comparison))
        self.assertTrue(report["ok"], report)
        self.assertEqual(
            report["computed"]["producer_summary"]["mismatch_codes"],
            ["state_mismatch"],
        )

    def test_observed_only_requires_not_evaluated_confidence(self) -> None:
        baseline = result_artifact()
        comparison = comparison_artifact(baseline, result_artifact())
        self.assertEqual(comparison["producer_summary"]["disposition"], "observed_only")
        comparison["confidence"] = {
            "disposition": "evaluated",
            "method": {
                "id": "bootstrap",
                "revision": "1",
                "ratification_locator": "https://example.invalid/decision",
            },
            "result": {"code": "inconclusive", "detail": "Not applicable."},
            "evidence_locator": "file:confidence.json",
        }
        comparison["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(comparison)
        self.assert_error(comparison, "confidence_disposition_invalid")

    def test_unknown_fields_and_non_integer_numbers_are_refused(self) -> None:
        value = result_artifact()
        value["unexpected"] = True
        value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
        self.assert_error(value, "schema_violation")
        raw = encoded(result_artifact()).replace(b'"duration_ns":10', b'"duration_ns":1.5')
        self.assert_error(raw, "non_integer_number")

    def test_budget_is_evidence_envelope_not_a_verdict(self) -> None:
        value = {
            "schema_id": VALIDATOR.BUDGET_SCHEMA_ID,
            "budget_id": "launch-p95-v1",
            "status": "unratified",
            "benchmark_id": "runtime.session.launch",
            "metric": {"name": "p95", "unit": "ns"},
            "threshold": {"operator": "less_than_or_equal", "value": 1000},
            "baseline": {
                "artifact_locator": "file:baseline.json",
                "checksum_sha256": "3" * 64,
            },
            "rationale": {
                "kind": "product",
                "detail": "Interactive launch target pending measured baseline.",
                "evidence_locator": "https://example.invalid/rationale",
            },
            "owner": "runtime-performance",
        }
        value["checksum_sha256"] = VALIDATOR.compute_artifact_checksum(value)
        report = VALIDATOR.validate_bytes(encoded(value))
        self.assertTrue(report["ok"], report)
        self.assertNotIn("verdict", report["computed"])


if __name__ == "__main__":
    unittest.main()
