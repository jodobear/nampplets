(function trustedShellEmbeddingContract(global) {
  "use strict";

  const MAX_REQUEST_ID_BYTES = 128;
  const MAX_SURFACE_ID_BYTES = 128;
  const MAX_SESSION_ID_BYTES = 256;
  const MAX_D_TAG_BYTES = 256;
  const MAX_TITLE_BYTES = 1024;
  const MAX_DOMAINS = 64;
  const HASH = /^[0-9a-f]{64}$/;

  function exactFields(value, fields, primitives) {
    if (!primitives.isPlainObject(value)) return false;
    const expected = fields.slice().sort();
    const actual = Object.keys(value).sort();
    return actual.length === expected.length &&
      actual.every((field, index) => field === expected[index]);
  }

  function validText(value, maximumBytes, allowEmpty = false) {
    return typeof value === "string" &&
      (allowEmpty || value.length > 0) &&
      new TextEncoder().encode(value).byteLength <= maximumBytes &&
      !/[\u0000-\u001f\u007f]/.test(value);
  }

  function createContract(primitives, hostModule) {
    function validRequestId(value) {
      return validText(value, MAX_REQUEST_ID_BYTES);
    }

    function validSurfaceId(value) {
      return validText(value, MAX_SURFACE_ID_BYTES);
    }

    function validSession(value) {
      return validText(value, MAX_SESSION_ID_BYTES);
    }

    function validBinding(value, surfaceId, session) {
      return exactFields(value, [
        "aggregateHash", "artifactDigest", "dTag", "manifestAuthor",
        "session", "surface"
      ], primitives) &&
        HASH.test(value.manifestAuthor) &&
        validText(value.dTag, MAX_D_TAG_BYTES) &&
        HASH.test(value.aggregateHash) &&
        HASH.test(value.artifactDigest) &&
        value.surface === surfaceId &&
        value.session === session;
    }

    function validMount(request) {
      if (!exactFields(request, [
        "configuration", "requestId", "surfaceId", "type"
      ], primitives) ||
          !validRequestId(request.requestId) ||
          !primitives.isPlainObject(request.configuration)) return false;
      const configuration = request.configuration;
      return exactFields(configuration, [
        "artifactBaseURL", "artifactHTML", "binding", "domains", "session", "title"
      ], primitives) &&
        validSurfaceId(request.surfaceId) &&
        validSession(configuration.session) &&
        typeof configuration.artifactHTML === "string" &&
        new TextEncoder().encode(configuration.artifactHTML).byteLength <=
          hostModule.MAX_ARTIFACT_HTML_BYTES &&
        primitives.isVerifiedArtifactBaseURL(configuration.artifactBaseURL) &&
        Array.isArray(configuration.domains) &&
        configuration.domains.length <= MAX_DOMAINS &&
        configuration.domains.every((domain) =>
          validText(domain, 64) && /^[a-z][a-z0-9-]*$/.test(domain)
        ) &&
        validText(configuration.title, MAX_TITLE_BYTES, true) &&
        validBinding(configuration.binding, request.surfaceId, configuration.session);
    }

    return Object.freeze({
      exactFields,
      validMount,
      validRequestId,
      validSession,
      validSurfaceId
    });
  }

  const exported = Object.freeze({ createContract });
  global.NMPTrustedShellEmbeddingContract = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
