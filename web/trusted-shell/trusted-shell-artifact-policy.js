(function trustedShellArtifactPolicyModule(global) {
  "use strict";

  const MEBIBYTE = 1024 * 1024;
  const DEFAULT_MAX_ARTIFACT_HTML_BYTES = 8 * MEBIBYTE;
  const DEFAULT_MAX_MATERIALIZED_HTML_BYTES = 16 * MEBIBYTE;
  const HARD_MAX_ARTIFACT_HTML_BYTES = 96 * MEBIBYTE;
  const HARD_MAX_MATERIALIZED_HTML_BYTES = 100 * MEBIBYTE;
  const HASH = /^[0-9a-f]{64}$/;
  const NO_VALUE = null;
  const normalizedPolicies = new WeakSet();

  function utf8ByteLength(value) {
    if (typeof value !== "string") return NO_VALUE;
    let length = 0;
    for (let index = 0; index < value.length; index += 1) {
      const code = value.charCodeAt(index);
      if (code <= 0x7f) length += 1;
      else if (code <= 0x7ff) length += 2;
      else if (code >= 0xd800 && code <= 0xdbff &&
          index + 1 < value.length) {
        const next = value.charCodeAt(index + 1);
        if (next >= 0xdc00 && next <= 0xdfff) {
          length += 4;
          index += 1;
        } else {
          length += 3;
        }
      } else {
        length += 3;
      }
    }
    return length;
  }

  async function digestText(environment, value) {
    if (!environment.crypto || !environment.crypto.subtle ||
        typeof environment.TextEncoder !== "function") {
      throw new Error("SHA-256 is unavailable");
    }
    const bytes = new environment.TextEncoder().encode(value);
    const digest = await environment.crypto.subtle.digest("SHA-256", bytes);
    return Array.from(new Uint8Array(digest))
      .map((byte) => byte.toString(16).padStart(2, "0")).join("");
  }

  function snapshotDataFields(value, requiredFields, optionalFields = []) {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      return NO_VALUE;
    }
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) return NO_VALUE;
    const descriptors = Object.getOwnPropertyDescriptors(value);
    const keys = Reflect.ownKeys(descriptors);
    if (keys.some((field) => typeof field !== "string")) return NO_VALUE;
    const actual = keys.sort();
    const required = requiredFields.slice().sort();
    const allowed = new Set(required.concat(optionalFields));
    if (!required.every((field) => actual.includes(field)) ||
        !actual.every((field) => allowed.has(field))) {
      return NO_VALUE;
    }
    const snapshot = {};
    for (const field of actual) {
      const descriptor = descriptors[field];
      if (!("value" in descriptor) || !descriptor.enumerable) return NO_VALUE;
      snapshot[field] = descriptor.value;
    }
    return Object.freeze(snapshot);
  }

  function snapshotArrayData(value, maximumLength) {
    if (!Array.isArray(value) || !Number.isSafeInteger(maximumLength) ||
        maximumLength < 0) return NO_VALUE;
    const descriptors = Object.getOwnPropertyDescriptors(value);
    const keys = Reflect.ownKeys(descriptors);
    const length = descriptors.length;
    if (keys.some((field) => typeof field !== "string") ||
        !length || !("value" in length) || !Number.isSafeInteger(length.value) ||
        length.value < 0 || length.value > maximumLength) {
      return NO_VALUE;
    }
    const expected = Array.from({ length: length.value }, (_, index) =>
      String(index));
    if (keys.length !== expected.length + 1 ||
        !keys.includes("length") || !expected.every((field) => {
          const descriptor = descriptors[field];
          return descriptor && "value" in descriptor && descriptor.enumerable;
        })) return NO_VALUE;
    return Object.freeze(expected.map((field) => descriptors[field].value));
  }

  function validPositiveInteger(value, maximum) {
    return Number.isSafeInteger(value) && value > 0 && value <= maximum;
  }

  function validDTag(value) {
    return typeof value === "string" && value.length > 0 &&
      utf8ByteLength(value) <= 256 && !/[\u0000-\u001f\u007f]/.test(value);
  }

  function validHash(value) {
    return typeof value === "string" && HASH.test(value);
  }

  function freezePolicy(value) {
    const frozen = Object.freeze(value);
    normalizedPolicies.add(frozen);
    return frozen;
  }

  const DEFAULT_POLICY = freezePolicy({
    elevated: false,
    maximumArtifactHTMLBytes: DEFAULT_MAX_ARTIFACT_HTML_BYTES,
    maximumMaterializedHTMLBytes: DEFAULT_MAX_MATERIALIZED_HTML_BYTES,
    exactArtifactHTMLBytes: null,
    exactMaterializedHTMLBytes: null,
    artifactDigest: null,
    materializedDigest: null,
    manifestAuthor: null,
    dTag: null,
    aggregateHash: null,
    exclusive: false
  });

  const ELEVATED_FIELDS = Object.freeze([
    "aggregateHash", "artifactDigest", "dTag", "exactArtifactHTMLBytes",
    "exactMaterializedHTMLBytes", "exclusive", "manifestAuthor",
    "materializedDigest", "maximumArtifactHTMLBytes",
    "maximumMaterializedHTMLBytes"
  ]);

  function normalizeArtifactPolicy(input) {
    if (typeof input === "undefined") return DEFAULT_POLICY;
    const snapshot = snapshotDataFields(input, ELEVATED_FIELDS);
    if (!snapshot || snapshot.exclusive !== true ||
        !validPositiveInteger(
          snapshot.maximumArtifactHTMLBytes,
          HARD_MAX_ARTIFACT_HTML_BYTES
        ) || snapshot.maximumArtifactHTMLBytes <= DEFAULT_MAX_ARTIFACT_HTML_BYTES ||
        !validPositiveInteger(
          snapshot.maximumMaterializedHTMLBytes,
          HARD_MAX_MATERIALIZED_HTML_BYTES
        ) || !validPositiveInteger(
          snapshot.exactArtifactHTMLBytes,
          snapshot.maximumArtifactHTMLBytes
        ) || snapshot.exactArtifactHTMLBytes <= DEFAULT_MAX_ARTIFACT_HTML_BYTES ||
        !validPositiveInteger(
          snapshot.exactMaterializedHTMLBytes,
          snapshot.maximumMaterializedHTMLBytes
        ) || !validHash(snapshot.artifactDigest) ||
        !validHash(snapshot.materializedDigest) ||
        !validHash(snapshot.manifestAuthor) ||
        !validHash(snapshot.aggregateHash) || !validDTag(snapshot.dTag)) {
      throw new TypeError("invalid trusted artifact policy");
    }
    return freezePolicy({ elevated: true, ...snapshot });
  }

  function isNormalizedPolicy(value) {
    return normalizedPolicies.has(value);
  }

  function constructorInput(policy) {
    if (!isNormalizedPolicy(policy)) {
      throw new TypeError("trusted artifact policy must be normalized");
    }
    if (!policy.elevated) return NO_VALUE;
    const input = {};
    for (const field of ELEVATED_FIELDS) input[field] = policy[field];
    return Object.freeze(input);
  }

  function matchesBinding(policy, binding) {
    if (!isNormalizedPolicy(policy)) return false;
    return !policy.elevated || Boolean(binding &&
      binding.artifactDigest === policy.artifactDigest &&
      binding.manifestAuthor === policy.manifestAuthor &&
      binding.dTag === policy.dTag &&
      binding.aggregateHash === policy.aggregateHash);
  }

  function acceptsArtifactHTML(policy, value) {
    if (!isNormalizedPolicy(policy)) return false;
    const length = utf8ByteLength(value);
    return length !== null && (policy.elevated
      ? length === policy.exactArtifactHTMLBytes
      : length <= policy.maximumArtifactHTMLBytes);
  }

  function acceptsMaterializedHTMLBytes(policy, value) {
    if (!isNormalizedPolicy(policy)) return false;
    const length = utf8ByteLength(value);
    if (length === null) return false;
    return policy.elevated
      ? length === policy.exactMaterializedHTMLBytes
      : length <= policy.maximumMaterializedHTMLBytes;
  }

  function acceptsMaterializedHTML(policy, value, digest) {
    return acceptsMaterializedHTMLBytes(policy, value) &&
      (!policy.elevated || digest === policy.materializedDigest);
  }

  function createAdmission(policy) {
    if (!isNormalizedPolicy(policy)) {
      throw new TypeError("trusted artifact policy must be normalized");
    }
    let consumed = false;
    let pending = null;
    let active = null;
    let disposed = false;

    function begin() {
      if (!policy.elevated) return NO_VALUE;
      if (disposed || consumed || pending || active) return false;
      consumed = true;
      pending = Object.freeze({});
      return pending;
    }

    function activate(token) {
      if (!policy.elevated) return true;
      if (disposed || pending !== token || active) return false;
      active = token;
      return true;
    }

    function settle(token) {
      if (pending === token) pending = null;
    }

    function release(token) {
      if (active === token) active = null;
    }

    function dispose() {
      disposed = true;
      active = null;
    }

    function counts() {
      const reserved = Boolean(pending || active);
      return Object.freeze({
        elevatedPending: pending ? 1 : 0,
        elevatedActive: active ? 1 : 0,
        reservedArtifactHTMLBytes: reserved
          ? policy.exactArtifactHTMLBytes
          : 0,
        reservedMaterializedHTMLBytes: reserved
          ? policy.exactMaterializedHTMLBytes
          : 0
      });
    }

    return Object.freeze({ activate, begin, counts, dispose, release, settle });
  }

  const exported = Object.freeze({
    DEFAULT_MAX_ARTIFACT_HTML_BYTES,
    DEFAULT_MAX_MATERIALIZED_HTML_BYTES,
    HARD_MAX_ARTIFACT_HTML_BYTES,
    HARD_MAX_MATERIALIZED_HTML_BYTES,
    acceptsArtifactHTML,
    acceptsMaterializedHTML,
    acceptsMaterializedHTMLBytes,
    constructorInput,
    createAdmission,
    digestText,
    isNormalizedPolicy,
    matchesBinding,
    normalizeArtifactPolicy,
    snapshotArrayData,
    snapshotDataFields,
    utf8ByteLength
  });
  global.NMPTrustedShellArtifactPolicy = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
