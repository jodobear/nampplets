const status = document.querySelector("#status");
status.textContent = typeof window.napplet === "object" ? "ready" : "missing prelude";
window.parent.postMessage({ type: "conformance.external-assets-ready" }, "*");
