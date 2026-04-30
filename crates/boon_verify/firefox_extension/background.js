"use strict";

const HOST = "boon_firefox_native_host";

browser.runtime.onMessage.addListener((message) => {
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
