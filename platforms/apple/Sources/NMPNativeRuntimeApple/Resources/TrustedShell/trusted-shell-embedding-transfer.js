(function trustedShellEmbeddingTransfer(global) {
  "use strict";

  const ARTIFACT_CHUNK_BYTES = 256 * 1024;
  const MAX_PENDING_TRANSFERS = 16;
  const TRANSFER_DEADLINE_MS = 30 * 1000;
  const artifactPolicySource = global.NMPTrustedShellArtifactPolicy ||
    (typeof require === "function"
      ? require("./trusted-shell-artifact-policy.js")
      : null);

  function createTransferManager(environment, dependencies) {
    const {
      artifactPolicy, beforeBegin, contract, onComplete, primitives,
      reserveAdmission, result, settleAdmission
    } = dependencies;
    const ArrayBufferType = environment && (environment.ArrayBuffer || global.ArrayBuffer);
    const TextDecoderType = environment && (environment.TextDecoder || global.TextDecoder);
    const Uint8ArrayType = environment && (environment.Uint8Array || global.Uint8Array);
    const allocateBytes = dependencies.allocateBytes ||
      ((length) => new Uint8ArrayType(length));
    const clearTimerSource = environment.clearTimeout || global.clearTimeout;
    const scheduleTimerSource = environment.setTimeout || global.setTimeout;
    const clearTimer = dependencies.clearTimeout ||
      ((token) => clearTimerSource.call(environment, token));
    const scheduleTimer = dependencies.setTimeout ||
      ((callback, delay) => scheduleTimerSource.call(environment, callback, delay));
    if (!artifactPolicySource || !environment ||
        typeof TextDecoderType !== "function" ||
        typeof Uint8ArrayType !== "function" ||
        typeof ArrayBufferType !== "function" ||
        typeof allocateBytes !== "function" ||
        typeof clearTimer !== "function" || typeof scheduleTimer !== "function" ||
        !artifactPolicySource.isNormalizedPolicy(artifactPolicy) ||
        typeof beforeBegin !== "function" || !contract ||
        typeof onComplete !== "function" ||
        !primitives || typeof reserveAdmission !== "function" ||
        typeof result !== "function" || typeof settleAdmission !== "function") {
      throw new TypeError("trusted artifact transfer dependencies are unavailable");
    }
    const transfers = new Map();
    let reservedBytes = 0;
    let disposed = false;

    function armDeadline(surfaceId, transfer) {
      if (transfer.timer !== null) clearTimer(transfer.timer);
      transfer.timer = scheduleTimer(() => {
        const current = transfers.get(surfaceId);
        if (current !== transfer) return;
        retire(surfaceId);
        result(transfer.expiryRequest.type, transfer.expiryRequest, false,
          "transfer-expired");
      }, TRANSFER_DEADLINE_MS);
    }

    function remove(surfaceId, settle = true) {
      const transfer = transfers.get(surfaceId);
      if (!transfer) return null;
      transfers.delete(surfaceId);
      clearTimer(transfer.timer);
      reservedBytes -= transfer.bytes.byteLength;
      if (settle && transfer.admissionToken) {
        settleAdmission(transfer.admissionToken);
      }
      return transfer;
    }

    function retire(surfaceId) {
      return Boolean(remove(surfaceId));
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
      if (disposed || transfers.size >= MAX_PENDING_TRANSFERS ||
          reservedBytes + configuration.artifactBytes >
            artifactPolicySource.HARD_MAX_ARTIFACT_HTML_BYTES) {
        result(request.type, request, false, "overloaded");
        return;
      }
      const domains = contract.snapshotDomains(configuration.domains);
      if (!domains) return;
      const admissionToken = artifactPolicy.elevated ? reserveAdmission() : null;
      if (artifactPolicy.elevated && !admissionToken) {
        result(request.type, request, false, "overloaded");
        return;
      }
      let bytes;
      try {
        bytes = allocateBytes(configuration.artifactBytes);
        if (!(bytes instanceof Uint8ArrayType) ||
            bytes.byteLength !== configuration.artifactBytes) {
          throw new TypeError("invalid transfer allocation");
        }
      } catch (_) {
        if (admissionToken) settleAdmission(admissionToken);
        result(request.type, request, false, "overloaded");
        return;
      }
      beforeBegin(request.surfaceId);
      retire(request.surfaceId);
      const expiryRequest = Object.freeze({
        type: request.type,
        requestId: request.requestId,
        surfaceId: request.surfaceId,
        configuration: Object.freeze({ session: configuration.session })
      });
      const transfer = {
        admissionToken,
        bytes,
        expiryRequest,
        nextOffset: 0,
        transferId: request.requestId,
        configuration: Object.freeze({
          artifactBaseURL: configuration.artifactBaseURL,
          binding: Object.freeze({ ...configuration.binding }),
          domains,
          session: configuration.session,
          title: configuration.title
        }),
        timer: null
      };
      transfers.set(request.surfaceId, transfer);
      reservedBytes += bytes.byteLength;
      armDeadline(request.surfaceId, transfer);
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
      armDeadline(request.surfaceId, transfer);
      result(request.type, request, true, null);
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
        artifactHTML = new TextDecoderType("utf-8", {
          fatal: true,
          ignoreBOM: true
        })
          .decode(transfer.bytes);
      } catch (_) {
        refuse(request, "transfer-refused");
        return;
      }
      const completed = remove(request.surfaceId, false);
      const configuration = Object.freeze({
        ...completed.configuration,
        artifactHTML
      });
      onComplete(Object.freeze({
        type: request.type,
        requestId: request.requestId,
        surfaceId: request.surfaceId,
        configuration
      }), completed.admissionToken);
    }

    function handle(request) {
      if (!request || typeof request.type !== "string") return false;
      if (request.type === "nmp.outer.mount.begin") begin(request);
      else if (request.type === "nmp.outer.mount.chunk") chunk(request);
      else if (request.type === "nmp.outer.mount.commit") commit(request);
      else return false;
      return true;
    }

    function bypassesParentRate(request) {
      if (!request || request.type !== "nmp.outer.mount.chunk" ||
          !validIdentity(request) || !contract.exactFields(request, [
            "bytes", "offset", "requestId", "session", "surfaceId", "transferId", "type"
          ], primitives) || !Number.isSafeInteger(request.offset) ||
          !(request.bytes instanceof ArrayBufferType)) return false;
      const transfer = transfers.get(request.surfaceId);
      if (!transfer || transfer.configuration.session !== request.session ||
          transfer.transferId !== request.transferId) return false;
      const remaining = transfer.bytes.byteLength - transfer.nextOffset;
      return request.offset === transfer.nextOffset &&
        request.bytes.byteLength > 0 &&
        request.bytes.byteLength <= ARTIFACT_CHUNK_BYTES &&
        request.bytes.byteLength === Math.min(ARTIFACT_CHUNK_BYTES, remaining);
    }

    function has(surfaceId, session) {
      const transfer = transfers.get(surfaceId);
      return Boolean(transfer && transfer.configuration.session === session);
    }

    function dispose() {
      disposed = true;
      for (const surfaceId of Array.from(transfers.keys())) retire(surfaceId);
    }

    function counts() {
      return Object.freeze({ pendingTransfers: transfers.size, reservedTransferBytes: reservedBytes });
    }

    return Object.freeze({
      bypassesParentRate, counts, dispose, handle, has, retire
    });
  }

  const exported = Object.freeze({
    ARTIFACT_CHUNK_BYTES,
    MAX_PENDING_TRANSFERS,
    TRANSFER_DEADLINE_MS,
    createTransferManager
  });
  global.NMPTrustedShellEmbeddingTransfer = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
