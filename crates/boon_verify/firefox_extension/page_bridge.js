"use strict";

window.__boonExtension = {
  native(message) {
    return sendToExtension(message || { type: "ping" });
  },
  captureVisibleTab() {
    return sendToExtension({ type: "capture-visible-tab" });
  }
};

function sendToExtension(message) {
    const id = `${Date.now()}-${Math.random()}`;
    return new Promise((resolve) => {
      function onMessage(event) {
        if (event.source === window && event.data && event.data.target === "boon-page" && event.data.id === id) {
          window.removeEventListener("message", onMessage);
          resolve(event.data.payload);
        }
      }
      window.addEventListener("message", onMessage);
      window.postMessage({
        target: "boon-extension",
        id,
        payload: message
      }, "*");
    });
}
