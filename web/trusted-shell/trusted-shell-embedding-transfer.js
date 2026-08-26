(function trustedShellEmbeddingTransfer(global) {
  "use strict";

  const ARTIFACT_CHUNK_BYTES = 256 * 1024;
  const MAX_PENDING_TRANSFERS = 16;
  const artifactPolicySource = global.NMPTrustedShellArtifactPolicy ||
    (typeof require === "function"
      ? require("./trusted-shell-artifact-policy.js")
      : null);

  function createTransferManager(environment, dependencies) {
    const {
      artifactPolicy, beforeBegin, contract, onComplete, primitives, result
    } = dependencies;
    const ArrayBufferType = environment && (environment.ArrayBuffer || global.ArrayBuffer);
    const TextDecoderType = environment && (environment.TextDecoder || global.TextDecoder);
    const Uint8ArrayType = environment && (environment.Uint8Array || global.Uint8Array);
    if (!artifactPolicySource || !environment ||
        typeof TextDecoderType !== "function" ||
        typeof Uint8ArrayType !== "function" ||
        typeof ArrayBufferType !== "function" ||
        !artifactPolicySource.isNormalizedPolicy(artifactPolicy) ||
        typeof beforeBegin !== "function" || !contract ||
        typeof onComplete !== "function" ||
        !primitives || typeof result !== "function") {
      throw new TypeError("trusted artifact transfer dependencies are unavailable");
    }
    const transfers = new Map();
    let reservedBytes = 0;
    let disposed = false;

    function retire(surfaceId) {
      const transfer = transfers.get(surfaceId);
      if (!transfer) return false;
      transfers.delete(surfaceId);
      reservedBytes -= transfer.bytes.byteLength;
      return true;
    }

    function validIdentity(request) {
      return contract.validRequestId(request.requestId) &&
        contract.validRequestId(request.transferId) &&
        contract.validSurfaceId(request.surfaceId) &&
        contract.validSession(request.session);
    }

    function begin(request) {
      if (!contract.validMountBegin(request)) return;
      const configuration = request.configuration;
      beforeBegin(request.surfaceId);
      retire(request.surfaceId);
      if (disposed || transfers.size >= MAX_PENDING_TRANSFERS ||
          reservedBytes + configuration.artifactBytes >
            artifactPolicySource.HARD_MAX_ARTIFACT_HTML_BYTES) {
        result(request.type, request, false, "overloaded");
        return;
      }
      const domains = contract.snapshotDomains(configuration.domains);
      if (!domains) return;
      let bytes;
      try {
        bytes = new Uint8ArrayType(configuration.artifactBytes);
      } catch (_) {
        result(request.type, request, false, "overloaded");
        return;
      }
      transfers.set(request.surfaceId, {
        bytes,
        nextOffset: 0,
        transferId: request.requestId,
        configuration: Object.freeze({
          artifactBaseURL: configuration.artifactBaseURL,
          binding: Object.freeze({ ...configuration.binding }),
          domains,
          session: configuration.session,
          title: configuration.title
        })
      });
      reservedBytes += bytes.byteLength;
      result(request.type, request, true, null);
    }

    function refuse(request, error) {
      retire(request.surfaceId);
      result(request.type, request, false, error);
    }

    function chunk(request) {
      if (!validIdentity(request)) return;
      const transfer = transfers.get(request.surfaceId);
      if (!transfer || transfer.configuration.session !== request.session ||
          transfer.transferId !== request.transferId) {
        result(request.type, request, false, "stale");
        return;
      }
      if (!contract.exactFields(request, [
        "bytes", "offset", "requestId", "session", "surfaceId", "transferId", "type"
      ], primitives) || !Number.isSafeInteger(request.offset) || request.offset < 0 ||
          !(request.bytes instanceof ArrayBufferType)) {
        refuse(request, "transfer-refused");
        return;
      }
      const remaining = transfer.bytes.byteLength - transfer.nextOffset;
      if (request.offset !== transfer.nextOffset || request.bytes.byteLength === 0 ||
          request.bytes.byteLength > ARTIFACT_CHUNK_BYTES ||
          request.bytes.byteLength !== Math.min(ARTIFACT_CHUNK_BYTES, remaining)) {
        refuse(request, "transfer-refused");
        return;
      }
      transfer.bytes.set(new Uint8ArrayType(request.bytes), request.offset);
      transfer.nextOffset += request.bytes.byteLength;
    }

    function commit(request) {
      if (!validIdentity(request)) return;
      const transfer = transfers.get(request.surfaceId);
      if (!transfer || transfer.configuration.session !== request.session ||
          transfer.transferId !== request.transferId) {
        result(request.type, request, false, "stale");
        return;
      }
      if (!contract.exactFields(request, [
        "requestId", "session", "surfaceId", "transferId", "type"
      ], primitives)) {
        refuse(request, "transfer-refused");
        return;
      }
      if (transfer.nextOffset !== transfer.bytes.byteLength) {
        refuse(request, "transfer-incomplete");
        return;
      }
      let artifactHTML;
      try {
        artifactHTML = new TextDecoderType("utf-8", { fatal: true })
          .decode(transfer.bytes);
      } catch (_) {
        refuse(request, "transfer-refused");
        return;
      }
      const configuration = Object.freeze({
        ...transfer.configuration,
        artifactHTML
      });
      retire(request.surfaceId);
      onComplete(Object.freeze({
        type: request.type,
        requestId: request.requestId,
        surfaceId: request.surfaceId,
        configuration
      }));
    }

    function handle(request) {
      if (!request || typeof request.type !== "string") return false;
      if (request.type === "nmp.outer.mount.begin") begin(request);
      else if (request.type === "nmp.outer.mount.chunk") chunk(request);
      else if (request.type === "nmp.outer.mount.commit") commit(request);
      else return false;
      return true;
    }

    function has(surfaceId, session) {
      const transfer = transfers.get(surfaceId);
      return Boolean(transfer && transfer.configuration.session === session);
    }

    function dispose() {
      disposed = true;
      transfers.clear();
      reservedBytes = 0;
    }

    function counts() {
      return Object.freeze({ pendingTransfers: transfers.size, reservedTransferBytes: reservedBytes });
    }

    return Object.freeze({ counts, dispose, handle, has, retire });
  }

  const exported = Object.freeze({
    ARTIFACT_CHUNK_BYTES,
    MAX_PENDING_TRANSFERS,
    createTransferManager
  });
  global.NMPTrustedShellEmbeddingTransfer = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
