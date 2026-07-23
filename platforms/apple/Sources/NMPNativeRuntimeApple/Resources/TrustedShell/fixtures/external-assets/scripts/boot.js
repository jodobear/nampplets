(function verifiedExternalAssetFixture() {
  "use strict";
  const image = document.getElementById("verified-image");
  let reported = false;
  function report() {
    const element = document.getElementById("verified-external");
    const styled = getComputedStyle(element).color === "rgb(17, 91, 61)";
    const imageLoaded = image.complete && image.naturalWidth === 2;
    if (!reported && styled && imageLoaded) {
      reported = true;
      window.napplet.shell.ping({ source: "verified-external-assets" });
    }
  }
  image.addEventListener("load", report, { once: true });
  addEventListener("load", report, { once: true });
})();
