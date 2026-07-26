#!/usr/bin/env python3
"""Validate bounded, canonical Nampplets performance evidence v1."""
from __future__ import annotations
from collections import Counter
import argparse, hashlib, json, re, sys
from pathlib import Path
from typing import Any
RESULT_SCHEMA_ID, COMPARISON_SCHEMA_ID, BUDGET_SCHEMA_ID = (
    "urn:nampplets:performance:result:v1", "urn:nampplets:performance:comparison:v1", "urn:nampplets:performance:budget:v1")
SCHEMA_DIR = Path(__file__).resolve().parents[2] / "performance" / "schema"
SCHEMA_FILES = {RESULT_SCHEMA_ID: "result-v1.schema.json", COMPARISON_SCHEMA_ID: "comparison-v1.schema.json", BUDGET_SCHEMA_ID: "budget-v1.schema.json"}
MAX_INPUT_BYTES, MAX_SAMPLES, MAX_WARMUPS = 16 * 1024 * 1024, 10_000, 1_000
MAX_SAMPLE_DEADLINE_NS, MAX_RUN_DEADLINE_NS = 5 * 60 * 1_000_000_000, 2 * 60 * 60 * 1_000_000_000
OUTCOMES = ("success", "refused", "failed", "deadline_exceeded")
MISMATCH_FIELDS = (
    ("benchmark_id", "benchmark_mismatch"), ("state", "state_mismatch"), ("reset_scopes", "reset_scope_mismatch"),
    ("fixture", "fixture_mismatch"), ("protocol", "protocol_mismatch"), ("build_mode", "build_mode_mismatch"),
    ("toolchain", "toolchain_mismatch"), ("environment_class", "environment_class_mismatch"),
    ("os", "os_mismatch"), ("hardware", "hardware_mismatch"), ("power_state", "power_state_mismatch"),
    ("thermal_state", "thermal_state_mismatch"), ("measurement_availability", "measurement_availability_mismatch"))
class EvidenceError(Exception):
    def __init__(self, code: str, detail: str, path: str = "$") -> None:
        super().__init__(detail)
        self.code, self.detail, self.path = code, detail[:512], path
class SchemaError(Exception): pass
def canonical_json(value: Any) -> bytes:
    options = {"ensure_ascii": False, "sort_keys": True, "separators": (",", ":"),
               "allow_nan": False}
    return json.dumps(value, **options).encode("utf-8")
def compute_artifact_checksum(value: dict[str, Any]) -> str:
    unsigned = {key: item for key, item in value.items() if key != "checksum_sha256"}
    return hashlib.sha256(canonical_json(unsigned)).hexdigest()
def compute_comparison_key(identity: dict[str, Any]) -> str:
    return hashlib.sha256(canonical_json(identity)).hexdigest()
