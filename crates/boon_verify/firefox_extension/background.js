"use strict";

const HOST = "boon_firefox_native_host";

browser.runtime.onMessage.addListener((message) => {
  if (message && message.target === "boon-capture-visible-tab") {
    return browser.tabs.query({ active: true, currentWindow: true })
      .then((tabs) => {
        const active = tabs && tabs[0];
        if (browser.tabs.captureTab && active && active.id !== undefined) {
          return browser.tabs.captureTab(active.id, { format: "png" });
        }
        if (browser.tabs.captureVisibleTab) {
          return browser.tabs.captureVisibleTab(null, { format: "png" });
        }
        throw new Error("Firefox tabs screenshot API is unavailable");
      })
      .then((dataUrl) => ({ ok: true, data_url: dataUrl }))
      .catch((error) => ({
        ok: false,
        error: String(error && error.message || error),
        last_error: browser.runtime.lastError ? String(browser.runtime.lastError.message) : null
      }));
  }
  if (!message || message.target !== "boon-native-host") {
    return undefined;
  }
  return browser.runtime.sendNativeMessage(HOST, message.payload || { type: "ping" })
    .then((response) => ({ ok: true, response }))
    .catch((error) => ({
      ok: false,
      error: String(error && error.message || error),
      last_error: browser.runtime.lastError ? String(browser.runtime.lastError.message) : null
    }));
});
