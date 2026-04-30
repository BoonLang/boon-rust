"use strict";

function injectBridge() {
  const parent = document.documentElement || document.head;
  if (!parent) {
    setTimeout(injectBridge, 10);
    return;
  }
  const script = document.createElement("script");
  script.src = browser.runtime.getURL("page_bridge.js");
  script.onload = () => script.remove();
  parent.appendChild(script);
}

injectBridge();

window.addEventListener("message", async (event) => {
  if (event.source !== window || !event.data || event.data.target !== "boon-extension") {
    return;
  }
  const payload = event.data.payload || { type: "ping" };
  const target = payload.type === "capture-visible-tab"
    ? "boon-capture-visible-tab"
    : "boon-native-host";
  const response = await browser.runtime.sendMessage({
    target,
    payload
  });
  window.postMessage({
    target: "boon-page",
    id: event.data.id,
    payload: response
  }, "*");
});