def _percentile(values: list[int], percent: int) -> int:
    return sorted(values)[((len(values) * percent + 99) // 100) - 1]
def _distribution(outcome: str, values: list[int]) -> dict[str, Any]:
    count, total = len(values), sum(values)
    numerator = count * sum(value * value for value in values) - total * total
    denominator = count * count
    return {"outcome": outcome, "count": count,
        "p50_ns": _percentile(values, 50), "p95_ns": _percentile(values, 95),
        "p99_ns": _percentile(values, 99), "max_ns": max(values),
        "population_variance_ns2": {"numerator": str(numerator),
                                    "denominator": str(denominator)}}
def summarize(samples: list[dict[str, Any]]) -> dict[str, Any]:
    counts = Counter(item["outcome"] for item in samples)
    groups = Counter(
        (item["refusal"]["domain"], item["refusal"]["code"])
        for item in samples
        if item["outcome"] == "refused"
    )
    successes = [item for item in samples if item["outcome"] == "success"]
    outliers: list[int] = []
    if successes:
        durations = [item["duration_ns"] for item in successes]
        q1, q3 = _percentile(durations, 25), _percentile(durations, 75)
        ceiling = q3 + 3 * (q3 - q1)
        outliers = [item["sequence"] for item in successes if item["duration_ns"] > ceiling]
    diagnostic = bool(counts["failed"] or counts["deadline_exceeded"] or outliers)
    return {
        "sample_count": len(samples),
        "outcome_counts": {outcome: counts[outcome] for outcome in OUTCOMES},
        "distributions": [_distribution(
            outcome, [item["duration_ns"] for item in samples
                      if item["outcome"] == outcome])
            for outcome in OUTCOMES if counts[outcome]],
        "refusal_groups": [{"domain": domain, "code": code, "count": count}
                           for (domain, code), count in sorted(groups.items())],
        "diagnostic_outlier_sequences": outliers,
        "disposition": "diagnostic" if diagnostic else "valid",
    }
def summarize_comparison(baseline: dict[str, Any], candidate: dict[str, Any],
                         confidence: dict[str, Any] | None = None) -> dict[str, Any]:
    mismatches = [
        code for field, code in MISMATCH_FIELDS if baseline.get(field) != candidate.get(field)
    ]
    disposition = "incomparable" if mismatches else "observed_only"
    if not mismatches and confidence and confidence.get("disposition") == "evaluated":
        disposition = "method_evaluated"
    return {"disposition": disposition, "mismatch_codes": mismatches}
def _schema(name: str) -> dict[str, Any]:
    return json.loads((SCHEMA_DIR / name).read_text(encoding="utf-8"))
def _resolve(reference: str, root: dict[str, Any]) -> tuple[Any, dict]:
    file_name, _, fragment = reference.partition("#")
    if file_name:
        root = _schema(file_name)
    target: Any = root
    if fragment:
        for token in fragment.removeprefix("/").split("/"):
            target = target[token.replace("~1", "/").replace("~0", "~")]
    return target, root
def _check(value: Any, rule: dict[str, Any], root: dict[str, Any]) -> None:
    if "$ref" in rule:
        target, target_root = _resolve(rule["$ref"], root)
        _check(value, target, target_root)
        return
    if "oneOf" in rule:
        matches = 0
        for option in rule["oneOf"]:
            try:
                _check(value, option, root)
                matches += 1
            except SchemaError:
                pass
        if matches != 1:
            raise SchemaError("must match exactly one allowed shape")
        return
    kind = rule.get("type")
    valid_type = {"object": isinstance(value, dict), "array": isinstance(value, list),
                  "string": isinstance(value, str),
                  "integer": isinstance(value, int) and not isinstance(value, bool)
                  }.get(kind, True)
    if not valid_type:
        raise SchemaError(f"must be {kind}")
    if "const" in rule and value != rule["const"]:
        raise SchemaError("does not match constant")
    if "enum" in rule and value not in rule["enum"]:
        raise SchemaError("is not an allowed value")
    if kind == "object":
        required, properties = rule.get("required", []), rule.get("properties", {})
        missing = [field for field in required if field not in value]
        extras = [field for field in value if field not in properties]
        if missing:
            raise SchemaError(f"missing required members: {missing}")
        if rule.get("additionalProperties") is False and extras:
            raise SchemaError(f"unknown members: {extras}")
        for field, item in value.items():
            if field in properties:
                _check(item, properties[field], root)
    elif kind == "array":
        if not rule.get("minItems", 0) <= len(value) <= rule.get("maxItems", sys.maxsize):
            raise SchemaError("array length is outside bounds")
        if rule.get("uniqueItems") and len({canonical_json(item) for item in value}) != len(value):
            raise SchemaError("array members must be unique")
        for item in value:
            _check(item, rule.get("items", {}), root)
    elif kind == "string":
        if not rule.get("minLength", 0) <= len(value) <= rule.get("maxLength", sys.maxsize):
            raise SchemaError("string length is outside bounds")
        if "pattern" in rule and re.fullmatch(rule["pattern"], value) is None:
            raise SchemaError("string does not match required pattern")
        if rule.get("format") == "date-time" and re.fullmatch(
            r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)", value
        ) is None:
            raise SchemaError("string is not an RFC 3339 date-time")
    elif kind == "integer" and not (
        rule.get("minimum", value) <= value <= rule.get("maximum", value)):
        raise SchemaError("integer is outside bounds")
def _safety_checks(value: Any) -> None:
    if not isinstance(value, dict):
        return
    samples = value.get("samples")
    if isinstance(samples, list) and len(samples) > MAX_SAMPLES:
        raise EvidenceError("sample_limit_exceeded", "measured sample ceiling is 10000")
    identity = value.get("identity")
    protocol = identity.get("protocol") if isinstance(identity, dict) else None
    if not isinstance(protocol, dict):
        return
    bounds = (("warmup_count", MAX_WARMUPS, "warmup_limit_exceeded"),
              ("sample_count", MAX_SAMPLES, "sample_limit_exceeded"),
              ("per_sample_deadline_ns", MAX_SAMPLE_DEADLINE_NS,
               "sample_deadline_limit_exceeded"),
              ("run_deadline_ns", MAX_RUN_DEADLINE_NS,
               "run_deadline_limit_exceeded"))
    for field, limit, code in bounds:
        item = protocol.get(field)
        if isinstance(item, int) and not isinstance(item, bool) and item > limit:
            raise EvidenceError(code, f"{field} exceeds parser/work-runner safety ceiling")
def _validate_result(value: dict[str, Any]) -> dict[str, Any]:
    samples, identity = value["samples"], value["identity"]
    if len(samples) != identity["protocol"]["sample_count"]:
        raise EvidenceError("sample_count_mismatch", "declared and measured samples differ")
    if [item["sequence"] for item in samples] != list(range(len(samples))):
        raise EvidenceError("sample_sequence_mismatch", "sample sequence must be ordered from zero")
    for item in samples:
        if (item["outcome"] == "refused"
                and "timeout" in item["refusal"]["code"].lower()):
            raise EvidenceError("timeout_is_not_refusal", "timeout is deadline_exceeded, not refused")
    availability = identity["measurement_availability"]
    for metric, status in availability.items():
        present = [metric in item for item in samples]
        mismatched = status == "available" and not all(present)
        mismatched |= status == "unavailable" and any(present)
        if mismatched:
            raise EvidenceError("measurement_availability_mismatch",
                                f"{metric} availability disagrees with samples")
    computed_summary = summarize(samples)
    if value["producer_summary"] != computed_summary:
        raise EvidenceError("summary_mismatch", "producer summary is not authoritative recomputation")
    key = compute_comparison_key(identity)
    if value["comparison_key"] != key:
        raise EvidenceError("comparison_key_mismatch", "comparison key does not match identity")
    checksum = compute_artifact_checksum(value)
    if value["checksum_sha256"] != checksum:
        raise EvidenceError("checksum_mismatch", "checksum does not match canonical artifact")
    return {"producer_summary": computed_summary, "comparison_key": key, "checksum_sha256": checksum}
def _validate_comparison(value: dict[str, Any]) -> dict[str, Any]:
    for side in ("baseline", "candidate"):
        reference = value[side]
        if reference["comparison_key"] != compute_comparison_key(reference["identity"]):
            raise EvidenceError("comparison_key_mismatch", f"{side} comparison key is invalid")
    confidence, produced = value["confidence"], value["producer_summary"]
    confidence_disposition = confidence["disposition"]
    no_confidence = produced["disposition"] in {"observed_only", "incomparable"}
    wrong_confidence = no_confidence and confidence_disposition != "not_evaluated"
    wrong_confidence |= produced["disposition"] == "method_evaluated" and (
        confidence_disposition != "evaluated")
    if wrong_confidence:
        raise EvidenceError("confidence_disposition_invalid",
                            "disposition cannot imply unratified confidence")
    computed = summarize_comparison(value["baseline"]["identity"],
                                    value["candidate"]["identity"], confidence)
    if produced != computed:
        raise EvidenceError("comparison_summary_mismatch", "comparison disposition is not authoritative")
    reason = confidence.get("reason", {}).get("code")
    if computed["disposition"] == "incomparable" and reason != "incomparable_inputs":
        raise EvidenceError("confidence_reason_invalid", "incomparable evidence requires its reason")
    if computed["disposition"] == "observed_only" and reason == "incomparable_inputs":
        raise EvidenceError("confidence_reason_invalid", "observed evidence is comparable")
    checksum = compute_artifact_checksum(value)
    if value["checksum_sha256"] != checksum:
        raise EvidenceError("checksum_mismatch", "checksum does not match canonical artifact")
    return {"producer_summary": computed, "checksum_sha256": checksum}
def _validate_budget(value: dict[str, Any]) -> dict[str, Any]:
    has_ratification = "ratification" in value
    if (value["status"] in {"ratified", "retired"}) != has_ratification:
        raise EvidenceError("ratification_envelope_mismatch", "status and ratification must agree")
    checksum = compute_artifact_checksum(value)
    if value["checksum_sha256"] != checksum:
        raise EvidenceError("checksum_mismatch", "checksum does not match canonical artifact")
    return {"checksum_sha256": checksum}
def _decode(raw: bytes) -> Any:
    def members(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        value: dict[str, Any] = {}
        for key, item in pairs:
            if key in value:
                raise EvidenceError("duplicate_member", f"duplicate JSON member: {key}")
            value[key] = item
        return value
    def non_integer(_: str) -> None:
        raise EvidenceError("non_integer_number", "floating-point JSON numbers are prohibited")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise EvidenceError("invalid_utf8", str(error)) from error
    try:
        return json.loads(text, object_pairs_hook=members, parse_float=non_integer,
                          parse_constant=non_integer)
    except EvidenceError:
        raise
    except (json.JSONDecodeError, ValueError, RecursionError) as error:
        raise EvidenceError("invalid_json", str(error)) from error
def validate_bytes(raw: bytes) -> dict[str, Any]:
    try:
        if len(raw) > MAX_INPUT_BYTES:
            raise EvidenceError("input_too_large", "input artifact exceeds 16 MiB")
        value = _decode(raw)
        try:
            canonical = canonical_json(value)
        except (UnicodeEncodeError, RecursionError, ValueError) as error:
            raise EvidenceError("invalid_json", str(error)) from error
        if raw != canonical:
            raise EvidenceError("non_canonical_json", "artifact is not canonical JSON v1")
        if not isinstance(value, dict):
            raise EvidenceError("root_not_object", "artifact root must be an object")
        schema_id = value.get("schema_id")
        if schema_id not in SCHEMA_FILES:
            raise EvidenceError("unsupported_schema", "schema_id is not a supported v1 identifier")
        _safety_checks(value)
        schema = _schema(SCHEMA_FILES[schema_id])
        try:
            _check(value, schema, schema)
        except SchemaError as error:
            raise EvidenceError("schema_violation", str(error)) from error
        validators = {
            RESULT_SCHEMA_ID: ("result", _validate_result),
            COMPARISON_SCHEMA_ID: ("comparison", _validate_comparison),
            BUDGET_SCHEMA_ID: ("budget", _validate_budget),
        }
        kind, validator = validators[schema_id]
        return {"ok": True, "kind": kind, "computed": validator(value), "errors": []}
    except EvidenceError as error:
        item = {"code": error.code, "path": error.path, "detail": error.detail}
        return {"ok": False, "errors": [item]}
def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path); args = parser.parse_args()
    try:
        with args.artifact.open("rb") as handle:
            raw = handle.read(MAX_INPUT_BYTES + 1)
    except OSError as error:
        report = {"ok": False, "errors": [{"code": "input_io_error", "path": "$", "detail": str(error)[:512]}]}
    else: report = validate_bytes(raw)
    print(canonical_json(report).decode("utf-8"))
    return 0 if report["ok"] else 1
if __name__ == "__main__":
    raise SystemExit(main())
